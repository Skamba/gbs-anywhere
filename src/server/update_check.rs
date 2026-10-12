//! Looks for a newer release once a day, so the app can say when one is out
//! (`update` in `GET /api/state`). It asks GitHub for the latest release of
//! gbs-anywhere and sends nothing but that request, with the version in the
//! User-Agent. `--update-check off` turns it off.

use std::cmp::Ordering;
use std::sync::{Arc, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::server::Server;

/// Where the latest release is asked for.
pub const RELEASES_API: &str = "https://api.github.com/repos/Skamba/gbs-anywhere/releases/latest";

/// The page people open to get it.
pub const RELEASES_PAGE: &str = "https://github.com/Skamba/gbs-anywhere/releases/latest";

/// A release description is a few kilobytes; anything this big is not one.
const MAX_REPLY_BYTES: usize = 1024 * 1024;

/// When and where to look.
#[derive(Debug, Clone)]
pub struct UpdateConfig {
    /// [`RELEASES_API`], or a local fake in tests.
    pub url: String,
    /// After the start, before the first look.
    pub first: Duration,
    /// Between looks.
    pub every: Duration,
    /// After a failed look.
    pub retry: Duration,
    pub timeout: Duration,
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
    enabled: bool,
    /// `0.3.0`, from the release tag `v0.3.0`.
    latest: Option<String>,
    /// When `latest` was found.
    checked: Option<Instant>,
    /// The last look failed (said once in the log, not every hour).
    failing: bool,
}

impl Server {
    /// Starts looking for newer releases.
    pub fn start_update_check(self: &Arc<Self>, cfg: UpdateConfig) {
        self.update_state().enabled = true;
        tokio::spawn(run(self.clone(), cfg));
    }

    /// `update` in `GET /api/state`.
    pub fn update_json(&self) -> Value {
        let current = crate::version();
        let s = self.update_state();
        let available = s
            .latest
            .as_deref()
            .and_then(Version::parse)
            .zip(Version::parse(current))
            .is_some_and(|(latest, running)| latest > running);
        json!({
            "enabled": s.enabled,
            "current": current,
            "latest": s.latest,
            "available": available,
            "url": RELEASES_PAGE,
            "checked_age_s": s.checked.map(|t| t.elapsed().as_secs()),
        })
    }

    fn update_state(&self) -> MutexGuard<'_, UpdateState> {
        self.update.lock().unwrap_or_else(|e| e.into_inner())
    }
}

async fn run(server: Arc<Server>, cfg: UpdateConfig) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = match reqwest::Client::builder()
        .user_agent(format!("gbs-anywhere/{}", crate::version()))
        .timeout(cfg.timeout)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("update check: cannot build the HTTP client: {e}");
            return;
        }
    };
    tokio::time::sleep(cfg.first).await;
    loop {
        let wait = match latest_release(&http, &cfg.url).await {
            Ok(latest) => {
                let newer = Version::parse(&latest)
                    .zip(Version::parse(crate::version()))
                    .is_some_and(|(l, r)| l > r);
                let mut s = server.update_state();
                if newer && s.latest.as_deref() != Some(latest.as_str()) {
                    tracing::info!(
                        "update: gbs-anywhere {latest} is out (this is {}): {RELEASES_PAGE}",
                        crate::version()
                    );
                }
                s.latest = Some(latest);
                s.checked = Some(Instant::now());
                s.failing = false;
                cfg.every
            }
            Err(e) => {
                let mut s = server.update_state();
                if !s.failing {
                    tracing::info!(
                        "update check: could not look for a newer version, trying again in {}: \
                         {e:#}",
                        spoken(cfg.retry)
                    );
                }
                s.failing = true;
                cfg.retry
            }
        };
        tokio::time::sleep(wait).await;
    }
}

/// A wait as people say it: `1 hour`, `5 minutes`, `30 s`.
fn spoken(d: Duration) -> String {
    let secs = d.as_secs();
    let plural = |n: u64, one: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {one}s")
        }
    };
    match secs {
        s if s >= 3600 && s % 3600 == 0 => plural(s / 3600, "hour"),
        s if s >= 60 && s % 60 == 0 => plural(s / 60, "minute"),
        s => format!("{s} s"),
    }
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
    let mut bytes = Vec::new();
    while let Some(chunk) = res.chunk().await.context("reading GitHub's reply")? {
        if bytes.len() + chunk.len() > MAX_REPLY_BYTES {
            bail!("GitHub's reply is too large");
        }
        bytes.extend_from_slice(&chunk);
    }
    let release: Release =
        serde_json::from_slice(&bytes).context("unexpected reply from GitHub")?;
    let version = release.tag_name.trim().trim_start_matches('v');
    if Version::parse(version).is_none() {
        bail!(
            "the latest release is tagged {:?}, not vX.Y.Z",
            release.tag_name
        );
    }
    Ok(version.to_owned())
}

/// A version as `X.Y.Z`, with an optional pre-release (`-rc.1`); build
/// metadata (`+1a2b3c4`, a commit between releases) is left out. Ordered as
/// in Semantic Versioning.
#[derive(Debug, PartialEq, Eq)]
struct Version {
    core: (u64, u64, u64),
    pre: Vec<String>,
}

impl Version {
    fn parse(s: &str) -> Option<Self> {
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((core, pre)) => (core, pre.split('.').map(str::to_owned).collect()),
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
    fn waits_read_naturally() {
        assert_eq!(spoken(Duration::from_secs(3600)), "1 hour");
        assert_eq!(spoken(Duration::from_secs(7200)), "2 hours");
        assert_eq!(spoken(Duration::from_secs(300)), "5 minutes");
        assert_eq!(spoken(Duration::from_millis(50)), "0 s");
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
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "1.x.3",
            "1.2.3-",
            "1.2.3-a..b",
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
        assert_eq!(state["enabled"], false);
        assert_eq!(state["latest"], Value::Null);
        assert_eq!(state["checked_age_s"], Value::Null);

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
        assert_eq!(state["enabled"], true);
        assert_eq!(state["available"], true);
        assert_eq!(state["current"], crate::version());
        assert_eq!(state["url"], RELEASES_PAGE);
        assert!(state["checked_age_s"].as_u64().unwrap() < 2);
        let agent = agents.lock().unwrap()[0].clone();
        assert_eq!(agent, format!("gbs-anywhere/{}", crate::version()));

        // GitHub fails: the last result stays.
        *reply.lock().unwrap() = (StatusCode::SERVICE_UNAVAILABLE, String::new());
        let asked = agents.lock().unwrap().len();
        wait_for("a failed look", || agents.lock().unwrap().len() > asked + 1).await;
        assert_eq!(server.update_json()["latest"], "999.0.0");
        assert!(server.update_state().failing);

        // The running version is the latest: nothing to get.
        let current = crate::version().split('+').next().unwrap().to_owned();
        *reply.lock().unwrap() = (StatusCode::OK, format!(r#"{{"tag_name":"v{current}"}}"#));
        wait_for("the same version", || {
            server.update_json()["latest"] == current.as_str()
        })
        .await;
        assert_eq!(server.update_json()["available"], false);
    }

    #[tokio::test]
    async fn an_odd_tag_is_not_a_version() {
        let reply = Arc::new(Mutex::new((
            StatusCode::OK,
            r#"{"tag_name":"nightly"}"#.to_owned(),
        )));
        let url = fake(reply, Arc::default()).await;
        let http = reqwest::Client::new();
        let e = latest_release(&http, &url).await.unwrap_err();
        assert!(e.to_string().contains("not vX.Y.Z"), "{e}");
    }
}
