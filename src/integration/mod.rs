//! Machine integrations: anything that watches an espresso machine (a vendor
//! cloud, a home-automation hub, a scale, ...) and reports the shot to the
//! grinder so nobody has to type it.
//!
//! An integration is a long-running task. It gets a `Link` to the machine
//! emulation, which tells it when the grinder is waiting for a shot
//! (`Link::grinder_brews`, one `Brew` at a time) and takes the measured
//! numbers for that brew (`Link::report`), and a `Status` to tell people how it is doing (shown
//! in the app and in `GET /api/state`). Everything in between, protocols and
//! logins and reconnects, is the integration's own business.
//!
//! Each kind of integration is a [`Kind`]: a name, an icon, a setup form and a way to
//! build the integration from that form. People add one in the app with the
//! **+** button (saved in the `--config` file) or on the command line;
//! [`Integrations`] runs them all, so several can run side by side. The
//! first one to report a shot wins.
//!
//! Included: `la_marzocco`, which reads the La Marzocco cloud's coffee
//! log, and `half_decent`, which weighs the shot on a Half Decent Scale.
//!
//! # Adding one
//!
//! Every integration is a folder `src/integration/<id>/` with this layout:
//!
//! | file | what |
//! |---|---|
//! | `mod.rs` | module docs and `pub(crate) static KIND: Kind`, wiring the files below |
//! | `config.rs` | the setup form (`FIELDS`), the typed config built from [`Settings`], and the `clap::Args` for its flags |
//! | `run.rs` | the task: a type implementing `Integration` |
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
//!   `Kind::create` checks required fields and numbers for both; check
//!   anything else when building the config.
//! * `run.rs`: in `run`, loop forever: connect, set the status, wait for
//!   the next `Brew` from `Link::grinder_brews`, measure while
//!   `Link::waits_for` that brew, call `Link::report` with it; on failure
//!   set `Status::error` and back off with `Backoff::retry`.
//!   Status lines are English. Give each one a key with `Detail::keyed`
//!   (`<id>.<state>`, e.g. `la_marzocco.last_coffee`) and its numbers with
//!   `Detail::var`, and the app can show it in the person's language
//!   (`integ.detail.<key>` in `web/strings/<lang>.json`); a line without a
//!   key, or one the catalog lacks, is shown in English. Errors stay English.
//! * Then list it here: `pub(crate) mod <id>;`, its `KIND` in [`KINDS`], and its
//!   args in [`CliArgs`] and [`CliArgs::configured`]. `cargo test` checks
//!   that each kind's flags turn it on.
//!
//! `Link::report` applies the rules every integration must follow: a shot
//! only counts while the grinder is waiting after a knob press (on an E65/E80
//! GbS: after a Grind-by-Sync grind), only for the brew it was measured in,
//! and a missing weight falls back to the recipe weight the grinder sent. Keep vendor code free of those so all
//! integrations behave the same.

mod kind;
mod manager;

pub(crate) mod half_decent;
pub(crate) mod la_marzocco;

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::broadcast;

use crate::protocol::machine::{EventRecord, Machine};
use crate::protocol::{MachineError, MachineEvent, Phase, ShotResult};
use crate::server::Server;

pub(crate) use kind::{Field, Input};
pub use kind::{Kind, Settings};
pub(crate) use manager::ChangeError;
pub use manager::Integrations;

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
pub(crate) type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Something that reports shots on its own.
pub(crate) trait Integration: Send + 'static {
    /// Runs until the process ends or it is removed: reconnect and retry
    /// inside. Returning marks the integration as stopped.
    fn run(self: Box<Self>, link: Link) -> BoxFuture;
}

// ---------------------------------------------------------------------------
// Link: what an integration can do with the machine
// ---------------------------------------------------------------------------

/// An integration's handle on the machine emulation.
#[derive(Clone)]
pub(crate) struct Link {
    pub(crate) server: Arc<Server>,
    pub(crate) status: Status,
}

/// One brew the grinder asked for (a knob press, or an E65/E80
/// Grind-by-Sync grind), named by the moment it started. An integration
/// measures and reports against it, so a shot is only ever reported for the
/// brew it was measured in, never for the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Brew {
    /// `Phase::Running.since`, which is also the `at` of its `ShotStarted`.
    started: Instant,
}

/// What happened to a reported shot.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ReportOutcome {
    /// Shown to the grinder with this weight (grams) and where it came from.
    Reported { grams: f64, source: String },
    /// The grinder was not waiting: no knob press (or E65/E80 grind), or
    /// this brew was already answered, aborted or timed out.
    NotWaiting,
    /// The brew is over and the grinder waits for a newer one, which this
    /// shot is not.
    BrewOver,
    /// The machine refused it (e.g. someone typed a result a moment earlier).
    Refused(MachineError),
}

/// The brew the grinder is waiting for, if any.
fn waiting_brew(m: &mut Machine, now: Instant) -> Option<Brew> {
    match m.phase(now) {
        Phase::Running {
            since,
            requested_by_grinder: true,
        } => Some(Brew { started: since }),
        _ => None,
    }
}

impl Link {
    /// The brew the grinder asked for and is waiting on, if any.
    pub(crate) fn waiting_brew(&self) -> Option<Brew> {
        self.server.with(waiting_brew)
    }

    /// Whether the grinder still waits for the result of `brew`.
    pub(crate) fn waits_for(&self, brew: Brew) -> bool {
        self.waiting_brew() == Some(brew)
    }

    /// The brews the grinder asks for (knob presses, or E65/E80
    /// Grind-by-Sync grinds): first the one it is waiting on already, if
    /// any, then each new one.
    pub(crate) fn grinder_brews(&self) -> GrinderBrews {
        // Subscribed first, so a brew that starts in between is not missed.
        let events = self.server.subscribe();
        GrinderBrews {
            events,
            server: self.server.clone(),
            pending: self.waiting_brew(),
            last: None,
        }
    }

    /// Reports a shot measured during `brew`: `time` as the machine ran it,
    /// `weight` in grams with a label saying where it came from (`None` when
    /// the integration has no weight: the grinder's recipe weight is used).
    /// Applies the rules and logs the outcome under this integration's name.
    pub(crate) fn report(
        &self,
        brew: Brew,
        time: Duration,
        weight: Option<(f64, String)>,
    ) -> ReportOutcome {
        let title = self.status.title();
        let secs = time.as_secs_f64();
        let outcome = self.server.with(|m, now| {
            match waiting_brew(m, now) {
                Some(b) if b == brew => {}
                Some(_) => return ReportOutcome::BrewOver,
                None => return ReportOutcome::NotWaiting,
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
            let shot = ShotResult::new(time, grams);
            match m.report_shot_by(now, shot, Some(self.status.id())) {
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
            ReportOutcome::BrewOver => tracing::info!(
                "{title}: {secs:.1} s shot belongs to a brew that is over, not reported for the \
                 next one"
            ),
            ReportOutcome::Refused(e) => {
                tracing::warn!("{title}: {secs:.1} s shot not reported: {e}");
            }
        }
        outcome
    }
}

/// The grinder's brews as they start. See [`Link::grinder_brews`].
pub(crate) struct GrinderBrews {
    events: broadcast::Receiver<EventRecord>,
    server: Arc<Server>,
    /// The brew already waiting when this was made.
    pending: Option<Brew>,
    /// The last brew handed out; only newer ones follow.
    last: Option<Brew>,
}

impl GrinderBrews {
    /// Waits for the next brew the grinder is waiting on. Each brew comes
    /// once, and brews that are over by the time they would come (answered,
    /// aborted, timed out) are skipped. `None` once the server is gone.
    pub(crate) async fn next(&mut self) -> Option<Brew> {
        if let Some(brew) = self.pending.take()
            && self.take(brew)
        {
            return Some(brew);
        }
        loop {
            let brew = match self.events.recv().await {
                Ok(EventRecord {
                    at,
                    event:
                        MachineEvent::ShotStarted {
                            requested_by_grinder: true,
                            ..
                        },
                    ..
                }) => Brew { started: at },
                Ok(_) => continue,
                // Missed some: the brew waiting now, if any, is the one.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    match self.server.with(waiting_brew) {
                        Some(brew) => brew,
                        None => continue,
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            };
            if self.take(brew) {
                return Some(brew);
            }
        }
    }

    /// Whether `brew` is newer than the last one handed out and still
    /// waited for; if so, it is the last one now.
    fn take(&mut self, brew: Brew) -> bool {
        if self.last.is_some_and(|last| brew <= last)
            || self.server.with(waiting_brew) != Some(brew)
        {
            return false;
        }
        self.last = Some(brew);
        true
    }
}

// ---------------------------------------------------------------------------
// Status: how an integration is doing
// ---------------------------------------------------------------------------

/// The values a keyed status line is filled with. See [`Detail::var`].
pub(crate) type DetailVars = serde_json::Map<String, serde_json::Value>;

/// Rounded to 0.1, as status lines show times and weights. Use it for both the
/// English text and the value, so every language shows the same number (a
/// vendor cloud may also send float noise like 37.4000015).
pub(crate) fn tenths(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// A status line: the English text, and optionally a key and values so the
/// app can say it in the person's language. Plain text converts into a line
/// without a key, which every language shows as it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Detail {
    text: String,
    key: Option<&'static str>,
    vars: DetailVars,
}

impl Detail {
    /// `text`, which the app may replace with its own wording for `key`:
    /// `<integration id>.<state>`, such as `la_marzocco.last_coffee`.
    pub(crate) fn keyed(key: &'static str, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            key: Some(key),
            vars: DetailVars::new(),
        }
    }

    /// The English line.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Adds a value the app fills into its wording. Numbers are in plain
    /// units, with the unit in the name (`age_ms`, `time_ms`, `weight_g`,
    /// `every_s`); text goes in as it is (`host`).
    pub(crate) fn var(mut self, name: &str, value: impl Into<serde_json::Value>) -> Self {
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
pub(crate) enum Health {
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
pub(crate) struct StatusSnapshot {
    /// This integration, e.g. `la_marzocco` or `la_marzocco-2`.
    pub(crate) id: String,
    /// Its kind ([`Kind::id`]).
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) health: Health,
    /// What is being watched, once known: `MI000000 · Linea Mini R`.
    pub(crate) subject: String,
    /// The current state in one line, e.g. `last coffee 3 min ago: 25.1 s`.
    pub(crate) detail: String,
    /// Names `detail` for translation, e.g. `la_marzocco.last_coffee`;
    /// `None` when the integration gives no key for it.
    pub(crate) detail_key: Option<&'static str>,
    /// The values the app fills into its wording for `detail_key`: e.g.
    /// `{"age_ms": 180000, "time_ms": 25100}`. An `age_ms` is as of now,
    /// the rest as of when `detail` was set (whose English words keep the
    /// age of then).
    pub(crate) detail_vars: DetailVars,
    pub(crate) last_error: Option<String>,
    /// How long the current health has been in place.
    pub(crate) health_age_ms: u64,
    pub(crate) reports: u32,
    pub(crate) last_report: Option<ShotResult>,
}

#[derive(Debug)]
struct StatusInner {
    health: Health,
    subject: String,
    detail: Detail,
    /// When `detail` was set, for its `age_ms` as of a snapshot.
    detail_since: Instant,
    last_error: Option<String>,
    since: Instant,
    reports: u32,
    last_report: Option<ShotResult>,
}

/// Shared, cheap to clone. Integrations set it; the server reads it.
#[derive(Debug, Clone)]
pub(crate) struct Status {
    id: Arc<str>,
    kind: Arc<str>,
    title: Arc<str>,
    inner: Arc<Mutex<StatusInner>>,
}

impl Status {
    pub(crate) fn new(id: impl Into<Arc<str>>, kind: &str, title: &str) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            title: title.into(),
            inner: Arc::new(Mutex::new(StatusInner {
                health: Health::Starting,
                subject: String::new(),
                detail: Detail::default(),
                detail_since: Instant::now(),
                last_error: None,
                since: Instant::now(),
                reports: 0,
                last_report: None,
            })),
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    /// Names what is being watched (machine, account); shown above the
    /// detail line. Keep it stable.
    pub(crate) fn subject(&self, subject: impl Into<String>) {
        self.lock().subject = subject.into();
    }

    /// The state and its line: plain English text, or a [`Detail`] with a
    /// key the app can translate.
    pub(crate) fn starting(&self, detail: impl Into<Detail>) {
        self.set(Health::Starting, detail.into());
    }

    pub(crate) fn connected(&self, detail: impl Into<Detail>) {
        self.set(Health::Connected, detail.into());
    }

    pub(crate) fn watching(&self, detail: impl Into<Detail>) {
        self.set(Health::Watching, detail.into());
    }

    /// Records a failure; the message stays visible as `last_error` until the
    /// next successful state.
    pub(crate) fn error(&self, message: impl Into<String>) {
        let message = message.into();
        let mut s = self.lock();
        if s.health != Health::Failing {
            s.since = Instant::now();
        }
        s.health = Health::Failing;
        s.detail = Detail::from(message.as_str());
        s.detail_since = Instant::now();
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
        s.detail_since = Instant::now();
        if health != Health::Failing {
            s.last_error = None;
        }
    }

    fn note_report(&self, shot: ShotResult) {
        let mut s = self.lock();
        s.reports += 1;
        s.last_report = Some(shot);
    }

    pub(crate) fn snapshot(&self) -> StatusSnapshot {
        let s = self.lock();
        let mut detail_vars = s.detail.vars.clone();
        // An age goes on from when the line was set.
        if let Some(age) = detail_vars.get_mut("age_ms")
            && let Some(then) = age.as_u64()
        {
            let since = u64::try_from(s.detail_since.elapsed().as_millis()).unwrap_or(u64::MAX);
            *age = then.saturating_add(since).into();
        }
        StatusSnapshot {
            id: self.id.to_string(),
            kind: self.kind.to_string(),
            title: self.title.to_string(),
            health: s.health,
            subject: s.subject.clone(),
            detail: s.detail.text.clone(),
            detail_key: s.detail.key,
            detail_vars,
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
pub(crate) struct Backoff {
    min: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    pub(crate) fn new(min: Duration, max: Duration) -> Self {
        Self {
            min,
            max,
            next: min,
        }
    }

    /// Back to `min`; call after a success.
    pub(crate) fn reset(&mut self) {
        self.next = self.min;
    }

    /// The delay the next [`Backoff::wait`] will sleep.
    pub(crate) fn peek(&self) -> Duration {
        self.next
    }

    /// Sleeps the current delay and doubles it for next time.
    pub(crate) async fn wait(&mut self) -> Duration {
        let d = self.advance();
        tokio::time::sleep(d).await;
        d
    }

    /// The current delay, doubling it for next time (no sleep).
    pub(crate) fn advance(&mut self) -> Duration {
        let d = self.next;
        self.next = (d * 2).min(self.max);
        d
    }

    /// Logs `<title>: retrying in N s`, then waits as [`Backoff::wait`] does.
    pub(crate) async fn retry(&mut self, title: &str) -> Duration {
        tracing::info!("{title}: retrying in {} s", self.peek().as_secs());
        self.wait().await
    }

    /// As [`Backoff::retry`], with the line in the debug log only: for a
    /// retry after an error left out of the log as a repeat.
    pub(crate) async fn retry_quietly(&mut self, title: &str) -> Duration {
        tracing::debug!("{title}: retrying in {} s", self.peek().as_secs());
        self.wait().await
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

    /// A knob press `after` from now (later than any earlier one), and the
    /// brew it starts.
    fn knob_press(l: &Link, after: Duration) -> Brew {
        let at = Instant::now() + after;
        l.server.with(|m, _| m.on_start_request(at, Some(9)));
        l.waiting_brew().expect("the grinder waits")
    }

    #[test]
    fn a_status_lines_age_is_as_of_now() {
        let status = Status::new("test", "test", "Test");
        let line = Detail::keyed("test.last", "last 1 s ago: 25.1 s")
            .var("age_ms", 1_000)
            .var("time_ms", 25_100);
        status.connected(line);
        std::thread::sleep(Duration::from_millis(50));
        let snap = status.snapshot();
        let age = snap.detail_vars["age_ms"].as_u64().unwrap();
        assert!((1_050..10_000).contains(&age), "{age} ms");
        // The rest, and the English words, stay as they were set.
        assert_eq!(snap.detail_vars["time_ms"], 25_100);
        assert_eq!(snap.detail, "last 1 s ago: 25.1 s");
    }

    #[test]
    fn report_only_counts_while_the_grinder_waits() {
        let l = link();
        let t = Duration::from_millis(25_100);
        let none = Brew {
            started: Instant::now(),
        };
        assert_eq!(
            l.report(none, t, Some((38.1, "scale".into()))),
            ReportOutcome::NotWaiting
        );
        assert_eq!(l.waiting_brew(), None);

        // A manual start is not a grinder request.
        l.server.with(|m, now| m.start_shot(now)).unwrap();
        assert_eq!(l.waiting_brew(), None);
        assert_eq!(l.report(none, t, None), ReportOutcome::NotWaiting);
        l.server.with(|m, now| m.abort(now)).unwrap();

        // Knob press: measured weight wins.
        let brew = knob_press(&l, Duration::from_secs(30));
        assert!(l.waits_for(brew));
        assert_eq!(
            l.report(brew, t, Some((38.1, "scale".into()))),
            ReportOutcome::Reported {
                grams: 38.1,
                source: "scale".into()
            }
        );
        let snap = l.status.snapshot();
        assert_eq!(snap.reports, 1);
        assert_eq!(snap.last_report.unwrap().time_ms, 25_100);
        // The shot's event names the integration.
        let reported = l.server.with(|m, _| {
            m.events_after(0)
                .find_map(|e| match &e.event {
                    MachineEvent::ShotReported { integration_id, .. } => {
                        Some(integration_id.clone())
                    }
                    _ => None,
                })
                .flatten()
        });
        assert_eq!(reported.as_deref(), Some("test"));
        // Answered: the grinder is no longer waiting.
        assert_eq!(l.report(brew, t, None), ReportOutcome::NotWaiting);
        assert!(!l.waits_for(brew));
    }

    #[test]
    fn a_shot_is_only_reported_for_its_own_brew() {
        let l = link();
        let t = Duration::from_secs(25);
        let first = knob_press(&l, Duration::from_secs(30));
        l.server.with(|m, now| m.abort(now)).unwrap();
        let second = knob_press(&l, Duration::from_secs(60));
        assert_ne!(first, second);
        assert!(!l.waits_for(first) && l.waits_for(second));
        // The first brew's shot is not the second's.
        assert_eq!(
            l.report(first, t, Some((38.0, "scale".into()))),
            ReportOutcome::BrewOver
        );
        assert_eq!(l.status.snapshot().reports, 0);
        assert!(matches!(
            l.report(second, t, Some((38.0, "scale".into()))),
            ReportOutcome::Reported { .. }
        ));
    }

    #[test]
    fn report_falls_back_to_the_recipe_weight() {
        let l = link();
        let brew = knob_press(&l, Duration::from_secs(30));
        // No weight and no recipe weight: refused rather than sending 0 g.
        assert_eq!(
            l.report(brew, Duration::from_secs(30), None),
            ReportOutcome::Refused(MachineError::NoWeight)
        );

        let l = link();
        let grind = GrindResult::parse(br#"{"SYNC_BEVERAGE_WEIGHT": 38}"#).unwrap();
        l.server.with(|m, now| m.on_grind_result(now, grind));
        let brew = knob_press(&l, Duration::from_secs(30));
        // An unusable weight also falls back.
        assert_eq!(
            l.report(brew, Duration::from_secs(30), Some((0.0, "scale".into()))),
            ReportOutcome::Reported {
                grams: 38.0,
                source: "grinder recipe weight".into()
            }
        );
    }

    /// The next brew, or `None` when none comes within a moment.
    async fn next_soon(brews: &mut GrinderBrews) -> Option<Brew> {
        tokio::time::timeout(Duration::from_millis(50), brews.next())
            .await
            .ok()
            .flatten()
    }

    #[tokio::test]
    async fn brews_come_once_and_only_while_waited_for() {
        let l = link();
        let mut brews = l.grinder_brews();
        // A brew that is over before anyone looks is skipped.
        knob_press(&l, Duration::from_secs(30));
        l.server.with(|m, now| m.abort(now)).unwrap();
        let second = knob_press(&l, Duration::from_secs(60));
        assert_eq!(next_soon(&mut brews).await, Some(second));
        assert_eq!(next_soon(&mut brews).await, None, "each brew once");

        // Subscribed while a brew waits (pressed during a login): that one
        // first. Its start event may have come in too; it is still one brew.
        let mut late = l.grinder_brews();
        assert_eq!(late.pending, Some(second));
        let mut raced = l.grinder_brews();
        raced.pending = None;
        let ended = l.report(second, Duration::from_secs(25), Some((38.0, "x".into())));
        assert!(matches!(ended, ReportOutcome::Reported { .. }));
        let third = knob_press(&l, Duration::from_secs(90));
        raced.pending = Some(third);
        assert_eq!(next_soon(&mut raced).await, Some(third));
        assert_eq!(next_soon(&mut raced).await, None);
        // `late` saw the second brew end: it skips to the third.
        assert_eq!(next_soon(&mut late).await, Some(third));
        assert_eq!(next_soon(&mut late).await, None);
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

    /// The flags as the binary takes them.
    #[derive(clap::Parser)]
    struct Cli {
        #[command(flatten)]
        integrations: CliArgs,
    }

    /// The kinds `argv` turns on.
    fn configured(argv: &[String]) -> std::collections::BTreeSet<&'static str> {
        use clap::Parser;
        let cli = Cli::try_parse_from(argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
        let on = cli.integrations.configured();
        on.iter().map(|(kind, _)| kind.id).collect()
    }

    /// Registering one takes four lines here; a missed `CliArgs::configured`
    /// line would ignore its flags without a word.
    #[test]
    fn every_integration_has_flags_that_turn_it_on() {
        use clap::CommandFactory;
        let cmd = Cli::command();
        let kinds: std::collections::BTreeSet<&str> = KINDS.iter().map(|k| k.id).collect();
        // Each `#[group(id = "<id>")]`; clap adds one for each struct, too.
        let groups: Vec<_> = cmd
            .get_groups()
            .filter(|g| !["Cli", "CliArgs"].contains(&g.get_id().as_str()))
            .collect();
        let ids: std::collections::BTreeSet<&str> =
            groups.iter().map(|g| g.get_id().as_str()).collect();
        assert_eq!(ids, kinds, "one flag group for each kind in KINDS");
        for arg in cmd.get_arguments() {
            assert!(
                groups
                    .iter()
                    .any(|g| g.get_args().any(|a| a == arg.get_id())),
                "--{} is in no integration's group",
                arg.get_id()
            );
        }

        // Set in the environment (`LM_USERNAME`, say), not by the flags.
        let ambient = configured(&["gbs-anywhere".to_owned()]);
        for group in groups {
            let id = group.get_id().as_str();
            let mut argv = vec!["gbs-anywhere".to_owned()];
            for arg_id in group.get_args() {
                let arg = cmd.get_arguments().find(|a| a.get_id() == arg_id).unwrap();
                argv.push(format!("--{}", arg.get_long().expect("a long flag")));
                if arg.get_action().takes_values() {
                    // Valid as text and as a number.
                    argv.push("1".to_owned());
                }
            }
            let on = configured(&argv);
            assert!(
                on.contains(id),
                "{argv:?} does not turn on {id}: is it in CliArgs::configured?"
            );
            assert!(
                on.iter().all(|k| *k == id || ambient.contains(k)),
                "{argv:?} turns on {on:?}"
            );
        }
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
