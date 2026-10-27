//! The shots the grinder asked for, newest first: when, how they ended and
//! the numbers sent (`GET /api/history`). Kept in `history.json` next to the
//! settings file, or in memory without one.
//!
//! A shot counts from the grinder's request (the E64 WS's knob press, an
//! E65/E80 brew job) to its end: a reported result, or an abort. A shot
//! started by hand (a flush) is left out.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;
use serde::{Deserialize, Deserializer, Serialize};
use tokio::sync::broadcast::{self, error::RecvError};

use crate::protocol::machine::EventRecord;
use crate::protocol::{AbortCause, GrindResult, MachineEvent, ShotResult};
use crate::server::Server;
use crate::server::settings::{make_private, save_private};

/// Shots kept.
pub const CAPACITY: usize = 200;

/// Far more than [`CAPACITY`] shots take: a bigger file is not a history.
const MAX_FILE_BYTES: u64 = 4 << 20;

/// How a shot ended (`outcome`). Files written by earlier builds say
/// `success` and `error`; they read as `succeeded` and `failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShotOutcome {
    /// Measured and sent to the grinder.
    #[serde(alias = "success")]
    Succeeded,
    /// Ended on purpose ([`AbortCause::on_purpose`]): aborted by a person,
    /// replaced by a newer grind, or the grinder address changed.
    Aborted,
    /// Nothing came in time, the grinder stopped waiting, or the end was
    /// missed.
    #[serde(alias = "error")]
    Failed,
}

impl ShotOutcome {
    /// How a shot that ended for `cause` is listed.
    fn of(cause: AbortCause) -> Self {
        if cause.on_purpose() {
            Self::Aborted
        } else {
            Self::Failed
        }
    }
}

/// One shot in the history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "StoredShot")]
pub struct Shot {
    pub id: String,
    /// When the grinder asked for it (UTC): `ended_at` less how long the
    /// shot took in whole seconds, counted on the machine's clock.
    pub started_at: String,
    /// When it ended, as the wall clock read then (UTC). A clock step during
    /// the shot does not change how far apart the two are.
    pub ended_at: String,
    pub outcome: ShotOutcome,
    /// How long it ran, from the grinder's request to its end, counted on
    /// the machine's clock (as `running_ms` on its `shot_reported` or
    /// `shot_aborted` event). Files from before it was kept give
    /// `ended_at` less `started_at`, in whole seconds.
    pub running_ms: Option<u64>,
    /// Only for a success.
    pub time_ms: Option<u32>,
    pub weight_g: Option<f64>,
    /// Only for a success: whether the grinder adjusts from its time, as on
    /// its `shot_reported` event. None when the shot's window is not known
    /// (an E65/E80 brew job without a target time); older files have none.
    pub in_grinder_range: Option<bool>,
    /// What the grinder aims for, when it said.
    pub target_time_ms: Option<u32>,
    pub target_weight_g: Option<f64>,
    /// Why it was not a success, in words.
    pub reason: Option<String>,
    /// The same as a fixed code, for the app to match on. Older files have
    /// none and get it from `reason` when read.
    pub cause: Option<AbortCause>,
}

/// A shot as files keep it, including those written by earlier builds:
/// `at`, `kind`, and the times in seconds (`time_s`, `target_time_s`).
#[derive(Deserialize)]
struct StoredShot {
    id: String,
    #[serde(alias = "at")]
    started_at: String,
    ended_at: String,
    #[serde(alias = "kind")]
    outcome: ShotOutcome,
    #[serde(default)]
    running_ms: Option<u64>,
    time_ms: Option<u32>,
    time_s: Option<f64>,
    weight_g: Option<f64>,
    in_grinder_range: Option<bool>,
    target_time_ms: Option<u32>,
    target_time_s: Option<f64>,
    target_weight_g: Option<f64>,
    reason: Option<String>,
    #[serde(default, deserialize_with = "known_cause")]
    cause: Option<AbortCause>,
}

impl From<StoredShot> for Shot {
    fn from(s: StoredShot) -> Self {
        let ms = |secs: Option<f64>| {
            secs.filter(|s| s.is_finite() && *s >= 0.0)
                .map(|s| (s * 1000.0).round().min(f64::from(u32::MAX)) as u32)
        };
        // Saved before it was kept: the shot's two times are its length.
        let running_ms = s.running_ms.or_else(|| {
            let span = super::utc::seconds(&s.ended_at)?
                .checked_sub(super::utc::seconds(&s.started_at)?)?;
            Some(span * 1000)
        });
        Self {
            id: s.id,
            started_at: s.started_at,
            ended_at: s.ended_at,
            outcome: s.outcome,
            running_ms,
            time_ms: s.time_ms.or_else(|| ms(s.time_s)),
            weight_g: s.weight_g,
            in_grinder_range: s.in_grinder_range,
            target_time_ms: s.target_time_ms.or_else(|| ms(s.target_time_s)),
            target_weight_g: s.target_weight_g,
            reason: s.reason,
            cause: s.cause,
        }
    }
}

/// A cause this version does not know (written by a newer one) reads as
/// none, so the shot is kept with its `reason`. `use_file` writes the file
/// back at once, so after going back to an older version such a code is
/// saved as `null`, and after the next upgrade it comes back only if
/// [`cause_of`] still knows the shot's words: `cause_of` and the old words
/// stay for good.
fn known_cause<'de, D: Deserializer<'de>>(d: D) -> Result<Option<AbortCause>, D::Error> {
    let code = Option::<serde_json::Value>::deserialize(d)?;
    Ok(code.and_then(|c| serde_json::from_value(c).ok()))
}

/// The cause of a shot saved before `cause` was kept, from its `reason`.
/// So far each reason is worded as [`AbortCause::text`] words it; if one is
/// reworded, its old words go here (the tests list them).
fn cause_of(reason: &str) -> Option<AbortCause> {
    AbortCause::ALL.into_iter().find(|c| c.text() == reason)
}

/// The shots, and the file they are kept in.
#[derive(Default)]
pub struct History {
    shots: Mutex<VecDeque<Shot>>,
    file: OnceLock<PathBuf>,
    /// Whether the last save worked. Each save writes every shot, so the
    /// next one that works makes the file whole again.
    saved: AtomicBool,
}

#[derive(Serialize, Deserialize)]
struct HistoryFile {
    shots: VecDeque<Shot>,
}

impl History {
    /// Newest first.
    pub fn list(&self) -> Vec<Shot> {
        self.lock().iter().cloned().collect()
    }

    /// Whether the history survives a restart: the last save worked (a
    /// volume can fill up or turn read-only later). The file is set only
    /// once the first save worked, so without one this stays false.
    pub fn persistent(&self) -> bool {
        self.saved.load(Ordering::Relaxed)
    }

    /// Keeps the history in `path` from now on, starting with what it holds.
    /// A file that cannot be read as a history is set aside (`.bad`) and a
    /// new one is started; if it cannot be set aside or written, the history
    /// is kept in memory only.
    fn use_file(&self, path: &Path) -> anyhow::Result<()> {
        make_private(path);
        let loaded = match read_file(path)? {
            Ok(shots) => shots,
            Err(why) => {
                let mut bad = path.as_os_str().to_owned();
                bad.push(".bad");
                let bad = PathBuf::from(bad);
                if let Err(e) = std::fs::rename(path, &bad) {
                    tracing::warn!(
                        "{}: not a shot history ({why}), and setting it aside failed ({e}); \
                         the history is kept in memory until the next start",
                        path.display()
                    );
                    return Ok(());
                }
                tracing::warn!(
                    "{}: not a shot history ({why}), starting a new one; the old file is \
                     kept as {}",
                    path.display(),
                    bad.display()
                );
                VecDeque::new()
            }
        };
        let mut loaded = loaded;
        loaded.truncate(CAPACITY);
        // Written back at once, like the settings file, so a volume that
        // cannot be written shows as "memory only" from the start.
        let file = HistoryFile {
            shots: loaded.clone(),
        };
        match save(path, &file) {
            Ok(()) => {
                let _ = self.file.set(path.to_owned());
                self.saved.store(true, Ordering::Relaxed);
            }
            Err(e) => tracing::warn!(
                "shot history: {e:#}; the history is kept in memory until the next start"
            ),
        }
        *self.lock() = loaded;
        Ok(())
    }

    /// Adds a shot at the top and saves the file. Only the recording task
    /// adds, so writes never overlap; the file is small, so writing it right
    /// here (blocking) is fine.
    fn add(&self, shot: Shot) {
        let file = {
            let mut shots = self.lock();
            shots.push_front(shot);
            shots.truncate(CAPACITY);
            HistoryFile {
                shots: shots.clone(),
            }
        };
        let Some(path) = self.file.get() else {
            return;
        };
        let saved = match save(path, &file) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(
                    "shot history: {e:#}; the newest shots are kept in memory only until a \
                     save works again"
                );
                false
            }
        };
        let was_saved = self.saved.swap(saved, Ordering::Relaxed);
        if saved && !was_saved {
            tracing::info!("shot history: saved again, with every shot");
        }
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<Shot>> {
        self.shots.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The shots in `path` (none when there is no file), or why it is not a
/// history.
fn read_file(path: &Path) -> anyhow::Result<Result<VecDeque<Shot>, String>> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Ok(VecDeque::new())),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Ok(Err(format!("larger than {} MiB", MAX_FILE_BYTES >> 20)));
    }
    #[derive(Deserialize)]
    struct Raw {
        shots: Vec<serde_json::Value>,
    }
    let raw: Raw = match serde_json::from_slice(&bytes) {
        Ok(raw) => raw,
        Err(e) => return Ok(Err(e.to_string())),
    };
    // A shot this version cannot read (say, an outcome added later) is dropped,
    // not the whole file.
    let total = raw.shots.len();
    let shots: VecDeque<Shot> = raw
        .shots
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Shot>(v).ok())
        .map(|mut shot| {
            if shot.cause.is_none() {
                shot.cause = shot.reason.as_deref().and_then(cause_of);
            }
            shot
        })
        .collect();
    if shots.len() < total {
        tracing::warn!(
            "{}: dropping {} shots this version cannot read; the others are kept",
            path.display(),
            total - shots.len()
        );
    }
    Ok(Ok(shots))
}

fn save(path: &Path, file: &HistoryFile) -> anyhow::Result<()> {
    let json = serde_json::to_vec_pretty(file)?;
    save_private(path, &json)
}

/// What the last grind asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Target {
    time_ms: Option<u32>,
    weight_g: Option<f64>,
}

impl Target {
    fn of(result: &GrindResult) -> Self {
        Self {
            time_ms: None,
            weight_g: result.target_weight_g.filter(|g| g.is_finite() && *g > 0.0),
        }
    }
}

/// A shot the grinder asked for that has not ended yet.
#[derive(Debug, Clone)]
struct Open {
    /// When it started, on the machine's clock, which a clock step does not
    /// move.
    since: Instant,
    target: Target,
}

/// How a shot ended.
#[derive(Debug, Clone, Copy)]
enum End {
    /// With a shot result, and whether the grinder adjusts from its time.
    Shot {
        result: ShotResult,
        in_grinder_range: Option<bool>,
    },
    /// Without one, for this cause.
    Aborted(AbortCause),
}

/// Turns the machine's events into shots.
#[derive(Debug, Default)]
struct Recorder {
    /// The grind since the last shot started.
    grind: Option<Target>,
    open: Option<Open>,
}

impl Recorder {
    /// The finished shot, once `event` ends one. `at` is when the event
    /// happened on the machine's clock, and `wall` the same moment on the
    /// wall clock. Only a shot's end reads `wall`; its start is counted back
    /// from there with the machine's clock. Until it syncs, a computer's
    /// clock may still read the last shutdown time, or 1970 or the system's
    /// build date, and then steps forward: the later reading is the one to
    /// trust, and a step during the shot does not change how long it took.
    fn observe(&mut self, event: &MachineEvent, at: Instant, wall: SystemTime) -> Option<Shot> {
        match event {
            MachineEvent::GrinderGrindFinished { result } => {
                self.grind = Some(Target::of(result));
                None
            }
            // An E65/E80 says the time it aims for with its request.
            MachineEvent::GrinderShotRequested {
                target_time_ms: Some(ms),
                ..
            } => {
                self.grind.get_or_insert_default().time_ms = Some(*ms);
                None
            }
            MachineEvent::ShotStarted {
                requested_by_grinder,
                ..
            } => {
                // Ended when this one started, which can be hours later.
                let missed = self.open.take().map(|open| {
                    let end = End::Aborted(AbortCause::ShotEndMissed);
                    shot(&open, at, wall, end)
                });
                self.open = requested_by_grinder.then(|| Open {
                    since: at,
                    target: self.grind.take().unwrap_or_default(),
                });
                missed
            }
            MachineEvent::ShotReported {
                result,
                in_grinder_range,
                ..
            } => {
                let open = self.open.take()?;
                let end = End::Shot {
                    result: *result,
                    in_grinder_range: *in_grinder_range,
                };
                Some(shot(&open, at, wall, end))
            }
            MachineEvent::ShotAborted { cause, .. } => {
                let open = self.open.take()?;
                Some(shot(&open, at, wall, End::Aborted(*cause)))
            }
            MachineEvent::GrinderShotRequested { .. } | MachineEvent::ShotFinished => None,
        }
    }
}

/// The shot that ended at `ended` on the machine's clock and at
/// `wall` on the wall clock; one that ended for a cause also gets its words
/// as `reason`.
fn shot(open: &Open, ended: Instant, wall: SystemTime, end: End) -> Shot {
    let running = ended.saturating_duration_since(open.since);
    // Whole seconds, as the history keeps its times: `started_at` and
    // `ended_at` are then exactly this far apart, so a 3:00 timeout does not
    // show as 3:01.
    let took = Duration::from_secs(running.as_secs());
    let (outcome, result, in_grinder_range, cause) = match end {
        End::Shot {
            result,
            in_grinder_range,
        } => (ShotOutcome::Succeeded, Some(result), in_grinder_range, None),
        End::Aborted(cause) => (ShotOutcome::of(cause), None, None, Some(cause)),
    };
    Shot {
        id: uuid::Uuid::new_v4().simple().to_string(),
        started_at: super::utc::with_seconds(wall.checked_sub(took).unwrap_or(wall)),
        ended_at: super::utc::with_seconds(wall),
        outcome,
        running_ms: Some(u64::try_from(running.as_millis()).unwrap_or(u64::MAX)),
        time_ms: result.map(|r| r.time_ms),
        weight_g: result.map(|r| r.weight_g),
        in_grinder_range,
        target_time_ms: open.target.time_ms,

        target_weight_g: open.target.weight_g,
        reason: cause.map(|c| c.text().to_owned()),
        cause,
    }
}

/// The wall-clock time of a machine event.
fn wall_time(at: Instant) -> SystemTime {
    let now = SystemTime::now();
    now.checked_sub(Instant::now().saturating_duration_since(at))
        .unwrap_or(now)
}

impl Server {
    /// The shots so far.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Records the shots from now on, kept in `file` (`history.json` next to
    /// the settings file) when given.
    pub fn record_history(self: &Arc<Self>, file: Option<&Path>) -> anyhow::Result<()> {
        if let Some(path) = file {
            self.history.use_file(path)?;
        }
        // Subscribed before the task runs, so no event slips through.
        let events = self.subscribe();
        tokio::spawn(record(self.clone(), events));
        Ok(())
    }
}

async fn record(server: Arc<Server>, mut events: broadcast::Receiver<EventRecord>) {
    let mut recorder = Recorder::default();
    let mut lagged = false;
    loop {
        match events.recv().await {
            Ok(rec) => {
                if let Some(shot) = recorder.observe(&rec.event, rec.at, wall_time(rec.at)) {
                    server.history.add(shot);
                }
            }
            Err(RecvError::Lagged(n)) => {
                // What was open cannot be finished correctly any more.
                recorder = Recorder::default();
                if !std::mem::replace(&mut lagged, true) {
                    tracing::warn!(
                        "shot history: missed {n} machine events, a shot may be left out"
                    );
                }
            }
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GrinderRange;
    use serde_json::{Value, json};
    use std::sync::LazyLock;
    use std::time::UNIX_EPOCH;

    /// When the tests started, on the machine's clock.
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);

    /// `secs` after the start, on the machine's clock.
    fn at(secs: u64) -> Instant {
        *START + Duration::from_secs(secs)
    }

    /// A wall clock in step with it: 2026-10-11T12:00:00Z at the start.
    fn wall(at: Instant) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_791_720_000) + at.duration_since(*START)
    }

    impl Recorder {
        /// [`Recorder::observe`] with the wall clock in step.
        fn feed(&mut self, event: &MachineEvent, at: Instant) -> Option<Shot> {
            self.observe(event, at, wall(at))
        }
    }

    /// A brew that started at `at(0)`.
    fn open() -> Open {
        Open {
            since: at(0),
            target: Target::default(),
        }
    }

    /// The shot of [`open`] that ended at `at(secs)`, with the wall clock in
    /// step.
    fn ended(secs: u64, end: End) -> Shot {
        shot(&open(), at(secs), wall(at(secs)), end)
    }

    /// A shot of `secs` seconds and 36 g, after an E64 WS knob press.
    fn success(secs: u64) -> End {
        let result = ShotResult::new(Duration::from_secs(secs), 36.0);
        End::Shot {
            result,
            in_grinder_range: Some(GrinderRange::E64.contains(result.time_ms)),
        }
    }

    /// A folder of its own under the system's temp directory, not created.
    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("gbs-anywhere-history-{}", uuid::Uuid::new_v4()))
    }

    fn grind(weight: f64) -> GrindResult {
        GrindResult::parse(format!(r#"{{"SYNC_BEVERAGE_WEIGHT":{weight}}}"#).as_bytes()).unwrap()
    }

    fn started(by_grinder: bool) -> MachineEvent {
        MachineEvent::ShotStarted {
            requested_by_grinder: by_grinder,
            human_action: "",
        }
    }

    /// A shot of `ms` and `grams`, checked against the E64 WS's window.
    fn reported(ms: u32, grams: f64) -> MachineEvent {
        let result = ShotResult::new(Duration::from_millis(ms.into()), grams);
        MachineEvent::ShotReported {
            result,
            in_grinder_range: Some(GrinderRange::E64.contains(result.time_ms)),
            // Not read: the history counts from the request itself.
            running_ms: 0,
        }
    }

    /// The event for a brew that ended for `cause`; how long it ran is not
    /// read, the history counts that from the request itself.
    fn aborted(cause: AbortCause) -> MachineEvent {
        MachineEvent::shot_aborted(cause, Duration::ZERO)
    }

    #[test]
    fn records_grinder_brews_only() {
        let mut r = Recorder::default();
        // An E65/E80 job: target time and weight from the job, as the grind
        // and the request.
        let grind_finished = MachineEvent::GrinderGrindFinished {
            result: grind(40.0),
        };
        let requested = MachineEvent::GrinderShotRequested {
            script_id: None,
            target_time_ms: Some(28_000),
        };
        assert_eq!(r.feed(&grind_finished, at(0)), None);
        assert_eq!(r.feed(&requested, at(0)), None);
        assert_eq!(r.feed(&started(true), at(0)), None);

        let shot = r.feed(&reported(28_500, 40.9), at(40)).unwrap();
        assert_eq!(shot.outcome, ShotOutcome::Succeeded);
        assert_eq!(shot.running_ms, Some(40_000));
        assert_eq!(
            (
                shot.time_ms,
                shot.weight_g,
                shot.target_time_ms,
                shot.target_weight_g
            ),
            (Some(28_500), Some(40.9), Some(28_000), Some(40.0))
        );
        assert_eq!(
            (shot.started_at.as_str(), shot.ended_at.as_str()),
            ("2026-10-11T12:00:00Z", "2026-10-11T12:00:40Z")
        );
        assert_eq!((shot.reason, shot.cause), (None, None));
        assert_eq!(shot.in_grinder_range, Some(true));

        // A flush started by hand is no shot.
        assert_eq!(r.feed(&started(false), at(60)), None);
        assert_eq!(r.feed(&reported(5_000, 20.0), at(65)), None);

        // An E64 WS knob press after a grind result: the recipe weight, no
        // target time.
        let e64 = MachineEvent::GrinderGrindFinished {
            result: grind(36.0),
        };
        r.feed(&e64, at(100));
        r.feed(
            &MachineEvent::GrinderShotRequested {
                script_id: Some(9),
                target_time_ms: None,
            },
            at(110),
        );
        r.feed(&started(true), at(110));
        let abort = aborted(AbortCause::ShotAbortedByUser);
        let shot = r.feed(&abort, at(120)).unwrap();
        assert_eq!(shot.running_ms, Some(10_000), "from the request");
        assert_eq!(shot.outcome, ShotOutcome::Aborted);
        assert_eq!((shot.time_ms, shot.target_time_ms), (None, None));
        assert_eq!(shot.target_weight_g, Some(36.0));
        assert_eq!(shot.reason.as_deref(), Some("aborted by user"));
        assert_eq!(shot.cause, Some(AbortCause::ShotAbortedByUser));
        assert_eq!(shot.in_grinder_range, None, "only for a success");

        // A knob press without a new grind: the target is not known.
        assert_eq!(r.feed(&started(true), at(200)), None);
        let timeout = aborted(AbortCause::ShotTimedOut);
        let shot = r.feed(&timeout, at(380)).unwrap();
        assert_eq!(shot.outcome, ShotOutcome::Failed);
        assert_eq!(shot.target_weight_g, None);
    }

    #[test]
    fn records_whether_the_grinder_adjusts_from_the_shot() {
        let mut r = Recorder::default();
        for (ms, adjusts) in [
            (8_000, false),
            (10_000, false),
            (28_500, true),
            (95_000, false),
        ] {
            r.feed(&started(true), at(0));
            let shot = r.feed(&reported(ms, 36.0), at(100)).unwrap();
            assert_eq!(shot.outcome, ShotOutcome::Succeeded, "{ms} ms");
            assert_eq!(shot.in_grinder_range, Some(adjusts), "{ms} ms");
        }
    }

    #[test]
    fn a_clock_step_does_not_change_how_long_a_brew_took() {
        // Until it syncs, a clock may still read the last shutdown time, or
        // 1970 or the system's build date, and then steps forward. The end
        // is read when the brew ends, and the start counted back from it.
        let mut r = Recorder::default();
        let unset = UNIX_EPOCH + Duration::from_secs(95);
        let shutdown = wall(at(0)) - Duration::from_secs(3 * 86_400);

        // Set during the brew, from the last shutdown time: the right times.
        r.observe(&started(true), at(0), shutdown);
        let shot = r.feed(&reported(28_500, 36.0), at(40)).unwrap();
        assert_eq!(
            (shot.started_at.as_str(), shot.ended_at.as_str()),
            ("2026-10-11T12:00:00Z", "2026-10-11T12:00:40Z")
        );

        // Set during a timeout, from 1970: the right day, 3 minutes long.
        r.observe(&started(true), at(100), unset);
        let timeout = aborted(AbortCause::ShotTimedOut);
        let shot = r.feed(&timeout, at(280)).unwrap();
        assert_eq!(
            (shot.started_at.as_str(), shot.ended_at.as_str()),
            ("2026-10-11T12:01:40Z", "2026-10-11T12:04:40Z")
        );

        // A brew whose end was missed ends when the next one starts, maybe
        // hours later, with the clock set by then: dated right too.
        r.observe(&started(true), at(300), unset);
        let missed = r.feed(&started(true), at(4 * 3600)).unwrap();
        assert_eq!(
            (missed.started_at.as_str(), missed.ended_at.as_str()),
            ("2026-10-11T12:05:00Z", "2026-10-11T16:00:00Z")
        );

        // Still unset when the brew ends: the shot is dated in 1970, with
        // its length right.
        r.observe(&started(true), at(5 * 3600), unset);
        let shot = r.observe(
            &reported(30_000, 36.0),
            at(5 * 3600 + 32),
            unset + Duration::from_secs(32),
        );
        let shot = shot.unwrap();
        assert_eq!(
            (shot.started_at.as_str(), shot.ended_at.as_str()),
            ("1970-01-01T00:01:35Z", "1970-01-01T00:02:07Z")
        );
    }

    #[test]
    fn a_brew_is_kept_in_whole_seconds() {
        // A brew that ended 0.05 s into a second, after a timeout that came
        // on the machine's next tick, 0.25 s after the 3 minutes. Read from
        // the clock at both ends, it started 0.8 s into a second, and the
        // two times cut to whole seconds would show 3:01. Counted back from
        // the end in whole seconds: 3:00.
        let mut r = Recorder::default();
        let end = at(180) + Duration::from_millis(250);
        let end_wall = wall(at(0)) + Duration::from_millis(181_050);
        r.observe(
            &started(true),
            at(0),
            end_wall - Duration::from_millis(180_250),
        );
        let timeout = aborted(AbortCause::ShotTimedOut);
        let shot = r.observe(&timeout, end, end_wall).unwrap();
        assert_eq!(
            (shot.started_at.as_str(), shot.ended_at.as_str()),
            ("2026-10-11T12:00:01Z", "2026-10-11T12:03:01Z")
        );
        // How long it ran stays to the millisecond.
        assert_eq!(shot.running_ms, Some(180_250));
    }

    #[test]
    fn abort_causes_are_sorted() {
        for (cause, kind) in [
            (AbortCause::ShotAbortedByUser, ShotOutcome::Aborted),
            (AbortCause::GrinderShotReplaced, ShotOutcome::Aborted),
            (AbortCause::GrinderAddressChanged, ShotOutcome::Aborted),
            (AbortCause::ShotTimedOut, ShotOutcome::Failed),
            (AbortCause::GrinderShotDropped, ShotOutcome::Failed),
            (AbortCause::GrinderShotRefused, ShotOutcome::Failed),
            (AbortCause::GrinderUnpaired, ShotOutcome::Failed),
        ] {
            let mut r = Recorder::default();
            r.feed(&started(true), at(0));
            let shot = r.feed(&aborted(cause), at(1));
            let shot = shot.unwrap();
            assert_eq!((shot.outcome, shot.cause), (kind, Some(cause)));
            assert_eq!(shot.reason.as_deref(), Some(cause.text()));
        }
    }

    #[test]
    fn a_brew_whose_end_was_missed_is_an_error() {
        let mut r = Recorder::default();
        r.feed(&started(true), at(0));
        let missed = r.feed(&started(true), at(100)).unwrap();
        assert_eq!(missed.outcome, ShotOutcome::Failed);
        assert_eq!(missed.running_ms, Some(100_000), "until the next one");
        assert_eq!(missed.cause, Some(AbortCause::ShotEndMissed));
        assert_eq!(
            missed.reason.as_deref(),
            Some("the next brew started before this one ended")
        );
        assert_eq!(
            r.feed(&reported(25_000, 36.0), at(130)).unwrap().outcome,
            ShotOutcome::Succeeded
        );
    }

    #[test]
    fn keeps_the_newest_in_the_file() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        let h = History::default();
        h.use_file(&path).unwrap();
        assert!(h.persistent());
        for i in 0..CAPACITY + 5 {
            let mut s = ended(100, success(i as u64));
            s.id = format!("s{i}");
            h.add(s);
        }
        assert_eq!(h.list().len(), CAPACITY);
        assert_eq!(h.list()[0].id, format!("s{}", CAPACITY + 4), "newest first");

        let again = History::default();
        again.use_file(&path).unwrap();
        assert_eq!(again.list(), h.list());

        // A damaged file is set aside, not fatal.
        std::fs::write(&path, "{ not json").unwrap();
        let fresh = History::default();
        fresh.use_file(&path).unwrap();
        assert!(fresh.list().is_empty());
        assert!(dir.join("history.json.bad").exists());

        // So is one too large to be a history, even if it is valid JSON.
        std::fs::remove_file(dir.join("history.json.bad")).unwrap();
        let mut big = b"{\"shots\":[]".to_vec();
        big.resize(MAX_FILE_BYTES as usize + 1, b' ');
        big.push(b'}');
        std::fs::write(&path, big).unwrap();
        let fresh = History::default();
        fresh.use_file(&path).unwrap();
        assert!(fresh.persistent());
        assert!(dir.join("history.json.bad").exists());

        // Owner only, like the settings file.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_shot_this_version_cannot_read_is_left_out_not_the_file() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        let known = serde_json::to_value(ended(30, success(28))).unwrap();
        let mut newer = known.clone();
        newer["outcome"] = "partial".into();
        newer["id"] = "from-a-newer-version".into();
        std::fs::write(
            &path,
            serde_json::json!({ "shots": [newer, known] }).to_string(),
        )
        .unwrap();
        let h = History::default();
        h.use_file(&path).unwrap();
        assert_eq!(h.list().len(), 1);
        assert_eq!(h.list()[0].time_ms, Some(28_000));
        assert!(!dir.join("history.json.bad").exists(), "not set aside");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn older_shots_get_their_cause_from_the_reason() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        // The reasons as saved before `cause` was kept: these words stay.
        let older = [
            ("aborted by user", Some("shot_aborted_by_user")),
            (
                "a newer Grind-by-Sync grind replaced it",
                Some("grinder_shot_replaced"),
            ),
            (
                "the grinder address changed",
                Some("grinder_address_changed"),
            ),
            ("no result reported in time", Some("shot_timed_out")),
            (
                "the grinder stopped waiting before the shot came",
                Some("grinder_shot_dropped"),
            ),
            ("the grinder refused the job", Some("grinder_shot_refused")),
            (
                "the grinder no longer knows this gbs-anywhere",
                Some("grinder_unpaired"),
            ),
            (
                "the next brew started before this one ended",
                Some("shot_end_missed"),
            ),
            ("a reason this version does not know", None),
        ];
        let mut shots: Vec<_> = older
            .iter()
            .map(|(reason, _)| {
                let error = ended(180, End::Aborted(AbortCause::ShotTimedOut));
                let mut v = serde_json::to_value(error).unwrap();
                v.as_object_mut().unwrap().remove("cause");
                v.as_object_mut().unwrap().remove("in_grinder_range");
                v["reason"] = (*reason).into();
                v
            })
            .collect();
        // A cause from a newer version reads as none: the shot stays, and
        // its reason gives it a cause this version knows.
        let mut newer = shots[0].clone();
        newer["cause"] = "a_cause_added_later".into();
        shots.push(newer);
        std::fs::write(&path, serde_json::json!({ "shots": shots }).to_string()).unwrap();

        let h = History::default();
        h.use_file(&path).unwrap();
        let causes: Vec<_> = h
            .list()
            .iter()
            .map(|s| s.cause.map(|c| serde_json::to_value(c).unwrap()))
            .collect();
        let mut expected: Vec<_> = older.iter().map(|(_, c)| c.map(Value::from)).collect();
        expected.push(Some("shot_aborted_by_user".into()));
        assert_eq!(causes, expected);
        // Written back with them, so the words no longer matter.
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["shots"][3]["cause"], "shot_timed_out");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn shots_from_earlier_builds_still_load() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        // Shots as saved before `cause` and `in_grinder_range` were kept,
        // with the names of then (`at`, `kind`, times in seconds) and the
        // kinds of then (`success`, `error`). Whether the grinder took the
        // success is not known, and stays so.
        let older = r#"{"shots":[{"id":"older","at":"2026-10-11T12:00:00Z",
            "ended_at":"2026-10-11T12:00:30Z","kind":"success","time_s":8.25,
            "weight_g":18.0,"target_time_s":28.0,"target_weight_g":36.0,
            "reason":null},{"id":"older-error","at":"2026-10-11T11:00:00Z",
            "ended_at":"2026-10-11T11:03:00Z","kind":"error","time_s":null,
            "weight_g":null,"target_time_s":null,"target_weight_g":null,
            "reason":"no result reported in time"},{"id":"older-abort",
            "at":"2026-10-11T10:00:00Z","ended_at":"2026-10-11T10:00:20Z",
            "kind":"aborted","time_s":null,"weight_g":null,
            "target_time_s":null,"target_weight_g":null,
            "reason":"aborted by user"}]}"#;
        std::fs::write(&path, older).unwrap();
        let h = History::default();
        h.use_file(&path).unwrap();
        let shots = h.list();
        let outcomes: Vec<_> = shots.iter().map(|s| s.outcome).collect();
        assert_eq!(
            outcomes,
            [
                ShotOutcome::Succeeded,
                ShotOutcome::Failed,
                ShotOutcome::Aborted
            ]
        );
        assert_eq!(
            (
                shots[0].time_ms,
                shots[0].target_time_ms,
                shots[0].in_grinder_range
            ),
            (Some(8_250), Some(28_000), None)
        );
        assert_eq!(shots[0].started_at, "2026-10-11T12:00:00Z");
        assert_eq!(shots[1].cause, Some(AbortCause::ShotTimedOut));
        // How long each ran, from its two times.
        let running: Vec<_> = shots.iter().map(|s| s.running_ms).collect();
        assert_eq!(running, [Some(30_000), Some(180_000), Some(20_000)]);
        // Written back with the names of now.
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let saved_outcomes: Vec<_> = saved["shots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["outcome"].as_str().unwrap())
            .collect();
        assert_eq!(saved_outcomes, ["succeeded", "failed", "aborted"]);
        let first = &saved["shots"][0];
        assert_eq!(first["in_grinder_range"], Value::Null);
        assert_eq!(
            (
                &first["started_at"],
                &first["time_ms"],
                &first["target_time_ms"]
            ),
            (
                &json!("2026-10-11T12:00:00Z"),
                &json!(8_250),
                &json!(28_000)
            )
        );
        for old in ["at", "kind", "time_s", "target_time_s"] {
            assert!(first.get(old).is_none(), "{old}");
        }
        assert_eq!(first["running_ms"], 30_000);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_history_that_cannot_be_written_is_not_called_kept() {
        // The folder does not exist, so the file cannot be written.
        let h = History::default();
        h.use_file(&temp_dir().join("history.json")).unwrap();
        assert!(!h.persistent());
    }

    #[test]
    fn a_save_that_fails_later_is_not_called_kept() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        let h = History::default();
        h.use_file(&path).unwrap();
        assert!(h.persistent());

        // The folder is gone (say, a volume that came off): the next save
        // fails, and the shot is kept in memory only.
        std::fs::remove_dir_all(&dir).unwrap();
        h.add(ended(30, success(30)));
        assert!(!h.persistent());
        assert_eq!(h.list().len(), 1);

        // The next save that works writes every shot again.
        std::fs::create_dir_all(&dir).unwrap();
        h.add(ended(30, success(28)));
        assert!(h.persistent());
        let again = History::default();
        again.use_file(&path).unwrap();
        assert_eq!(again.list(), h.list());
        assert_eq!(again.list().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
