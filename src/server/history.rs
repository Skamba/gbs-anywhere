//! The shots the grinder asked for, newest first: when, how they ended and
//! the numbers sent (`GET /api/history`). Kept in `history.json` next to the
//! settings file, or in memory without one.
//!
//! A brew counts from the grinder's request (the E64 WS's knob press, an
//! E65/E80 brew job) to its end: a reported shot, or an abort. A brew
//! started by hand (a flush) is not a shot and is left out.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Instant, SystemTime};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::{self, error::RecvError};

use crate::protocol::machine::{ABORTED_BY_USER, EventRecord, REPLACED_BY_NEWER_GRIND};
use crate::protocol::{GrindResult, MachineEvent};
use crate::server::Server;
use crate::server::settings::GRINDER_CHANGED;

/// Shots kept.
pub const CAPACITY: usize = 200;

/// Far more than [`CAPACITY`] shots take: a bigger file is not a history.
const MAX_FILE_BYTES: u64 = 4 << 20;

/// Why a brew still open when the grinder asked for the next one is listed
/// as an error (its end was missed).
const NO_END: &str = "the next brew started before this one ended";

/// Abort reasons that were someone's choice rather than a failure.
const ON_PURPOSE: [&str; 3] = [ABORTED_BY_USER, REPLACED_BY_NEWER_GRIND, GRINDER_CHANGED];

/// How a shot ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShotKind {
    /// Measured and sent to the grinder.
    Success,
    /// Ended on purpose: aborted by a person, replaced by a newer grind, or
    /// the grinder address changed.
    Aborted,
    /// Nothing came in time, or the grinder stopped waiting.
    Error,
}

/// One shot in the history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    pub id: String,
    /// When the grinder asked for it (UTC).
    pub at: String,
    pub ended_at: String,
    pub kind: ShotKind,
    /// Only for a success.
    pub time_s: Option<f64>,
    pub weight_g: Option<f64>,
    /// What the grinder aims for, when it said.
    pub target_time_s: Option<f64>,
    pub target_weight_g: Option<f64>,
    /// Why it was not a success.
    pub reason: Option<String>,
}

/// The shots, and the file they are kept in.
#[derive(Default)]
pub struct History {
    shots: Mutex<VecDeque<Shot>>,
    file: OnceLock<PathBuf>,
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

    /// Whether the history survives a restart.
    pub fn persistent(&self) -> bool {
        self.file.get().is_some()
    }

    /// Keeps the history in `path` from now on, starting with what it holds.
    /// A file that cannot be read as a history is set aside (`.bad`) and a
    /// new one is started; if it cannot be set aside or written, the history
    /// is kept in memory only.
    fn use_file(&self, path: &Path) -> anyhow::Result<()> {
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
        if let Err(e) = save(path, &file) {
            tracing::warn!("shot history: {e:#}");
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
    // A shot this version cannot read (say, a kind added later) is dropped,
    // not the whole file.
    let total = raw.shots.len();
    let shots: VecDeque<Shot> = raw
        .shots
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
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
    // Write next to it and rename, so a crash never leaves half a file.
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let write = |bytes: &[u8]| -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        // Owner only, like the settings file: it shows when someone is home.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    };
    write(&json).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

/// What the last grind asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Target {
    time_s: Option<f64>,
    weight_g: Option<f64>,
}

impl Target {
    fn of(result: &GrindResult, target_time_ms: Option<u32>) -> Self {
        Self {
            time_s: target_time_ms.map(|ms| f64::from(ms) / 1000.0),
            weight_g: result
                .beverage_weight_g
                .filter(|g| g.is_finite() && *g > 0.0),
        }
    }
}

/// A brew the grinder asked for that has not ended yet.
#[derive(Debug, Clone)]
struct Open {
    at: SystemTime,
    target: Target,
}

/// Turns the machine's events into shots.
#[derive(Debug, Default)]
struct Recorder {
    /// The grind since the last brew started.
    grind: Option<Target>,
    open: Option<Open>,
}

impl Recorder {
    /// The finished shot, once `event` (which happened at `at`) ends one.
    fn observe(&mut self, event: &MachineEvent, at: SystemTime) -> Option<Shot> {
        match event {
            MachineEvent::GrindResult { result } => {
                self.grind = Some(Target::of(result, None));
                None
            }
            MachineEvent::BrewJob {
                result,
                target_time_ms,
            } => {
                self.grind = Some(Target::of(result, *target_time_ms));
                None
            }
            MachineEvent::BrewStarted {
                requested_by_grinder,
                ..
            } => {
                let missed = self.open.take().map(|open| {
                    let reason = Some(NO_END.to_owned());
                    shot(&open, at, ShotKind::Error, None, None, reason)
                });
                self.open = requested_by_grinder.then(|| Open {
                    at,
                    target: self.grind.take().unwrap_or_default(),
                });
                missed
            }
            MachineEvent::ShotReported { result, .. } => {
                let open = self.open.take()?;
                Some(shot(
                    &open,
                    at,
                    ShotKind::Success,
                    Some(f64::from(result.time_ms) / 1000.0),
                    Some(result.volume_ml),
                    None,
                ))
            }
            MachineEvent::ShotAborted { reason } => {
                let open = self.open.take()?;
                let kind = if ON_PURPOSE.contains(reason) {
                    ShotKind::Aborted
                } else {
                    ShotKind::Error
                };
                Some(shot(&open, at, kind, None, None, Some(reason.to_string())))
            }
            MachineEvent::StartRequested { .. } | MachineEvent::Ready => None,
        }
    }
}

fn shot(
    open: &Open,
    ended: SystemTime,
    kind: ShotKind,
    time_s: Option<f64>,
    weight_g: Option<f64>,
    reason: Option<String>,
) -> Shot {
    Shot {
        id: uuid::Uuid::new_v4().simple().to_string(),
        at: super::utc::with_seconds(open.at),
        ended_at: super::utc::with_seconds(ended),
        kind,
        time_s,
        weight_g,
        target_time_s: open.target.time_s,
        target_weight_g: open.target.weight_g,
        reason,
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
                if let Some(shot) = recorder.observe(&rec.event, wall_time(rec.at)) {
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
    use crate::protocol::ShotResult;
    use std::time::{Duration, UNIX_EPOCH};

    fn grind(weight: f64) -> GrindResult {
        GrindResult::parse(format!(r#"{{"SYNC_BEVERAGE_WEIGHT":{weight}}}"#).as_bytes()).unwrap()
    }

    fn started(by_grinder: bool) -> MachineEvent {
        MachineEvent::BrewStarted {
            requested_by_grinder: by_grinder,
            human_action: "",
        }
    }

    fn reported(ms: u32, grams: f64) -> MachineEvent {
        let result = ShotResult::new(Duration::from_millis(ms.into()), grams);
        MachineEvent::ShotReported {
            result,
            in_grinder_range: true,
        }
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_791_720_000 + secs)
    }

    #[test]
    fn records_grinder_brews_only() {
        let mut r = Recorder::default();
        // An E65/E80 job: target time and weight from the job.
        let job = MachineEvent::BrewJob {
            result: grind(40.0),
            target_time_ms: Some(28_000),
        };
        assert_eq!(r.observe(&job, at(0)), None);
        assert_eq!(r.observe(&started(true), at(0)), None);
        let shot = r.observe(&reported(28_500, 40.9), at(40)).unwrap();
        assert_eq!(shot.kind, ShotKind::Success);
        assert_eq!(
            (
                shot.time_s,
                shot.weight_g,
                shot.target_time_s,
                shot.target_weight_g
            ),
            (Some(28.5), Some(40.9), Some(28.0), Some(40.0))
        );
        assert_eq!(
            (shot.at.as_str(), shot.ended_at.as_str()),
            ("2026-10-11T12:00:00Z", "2026-10-11T12:00:40Z")
        );
        assert_eq!(shot.reason, None);

        // A flush started by hand is no shot.
        assert_eq!(r.observe(&started(false), at(60)), None);
        assert_eq!(r.observe(&reported(5_000, 20.0), at(65)), None);

        // An E64 WS knob press after a grind result: the recipe weight, no
        // target time.
        let e64 = MachineEvent::GrindResult {
            result: grind(36.0),
        };
        r.observe(&e64, at(100));
        r.observe(
            &MachineEvent::StartRequested { script_id: Some(9) },
            at(110),
        );
        r.observe(&started(true), at(110));
        let reason = ABORTED_BY_USER;
        let shot = r
            .observe(&MachineEvent::ShotAborted { reason }, at(120))
            .unwrap();
        assert_eq!(shot.kind, ShotKind::Aborted);
        assert_eq!((shot.time_s, shot.target_time_s), (None, None));
        assert_eq!(shot.target_weight_g, Some(36.0));
        assert_eq!(shot.reason.as_deref(), Some("aborted by user"));

        // A knob press without a new grind: the target is not known.
        assert_eq!(r.observe(&started(true), at(200)), None);
        let reason = "no result reported in time";
        let shot = r
            .observe(&MachineEvent::ShotAborted { reason }, at(380))
            .unwrap();
        assert_eq!(shot.kind, ShotKind::Error);
        assert_eq!(shot.target_weight_g, None);
    }

    #[test]
    fn abort_reasons_are_sorted() {
        for (reason, kind) in [
            (ABORTED_BY_USER, ShotKind::Aborted),
            (REPLACED_BY_NEWER_GRIND, ShotKind::Aborted),
            ("no result reported in time", ShotKind::Error),
            (
                "the grinder stopped waiting before the shot came",
                ShotKind::Error,
            ),
            ("the grinder refused the job", ShotKind::Error),
            (GRINDER_CHANGED, ShotKind::Aborted),
        ] {
            let mut r = Recorder::default();
            r.observe(&started(true), at(0));
            let shot = r.observe(&MachineEvent::ShotAborted { reason }, at(1));
            assert_eq!(shot.unwrap().kind, kind, "{reason}");
        }
    }

    #[test]
    fn a_brew_whose_end_was_missed_is_an_error() {
        let mut r = Recorder::default();
        r.observe(&started(true), at(0));
        let missed = r.observe(&started(true), at(100)).unwrap();
        assert_eq!(missed.kind, ShotKind::Error);
        assert_eq!(missed.reason.as_deref(), Some(NO_END));
        assert_eq!(
            r.observe(&reported(25_000, 36.0), at(130)).unwrap().kind,
            ShotKind::Success
        );
    }

    #[test]
    fn keeps_the_newest_in_the_file() {
        let dir =
            std::env::temp_dir().join(format!("gbs-anywhere-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        let h = History::default();
        h.use_file(&path).unwrap();
        assert!(h.persistent());
        let open = Open {
            at: at(0),
            target: Target::default(),
        };
        for i in 0..CAPACITY + 5 {
            let mut s = shot(&open, at(1), ShotKind::Success, Some(i as f64), None, None);
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
        let dir =
            std::env::temp_dir().join(format!("gbs-anywhere-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        let open = Open {
            at: at(0),
            target: Target::default(),
        };
        let known = serde_json::to_value(shot(
            &open,
            at(1),
            ShotKind::Success,
            Some(28.0),
            None,
            None,
        ))
        .unwrap();
        let mut newer = known.clone();
        newer["kind"] = "partial".into();
        newer["id"] = "from-a-newer-version".into();
        std::fs::write(
            &path,
            serde_json::json!({ "shots": [newer, known] }).to_string(),
        )
        .unwrap();
        let h = History::default();
        h.use_file(&path).unwrap();
        assert_eq!(h.list().len(), 1);
        assert_eq!(h.list()[0].time_s, Some(28.0));
        assert!(!dir.join("history.json.bad").exists(), "not set aside");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_history_that_cannot_be_written_is_not_called_kept() {
        let dir =
            std::env::temp_dir().join(format!("gbs-anywhere-history-{}", uuid::Uuid::new_v4()));
        // The folder does not exist, so the file cannot be written.
        let h = History::default();
        h.use_file(&dir.join("history.json")).unwrap();
        assert!(!h.persistent());
    }
}
