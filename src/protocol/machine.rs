//! The machine side of a shot a person runs. It answers the grinder's mako polls and
//! walks the states the grinder needs to count a shot, while a human (or an
//! app) does the real brewing and reports time and weight:
//!
//! ```text
//! Ready (ON)
//!   | grinder posts scripts/execute {"ID":9}  (E64 WS: knob press after a grind)
//!   | or on_brew_job                          (E65/E80: a Grind-by-Sync grind)
//!   v                                          -> tell the human "start the shot"
//! Running (ACTIVE SERVING, time rising)
//!   | report_shot(time, weight)                -> the human's numbers
//!   v
//! Finishing (ACTIVE FINISHING, final time)     -> grinder takes the result here
//!   | after `finishing_hold`
//!   v
//! Ready (ON)
//! ```
//!
//! No I/O and no clock of its own: every call takes the current `Instant`, so
//! it runs the same in a server, an app, or a test.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{
    EXTRACTION_STATUS_BREWING, EXTRACTION_STATUS_FINISHED, EXTRACTION_STATUS_IDLE,
    EXTRACTION_STATUS_USER_ABORT, GrindResult, MAX_EXTRACTION_TIME_MS, MAX_OFF_TARGET_MS,
    MIN_EXTRACTION_TIME_MS, MachineIdentity, MachineStatus, MakoState,
};

/// Tunables, shown as `machine.config` in `GET /api/state`.
#[derive(Debug, Clone)]
pub struct MachineConfig {
    pub identity: MachineIdentity,
    /// How long `ACTIVE FINISHING` is shown before going back to `ON`. The
    /// E64 WS takes the result only from a poll that sees it, so this is
    /// [`MIN_FINISHING_HOLD`] to [`MAX_FINISHING_HOLD`] (`--finishing-hold-s`
    /// checks it).
    ///
    /// [`MIN_FINISHING_HOLD`]: super::MIN_FINISHING_HOLD
    /// [`MAX_FINISHING_HOLD`]: super::MAX_FINISHING_HOLD
    pub finishing_hold: Duration,
    /// The shot window: a shot with no reported result is aborted after this
    /// long. `None` for no limit, sent as 0 as in the settings.
    pub shot_window: Option<Duration>,
}

impl Serialize for MachineConfig {
    /// With the fixed values next to the tunables, as the API has always
    /// shown them: the machine serves a start request at once, and the
    /// extraction status it sends.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Shown<'a> {
            identity: &'a MachineIdentity,
            finishing_hold_s: f64,
            shot_window_s: u64,
            serve_on_start_request: bool,
            extraction_status_idle: i64,
            extraction_status_brewing: i64,
            extraction_status_finished: i64,
        }
        Shown {
            identity: &self.identity,
            finishing_hold_s: self.finishing_hold.as_secs_f64(),
            shot_window_s: self.shot_window.map_or(0, |d| d.as_secs()),
            serve_on_start_request: true,
            extraction_status_idle: EXTRACTION_STATUS_IDLE,
            extraction_status_brewing: EXTRACTION_STATUS_BREWING,
            extraction_status_finished: EXTRACTION_STATUS_FINISHED,
        }
        .serialize(s)
    }
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            identity: MachineIdentity::default(),
            finishing_hold: Duration::from_secs(6),
            shot_window: Some(Duration::from_secs(180)),
        }
    }
}

/// The measured numbers of one shot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ShotResult {
    pub time_ms: u32,
    /// The weight in the cup. Sent as `PU_SENS_FLOW_METER_VOLUME`, a volume
    /// in ml: 1 g ≈ 1 ml is the working assumption.
    pub weight_g: f64,
}

impl ShotResult {
    /// Longest shot time a person can enter. Well past the grinder's 80 s, so
    /// a slow shot still reaches it (and is skipped there); more is a typo.
    pub const MAX_TIME: Duration = Duration::from_secs(600);

    /// A typed shot time in seconds, or `None` unless it is a number from 0
    /// to [`Self::MAX_TIME`].
    pub fn time_from_secs(s: f64) -> Option<Duration> {
        (0.0..=Self::MAX_TIME.as_secs_f64())
            .contains(&s)
            .then(|| Duration::from_secs_f64(s))
    }

    /// From what a person reads off a timer and a scale.
    pub fn new(time: Duration, weight_g: f64) -> Self {
        Self {
            time_ms: u32::try_from(time.as_millis()).unwrap_or(u32::MAX),
            weight_g,
        }
    }
}

/// The shot times the grinder adjusts from: over `min_ms`, up to `max_ms`
/// (`grinder_range` in `GET /api/state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GrinderRange {
    pub min_ms: u32,
    pub max_ms: u32,
}

impl GrinderRange {
    /// The E64 WS's: over 10 s ([`MIN_EXTRACTION_TIME_MS`]), up to 80 s
    /// ([`MAX_EXTRACTION_TIME_MS`]).
    pub const E64: Self = Self {
        min_ms: MIN_EXTRACTION_TIME_MS,
        max_ms: MAX_EXTRACTION_TIME_MS,
    };

    /// An E65/E80 GbS's, for a brew job that aims for `target_ms`: within
    /// 15 s of it ([`MAX_OFF_TARGET_MS`]), but never down to 10 s
    /// ([`MIN_EXTRACTION_TIME_MS`]).
    pub fn around(target_ms: u32) -> Self {
        Self {
            min_ms: target_ms
                .saturating_sub(MAX_OFF_TARGET_MS)
                .max(MIN_EXTRACTION_TIME_MS),
            max_ms: target_ms.saturating_add(MAX_OFF_TARGET_MS),
        }
    }

    /// Whether the grinder adjusts from a shot of `time_ms`.
    pub fn contains(self, time_ms: u32) -> bool {
        time_ms > self.min_ms && time_ms <= self.max_ms
    }
}

/// Where the machine is in a shot (`shot` in `GET /api/state`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    Ready,
    Running {
        #[serde(skip)]
        since: Instant,
        /// `true` if the grinder asked for it (only then does it count).
        requested_by_grinder: bool,
    },
    Finishing {
        #[serde(skip)]
        until: Instant,
        result: ShotResult,
        aborted: bool,
    },
}

/// Things that happened, for a UI or a transcript. `kind` reads
/// `<group>_<object>_<what happened>`, as [`AbortCause`] does: `grinder_*`
/// for what the grinder did, `shot_*` for a shot's course, whoever caused it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MachineEvent {
    /// The grinder finished a grind: an E64 WS posted `/api/v2/brewratio`,
    /// or an E65/E80 sent a brew job (followed by
    /// [`MachineEvent::GrinderShotRequested`]).
    GrinderGrindFinished { result: GrindResult },
    /// The grinder asked for the shot: an E64 WS's knob press (with the
    /// script it runs), or an E65/E80 brew job (with the time it aims for).
    GrinderShotRequested {
        script_id: Option<i64>,
        target_time_ms: Option<u32>,
    },
    /// We entered `ACTIVE SERVING`. `human_action` is what to tell the person.
    ShotStarted {
        requested_by_grinder: bool,
        human_action: &'static str,
    },
    /// A shot result was reported and is being shown to the grinder.
    ShotReported {
        result: ShotResult,
        /// Whether the grinder adjusts from its time
        /// ([`Machine::grinder_range`]); `None` when that is not known.
        in_grinder_range: Option<bool>,
    },
    /// The shot ended without a result; the grinder skips it. Made with
    /// [`MachineEvent::shot_aborted`].
    ShotAborted {
        /// Why, in English words ([`AbortCause::text`]), as apps from before
        /// `cause` read it.
        reason: &'static str,
        /// Why, as a fixed code for apps to match on.
        cause: AbortCause,
    },
    /// The shot is over and the machine back to `ON`.
    ShotFinished,
}

impl MachineEvent {
    /// The event for a brew that ended for `cause`, with its words as `reason`.
    pub fn shot_aborted(cause: AbortCause) -> Self {
        Self::ShotAborted {
            reason: cause.text(),
            cause,
        }
    }
}

/// Why a brew ended without a result, as a fixed code (`cause` next to the
/// English `reason` in the API and the shot history). Apps match on the code,
/// so the words can change without turning a stop made on purpose into an
/// error. Codes read `<group>_<object>_<what happened>`: the group is `shot`
/// or `grinder` (the grinder or the link to it), and the object is left out
/// when the group itself is meant. A shot includes the grinder's request for
/// one (an E65/E80 brew job).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortCause {
    /// A person aborted it (`POST /api/shot/abort`).
    ShotAbortedByUser,
    /// An E65/E80 grinder asked for a newer shot (a new brew job) before this
    /// one came: that grind is the shot now.
    GrinderShotReplaced,
    /// The grinder's address changed while it waited for the shot.
    GrinderAddressChanged,
    /// No result came within the shot window.
    ShotTimedOut,
    /// The E65/E80 grinder dropped the shot it asked for (its brew job): it
    /// stopped waiting for it.
    GrinderShotDropped,
    /// The E65/E80 grinder refused to hand the shot it asked for (its brew
    /// job) to this gbs-anywhere.
    GrinderShotRefused,
    /// The E65/E80 grinder no longer knows this gbs-anywhere (its pairing was
    /// cleared there).
    GrinderUnpaired,
    /// Not sent by the machine: the shot history's name for a brew still open
    /// when the grinder asked for the next one (its end was missed).
    ShotEndMissed,
}

impl AbortCause {
    /// Every cause, one entry per variant.
    pub const ALL: [Self; 8] = [
        Self::ShotAbortedByUser,
        Self::GrinderShotReplaced,
        Self::GrinderAddressChanged,
        Self::ShotTimedOut,
        Self::GrinderShotDropped,
        Self::GrinderShotRefused,
        Self::GrinderUnpaired,
        Self::ShotEndMissed,
    ];

    /// In English words, for logs and the `reason` next to `cause`.
    pub fn text(self) -> &'static str {
        match self {
            Self::ShotAbortedByUser => "aborted by user",
            Self::GrinderShotReplaced => "a newer Grind-by-Sync grind replaced it",
            Self::GrinderAddressChanged => "the grinder address changed",
            Self::ShotTimedOut => "no result reported in time",
            Self::GrinderShotDropped => "the grinder stopped waiting before the shot came",
            Self::GrinderShotRefused => "the grinder refused the job",
            Self::GrinderUnpaired => "the grinder no longer knows this gbs-anywhere",
            Self::ShotEndMissed => "the next brew started before this one ended",
        }
    }

    /// Whether someone chose to end the brew (aborted), rather than it
    /// failing (an error).
    pub fn on_purpose(self) -> bool {
        match self {
            Self::ShotAbortedByUser => true,
            Self::GrinderShotReplaced => true,
            Self::GrinderAddressChanged => true,
            Self::ShotTimedOut => false,
            Self::GrinderShotDropped => false,
            Self::GrinderShotRefused => false,
            Self::GrinderUnpaired => false,
            Self::ShotEndMissed => false,
        }
    }
}

/// A recorded event with a sequence number (monotonic, starts at 1).
#[derive(Debug, Clone, Serialize)]
pub struct EventRecord {
    pub seq: u64,
    #[serde(skip)]
    pub at: Instant,
    #[serde(flatten)]
    pub event: MachineEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineError {
    /// A result or abort came in while no shot was running.
    NotRunning,
    /// A shot was started while one is running.
    AlreadyRunning,
    /// A result came without a weight, and the grinder sent no recipe weight
    /// to fall back on.
    NoWeight,
}

impl std::fmt::Display for MachineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotRunning => "no shot is running",
            Self::AlreadyRunning => "a shot is already running",
            Self::NoWeight => "no weight given, and the grinder sent no recipe weight",
        })
    }
}

impl MachineError {
    /// As a fixed code, for `code` in the API's errors.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotRunning => "shot_not_running",
            Self::AlreadyRunning => "shot_already_running",
            Self::NoWeight => "shot_weight_missing",
        }
    }
}

impl std::error::Error for MachineError {}

const EVENT_CAP: usize = 500;

/// The machine. See the module docs for the state walk.
#[derive(Debug, Clone)]
pub struct Machine {
    config: MachineConfig,
    /// Values served while `Ready` (status, temperatures, tank, script, ...).
    /// Change them to test the grind gate.
    pub ready: MakoState,
    /// Raw keys merged over every mako reply, for experiments the typed model
    /// cannot express (odd status codes, extra fields). Empty normally.
    pub overrides: Map<String, Value>,
    phase: Phase,
    /// The brew running (or last run) came from an E65/E80 brew job.
    brew_job: bool,
    /// See [`Machine::grinder_range`].
    range: Option<GrinderRange>,
    last_shot: Option<ShotResult>,
    last_grind: Option<GrindResult>,
    events: VecDeque<EventRecord>,
    seq: u64,
}

impl Machine {
    pub fn new(config: MachineConfig) -> Self {
        Self {
            config,
            ready: MakoState::default(),
            overrides: Map::new(),
            phase: Phase::Ready,
            brew_job: false,
            range: None,
            last_shot: None,
            last_grind: None,
            events: VecDeque::new(),
            seq: 0,
        }
    }

    pub fn config(&self) -> &MachineConfig {
        &self.config
    }

    /// Changes the shot window: how long a shot waits for its result. A shot
    /// already running keeps its start and ends once the new limit has
    /// passed.
    pub fn set_shot_window(&mut self, limit: Option<Duration>) {
        self.config.shot_window = limit;
    }

    pub fn identity(&self) -> &MachineIdentity {
        &self.config.identity
    }

    pub fn phase(&mut self, now: Instant) -> Phase {
        self.tick(now);
        self.phase
    }

    pub fn last_shot(&self) -> Option<ShotResult> {
        self.last_shot
    }

    pub fn last_grind(&self) -> Option<&GrindResult> {
        self.last_grind.as_ref()
    }

    /// The shot times the grinder adjusts from, for its current or last
    /// request: the E64 WS's after a knob press, and within 15 s of the
    /// target time (over 10 s) after an E65/E80 brew job. `None` before the
    /// first request, and after a brew job without a target time.
    pub fn grinder_range(&self) -> Option<GrinderRange> {
        self.range
    }

    /// The target weight of the last grind's recipe, if the grinder sent one.
    pub fn recipe_weight_g(&self) -> Option<f64> {
        let g = self.last_grind.as_ref()?.target_weight_g?;
        (g.is_finite() && g > 0.0).then_some(g)
    }

    /// Events with `seq > after`, oldest first (the last few hundred are kept).
    pub fn events_after(&self, after: u64) -> impl Iterator<Item = &EventRecord> {
        self.events.iter().filter(move |e| e.seq > after)
    }

    pub fn last_seq(&self) -> u64 {
        self.seq
    }

    /// `POST /api/v2/brewratio` from the grinder.
    pub fn on_grind_result(&mut self, now: Instant, result: GrindResult) {
        self.last_grind = Some(result.clone());
        self.push(now, MachineEvent::GrinderGrindFinished { result });
    }

    /// `POST /api/v2/scripts/execute` from the grinder: the brew starts at
    /// once, since the grinder re-sends the request only a few times before
    /// it gives up on a machine that does not enter `ACTIVE SERVING`.
    /// Repeats while already brewing are those retries and are ignored.
    pub fn on_start_request(&mut self, now: Instant, script_id: Option<i64>) {
        self.tick(now);
        if matches!(self.phase, Phase::Running { .. }) {
            return;
        }
        self.range = Some(GrinderRange::E64);
        self.push(
            now,
            MachineEvent::GrinderShotRequested {
                script_id,
                target_time_ms: None,
            },
        );
        self.begin(now, true);
    }

    /// A brew job from an E65/E80 grinder: like a grind result followed by a
    /// knob press, and sent as the same two events. Only taken while
    /// `Ready`; returns whether the shot started.
    pub fn on_brew_job(
        &mut self,
        now: Instant,
        result: GrindResult,
        target_time_ms: Option<u32>,
    ) -> bool {
        self.tick(now);
        if !matches!(self.phase, Phase::Ready) {
            return false;
        }
        self.last_grind = Some(result.clone());
        self.range = target_time_ms.map(GrinderRange::around);
        self.push(now, MachineEvent::GrinderGrindFinished { result });
        self.push(
            now,
            MachineEvent::GrinderShotRequested {
                script_id: None,
                target_time_ms,
            },
        );
        self.begin(now, true);
        self.brew_job = true;
        true
    }

    /// Whether a brew is running for an E65/E80 brew job (not for an E64 WS
    /// knob press or by hand).
    pub fn brewing_for_brew_job(&mut self, now: Instant) -> bool {
        matches!(self.phase(now), Phase::Running { .. }) && self.brew_job
    }

    /// Start a shot without a grinder request (the grinder will treat it as a
    /// flush; useful to test exactly that).
    pub fn start_shot(&mut self, now: Instant) -> Result<(), MachineError> {
        self.tick(now);
        if matches!(self.phase, Phase::Running { .. }) {
            return Err(MachineError::AlreadyRunning);
        }
        self.begin(now, false);
        Ok(())
    }

    /// The human's measured numbers: ends the brew and shows the result. A
    /// weight of 0 (none given) becomes the recipe weight; with no recipe
    /// weight either, the result is refused.
    pub fn report_shot(&mut self, now: Instant, result: ShotResult) -> Result<(), MachineError> {
        self.tick(now);
        if !matches!(self.phase, Phase::Running { .. }) {
            return Err(MachineError::NotRunning);
        }
        // Never send 0 g: without a weight, use the recipe's.
        let result = if result.weight_g.is_finite() && result.weight_g > 0.0 {
            result
        } else {
            ShotResult {
                weight_g: self.recipe_weight_g().ok_or(MachineError::NoWeight)?,

                ..result
            }
        };
        self.phase = Phase::Finishing {
            until: now + self.config.finishing_hold,
            result,
            aborted: false,
        };
        self.last_shot = Some(result);
        self.push(
            now,
            MachineEvent::ShotReported {
                result,
                in_grinder_range: self.range.map(|r| r.contains(result.time_ms)),
            },
        );
        Ok(())
    }

    /// Ends the brew as a user abort (`MA_EXTRACTION_STATUS = 2`): the grinder
    /// skips the result.
    pub fn abort(&mut self, now: Instant) -> Result<(), MachineError> {
        self.give_up(now, AbortCause::ShotAbortedByUser)
    }

    /// Ends the brew for `cause` (say, the grinder stopped waiting for it),
    /// like [`Machine::abort`] but with that cause in the event.
    pub fn give_up(&mut self, now: Instant, cause: AbortCause) -> Result<(), MachineError> {
        self.tick(now);
        let Phase::Running { since, .. } = self.phase else {
            return Err(MachineError::NotRunning);
        };
        self.abort_shot(now, since, cause);
        Ok(())
    }

    /// Applies time-based transitions. Called by every other method; call it
    /// periodically too if you want events without polls.
    pub fn tick(&mut self, now: Instant) {
        match self.phase {
            Phase::Running { since, .. } => {
                if let Some(limit) = self.config.shot_window
                    && now.saturating_duration_since(since) >= limit
                {
                    self.abort_shot(now, since, AbortCause::ShotTimedOut);
                }
            }
            Phase::Finishing { until, .. } if now >= until => {
                self.phase = Phase::Ready;
                self.push(now, MachineEvent::ShotFinished);
            }
            _ => {}
        }
    }

    /// The typed reply to `GET /api/v2/mako` at `now` (without overrides).
    pub fn mako(&mut self, now: Instant) -> MakoState {
        self.tick(now);
        let mut s = self.ready.clone();
        match self.phase {
            Phase::Ready => {
                s.extraction_status = EXTRACTION_STATUS_IDLE;
                if let Some(shot) = self.last_shot {
                    s.real_extraction_time_ms = shot.time_ms;
                    s.flow_meter_volume_ml = shot.weight_g;
                }
            }
            Phase::Running { since, .. } => {
                s.status = MachineStatus::ActiveServing;
                s.extraction_status = EXTRACTION_STATUS_BREWING;
                s.real_extraction_time_ms =
                    u32::try_from(now.saturating_duration_since(since).as_millis())
                        .unwrap_or(u32::MAX);
                s.flow_meter_volume_ml = 0.0;
            }
            Phase::Finishing {
                result, aborted, ..
            } => {
                s.status = MachineStatus::ActiveFinishing;
                s.extraction_status = if aborted {
                    EXTRACTION_STATUS_USER_ABORT
                } else {
                    EXTRACTION_STATUS_FINISHED
                };
                s.real_extraction_time_ms = result.time_ms;
                s.flow_meter_volume_ml = result.weight_g;
            }
        }
        s
    }

    /// The JSON body for `GET /api/v2/mako` at `now`, overrides applied.
    pub fn mako_json(&mut self, now: Instant) -> Value {
        let mut v = serde_json::to_value(self.mako(now)).unwrap_or(Value::Null);
        if let Value::Object(obj) = &mut v {
            for (k, val) in &self.overrides {
                obj.insert(k.clone(), val.clone());
            }
        }
        v
    }

    fn begin(&mut self, now: Instant, requested_by_grinder: bool) {
        self.phase = Phase::Running {
            since: now,
            requested_by_grinder,
        };
        self.brew_job = false;
        let human_action = if requested_by_grinder {
            "Start the shot on the machine now, then report time and weight."
        } else {
            "Brew started without a grinder request: the grinder will count it as a flush."
        };
        self.push(
            now,
            MachineEvent::ShotStarted {
                requested_by_grinder,
                human_action,
            },
        );
    }

    fn abort_shot(&mut self, now: Instant, since: Instant, cause: AbortCause) {
        self.phase = Phase::Finishing {
            until: now + self.config.finishing_hold,
            result: ShotResult::new(now.saturating_duration_since(since), 0.0),
            aborted: true,
        };
        self.push(now, MachineEvent::shot_aborted(cause));
    }

    fn push(&mut self, at: Instant, event: MachineEvent) {
        self.seq += 1;
        if self.events.len() == EVENT_CAP {
            self.events.pop_front();
        }
        self.events.push_back(EventRecord {
            seq: self.seq,
            at,
            event,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{BrewState, GrinderEvent, GrinderModel};

    const POLL: Duration = Duration::from_secs(2);

    /// Polls the machine every 2 s through the grinder model, like the real
    /// grinder, and returns the accepted shot time if any.
    fn poll_until_ready(m: &mut Machine, g: &mut GrinderModel, mut t: Instant) -> Option<u32> {
        let mut accepted = None;
        for _ in 0..20 {
            for e in g.observe(&m.mako(t)) {
                if let GrinderEvent::ShotAccepted { time_ms, .. } = e {
                    accepted = Some(time_ms);
                }
            }
            if matches!(m.phase(t), Phase::Ready) {
                break;
            }
            t += POLL;
        }
        accepted
    }

    #[test]
    fn typed_shot_time_stays_in_range() {
        let secs = ShotResult::time_from_secs;
        assert_eq!(secs(28.5), Some(Duration::from_millis(28_500)));
        assert_eq!(secs(0.0), Some(Duration::ZERO));
        assert_eq!(secs(600.0), Some(ShotResult::MAX_TIME));
        for bad in [-1.0, 600.1, 1e20, f64::INFINITY, f64::NAN] {
            assert_eq!(secs(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_e64_adjusts_from_shots_over_10_s_up_to_80_s() {
        for (ms, expected) in [
            (0, false),
            (10_000, false),
            (10_001, true),
            (28_500, true),
            (80_000, true),
            (80_001, false),
        ] {
            assert_eq!(GrinderRange::E64.contains(ms), expected, "{ms} ms");
        }
    }

    #[test]
    fn an_e65_adjusts_from_shots_within_15_s_of_the_target_over_10_s() {
        let window = |target_ms| {
            let r = GrinderRange::around(target_ms);
            (r.min_ms, r.max_ms)
        };
        assert_eq!(window(28_000), (13_000, 43_000));
        // Never down to 10 s, however short the target.
        assert_eq!(window(20_000), (10_000, 35_000));
        assert_eq!(window(12_000), (10_000, 27_000));
        assert_eq!(window(5_000), (10_000, 20_000));
        for (target_ms, ms, expected) in [
            (28_000, 13_000, false),
            (28_000, 43_000, true),
            (24_000, 11_000, true),
            (20_000, 10_000, false),
            (20_000, 10_001, true),
        ] {
            let adjusts = GrinderRange::around(target_ms).contains(ms);
            assert_eq!(adjusts, expected, "{ms} ms for {target_ms} ms");
        }
    }

    /// Reports a shot of `ms` and returns its `in_grinder_range`; `t` moves
    /// on past the time the shot is shown.
    fn reported(m: &mut Machine, t: &mut Instant, ms: u64) -> Option<bool> {
        m.report_shot(*t, ShotResult::new(Duration::from_millis(ms), 40.0))
            .unwrap();
        *t += Duration::from_secs(10);
        match m.events_after(0).last().map(|e| &e.event) {
            Some(MachineEvent::ShotReported {
                in_grinder_range, ..
            }) => *in_grinder_range,
            other => panic!("not a shot: {other:?}"),
        }
    }

    #[test]
    fn each_shot_is_checked_against_the_window_of_its_request() {
        let mut t = Instant::now();
        let mut m = Machine::new(MachineConfig::default());
        assert_eq!(m.grinder_range(), None, "no request yet");
        let job = GrindResult::parse(br#"{"SYNC_BEVERAGE_WEIGHT":40}"#).unwrap();

        assert!(m.on_brew_job(t, job.clone(), Some(28_000)));
        assert_eq!(m.grinder_range(), Some(GrinderRange::around(28_000)));
        assert_eq!(reported(&mut m, &mut t, 12_000), Some(false));
        assert!(m.on_brew_job(t, job.clone(), Some(24_000)));
        // A knob press while that brew runs is ignored, its window too.
        m.on_start_request(t, Some(9));
        assert_eq!(m.grinder_range(), Some(GrinderRange::around(24_000)));
        assert_eq!(reported(&mut m, &mut t, 11_000), Some(true));

        // Without a target time, the window is not known.
        assert!(m.on_brew_job(t, job.clone(), None));
        assert_eq!(m.grinder_range(), None);
        assert_eq!(reported(&mut m, &mut t, 50_000), None);

        m.on_start_request(t, Some(9));
        assert_eq!(m.grinder_range(), Some(GrinderRange::E64), "a knob press");
        // A brew job while that brew runs is refused and leaves its window.
        assert!(!m.on_brew_job(t, job.clone(), Some(40_000)));
        assert_eq!(m.grinder_range(), Some(GrinderRange::E64));
        assert_eq!(reported(&mut m, &mut t, 50_000), Some(true));
        // A flush started by hand keeps the window of the last request.
        m.start_shot(t).unwrap();
        assert_eq!(m.grinder_range(), Some(GrinderRange::E64));
    }

    #[test]
    fn human_shot_is_accepted_by_the_grinder_model() {
        let t0 = Instant::now();
        let mut m = Machine::new(MachineConfig::default());
        let mut g = GrinderModel::default();
        g.observe(&m.mako(t0));

        g.request_start();
        m.on_start_request(t0, Some(9));
        let t1 = t0 + POLL;
        g.observe(&m.mako(t1));
        assert_eq!(g.brew_state(), BrewState::Extraction);
        assert_eq!(m.mako(t1).real_extraction_time_ms, 2000);

        // The grinder retries while we brew: ignored.
        m.on_start_request(t1, Some(9));

        let t2 = t1 + Duration::from_secs(28);
        g.observe(&m.mako(t2));
        m.report_shot(t2, ShotResult::new(Duration::from_secs(30), 36.0))
            .unwrap();
        assert_eq!(poll_until_ready(&mut m, &mut g, t2), Some(30_000));
        assert_eq!(
            m.mako(t2 + Duration::from_secs(60)).status,
            MachineStatus::On
        );
    }

    /// Runs a shot the grinder asked for, reports it, and polls like the
    /// grinder: the first poll `phase` before the report, then `gaps` apart
    /// (in turn, from the `first` one). Says whether the grinder took it.
    fn taken(hold: Duration, phase: Duration, gaps: &[Duration], first: usize) -> bool {
        let t0 = Instant::now();
        let mut m = Machine::new(MachineConfig {
            finishing_hold: hold,
            ..MachineConfig::default()
        });
        let mut g = GrinderModel::default();
        g.observe(&m.mako(t0));
        g.request_start();
        m.on_start_request(t0, Some(9));
        g.observe(&m.mako(t0 + POLL));
        let reported = t0 + Duration::from_secs(30);
        let mut t = reported - phase;
        g.observe(&m.mako(t));
        m.report_shot(reported, ShotResult::new(Duration::from_secs(28), 36.0))
            .unwrap();
        for gap in gaps.iter().cycle().skip(first).take(20) {
            t += *gap;
            let accepted = g
                .observe(&m.mako(t))
                .iter()
                .any(|e| matches!(e, GrinderEvent::ShotAccepted { .. }));
            if accepted {
                return true;
            }
        }
        false
    }

    #[test]
    fn a_shot_held_for_the_shortest_hold_is_always_taken() {
        // About every 2 s, some polls late.
        let gaps = [2000, 2400, 1600, 2900, 2000].map(Duration::from_millis);
        let mut missed = 0;
        for first in 0..gaps.len() {
            for tenths in 0..20 {
                let phase = Duration::from_millis(tenths * 100);
                assert!(
                    taken(crate::protocol::MIN_FINISHING_HOLD, phase, &gaps, first),
                    "phase {phase:?}, from gap {first}"
                );
                if !taken(Duration::from_secs(1), phase, &gaps, first) {
                    missed += 1;
                }
            }
        }
        assert!(missed > 0, "a hold shorter than a poll misses shots");
    }

    #[test]
    fn shot_without_weight_gets_the_recipe_weight_never_zero() {
        let t0 = Instant::now();
        let time = Duration::from_secs(30);
        let mut m = Machine::new(MachineConfig::default());
        m.on_start_request(t0, Some(9));
        assert_eq!(
            m.report_shot(t0, ShotResult::new(time, 0.0)),
            Err(MachineError::NoWeight),
            "no recipe weight to fall back on"
        );
        assert!(
            matches!(m.phase(t0), Phase::Running { .. }),
            "still brewing"
        );

        let recipe = GrindResult::parse(br#"{"SYNC_BEVERAGE_WEIGHT":36.5}"#).unwrap();
        m.on_grind_result(t0, recipe);
        for none in [0.0, -1.0, f64::NAN] {
            let mut m = m.clone();
            m.report_shot(t0, ShotResult::new(time, none)).unwrap();
            assert_eq!(m.last_shot().unwrap().weight_g, 36.5, "{none}");
        }
        m.report_shot(t0, ShotResult::new(time, 38.0)).unwrap();
        assert_eq!(m.last_shot().unwrap().weight_g, 38.0, "a weight is kept");
    }

    #[test]
    fn manual_start_counts_as_flush() {
        let t0 = Instant::now();
        let mut m = Machine::new(MachineConfig::default());
        let mut g = GrinderModel::default();
        g.observe(&m.mako(t0));
        m.start_shot(t0).unwrap();
        g.observe(&m.mako(t0 + POLL));
        assert_eq!(g.brew_state(), BrewState::Flushing);
        m.report_shot(t0 + POLL, ShotResult::new(Duration::from_secs(25), 30.0))
            .unwrap();
        assert_eq!(poll_until_ready(&mut m, &mut g, t0 + POLL), None);
    }

    #[test]
    fn abort_and_timeout() {
        let t0 = Instant::now();
        let mut m = Machine::new(MachineConfig::default());
        assert_eq!(m.abort(t0), Err(MachineError::NotRunning));
        m.on_start_request(t0, Some(9));
        m.abort(t0 + POLL).unwrap();
        assert_eq!(
            m.mako(t0 + POLL).extraction_status,
            EXTRACTION_STATUS_USER_ABORT
        );

        let mut m = Machine::new(MachineConfig::default());
        m.on_start_request(t0, Some(9));
        let late = t0 + Duration::from_secs(181);
        assert!(matches!(
            m.phase(late),
            Phase::Finishing { aborted: true, .. }
        ));
        assert!(m.events_after(0).any(|e| e.event
            == MachineEvent::ShotAborted {
                reason: "no result reported in time",
                cause: AbortCause::ShotTimedOut,
            }));
    }

    #[test]
    fn an_abort_says_why_in_words_and_as_a_code() {
        let t0 = Instant::now();
        let mut m = Machine::new(MachineConfig::default());
        m.on_start_request(t0, Some(9));
        m.abort(t0 + POLL).unwrap();
        let aborted = m.events_after(0).last().unwrap();
        assert_eq!(
            serde_json::to_value(aborted).unwrap(),
            serde_json::json!({
                "seq": 3,
                "kind": "shot_aborted",
                "reason": "aborted by user",
                "cause": "shot_aborted_by_user",
            })
        );
        // Each cause has its own words: the history finds the cause of a shot
        // saved before `cause` by them.
        let texts: std::collections::BTreeSet<_> =
            AbortCause::ALL.iter().map(|c| c.text()).collect();
        assert_eq!(texts.len(), AbortCause::ALL.len());
    }

    #[test]
    fn every_cause_is_in_all() {
        use AbortCause::*;
        let every = [
            ShotAbortedByUser,
            GrinderShotReplaced,
            GrinderAddressChanged,
            ShotTimedOut,
            GrinderShotDropped,
            GrinderShotRefused,
            GrinderUnpaired,
            ShotEndMissed,
        ];
        for cause in every {
            // No wildcard: a new cause stops this from compiling. Add it here,
            // to `every` above and to `AbortCause::ALL`.
            match cause {
                ShotAbortedByUser
                | GrinderShotReplaced
                | GrinderAddressChanged
                | ShotTimedOut
                | GrinderShotDropped
                | GrinderShotRefused
                | GrinderUnpaired
                | ShotEndMissed => {}
            }
        }
        assert_eq!(AbortCause::ALL, every);
    }

    #[test]
    fn a_brew_job_starts_a_grinder_brew_only_when_ready() {
        let t0 = Instant::now();
        let mut m = Machine::new(MachineConfig::default());
        let job = GrindResult::parse(br#"{"SYNC_BEVERAGE_WEIGHT":40}"#).unwrap();
        assert!(m.on_brew_job(t0, job.clone(), Some(28_000)));
        assert!(matches!(
            m.phase(t0),
            Phase::Running {
                requested_by_grinder: true,
                ..
            }
        ));
        assert_eq!(m.recipe_weight_g(), Some(40.0));
        assert!(m.brewing_for_brew_job(t0));
        assert!(!m.on_brew_job(t0, job, None), "one brew at a time");
        m.give_up(t0, AbortCause::GrinderShotDropped).unwrap();
        assert!(!m.brewing_for_brew_job(t0), "over");
        let mut knob = Machine::new(MachineConfig::default());
        knob.on_start_request(t0, Some(9));
        assert!(!knob.brewing_for_brew_job(t0), "an E64 WS knob press");
        assert!(m.events_after(0).any(|e| matches!(
            e.event,
            MachineEvent::ShotAborted {
                cause: AbortCause::GrinderShotDropped,
                ..
            }
        )));
    }

    #[test]
    fn a_new_shot_window_counts_for_the_running_shot() {
        let t0 = Instant::now();
        let mut m = Machine::new(MachineConfig::default());
        m.on_start_request(t0, Some(9));
        m.set_shot_window(Some(Duration::from_secs(90)));
        assert!(matches!(
            m.phase(t0 + Duration::from_secs(89)),
            Phase::Running { .. }
        ));
        assert!(matches!(
            m.phase(t0 + Duration::from_secs(90)),
            Phase::Finishing { aborted: true, .. }
        ));

        let mut m = Machine::new(MachineConfig::default());
        m.set_shot_window(None);
        m.on_start_request(t0, Some(9));
        let much_later = t0 + Duration::from_secs(4 * 3600);
        assert!(matches!(m.phase(much_later), Phase::Running { .. }));
    }

    #[test]
    fn the_config_shows_as_it_always_did() {
        let config = MachineConfig {
            finishing_hold: Duration::from_millis(6500),
            ..MachineConfig::default()
        };
        let mut shown = serde_json::to_value(&config).unwrap();
        assert_eq!(shown["identity"]["MA_SN"], "0000000000");
        shown.as_object_mut().unwrap().remove("identity");
        assert_eq!(
            shown,
            serde_json::json!({
                "finishing_hold_s": 6.5,
                "shot_window_s": 180,
                "serve_on_start_request": true,
                "extraction_status_idle": 0,
                "extraction_status_brewing": 1,
                "extraction_status_finished": 1,
            })
        );
        let unlimited = MachineConfig {
            shot_window: None,
            ..config
        };
        assert_eq!(
            serde_json::to_value(&unlimited).unwrap()["shot_window_s"],
            0
        );
    }

    #[test]
    fn overrides_win() {
        let t0 = Instant::now();
        let mut m = Machine::new(MachineConfig::default());
        m.overrides.insert("MA_STATUS".into(), Value::from(7));
        assert_eq!(m.mako_json(t0)["MA_STATUS"], 7);
    }
}
