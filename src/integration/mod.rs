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
//! Included: [`la_marzocco`], which reads the La Marzocco cloud's coffee
//! log, and [`half_decent`], which weighs the shot on a Half Decent Scale.
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
//!   Status lines are English. Give each one a key with [`Detail::keyed`]
//!   (`<id>.<state>`, e.g. `la_marzocco.last_coffee`) and its numbers with
//!   [`Detail::var`], and the app can show it in the person's language
//!   (`integ.detail.<key>` in `web/strings/<lang>.json`); a line without a
//!   key, or one the catalog lacks, is shown in English. Errors stay English.
//! * Then list it here: `pub mod <id>;`, its `KIND` in [`KINDS`], and its
//!   args in [`CliArgs`] and [`CliArgs::configured`].
//!
//! [`Link::report`] applies the rules every integration must follow: a shot
//! only counts while the grinder is waiting after a knob press (on an E65/E80
//! GbS: after a Grind-by-Sync grind), and a missing weight falls back to the
//! recipe weight the grinder sent. Keep vendor code free of those so all
//! integrations behave the same.

mod kind;
mod manager;

pub mod half_decent;
pub mod la_marzocco;

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::broadcast;

use crate::protocol::machine::EventRecord;
use crate::protocol::{MachineError, MachineEvent, Phase, ShotResult};
use crate::server::Server;

pub use kind::{Field, Input, Kind, Settings};
pub use manager::{ChangeError, Integrations, Source, View};

/// Every integration, in the order the app lists them.
pub static KINDS: &[&Kind] = &[&la_marzocco::KIND, &half_decent::KIND];

/// The command-line flags of every integration.
#[derive(Debug, Clone, clap::Args)]
pub struct CliArgs {
    #[command(flatten)]
    la_marzocco: la_marzocco::config::Args,
    #[command(flatten)]
    half_decent: half_decent::config::Args,
}

impl CliArgs {
    /// The integrations whose flags are set, with their settings.
    pub fn configured(&self) -> Vec<(&'static Kind, Settings)> {
        [
            (&la_marzocco::KIND, self.la_marzocco.settings()),
            (&half_decent::KIND, self.half_decent.settings()),
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

/// What happened to a reported shot.
#[derive(Debug, Clone, PartialEq)]
pub enum ReportOutcome {
    /// Shown to the grinder with this weight (grams) and where it came from.
    Reported { grams: f64, source: String },
    /// The grinder was not waiting: no knob press (or E65/E80 grind), or
    /// already answered.
    NotWaiting,
    /// The machine refused it (e.g. someone typed a result a moment earlier).
    Refused(MachineError),
}

impl Link {
    pub(crate) fn waiting_for(&self, started: Instant) -> bool {
        self.server.with(|m, now| {
            matches!(m.phase(now), Phase::Running {
            since, requested_by_grinder: true,
        } if since == started)
        })
    }
    /// Whether the grinder asked for a brew and is waiting for the result.
    pub fn grinder_waiting(&self) -> bool {
        self.server.with(|m, now| {
            matches!(
                m.phase(now),
                Phase::Running {
                    requested_by_grinder: true,
                    ..
                }
            )
        })
    }

    /// The moments the grinder asks for a brew (knob presses, or E65/E80
    /// Grind-by-Sync grinds), from now on.
    pub fn grinder_brews(&self) -> GrinderBrews {
        GrinderBrews(self.server.subscribe())
    }

    /// Reports a measured shot: `time` as the machine ran it, `weight` in
    /// grams with a label saying where it came from (`None` when the
    /// integration has no weight: the grinder's recipe weight is used).
    /// Applies the rules and logs the outcome under this integration's name.
    pub fn report(&self, time: Duration, weight: Option<(f64, String)>) -> ReportOutcome {
        self.report_inner(None, time, weight)
    }

    pub(crate) fn report_for(
        &self,
        started: Instant,
        time: Duration,
        weight: Option<(f64, String)>,
    ) -> ReportOutcome {
        self.report_inner(Some(started), time, weight)
    }

    fn report_inner(
        &self,
        started: Option<Instant>,
        time: Duration,
        weight: Option<(f64, String)>,
    ) -> ReportOutcome {
        let title = self.status.title();
        let secs = time.as_secs_f64();
        let outcome = self.server.with(|m, now| {
            if !matches!(
                m.phase(now),
                Phase::Running {
                    since,
                    requested_by_grinder: true,
                } if started.is_none_or(|expected| expected == since)
            ) {
                return ReportOutcome::NotWaiting;
            }
            let (grams, source) = match weight {
                Some((g, source)) if g.is_finite() && g > 0.0 => (g, source),
                // `report_shot` refuses the shot when there is no recipe
                // weight either.
                _ => (
                    m.recipe_weight_g().unwrap_or(0.0),
                    "grinder recipe weight".to_owned(),
                ),
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
                 grinding; an E65/E80 waits after a Grind-by-Sync grind)"
            ),
            ReportOutcome::Refused(e) => {
                tracing::warn!("{title}: {secs:.1} s shot not reported: {e}");
            }
        }
        outcome
    }
}

/// The grinder's brew requests as they happen. See [`Link::grinder_brews`].
pub struct GrinderBrews(broadcast::Receiver<EventRecord>);

impl GrinderBrews {
    /// Waits for the next grinder-requested brew. `false` once the server is
    /// gone.
    pub async fn next(&mut self) -> bool {
        self.next_started().await.is_some()
    }

    pub(crate) async fn next_started(&mut self) -> Option<Instant> {
        loop {
            match self.0.recv().await {
                Ok(rec)
                    if matches!(
                        rec.event,
                        MachineEvent::ShotStarted {
                            requested_by_grinder: true,
                            ..
                        }
                    ) =>
                {
                    return Some(rec.at);
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Status: how an integration is doing
// ---------------------------------------------------------------------------

/// The values a keyed status line is filled with. See [`Detail::var`].
pub type DetailVars = serde_json::Map<String, serde_json::Value>;

/// Rounded to 0.1, as status lines show times and weights. Use it for both the
/// English text and the value, so every language shows the same number (a
/// vendor cloud may also send float noise like 37.4000015).
pub fn tenths(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// A status line: the English text, and optionally a key and values so the
/// app can say it in the person's language. Plain text converts into a line
/// without a key, which every language shows as it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Detail {
    text: String,
    key: Option<&'static str>,
    vars: DetailVars,
}

impl Detail {
    /// `text`, which the app may replace with its own wording for `key`:
    /// `<integration id>.<state>`, such as `la_marzocco.last_coffee`.
    pub fn keyed(key: &'static str, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            key: Some(key),
            vars: DetailVars::new(),
        }
    }

    /// The English line.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Adds a value the app fills into its wording. Numbers are in plain
    /// units, with the unit in the name (`age_ms`, `time_ms`, `weight_g`,
    /// `every_s`); text goes in as it is (`host`).
    pub fn var(mut self, name: &str, value: impl Into<serde_json::Value>) -> Self {
        self.vars.insert(name.to_owned(), value.into());
        self
    }
}

impl From<&str> for Detail {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

impl From<String> for Detail {
    fn from(text: String) -> Self {
        Self {
            text,
            ..Self::default()
        }
    }
}

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
    Failing,
    /// The task ended.
    Stopped,
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
    /// Names `detail` for translation, e.g. `la_marzocco.last_coffee`;
    /// `None` when the integration gives no key for it.
    pub detail_key: Option<&'static str>,
    /// The values the app fills into its wording for `detail_key`, as of
    /// when `detail` was set: e.g. `{"age_ms": 180000, "time_ms": 25100}`.
    pub detail_vars: DetailVars,
    pub last_error: Option<String>,
    /// How long the current health has been in place.
    pub health_age_ms: u64,
    pub reports: u32,
    pub last_report: Option<ShotResult>,
}

#[derive(Debug)]
struct StatusInner {
    health: Health,
    subject: String,
    detail: Detail,
    last_error: Option<String>,
    since: Instant,
    reports: u32,
    last_report: Option<ShotResult>,
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
                detail: Detail::default(),
                last_error: None,
                since: Instant::now(),
                reports: 0,
                last_report: None,
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

    /// The state and its line: plain English text, or a [`Detail`] with a
    /// key the app can translate.
    pub fn starting(&self, detail: impl Into<Detail>) {
        self.set(Health::Starting, detail.into());
    }

    pub fn connected(&self, detail: impl Into<Detail>) {
        self.set(Health::Connected, detail.into());
    }

    pub fn watching(&self, detail: impl Into<Detail>) {
        self.set(Health::Watching, detail.into());
    }

    /// Records a failure; the message stays visible as `last_error` until the
    /// next successful state.
    pub fn error(&self, message: impl Into<String>) {
        let message = message.into();
        let mut s = self.lock();
        if s.health != Health::Failing {
            s.since = Instant::now();
        }
        s.health = Health::Failing;
        s.detail = Detail::from(message.as_str());
        s.last_error = Some(message);
    }

    fn stopped(&self) {
        self.set(Health::Stopped, "stopped".into());
    }

    fn set(&self, health: Health, detail: Detail) {
        let mut s = self.lock();
        if s.health != health {
            s.since = Instant::now();
        }
        s.health = health;
        s.detail = detail;
        if health != Health::Failing {
            s.last_error = None;
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
            detail: s.detail.text.clone(),
            detail_key: s.detail.key,
            detail_vars: s.detail.vars.clone(),
            last_error: s.last_error.clone(),
            health_age_ms: u64::try_from(s.since.elapsed().as_millis()).unwrap_or(u64::MAX),
            reports: s.reports,
            last_report: s.last_report,
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
    fn a_delayed_report_cannot_answer_the_next_brew() {
        let l = link();
        let first = Instant::now();
        l.server.with(|m, _| m.on_start_request(first, Some(9)));
        l.server.with(|m, now| m.abort(now)).unwrap();
        let second = first + Duration::from_secs(30);
        l.server.with(|m, _| m.on_start_request(second, Some(9)));
        assert!(!l.waiting_for(first));
        assert!(l.waiting_for(second));
        assert_eq!(
            l.report_for(first, Duration::from_secs(28), Some((30.0, "test".into()))),
            ReportOutcome::NotWaiting
        );
        assert!(l.waiting_for(second));
        assert!(matches!(
            l.report_for(second, Duration::from_secs(28), Some((30.0, "test".into()))),
            ReportOutcome::Reported { .. }
        ));
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
        l.server.with(|m, now| m.start_shot(now)).unwrap();
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
        // No weight and no recipe weight: refused rather than sending 0 g.
        assert_eq!(
            l.report(Duration::from_secs(30), None),
            ReportOutcome::Refused(MachineError::NoWeight)
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
        assert_eq!(snap.detail_key, None);
        assert_eq!(snap.last_error, None);
    }

    #[test]
    fn keyed_details_carry_their_values() {
        let s = Status::new("x", "x", "X");
        s.watching(
            Detail::keyed("x.flowing", "flowing · 12.3 g")
                .var("weight_g", 12.3)
                .var("host", "192.168.1.30"),
        );
        let snap = s.snapshot();
        assert_eq!(snap.health, Health::Watching);
        assert_eq!(snap.detail, "flowing · 12.3 g", "English stays");
        assert_eq!(snap.detail_key, Some("x.flowing"));
        assert_eq!(snap.detail_vars["weight_g"], 12.3);
        assert_eq!(snap.detail_vars["host"], "192.168.1.30");

        // The API shows both, next to the English line.
        let json = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["detail"], "flowing · 12.3 g");
        assert_eq!(json["detail_key"], "x.flowing");
        assert_eq!(json["detail_vars"]["weight_g"], 12.3);

        // A plain line or an error drops the key and its values.
        s.connected(String::from("ready"));
        let snap = s.snapshot();
        assert_eq!((snap.detail.as_str(), snap.detail_key), ("ready", None));
        assert!(snap.detail_vars.is_empty());
        s.watching(Detail::keyed("x.settling", "settling"));
        s.error("boom");
        let json = serde_json::to_value(s.snapshot()).unwrap();
        assert_eq!(json["detail"], "boom");
        assert_eq!(json["detail_key"], serde_json::Value::Null);
        assert_eq!(json["detail_vars"], serde_json::json!({}));
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
