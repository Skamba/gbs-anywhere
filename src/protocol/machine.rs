//! The machine side of a shot a person runs. It answers the grinder's mako polls and
//! walks the states the grinder needs to count a shot, while a human (or an
//! app) does the real brewing and reports time and weight:
//!
//! ```text
//! Ready (ON)
//!   | grinder posts scripts/execute {"ID":9}  (knob press after a grind)
//!   v                                          -> tell the human "start the shot"
//! Brewing (ACTIVE SERVING, time rising)
//!   | report_shot(time, volume)                -> the human's numbers
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

use serde::Serialize;
use serde_json::{Map, Value};

use super::{
    EXTRACTION_STATUS_USER_ABORT, GrindResult, MAX_EXTRACTION_TIME_MS, MachineIdentity,
    MachineStatus, MakoState,
};

/// Tunables. The extraction-status values for a normal brew are a best guess;
/// only `2` (user abort) has a known meaning to the grinder.
#[derive(Debug, Clone, Serialize)]
pub struct MachineConfig {
    pub identity: MachineIdentity,
    /// How long `ACTIVE FINISHING` is shown before going back to `ON`. The
    /// grinder polls about every 2 s, so this must span at least one poll.
    pub finishing_hold: Duration,
    /// A brew with no reported result is aborted after this long.
    pub brew_timeout: Option<Duration>,
    /// Enter `ACTIVE SERVING` as soon as the grinder asks to start. The grinder
    /// gives up after 3 unanswered requests, so leave this on.
    pub serve_on_start_request: bool,
    pub extraction_status_idle: i64,
    pub extraction_status_brewing: i64,
    pub extraction_status_finished: i64,
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            identity: MachineIdentity::default(),
            finishing_hold: Duration::from_secs(6),
            brew_timeout: Some(Duration::from_secs(180)),
            serve_on_start_request: true,
            extraction_status_idle: 0,
            extraction_status_brewing: 1,
            extraction_status_finished: 1,
        }
    }
}

/// The measured numbers of one shot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ShotResult {
    pub time_ms: u32,
    /// Sent as `PU_SENS_FLOW_METER_VOLUME`. A human with a scale has grams;
    /// 1 g ≈ 1 ml is the working assumption (see [`ShotResult::from_grams`]).
    pub volume_ml: f64,
}

impl ShotResult {
    pub fn new(time: Duration, volume_ml: f64) -> Self {
        Self {
            time_ms: u32::try_from(time.as_millis()).unwrap_or(u32::MAX),
            volume_ml,
        }
    }

    /// From what a person reads off a timer and a scale.
    pub fn from_grams(time: Duration, beverage_g: f64) -> Self {
        Self::new(time, beverage_g)
    }

    /// Whether the grinder will accept the time (it drops shots over 80 s).
    pub fn in_grinder_range(&self) -> bool {
        self.time_ms <= MAX_EXTRACTION_TIME_MS
    }
}

/// Where the machine is in a shot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    Ready,
    Brewing {
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

/// Things that happened, for a UI or a transcript.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MachineEvent {
    /// The grinder posted `/api/v2/brewratio` after a grind.
    GrindResult {
        result: GrindResult,
    },
    /// The grinder asked the machine to start a brew.
    StartRequested {
        script_id: Option<i64>,
    },
    /// We entered `ACTIVE SERVING`. `human_action` is what to tell the person.
    BrewStarted {
        requested_by_grinder: bool,
        human_action: &'static str,
    },
    /// A shot result was reported and is being shown to the grinder.
    ShotReported {
        result: ShotResult,
        in_grinder_range: bool,
    },
    ShotAborted {
        reason: &'static str,
    },
    /// Back to `ON`.
    Ready,
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
    /// A result or abort came in while no brew was running.
    NotBrewing,
    /// A brew was requested while one is running.
    AlreadyBrewing,
}

impl std::fmt::Display for MachineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotBrewing => "no brew is running",
            Self::AlreadyBrewing => "a brew is already running",
        })
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
            last_shot: None,
            last_grind: None,
            events: VecDeque::new(),
            seq: 0,
        }
    }

    pub fn config(&self) -> &MachineConfig {
        &self.config
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
        self.push(now, MachineEvent::GrindResult { result });
    }

    /// `POST /api/v2/scripts/execute` from the grinder. Repeats while already
    /// brewing are the grinder's retries and are ignored.
    pub fn on_start_request(&mut self, now: Instant, script_id: Option<i64>) {
        self.tick(now);
        if matches!(self.phase, Phase::Brewing { .. }) {
            return;
        }
        self.push(now, MachineEvent::StartRequested { script_id });
        if self.config.serve_on_start_request {
            self.begin(now, true);
        }
    }

    /// Start a brew without a grinder request (the grinder will treat it as a
    /// flush; useful to test exactly that).
    pub fn start_brew(&mut self, now: Instant) -> Result<(), MachineError> {
        self.tick(now);
        if matches!(self.phase, Phase::Brewing { .. }) {
            return Err(MachineError::AlreadyBrewing);
        }
        self.begin(now, false);
        Ok(())
    }

    /// The human's measured numbers: ends the brew and shows the result.
    pub fn report_shot(&mut self, now: Instant, result: ShotResult) -> Result<(), MachineError> {
        self.tick(now);
        if !matches!(self.phase, Phase::Brewing { .. }) {
            return Err(MachineError::NotBrewing);
        }
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
                in_grinder_range: result.in_grinder_range(),
            },
        );
        Ok(())
    }

    /// Ends the brew as a user abort (`MA_EXTRACTION_STATUS = 2`): the grinder
    /// skips the result.
    pub fn abort(&mut self, now: Instant) -> Result<(), MachineError> {
        self.tick(now);
        let Phase::Brewing { since, .. } = self.phase else {
            return Err(MachineError::NotBrewing);
        };
        self.abort_brew(now, since, "aborted by user");
        Ok(())
    }

    /// Applies time-based transitions. Called by every other method; call it
    /// periodically too if you want events without polls.
    pub fn tick(&mut self, now: Instant) {
        match self.phase {
            Phase::Brewing { since, .. } => {
                if let Some(limit) = self.config.brew_timeout
                    && now.saturating_duration_since(since) >= limit
                {
                    self.abort_brew(now, since, "no result reported in time");
                }
            }
            Phase::Finishing { until, .. } if now >= until => {
                self.phase = Phase::Ready;
                self.push(now, MachineEvent::Ready);
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
                s.extraction_status = self.config.extraction_status_idle;
                if let Some(shot) = self.last_shot {
                    s.real_extraction_time_ms = shot.time_ms;
                    s.flow_meter_volume_ml = shot.volume_ml;
                }
            }
            Phase::Brewing { since, .. } => {
                s.status = MachineStatus::ActiveServing;
                s.extraction_status = self.config.extraction_status_brewing;
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
                    self.config.extraction_status_finished
                };
                s.real_extraction_time_ms = result.time_ms;
                s.flow_meter_volume_ml = result.volume_ml;
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
        self.phase = Phase::Brewing {
            since: now,
            requested_by_grinder,
        };
        let human_action = if requested_by_grinder {
            "Start the shot on the machine now, then report time and weight."
        } else {
            "Brew started without a grinder request: the grinder will count it as a flush."
        };
        self.push(
            now,
            MachineEvent::BrewStarted {
                requested_by_grinder,
                human_action,
            },
        );
    }

    fn abort_brew(&mut self, now: Instant, since: Instant, reason: &'static str) {
        self.phase = Phase::Finishing {
            until: now + self.config.finishing_hold,
            result: ShotResult::new(now.saturating_duration_since(since), 0.0),
            aborted: true,
        };
        self.push(now, MachineEvent::ShotAborted { reason });
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
        m.report_shot(t2, ShotResult::from_grams(Duration::from_secs(30), 36.0))
            .unwrap();
        assert_eq!(poll_until_ready(&mut m, &mut g, t2), Some(30_000));
        assert_eq!(
            m.mako(t2 + Duration::from_secs(60)).status,
            MachineStatus::On
        );
    }

    #[test]
    fn manual_start_counts_as_flush() {
        let t0 = Instant::now();
        let mut m = Machine::new(MachineConfig::default());
        let mut g = GrinderModel::default();
        g.observe(&m.mako(t0));
        m.start_brew(t0).unwrap();
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
        assert_eq!(m.abort(t0), Err(MachineError::NotBrewing));
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
        assert!(
            m.events_after(0)
                .any(|e| matches!(e.event, MachineEvent::ShotAborted { .. }))
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
