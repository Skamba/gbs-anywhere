//! Looks for a newer release once a day, so the app can say when one is out
//! (`update` in `GET /api/state`). It asks GitHub for the latest release of
//! gbs-anywhere. GitHub sees the request like any other, including the IP
//! address; it carries nothing but the gbs-anywhere version (in the
//! User-Agent). It can be turned off in the app or with `--update-check off`
//! (see `settings.rs`); while it is off, nothing is sent.

use std::cmp::Ordering;
use std::sync::{Arc, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::server::Server;

/// Where the latest release is asked for.
pub(crate) const RELEASES_API: &str =
    "https://api.github.com/repos/Skamba/gbs-anywhere/releases/latest";

/// The page people open to get it.
pub(crate) const RELEASES_PAGE: &str = "https://github.com/Skamba/gbs-anywhere/releases/latest";

/// A release description is a few kilobytes; anything this big is not one.
const MAX_REPLY_BYTES: usize = 1024 * 1024;

/// A manual re-check (the app asks for one when the Status tab opens) is
/// ignored when the last look is this recent, so opening the tab again and
/// again does not keep asking GitHub.
const MANUAL_MIN_GAP: Duration = Duration::from_secs(60);

/// When and where to look.
#[derive(Debug, Clone)]
pub struct UpdateConfig {
    /// [`RELEASES_API`], or a local fake in tests.
    pub(crate) url: String,
    /// After the start (or being turned on), before the first look.
    pub(crate) first: Duration,
    /// Between looks.
    pub(crate) every: Duration,
    /// After a failed look.
    pub(crate) retry: Duration,
    pub(crate) timeout: Duration,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            url: RELEASES_API.to_owned(),
            first: Duration::from_secs(60),
            every: Duration::from_secs(24 * 3600),
            retry: Duration::from_secs(3600),
            timeout: Duration::from_secs(10),
        }
    }
}

/// What the last look found.
#[derive(Debug, Default)]
pub(super) struct UpdateState {
    /// Turned on (the default) in the app or by the flag.
    enabled: bool,
    /// `0.3.0`, from the release tag `v0.3.0`.
    latest: Option<String>,
    /// The last look failed (said once in the log, not every hour).
    failing: bool,
    /// A look is under way (so the app can say it is checking).
    checking: bool,
    /// When the last look finished, for `checked_age_ms` and the manual gap.
    checked: Option<Instant>,
    /// A manual re-check is waiting for the task to pick it up.
    recheck: bool,
}

impl Server {
    /// Starts the task that looks for newer releases while the check is on
    /// (see `Server::set_update_check`).
    pub fn start_update_check(self: &Arc<Self>, cfg: UpdateConfig) {
        let http = match crate::http::builder()
            .user_agent(format!("gbs-anywhere/{}", crate::version()))
            .timeout(cfg.timeout)
            // Only an answer from this address counts. If the repository is
            // ever renamed (GitHub redirects the old name), the check stops
            // rather than following it.
            .redirect(reqwest::redirect::Policy::none())
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("update check: cannot build the HTTP client: {e}");
                return;
            }
        };
        tokio::spawn(run(self.clone(), cfg, http));
    }

    /// Turns the check on or off; turned on, it looks again soon.
    pub(super) fn set_update_check(&self, on: bool) {
        let mut s = self.update_state();
        if s.enabled != on {
            s.enabled = on;
            s.failing = false;
            // Turned on again later, an old result would pass for a new one.
            if !on {
                s.latest = None;
                s.checked = None;
                s.recheck = false;
            }
            self.update_wake.notify_waiters();
        }
    }

    /// Asks for a look right away (the app does when the Status tab opens),
    /// skipped when the check is off, one is under way, or the last was less
    /// than [`MANUAL_MIN_GAP`] ago. The periodic look is unaffected.
    pub(crate) fn request_update_check(&self) {
        {
            let mut s = self.update_state();
            if !s.enabled || s.checking || s.checked.is_some_and(|t| t.elapsed() < MANUAL_MIN_GAP) {
                return;
            }
            s.recheck = true;
        }
        self.update_wake.notify_waiters();
    }

    /// Takes the pending manual re-check, if any.
    fn take_recheck(&self) -> bool {
        std::mem::take(&mut self.update_state().recheck)
    }

    /// Whether the check is on (`update_check` in the settings).
    #[cfg(test)]
    pub(super) fn update_check_on(&self) -> bool {
        self.update_state().enabled
    }

    /// `update` in `GET /api/state`.
    pub(crate) fn update_json(&self) -> Value {
        let s = self.update_state();
        json!({
            "latest": s.latest,
            "available": s.latest.as_deref().is_some_and(is_newer),
            "failing": s.enabled && s.failing,
            "checking": s.enabled && s.checking,
            "checked_age_ms": s.checked.map(|t| t.elapsed().as_millis() as u64),
            "url": RELEASES_PAGE,
        })
    }

    fn update_state(&self) -> MutexGuard<'_, UpdateState> {
        self.update.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Looks `cfg.first` after the check is on, then every `cfg.every` (after
/// a failed look, `cfg.retry`). While it is off, it sends nothing and waits
/// to be turned on.
async fn run(server: Arc<Server>, cfg: UpdateConfig, http: reqwest::Client) {
    let mut wait = cfg.first;
    loop {
        // Registered before the check, so a wake in between is not missed.
        let switched = server.update_wake.notified();
        if !server.update_state().enabled {
            switched.await;
            wait = cfg.first;
            continue;
        }
        // A manual re-check asked for while the last look ran is picked up
        // here; otherwise wait for the timer or a wake.
        if !server.take_recheck() {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = switched => {
                    // A manual re-check looks now; an on/off switch starts over.
                    if !server.take_recheck() {
                        wait = cfg.first;
                        continue;
                    }
                }
            }
        }
        {
            let mut s = server.update_state();
            // Turned off since the wake: send nothing.
            if !s.enabled {
                wait = cfg.first;
                continue;
            }
            s.checking = true;
        }
        let found = latest_release(&http, &cfg.url).await;
        let mut s = server.update_state();
        s.checking = false;
        s.checked = Some(Instant::now());
        if !s.enabled {
            // Turned off while it looked: the answer is not kept.
            wait = cfg.first;
            continue;
        }
        wait = match found {
            Ok(latest) => {
                if is_newer(&latest) && s.latest.as_deref() != Some(latest.as_str()) {
                    tracing::info!(
                        "update: gbs-anywhere {latest} is out (this is {}): {RELEASES_PAGE}",
                        crate::version()
                    );
                }
                s.latest = Some(latest);
                s.failing = false;
                cfg.every
            }
            Err(e) => {
                if !s.failing {
                    tracing::info!(
                        "update check: could not look for a newer version, trying again in {} \
                         s: {e:#}",
                        cfg.retry.as_secs()
                    );
                }
                s.failing = true;
                cfg.retry
            }
        };
    }
}

/// Whether `latest` is newer than the running version.
fn is_newer(latest: &str) -> bool {
    Version::parse(latest)
        .zip(Version::parse(crate::version()))
        .is_some_and(|(latest, running)| latest > running)
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

/// The latest release's version, e.g. `0.3.0` for the tag `v0.3.0`.
async fn latest_release(http: &reqwest::Client, url: &str) -> anyhow::Result<String> {
    let mut res = http
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .context("cannot reach GitHub")?;
    if !res.status().is_success() {
        bail!("GitHub answered HTTP {}", res.status());
    }
    let bytes = super::read_capped(&mut res, MAX_REPLY_BYTES, "GitHub's reply").await?;
    let release: Release =
        serde_json::from_slice(&bytes).context("unexpected reply from GitHub")?;
    let tag = release.tag_name.trim();
    let version = tag.strip_prefix('v').unwrap_or(tag);
    if Version::parse(version).is_none() {
        let shown: String = tag.chars().take(MAX_PRE_LEN).collect();
        bail!("the latest release is tagged {shown:?}, not vX.Y.Z");
    }
    Ok(version.to_owned())
}

/// A version as `X.Y.Z`, with an optional pre-release (`-rc.1`); build
/// metadata (`+1a2b3c4`, a commit between releases) is left out. Ordered as
/// in Semantic Versioning; equal when the order says so (`rc.01` is `rc.1`).
#[derive(Debug)]
struct Version {
    core: (u64, u64, u64),
    pre: Vec<String>,
}

/// The longest pre-release part taken, e.g. `rc.1`.
const MAX_PRE_LEN: usize = 64;

impl Version {
    fn parse(s: &str) -> Option<Self> {
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((core, pre)) => {
                // As Semantic Versioning allows, and short: a tag that gets
                // into the log can't carry anything else.
                let allowed = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '.';
                if pre.len() > MAX_PRE_LEN || !pre.chars().all(allowed) {
                    return None;
                }
                (core, pre.split('.').map(str::to_owned).collect())
            }
            None => (s, Vec::new()),
        };
        let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
        let version = Self {
            core: (parts.next()??, parts.next()??, parts.next()??),
            pre,
        };
        (parts.next().is_none() && !version.pre.iter().any(String::is_empty)).then_some(version)
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // A pre-release comes before its release.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(&other.pre) {
                        let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                            (Ok(a), Ok(b)) => a.cmp(&b),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => a.cmp(b),
                        };
                        if order != Ordering::Equal {
                            return order;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::MachineConfig;
    use crate::server::wait_for;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use std::sync::Mutex;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s}"))
    }

    #[test]
    fn versions_compare_as_semver() {
        assert!(v("0.3.0") > v("0.2.1"));
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(
            v("0.2.1"),
            v("0.2.1+1a2b3c4"),
            "a commit build is its release"
        );
        assert!(v("0.3.0") > v("0.3.0-rc.1"));
        assert!(v("0.3.0-rc.2") > v("0.3.0-rc.1"));
        assert!(v("0.3.0-rc.10") > v("0.3.0-rc.9"));
        assert!(v("0.3.0-rc") > v("0.3.0-1"));
        assert!(v("0.3.0-rc.1") > v("0.3.0-rc"));
        assert_eq!(v("0.3.0-rc.01"), v("0.3.0-rc.1"), "equal as ordered");
        let long = format!("1.2.3-{}", "x".repeat(MAX_PRE_LEN + 1));
        assert_eq!(Version::parse(&long), None, "too long a pre-release");
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "1.x.3",
            "1.2.3-",
            "1.2.3-a..b",
            "1.2.3-rc 1",
            "1.2.3-rc\n",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
    }

    /// A fake of the releases API: answers with `reply`, and records the
    /// User-Agent it was sent.
    async fn fake(
        reply: Arc<Mutex<(StatusCode, String)>>,
        agents: Arc<Mutex<Vec<String>>>,
    ) -> String {
        let app = axum::Router::new().route(
            "/latest",
            axum::routing::get(move |headers: HeaderMap| {
                let reply = reply.clone();
                let agents = agents.clone();
                async move {
                    let agent = headers
                        .get("user-agent")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default();
                    agents.lock().unwrap().push(agent.to_owned());
                    let (code, body) = reply.lock().unwrap().clone();
                    (code, body).into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/latest")
    }

    #[tokio::test]
    async fn finds_a_newer_release_and_keeps_it_when_a_look_fails() {
        let reply = Arc::new(Mutex::new((
            StatusCode::OK,
            r#"{"tag_name":"v999.0.0","name":"999.0.0","body":"..."}"#.to_owned(),
        )));
        let agents = Arc::new(Mutex::new(Vec::new()));
        let url = fake(reply.clone(), agents.clone()).await;
        let server = Server::new(MachineConfig::default());
        let state = server.update_json();
        assert!(!server.update_check_on());
        assert_eq!(state["latest"], Value::Null);

        server.set_update_check(true);
        server.start_update_check(UpdateConfig {
            url,
            first: Duration::from_millis(10),
            every: Duration::from_millis(50),
            retry: Duration::from_millis(50),
            timeout: Duration::from_secs(5),
        });
        wait_for("the first look", || {
            server.update_json()["latest"] == "999.0.0"
        })
        .await;
        let state = server.update_json();
        assert!(server.update_check_on());
        assert_eq!(state["available"], true);
        assert!(state.get("current").is_none(), "server.version says it");
        assert_eq!(state["url"], RELEASES_PAGE);
        assert_eq!(state["failing"], false);
        let agent = agents.lock().unwrap()[0].clone();
        assert_eq!(agent, format!("gbs-anywhere/{}", crate::version()));

        // GitHub fails: the last result stays, and the app hears of it.
        *reply.lock().unwrap() = (StatusCode::SERVICE_UNAVAILABLE, String::new());
        let asked = agents.lock().unwrap().len();
        wait_for("a failed look", || agents.lock().unwrap().len() > asked + 1).await;
        let state = server.update_json();
        assert_eq!(state["latest"], "999.0.0");
        assert_eq!(state["failing"], true);

        // The running version is the latest: nothing to get.
        let current = crate::version().split('+').next().unwrap().to_owned();
        *reply.lock().unwrap() = (StatusCode::OK, format!(r#"{{"tag_name":"v{current}"}}"#));
        wait_for("the same version", || {
            server.update_json()["latest"] == current.as_str()
        })
        .await;
        assert_eq!(server.update_json()["available"], false);
        assert_eq!(server.update_json()["failing"], false);
    }

    #[tokio::test]
    async fn sends_nothing_while_it_is_off() {
        let reply = Arc::new(Mutex::new((
            StatusCode::OK,
            r#"{"tag_name":"v999.0.0"}"#.to_owned(),
        )));
        let agents = Arc::new(Mutex::new(Vec::new()));
        let url = fake(reply, agents.clone()).await;
        let server = Server::new(MachineConfig::default());
        server.start_update_check(UpdateConfig {
            url,
            first: Duration::from_millis(10),
            every: Duration::from_millis(50),
            retry: Duration::from_millis(50),
            timeout: Duration::from_secs(5),
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(agents.lock().unwrap().is_empty(), "off: nothing sent");

        // Turned on in the app: it looks soon.
        server.set_update_check(true);
        wait_for("a look", || server.update_json()["latest"] == "999.0.0").await;

        // Off again: no more looks, and the page says it is off.
        server.set_update_check(false);
        let asked = agents.lock().unwrap().len();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            agents.lock().unwrap().len() <= asked + 1,
            "at most one look in flight"
        );
        let asked = agents.lock().unwrap().len();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(agents.lock().unwrap().len(), asked);
        let state = server.update_json();
        assert!(!server.update_check_on());
        assert_eq!(state["latest"], Value::Null, "an old result is not kept");
    }

    #[tokio::test]
    async fn an_answer_after_it_was_turned_off_is_not_kept() {
        // GitHub answers slowly; the check is turned off while it waits.
        let app = axum::Router::new().route(
            "/latest",
            axum::routing::get(|| async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                r#"{"tag_name":"v999.0.0"}"#
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let server = Server::new(MachineConfig::default());
        server.set_update_check(true);
        server.start_update_check(UpdateConfig {
            url: format!("http://{addr}/latest"),
            first: Duration::from_millis(10),
            every: Duration::from_secs(60),
            retry: Duration::from_secs(60),
            timeout: Duration::from_secs(5),
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        server.set_update_check(false);
        tokio::time::sleep(Duration::from_millis(400)).await;
        let state = server.update_json();
        assert!(!server.update_check_on());
        assert_eq!(state["latest"], Value::Null, "the late answer is dropped");
    }

    #[tokio::test]
    async fn an_odd_tag_is_not_a_version() {
        let reply = Arc::new(Mutex::new((
            StatusCode::OK,
            r#"{"tag_name":"nightly"}"#.to_owned(),
        )));
        let url = fake(reply, Arc::default()).await;
        let http = crate::http::client();
        let e = latest_release(&http, &url).await.unwrap_err();
        assert!(e.to_string().contains("not vX.Y.Z"), "{e}");
    }

    #[tokio::test]
    async fn only_one_v_is_taken_off() {
        let reply = Arc::new(Mutex::new((
            StatusCode::OK,
            r#"{"tag_name":"vv1.2.3"}"#.to_owned(),
        )));
        let url = fake(reply.clone(), Arc::default()).await;
        let http = crate::http::client();
        assert!(latest_release(&http, &url).await.is_err());
        *reply.lock().unwrap() = (StatusCode::OK, r#"{"tag_name":"v1.2.3"}"#.to_owned());
        assert_eq!(latest_release(&http, &url).await.unwrap(), "1.2.3");
    }

    #[tokio::test]
    async fn a_manual_check_looks_right_away_then_waits() {
        let reply = Arc::new(Mutex::new((
            StatusCode::OK,
            r#"{"tag_name":"v999.0.0"}"#.to_owned(),
        )));
        let agents = Arc::new(Mutex::new(Vec::new()));
        let url = fake(reply, agents.clone()).await;
        let server = Server::new(MachineConfig::default());
        server.set_update_check(true);
        // A long interval, so only a manual re-check makes it look soon.
        server.start_update_check(UpdateConfig {
            url,
            first: Duration::from_secs(600),
            every: Duration::from_secs(600),
            retry: Duration::from_secs(600),
            timeout: Duration::from_secs(5),
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            agents.lock().unwrap().is_empty(),
            "no look before it is due"
        );

        // Asking looks now, despite the long interval.
        server.request_update_check();
        wait_for("the manual look", || {
            server.update_json()["latest"] == "999.0.0"
        })
        .await;
        assert_eq!(agents.lock().unwrap().len(), 1, "one look");
        assert_eq!(server.update_json()["checking"], false, "the look finished");

        // Asking again right away is ignored (less than the minimum gap).
        server.request_update_check();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(agents.lock().unwrap().len(), 1, "debounced");
    }
}
