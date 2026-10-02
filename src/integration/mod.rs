//! Machine integrations: anything that watches an espresso machine (a vendor
//! cloud, a home-automation hub, a scale, ...) and reports the shot to the
//! grinder so nobody has to type it.
//!
//! An integration is a long-running task. It gets a [`Link`] to the machine
//! emulation, which tells it when the grinder is waiting for a shot
//! ([`Link::grinder_brews`]) and takes the measured numbers
//! ([`Link::report`]), and a [`Status`] to tell people how it is doing (shown
//! in the app and in `GET /api/state`). Everything in between, protocols and
//! logins and reconnects, is the integration's own business.
//!
//! Each kind of integration is a [`Kind`]: a name, an icon, a setup form and a way to
//! build the integration from that form. People add one in the app with the
//! **+** button (saved in the `--config` file) or on the command line;
//! [`Integrations`] runs them all, so several can run side by side. The
//! first one to report a shot wins.
//!
//! Included: [`la_marzocco`], which reads the La Marzocco cloud's coffee log,
//! and [`precisa`], which weighs and times the shot with a Eureka Precisa
//! scale over Bluetooth.
//!
//! A pump sensor integration (none included yet) can share what it sees
//! through [`Link::set_pump`]; the scale follows it with [`Link::pump`] to
//! time the shot by the pump and adds the weight. With nobody listening
//! ([`Link::pump_listened`]) a sensor would report the time itself. Without a
//! sensor the pump state stays unsensed and the scale times shots alone.
//!
//! Integrations that see the shot as it happens (a scale) can also watch
//! brews started by hand ([`Link::brews`]) as tests, end them with what they
//! measured ([`Link::finish_manual`]), and show live readings in the app
//! ([`Status::live`]).
//!
//! # Adding one
//!
//! Every integration is a folder `src/integration/<id>/` with this layout:
//!
//! | file | what |
//! |---|---|
//! | `mod.rs` | module docs and `pub static KIND: Kind`, wiring the files below |
//! | `config.rs` | the setup form (`FIELDS`), the typed config built from [`Settings`], and the `clap::Args` for its flags |
//! | `run.rs` | the task: a type implementing [`Integration`] |
//! | `icon.svg` | 24×24 glyph drawn with `currentColor`; no vendor logos (trademarks) |
//! | `README.md` | what it needs, how it behaves, its flags |
//!
//! Anything else the integration needs (a cloud client, a protocol parser) goes
//! into more files in the same folder.
//!
//! * `config.rs`: field keys are short (`username`); flags are prefixed
//!   (`--lm-username`), have `env` fallbacks, hide secrets in help, and
//!   `settings()` maps them to the same keys. Give the struct
//!   `#[group(id = "<id>")]` (every folder's is called `Args`) and a
//!   `next_help_heading`.
//!   [`Kind::create`] checks required fields and numbers for both; check
//!   anything else when building the config.
//! * `run.rs`: in `run`, loop forever: connect, set the status, wait for
//!   [`Link::grinder_brews`], measure, call [`Link::report`]; on failure set
//!   [`Status::error`] and back off with [`Backoff`].
//! * Then list it here: `pub mod <id>;`, its `KIND` in [`KINDS`], and its
//!   args in [`CliArgs`] and [`CliArgs::configured`].
//!
//! [`Link::report`] applies the rules every integration must follow: a shot
//! only counts while the grinder is waiting after a knob press, and a missing
//! weight falls back to the recipe weight the grinder sent. Keep vendor code
//! free of those so all integrations behave the same.

mod kind;
mod manager;

pub mod la_marzocco;
pub mod precisa;

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::{broadcast, watch};

use crate::protocol::machine::EventRecord;
use crate::protocol::{MachineError, MachineEvent, Phase, ShotResult};
use crate::server::Server;

pub use kind::{Field, Input, Kind, Settings};
pub use manager::{ChangeError, Integrations, Source, View};

/// Every integration, in the order the app lists them.
pub static KINDS: &[&Kind] = &[&la_marzocco::KIND, &precisa::KIND];

/// The command-line flags of every integration.
#[derive(Debug, Clone, clap::Args)]
pub struct CliArgs {
    #[command(flatten)]
    la_marzocco: la_marzocco::config::Args,
    #[command(flatten)]
    precisa: precisa::config::Args,
}

impl CliArgs {
    /// The integrations whose flags are set, with their settings.
    pub fn configured(&self) -> Vec<(&'static Kind, Settings)> {
        [
            (&la_marzocco::KIND, self.la_marzocco.settings()),
            (&precisa::KIND, self.precisa.settings()),
        ]
        .into_iter()
        .filter_map(|(kind, settings)| Some((kind, settings?)))
        .collect()
    }
}

/// The future an integration runs as.
pub type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Something that reports shots on its own.
pub trait Integration: Send + 'static {
    /// Runs until the process ends or it is removed: reconnect and retry
    /// inside. Returning marks the integration as stopped.
    fn run(self: Box<Self>, link: Link) -> BoxFuture;
}

// ---------------------------------------------------------------------------
// Link: what an integration can do with the machine
// ---------------------------------------------------------------------------

/// An integration's handle on the machine emulation.
#[derive(Clone)]
pub struct Link {
    pub server: Arc<Server>,
    pub status: Status,
}

/// Who started a brew. See [`Link::brews`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrewStart {
    /// A knob press: the grinder waits for the result.
    Grinder,
    /// Started by hand (a test without the grinder): nothing waits for a
    /// result. [`Link::report`] refuses it; [`Link::finish_manual`] ends it
    /// with measured numbers, as typing them in the app would.
    Manual,
}

/// The machine's pump as a sensor integration sees it, shared with the
/// others. See [`Link::pump`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pump {
    /// A sensor is connected and reporting.
    pub sensed: bool,
    /// The pump runs.
    pub running: bool,
    /// Pump starts seen so far: a new run is a new number.
    pub runs: u64,
    /// How long the last finished run took, by the sensor's clock; `None`
    /// while one runs or before any.
    pub last_run: Option<Duration>,
}

/// What happened to a reported shot.
#[derive(Debug, Clone, PartialEq)]
pub enum ReportOutcome {
    /// Shown to the grinder with this weight (grams) and where it came from.
    Reported { grams: f64, source: String },
    /// The grinder was not waiting: no knob press, or already answered.
    NotWaiting,
    /// The machine refused it (e.g. someone typed a result a moment earlier).
    Refused(MachineError),
}

impl Link {
    /// Whether the grinder asked for a brew and is waiting for the result.
    pub fn grinder_waiting(&self) -> bool {
        self.server.with(|m, now| {
            matches!(
                m.phase(now),
                Phase::Brewing {
                    requested_by_grinder: true,
                    ..
                }
            )
        })
    }

    /// Who started the running brew; `None` when no brew runs.
    pub fn brewing(&self) -> Option<BrewStart> {
        self.server.with(|m, now| match m.phase(now) {
            Phase::Brewing {
                requested_by_grinder,
                ..
            } => Some(if requested_by_grinder {
                BrewStart::Grinder
            } else {
                BrewStart::Manual
            }),
            _ => None,
        })
    }

    /// The moments the grinder asks for a brew (knob presses), from now on.
    pub fn grinder_brews(&self) -> GrinderBrews {
        GrinderBrews(self.server.subscribe())
    }

    /// Every brew start from now on, knob presses and manual ones.
    pub fn brews(&self) -> Brews {
        Brews(self.server.subscribe())
    }

    /// The pump as a sensor integration reports it, and its changes. Holding
    /// the receiver tells the sensor that someone times shots by it
    /// ([`Link::pump_listened`]).
    pub fn pump(&self) -> watch::Receiver<Pump> {
        self.server.integrations().pump().subscribe()
    }

    /// For a pump sensor: changes the shared pump state.
    pub fn set_pump(&self, change: impl FnOnce(&mut Pump)) {
        self.server.integrations().pump().send_modify(change);
    }

    /// Whether another integration (a scale) follows the pump and reports
    /// the shot; if not, the sensor reports it.
    pub fn pump_listened(&self) -> bool {
        self.server.integrations().pump().receiver_count() > 0
    }

    /// Reports a measured shot: `time` as the machine ran it, `weight` in
    /// grams with a label saying where it came from (`None` when the
    /// integration has no weight: the grinder's recipe weight is used).
    /// Applies the rules and logs the outcome under this integration's name.
    pub fn report(&self, time: Duration, weight: Option<(f64, String)>) -> ReportOutcome {
        let title = self.status.title();
        let secs = time.as_secs_f64();
        let outcome = self.server.with(|m, now| {
            if !matches!(
                m.phase(now),
                Phase::Brewing {
                    requested_by_grinder: true,
                    ..
                }
            ) {
                return ReportOutcome::NotWaiting;
            }
            let (grams, source) = match weight {
                Some((g, source)) if g.is_finite() && g > 0.0 => (g, source),
                _ => match m.last_grind().and_then(|g| g.beverage_weight_g) {
                    Some(g) => (g, "grinder recipe weight".to_owned()),
                    None => (0.0, "no weight known".to_owned()),
                },
            };
            let shot = ShotResult::from_grams(time, grams);
            match m.report_shot(now, shot) {
                Ok(()) => {
                    self.status.note_report(shot);
                    ReportOutcome::Reported { grams, source }
                }
                Err(e) => ReportOutcome::Refused(e),
            }
        });
        match &outcome {
            ReportOutcome::Reported { grams, source } => {
                tracing::info!("{title}: reported {secs:.1} s, {grams:.1} g ({source})");
            }
            ReportOutcome::NotWaiting => tracing::info!(
                "{title}: {secs:.1} s shot seen but grinder not waiting (press the knob after \
                 grinding)"
            ),
            ReportOutcome::Refused(e) => {
                tracing::warn!("{title}: {secs:.1} s shot not reported: {e}");
            }
        }
        outcome
    }

    /// Ends a brew started by hand with measured numbers, like entering them
    /// in the app (`POST /api/shot/result`): the grinder sees a flush. For
    /// tests of an integration without the grinder. `NotWaiting` unless a
    /// manual brew runs; never counted as a report to the grinder.
    pub fn finish_manual(&self, time: Duration, grams: f64, source: &str) -> ReportOutcome {
        let title = self.status.title();
        let secs = time.as_secs_f64();
        let outcome = self.server.with(|m, now| {
            if !matches!(
                m.phase(now),
                Phase::Brewing {
                    requested_by_grinder: false,
                    ..
                }
            ) {
                return ReportOutcome::NotWaiting;
            }
            match m.report_shot(now, ShotResult::from_grams(time, grams)) {
                Ok(()) => ReportOutcome::Reported {
                    grams,
                    source: source.to_owned(),
                },
                Err(e) => ReportOutcome::Refused(e),
            }
        });
        match &outcome {
            ReportOutcome::Reported { .. } => {
                tracing::info!("{title}: test shot {secs:.1} s, {grams:.1} g ended the manual brew");
            }
            ReportOutcome::NotWaiting => {
                tracing::info!("{title}: {secs:.1} s test shot seen but no manual brew running");
            }
            ReportOutcome::Refused(e) => {
                tracing::warn!("{title}: {secs:.1} s test shot not taken: {e}");
            }
        }
        outcome
    }
}

/// Knob presses as they happen. See [`Link::grinder_brews`].
pub struct GrinderBrews(broadcast::Receiver<EventRecord>);

impl GrinderBrews {
    /// Waits for the next grinder-requested brew. `false` once the server is
    /// gone.
    pub async fn next(&mut self) -> bool {
        loop {
            match self.0.recv().await {
                Ok(rec)
                    if matches!(
                        rec.event,
                        MachineEvent::BrewStarted {
                            requested_by_grinder: true,
                            ..
                        }
                    ) =>
                {
                    return true;
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return false,
            }
        }
    }
}

/// Brew starts as they happen, knob presses and manual ones. See
/// [`Link::brews`].
pub struct Brews(broadcast::Receiver<EventRecord>);

impl Brews {
    /// Waits for the next brew start. `None` once the server is gone.
    pub async fn next(&mut self) -> Option<BrewStart> {
        loop {
            match self.0.recv().await {
                Ok(rec) => {
                    if let MachineEvent::BrewStarted {
                        requested_by_grinder,
                        ..
                    } = rec.event
                    {
                        return Some(if requested_by_grinder {
                            BrewStart::Grinder
                        } else {
                            BrewStart::Manual
                        });
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Status: how an integration is doing
// ---------------------------------------------------------------------------

/// Coarse state, for a dot in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    /// Connecting or logging in.
    Starting,
    /// Ready and idle.
    Connected,
    /// The grinder is waiting and the integration is looking for the shot.
    Watching,
    /// Something failed; the integration is retrying.
    Error,
    /// The task ended.
    Stopped,
}

/// What an integration measures right now, for a live display in the app.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Live {
    /// Grams on the scale.
    pub grams: f64,
    /// The shot's time: running while `measuring`, afterwards the last
    /// measured shot's, until the next one starts; `None` before any.
    pub shot_s: Option<f64>,
    /// Whether a shot is being measured right now.
    pub measuring: bool,
    /// How often the app should refresh while showing this, in ms (the
    /// integration's setting).
    pub refresh_ms: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusSnapshot {
    /// This integration, e.g. `la_marzocco` or `la_marzocco-2`.
    pub id: String,
    /// Its kind ([`Kind::id`]).
    pub kind: String,
    pub title: String,
    pub health: Health,
    /// What is being watched, once known: `MI000000 · Linea Mini R`.
    pub subject: String,
    /// The current state in one line, e.g. `last coffee 3 min ago: 25.1 s`.
    pub detail: String,
    pub last_error: Option<String>,
    /// How long the current health has been in place.
    pub since_ms: u64,
    pub reports: u32,
    pub last_report: Option<ShotResult>,
    /// The current reading, for integrations that have one and are
    /// connected; `None` otherwise.
    pub live: Option<Live>,
}

#[derive(Debug)]
struct StatusInner {
    health: Health,
    subject: String,
    detail: String,
    last_error: Option<String>,
    since: Instant,
    reports: u32,
    last_report: Option<ShotResult>,
    live: Option<Live>,
}

/// Shared, cheap to clone. Integrations set it; the server reads it.
#[derive(Debug, Clone)]
pub struct Status {
    id: Arc<str>,
    kind: Arc<str>,
    title: Arc<str>,
    inner: Arc<Mutex<StatusInner>>,
}

impl Status {
    pub fn new(id: impl Into<Arc<str>>, kind: &str, title: &str) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            title: title.into(),
            inner: Arc::new(Mutex::new(StatusInner {
                health: Health::Starting,
                subject: String::new(),
                detail: String::new(),
                last_error: None,
                since: Instant::now(),
                reports: 0,
                last_report: None,
                live: None,
            })),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Names what is being watched (machine, account); shown above the
    /// detail line. Keep it stable.
    pub fn subject(&self, subject: impl Into<String>) {
        self.lock().subject = subject.into();
    }

    pub fn starting(&self, detail: impl Into<String>) {
        self.set(Health::Starting, detail.into());
    }

    pub fn connected(&self, detail: impl Into<String>) {
        self.set(Health::Connected, detail.into());
    }

    pub fn watching(&self, detail: impl Into<String>) {
        self.set(Health::Watching, detail.into());
    }

    /// The current reading; `None` clears it. Cleared by itself on errors and
    /// when the integration stops.
    pub fn live(&self, live: Option<Live>) {
        self.lock().live = live;
    }

    /// Records a failure; the message stays visible as `last_error` until the
    /// next successful state.
    pub fn error(&self, message: impl Into<String>) {
        let message = message.into();
        let mut s = self.lock();
        if s.health != Health::Error {
            s.since = Instant::now();
        }
        s.health = Health::Error;
        s.detail.clone_from(&message);
        s.live = None;
        s.last_error = Some(message);
    }

    fn stopped(&self) {
        self.set(Health::Stopped, "stopped".to_owned());
    }

    fn set(&self, health: Health, detail: String) {
        let mut s = self.lock();
        if s.health != health {
            s.since = Instant::now();
        }
        s.health = health;
        s.detail = detail;
        if health != Health::Error {
            s.last_error = None;
        }
        if matches!(health, Health::Error | Health::Stopped) {
            s.live = None;
        }
    }

    fn note_report(&self, shot: ShotResult) {
        let mut s = self.lock();
        s.reports += 1;
        s.last_report = Some(shot);
    }

    pub fn snapshot(&self) -> StatusSnapshot {
        let s = self.lock();
        StatusSnapshot {
            id: self.id.to_string(),
            kind: self.kind.to_string(),
            title: self.title.to_string(),
            health: s.health,
            subject: s.subject.clone(),
            detail: s.detail.clone(),
            last_error: s.last_error.clone(),
            since_ms: u64::try_from(s.since.elapsed().as_millis()).unwrap_or(u64::MAX),
            reports: s.reports,
            last_report: s.last_report,
            live: s.live,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StatusInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

// ---------------------------------------------------------------------------
// Backoff
// ---------------------------------------------------------------------------

/// Exponential backoff between attempts: `min`, doubling to `max`.
#[derive(Debug, Clone)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Self {
        Self {
            min,
            max,
            next: min,
        }
    }

    /// Back to `min`; call after a success.
    pub fn reset(&mut self) {
        self.next = self.min;
    }

    /// The delay the next [`Backoff::wait`] will sleep.
    pub fn peek(&self) -> Duration {
        self.next
    }

    /// Sleeps the current delay and doubles it for next time.
    pub async fn wait(&mut self) -> Duration {
        let d = self.advance();
        tokio::time::sleep(d).await;
        d
    }

    /// The current delay, doubling it for next time (no sleep).
    pub fn advance(&mut self) -> Duration {
        let d = self.next;
        self.next = (d * 2).min(self.max);
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{GrindResult, MachineConfig};

    fn link() -> Link {
        Link {
            server: Server::new(MachineConfig::default()),
            status: Status::new("test", "test", "Test"),
        }
    }

    #[test]
    fn report_only_counts_while_the_grinder_waits() {
        let l = link();
        let t = Duration::from_millis(25_100);
        assert_eq!(
            l.report(t, Some((38.1, "scale".into()))),
            ReportOutcome::NotWaiting
        );
        assert!(!l.grinder_waiting());

        // A manual start is not a grinder request.
        l.server.with(|m, now| m.start_brew(now)).unwrap();
        assert_eq!(l.report(t, None), ReportOutcome::NotWaiting);
        l.server.with(|m, now| m.abort(now)).unwrap();

        // Knob press: measured weight wins.
        let later = Instant::now() + Duration::from_secs(30);
        l.server.with(|m, _| m.on_start_request(later, Some(9)));
        assert!(l.grinder_waiting());
        assert_eq!(
            l.report(t, Some((38.1, "scale".into()))),
            ReportOutcome::Reported {
                grams: 38.1,
                source: "scale".into()
            }
        );
        let snap = l.status.snapshot();
        assert_eq!(snap.reports, 1);
        assert_eq!(snap.last_report.unwrap().time_ms, 25_100);
        // Answered: the grinder is no longer waiting.
        assert_eq!(l.report(t, None), ReportOutcome::NotWaiting);
        assert!(!l.grinder_waiting());
    }

    #[test]
    fn report_falls_back_to_the_recipe_weight() {
        let l = link();
        let later = Instant::now() + Duration::from_secs(30);
        l.server.with(|m, _| m.on_start_request(later, Some(9)));
        assert_eq!(
            l.report(Duration::from_secs(30), None),
            ReportOutcome::Reported {
                grams: 0.0,
                source: "no weight known".into()
            }
        );

        let l = link();
        let grind = GrindResult::parse(br#"{"SYNC_BEVERAGE_WEIGHT": 38}"#).unwrap();
        l.server.with(|m, now| m.on_grind_result(now, grind));
        let later = Instant::now() + Duration::from_secs(30);
        l.server.with(|m, _| m.on_start_request(later, Some(9)));
        // An unusable weight also falls back.
        assert_eq!(
            l.report(Duration::from_secs(30), Some((0.0, "scale".into()))),
            ReportOutcome::Reported {
                grams: 38.0,
                source: "grinder recipe weight".into()
            }
        );
    }

    #[tokio::test]
    async fn brews_tell_knob_presses_from_manual_starts() {
        let l = link();
        let mut brews = l.brews();
        l.server.with(|m, now| m.start_brew(now)).unwrap();
        assert_eq!(brews.next().await, Some(BrewStart::Manual));
        assert_eq!(l.brewing(), Some(BrewStart::Manual));
        assert!(!l.grinder_waiting());
        // A knob-press report is refused, a test result ends the brew.
        let t = Duration::from_secs(27);
        assert_eq!(
            l.report(t, Some((36.0, "scale".into()))),
            ReportOutcome::NotWaiting
        );
        assert!(matches!(
            l.finish_manual(t, 36.0, "scale"),
            ReportOutcome::Reported { .. }
        ));
        assert_eq!(l.brewing(), None);
        assert_eq!(l.status.snapshot().reports, 0);

        let later = Instant::now() + Duration::from_secs(30);
        l.server.with(|m, _| m.on_start_request(later, Some(9)));
        assert_eq!(brews.next().await, Some(BrewStart::Grinder));
        assert_eq!(l.brewing(), Some(BrewStart::Grinder));
        // A test result does not answer the grinder.
        assert_eq!(l.finish_manual(t, 36.0, "scale"), ReportOutcome::NotWaiting);
    }

    #[tokio::test]
    async fn the_pump_is_shared() {
        let sensor = link();
        let scale = Link {
            server: sensor.server.clone(),
            status: Status::new("scale", "scale", "Scale"),
        };
        assert!(!sensor.pump_listened());
        let mut pump = scale.pump();
        assert!(sensor.pump_listened());
        sensor.set_pump(|p| {
            p.sensed = true;
            p.running = true;
            p.runs += 1;
        });
        pump.changed().await.unwrap();
        assert_eq!(pump.borrow_and_update().runs, 1);
        sensor.set_pump(|p| {
            p.running = false;
            p.last_run = Some(Duration::from_secs(27));
        });
        pump.changed().await.unwrap();
        assert_eq!(pump.borrow().last_run, Some(Duration::from_secs(27)));
        drop(pump);
        assert!(!sensor.pump_listened());
    }

    #[test]
    fn status_tracks_health_and_errors() {
        let s = Status::new("x", "x", "X");
        assert_eq!(s.snapshot().health, Health::Starting);
        s.error("boom");
        assert_eq!(s.snapshot().last_error.as_deref(), Some("boom"));
        s.subject("thing");
        s.connected("fine");
        let snap = s.snapshot();
        assert_eq!(snap.health, Health::Connected);
        assert_eq!(snap.subject, "thing");
        assert_eq!(snap.detail, "fine");
        assert_eq!(snap.last_error, None);

        s.live(Some(Live {
            grams: 18.2,
            shot_s: Some(4.5),
            measuring: true,
            refresh_ms: 250,
        }));
        assert_eq!(s.snapshot().live.unwrap().grams, 18.2);
        s.error("gone");
        assert_eq!(s.snapshot().live, None);
    }

    #[test]
    fn backoff_doubles_and_resets() {
        let mut b = Backoff::new(Duration::from_secs(2), Duration::from_secs(5));
        assert_eq!(b.advance(), Duration::from_secs(2));
        assert_eq!(b.advance(), Duration::from_secs(4));
        assert_eq!(b.advance(), Duration::from_secs(5));
        assert_eq!(b.peek(), Duration::from_secs(5));
        b.reset();
        assert_eq!(b.advance(), Duration::from_secs(2));
    }
}
