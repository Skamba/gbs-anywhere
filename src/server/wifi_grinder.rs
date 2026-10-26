//! The link to an E65/E80 GbS grinder over its Local WiFi option (see
//! [`crate::protocol::wifi`]). gbs-anywhere pairs once, then asks the grinder
//! for brew jobs every couple of seconds, which also keeps it shown as active
//! there. A job (one per Grind-by-Sync grind) starts a brew of the shared
//! machine, like the E64 WS's knob press, so the app and the integrations
//! report the shot exactly as they do there.
//!
//! The grinder drops a job nobody takes within about a minute, and hands a
//! taken job back out about every 50 s while no result follows. So a job is
//! taken as soon as it shows up, and again whenever it is listed again: that
//! keeps it open for as long as the machine waits for the shot
//! (`--shot-window-s`), after which it is answered as cancelled. Until the
//! grinder has the result, it is retried on every poll, so a short network
//! outage loses no shot.
//!
//! The job that showed up last is the shot being pulled: a newer grind
//! cancels a job still waiting for its shot, and other open jobs are left
//! for the grinder to time out.

use std::collections::VecDeque;
use std::net::{Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::{OwnedMutexGuard, mpsc};
use tokio::time::MissedTickBehavior;

use crate::integration::Backoff;
use crate::protocol::machine::EventRecord;
use crate::protocol::wifi::{
    self, API_VERSION, BrewAccept, BrewDone, BrewJob, BrewJobsRes, BrewResult, Paired,
    PairingApplication, PairingApplicationRes, PairingStatusRes,
};
use crate::protocol::{AbortCause, GrindResult, MachineEvent, Phase};
use crate::server::Server;

/// Between job requests. The grinder shows the machine as active while it
/// asks.
const POLL: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const MIN_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// The longest wait after a failed request while unpaired: someone is
/// pairing, and a Pairing Code follows soon after the grinder answers again
/// (say, after it restarted).
const UNPAIRED_BACKOFF: Duration = Duration::from_secs(10);
/// How long the app is asked for the pairing ID before gbs-anywhere applies
/// to pair again, in case the pairing was cleared on the grinder meanwhile.
const ASK_WINDOW: Duration = Duration::from_secs(60);
/// How long nothing is sent to a grinder that stopped answering while
/// pairing. The grinder may freeze and restart after the person accepts
/// (about a minute and a half until it answers again); gbs-anywhere leaves
/// it alone until it is back. The API shows it as `quiet_ms`.
const RESTART_QUIET: Duration = Duration::from_secs(180);
/// How long from the person's confirmation in the app to the pairing check,
/// however long after the code it comes. Nothing is sent to the grinder
/// meanwhile, so it is left alone right after the accept; the app and its
/// API keep serving.
const ACCEPT_SETTLE: Duration = Duration::from_secs(30);
/// Between pairing checks while the code is shown and nobody has confirmed
/// in the app. The grinder keeps the pairing request open for a long time,
/// so the confirmation is waited for as long as it takes; this slow check
/// only finds an accept nobody confirms. Long enough that a person who
/// accepts on the grinder and then taps in the app normally gets the quiet
/// moment of `ACCEPT_SETTLE` first.
const UNCONFIRMED_POLL: Duration = Duration::from_secs(40);
/// Replies are a few hundred bytes; anything this big is not the grinder.
const MAX_REPLY_BYTES: usize = 64 * 1024;
/// How long the grinder must refuse every job request before the pairing
/// counts as cleared there. It also refuses for a few seconds while it starts
/// up (after a power cut or standby), with the pairing still valid.
const REFUSED_FOR: Duration = Duration::from_secs(60);
/// How long a measured shot is retried before it is given up.
const ANSWER_LIMIT: Duration = Duration::from_secs(120);
/// How long a cancelled shot is retried. The grinder times the job out by
/// itself anyway, and a newer grind waits until it is given up.
const CANCEL_LIMIT: Duration = Duration::from_secs(20);
/// Jobs remembered as handled or seen, so a late copy of one is never brewed
/// again and the newest is known.
const HANDLED_CAP: usize = 16;

/// Where the grinder is and where the pairing is kept.
#[derive(Debug, Clone)]
pub struct WifiConfig {
    /// `https://<grinder>:30001`, or a plain-HTTP fake in tests.
    pub base_url: String,
    /// Keeps the pairing across restarts. Without it, every start pairs anew.
    pub pairing_file: Option<PathBuf>,
    /// The pairing ID the grinder shows. Used when the file has none, or when
    /// the grinder refuses the file's.
    pub pairing_id: Option<u32>,
    /// Between job requests.
    pub poll: Duration,
    /// How long nothing is sent after a pairing check got no answer (see
    /// `RESTART_QUIET`).
    pub restart_quiet: Duration,
    /// How long refused job requests take to count as a cleared pairing (see
    /// `REFUSED_FOR`).
    pub refused_for: Duration,
    /// How long from the confirmation in the app to the pairing check (see
    /// `ACCEPT_SETTLE`).
    pub accept_settle: Duration,
    /// Between pairing checks while nobody has confirmed in the app (see
    /// `UNCONFIRMED_POLL`).
    pub unconfirmed_poll: Duration,
    /// The wait after a failed request, from `min_backoff` doubling up to
    /// `max_backoff`, and at most `unpaired_backoff` while unpaired (see
    /// `UNPAIRED_BACKOFF`).
    pub min_backoff: Duration,
    pub max_backoff: Duration,
    pub unpaired_backoff: Duration,
    /// How long the app is asked for the pairing ID at a time (see
    /// `ASK_WINDOW`).
    pub ask_window: Duration,
}

impl WifiConfig {
    /// From an address as people type it: `192.168.1.30`, `grinder.local`,
    /// `host:port`, or (on the command line, for tests) a full URL. The app
    /// can only set what [`grinder_address`] allows.
    pub fn from_host(host: &str) -> anyhow::Result<Self> {
        let host = host.trim().trim_end_matches('/');
        if host.is_empty() {
            bail!("the grinder address is empty");
        }
        if host.len() > MAX_URL_LEN || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
            bail!("the grinder address has spaces or is too long: {ADDRESS_HINT}");
        }
        let base_url = base_url(host);
        let url = reqwest::Url::parse(&base_url).ok();
        if url.as_ref().and_then(reqwest::Url::host_str).is_none() {
            bail!("`{host}` is not an address: enter the grinder's IP address, e.g. 192.168.1.30");
        }
        Ok(Self {
            base_url,
            pairing_file: None,
            pairing_id: None,
            poll: POLL,
            restart_quiet: RESTART_QUIET,
            refused_for: REFUSED_FOR,
            accept_settle: ACCEPT_SETTLE,
            unconfirmed_poll: UNCONFIRMED_POLL,
            min_backoff: MIN_BACKOFF,
            max_backoff: MAX_BACKOFF,
            unpaired_backoff: UNPAIRED_BACKOFF,
            ask_window: ASK_WINDOW,
        })
    }
}

/// What to type as the grinder's address.
const ADDRESS_HINT: &str = "enter the grinder's IP address, e.g. 192.168.1.30 (optionally :port)";

/// The longest address the app may set: a host name has at most 253
/// characters, then the port.
const MAX_ADDRESS_LEN: usize = 255;

/// The longest address `--grinder` takes (it may be a whole URL).
const MAX_URL_LEN: usize = 2048;

/// The grinder's address as the app may set it: `host`, `host:port` or
/// `[v6]:port`; no scheme, path, login, spaces or control characters, so
/// requests always go to `https://<host>:<port>` on the grinder. Returns it
/// tidied up: trimmed, in lower case, an IPv6 address in brackets, and
/// without an empty port.
pub fn grinder_address(input: &str) -> anyhow::Result<String> {
    let s = input.trim();
    if s.is_empty() || s.len() > MAX_ADDRESS_LEN {
        bail!("{ADDRESS_HINT}");
    }
    if let Ok(ip) = s.parse::<Ipv6Addr>() {
        return Ok(format!("[{ip}]"));
    }
    let (host, port) = match s.strip_prefix('[') {
        Some(rest) => {
            let (v6, after) = rest.split_once(']').context(ADDRESS_HINT)?;
            let ip: Ipv6Addr = v6.parse().ok().context(ADDRESS_HINT)?;
            let port = match after {
                "" => None,
                p => Some(p.strip_prefix(':').context(ADDRESS_HINT)?),
            };
            (format!("[{ip}]"), port)
        }
        None => {
            let (host, port) = match s.split_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (s, None),
            };
            let name_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '.';
            if host.is_empty() || host.starts_with(['-', '.']) || !host.chars().all(name_char) {
                bail!("{ADDRESS_HINT}");
            }
            (host.to_ascii_lowercase(), port)
        }
    };
    match port {
        None | Some("") => Ok(host),
        Some(p) => {
            let port = p
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .context(ADDRESS_HINT)?;
            Ok(format!("{host}:{port}"))
        }
    }
}

/// `https://host:30001` unless the address names a scheme or port.
fn base_url(host: &str) -> String {
    if host.contains("://") {
        return host.to_owned();
    }
    let port = wifi::DEFAULT_PORT;
    let with_port = match host.rsplit_once(':') {
        // `host:port` or `[v6]:port`.
        Some((h, p)) if p.parse::<u16>().is_ok() && (!h.contains(':') || h.ends_with(']')) => {
            host.to_owned()
        }
        // `host:` with the port left out.
        Some((h, "")) if !h.contains(':') => format!("{h}:{port}"),
        _ if host.starts_with('[') => format!("{host}:{port}"),
        // A bare IPv6 address.
        _ if host.contains(':') => format!("[{host}]:{port}"),
        _ => format!("{host}:{port}"),
    };
    format!("https://{with_port}")
}

/// The Local WiFi link as the app shows it (`grinder.wifi` in
/// `GET /api/state`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WifiLink {
    /// The grinder's address, as configured.
    pub grinder: String,
    pub state: WifiState,
    /// Since when `state` holds, set by the server as the link reports it
    /// (see `Server::set_wifi`); the API shows it as `state_age_ms`.
    #[serde(skip)]
    pub state_since: Option<Instant>,
    /// Where in the pairing the link is, while `state` is `pairing`; `None`
    /// otherwise.
    pub phase: Option<WifiPhase>,
    /// Since when `phase` holds; the API shows it as `phase_age_ms`.
    #[serde(skip)]
    pub phase_since: Option<Instant>,
    /// While pairing: the Pairing Code the grinder shows next to
    /// gbs-anywhere for this attempt (`pairingCode`).
    pub pairing_code: Option<u32>,
    /// While asking: the last pairing ID the grinder did not take, so the
    /// app can say so.
    pub unknown_pairing_id: Option<u32>,
    /// The grinder went quiet while pairing and came back without the
    /// pairing request: a pairing accepted before did not stick, and the
    /// app asks to accept again. Cleared once paired,
    /// or once the grinder turns out to have the pairing after all.
    pub pairing_lost: bool,
    /// How long gbs-anywhere sends the grinder nothing after a pairing check
    /// went unanswered (phase `quiet`), so an app can count down to the next
    /// check.
    pub quiet_ms: u64,
    /// What is going on, in one sentence.
    pub detail: String,
    /// What went wrong with the last shot, until the next one goes through.
    pub problem: Option<String>,
    /// The Pairing ID in use, the pairing's lasting number (`pairingId`);
    /// `None` while unpaired.
    pub pairing_id: Option<u32>,
    /// The grinder does not answer at all (off, in standby or gone from the
    /// network), as opposed to answering with an error. Cleared as soon as
    /// it answers again.
    pub unreachable: bool,
    /// Not paired, and the grinder answered the pairing application with a
    /// client error, as one not set up for pairing would: the app says it is
    /// not set up rather than failing. Cleared once it takes one, or fails
    /// some other way.
    pub not_set_up: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WifiState {
    Connecting,
    /// Pairing with the grinder; `WifiLink::phase` says where in it.
    Pairing,
    Paired,
    /// A request failed; retrying.
    Failing,
}

/// Where in the pairing the link is (`grinder.wifi.phase`), set only while
/// its state is [`WifiState::Pairing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WifiPhase {
    /// The code is shown: gbs-anywhere waits for the person to accept it on
    /// the grinder and confirm that in the app (`POST /api/grinder/accepted`
    /// is taken only now), for as long as it takes. Meanwhile it checks the
    /// pairing only every so often (see [`UNCONFIRMED_POLL`]).
    Accepting,
    /// Confirmed: gbs-anywhere waits a moment before it checks the pairing
    /// (see [`ACCEPT_SETTLE`]), and sends the grinder nothing meanwhile.
    Holding,
    /// Confirmed in the app: gbs-anywhere checks with the grinder whether the
    /// pairing was accepted. If not, it is back to `Accepting`.
    Checking,
    /// A pairing check went unanswered: gbs-anywhere sends the grinder
    /// nothing for a while (see [`RESTART_QUIET`]), then checks again.
    Quiet,
    /// The grinder has gbs-anywhere paired but the pairing ID is missing
    /// here: the app asks for the one the grinder shows
    /// (`POST /api/grinder/pairing` is taken only now).
    Asking,
}

/// Why a pairing ID or a confirmation from the app was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingIdRefusal {
    /// No `--grinder` link runs.
    NoLink,
    /// The link has a working pairing; it needs no ID.
    NotNeeded,
    /// The link is not paired, but not asking for an ID either: it is
    /// connecting or in another phase of the pairing.
    NotAsking,
    /// The link is not waiting for an accept right now, so there is nothing
    /// to confirm.
    NotAccepting,
    /// The last ID or confirmation is still being handled.
    Busy,
}

impl std::fmt::Display for PairingIdRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoLink => {
                "no E65/E80 grinder is set up: enter its address in the app (Set up grinder), or \
                 start gbs-anywhere with --grinder"
            }
            Self::NotNeeded => "gbs-anywhere is paired with the grinder and needs no pairing ID",
            Self::NotAsking => {
                "gbs-anywhere is not asking for a pairing ID right now: it takes one only when the \
                 grinder has it paired but the ID is missing here"
            }
            Self::NotAccepting => {
                "gbs-anywhere is not waiting for you to accept it on the grinder right now"
            }
            Self::Busy => "the last request is still being handled, try again in a moment",
        })
    }
}

impl PairingIdRefusal {
    /// As a fixed code, for `code` in the API's errors.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoLink => "grinder_not_set_up",
            Self::NotNeeded => "grinder_already_paired",
            Self::NotAsking => "grinder_not_asking",
            Self::NotAccepting => "grinder_not_accepting",
            Self::Busy => "grinder_pairing_busy",
        }
    }
}

/// Held by the link that may write the pairing file, for as long as its
/// task lives. A replaced link is aborted, which only ends it at its next
/// await, and it may be saving the pairing right then: the next link waits
/// for it before it loads the pairing, so it starts from what the old one
/// saved and the file always has one writer.
pub type PairingLock = Arc<tokio::sync::Mutex<()>>;

/// What survives a restart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Pairing {
    /// Who this gbs-anywhere is to the grinder; made once.
    brew_device_id: String,
    /// Set once the grinder accepted the pairing.
    pairing_id: Option<u32>,
    /// The pairing ID the grinder refused for a while, kept to be tried again
    /// before the app asks for one: the grinder may have refused it only
    /// while it was starting up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refused_pairing_id: Option<u32>,
}

impl Pairing {
    fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        let fresh = || Self {
            brew_device_id: uuid::Uuid::new_v4().to_string(),
            pairing_id: None,
            refused_pairing_id: None,
        };
        let Some(path) = path else {
            return Ok(fresh());
        };
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| {
                format!(
                    "{}: not a grinder pairing file (delete it to pair again)",
                    path.display()
                )
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(fresh()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// The pairing in `cfg`'s file, and the command line's pairing ID when
    /// it is another one, to fall back on.
    fn open(cfg: &WifiConfig) -> anyhow::Result<(Self, Option<u32>)> {
        let mut pairing = Self::load(cfg.pairing_file.as_deref())?;
        let mut spare_id = cfg.pairing_id.filter(|id| pairing.pairing_id != Some(*id));
        if pairing.pairing_id.is_none() {
            pairing.pairing_id = spare_id.take();
        }
        Ok((pairing, spare_id))
    }

    fn save(&self, path: Option<&Path>) -> anyhow::Result<()> {
        let Some(path) = path else {
            return Ok(());
        };
        let json = serde_json::to_vec_pretty(self)?;
        crate::server::settings::save_private(path, &json)
    }
}

/// A grinder reply: HTTP status, JSON body (`Null` when empty) and where it
/// came from.
struct Reply {
    http: StatusCode,
    body: Value,
    peer: Option<SocketAddr>,
}

impl Reply {
    fn status(&self) -> &str {
        self.body
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    /// `Err` with the grinder's status unless it is `Ok`.
    fn ok(&self) -> anyhow::Result<()> {
        match self.status() {
            wifi::STATUS_OK => Ok(()),
            "" => bail!("the grinder answered HTTP {}", self.http),
            s => bail!("the grinder answered {s:?}"),
        }
    }

    fn parse<T: DeserializeOwned>(self) -> anyhow::Result<T> {
        serde_json::from_value(self.body).context("unexpected reply from the grinder")
    }
}

/// The job whose shot is being brewed, held open on the grinder.
struct Held {
    job: BrewJob,
    /// The grinder has it as taken: it is not listed. A listed job is still
    /// (or again) waiting to be taken.
    accepted: bool,
}

/// A measured (or cancelled) shot on its way to the grinder.
struct Answer {
    job: BrewJob,
    /// Built once, so every retry carries the same event id.
    done: BrewDone,
    /// The grinder took the job; only the result is still to send.
    accepted: bool,
    since: Instant,
    /// The status line once it went through.
    line: String,
}

/// The task. Build it with [`WifiGrinder::new`], then [`WifiGrinder::run`] it.
pub struct WifiGrinder {
    cfg: WifiConfig,
    http: reqwest::Client,
    pairing: Pairing,
    /// A pairing ID from the command line to fall back on when the grinder
    /// refuses the saved one.
    spare_id: Option<u32>,
    /// The job whose shot is being brewed.
    current: Option<Held>,
    answer: Option<Answer>,
    handled: VecDeque<String>,
    /// Each job's grind id with the poll it first showed up in.
    first_seen: VecDeque<(String, u64)>,
    polls: u64,
    /// What went wrong with the last shot (shown in the app).
    problem: Option<String>,
    /// Which link this is to the server: what an older link that was
    /// replaced (new grinder address) still reports is ignored.
    owner: u64,
    /// Who may write the pairing file (see [`PairingLock`]).
    lock: PairingLock,
    /// Held while this link may write it.
    holding: Option<OwnedMutexGuard<()>>,
    /// The grinder could not be reached last time (it is off or in standby):
    /// further failures are not logged until it answers again, and the app
    /// says so (`WifiLink::unreachable`).
    unreachable: bool,
    /// The last failure logged as a warning, so the same one again (every
    /// 10 s while not paired) goes to debug instead of filling the log book.
    last_failure: Option<String>,
    /// The last pairing application was turned down (`WifiLink::not_set_up`).
    not_set_up: bool,
    /// The question for the pairing ID as last asked: what it said and the ID
    /// the grinder refused, carried into the next round of `ask_window`.
    asked: Option<(String, Option<u32>)>,
    /// The grinder came back from a restart without the pairing request (see
    /// `WifiLink::pairing_lost`).
    pairing_lost: bool,
    /// The phase shown while the link reports [`WifiState::Pairing`]:
    /// `Accepting` until the person confirms in the app (`Holding` while the
    /// check is then held), `Checking` while that check goes, `Quiet` while
    /// `stay_quiet` waits. `Asking` is shown by `ask_for_pairing_id`.
    phase: WifiPhase,
    /// Since when `phase` holds (for `Quiet`, the current round), for
    /// `WifiLink::phase_since`.
    phase_since: Instant,
}

impl WifiGrinder {
    /// Loads the pairing (or makes a device id) and saves it right away, so an
    /// unwritable file fails at startup rather than after pairing. While
    /// another link still holds the file (it is being replaced), the pairing
    /// is only read here, and loaded and saved again once that link has
    /// ended (see [`PairingLock`]).
    pub fn new(cfg: WifiConfig, lock: PairingLock) -> anyhow::Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        // Taken before the read: a link that ends in between may have saved.
        let holding = lock.clone().try_lock_owned().ok();
        let (pairing, spare_id) = Pairing::open(&cfg)?;
        if holding.is_some() {
            pairing.save(cfg.pairing_file.as_deref())?;
        }
        let http = reqwest::Client::builder()
            // The grinder's certificate is self-signed and names no host, so it
            // cannot be checked. Requests only go to the configured address,
            // never on to wherever a reply points.
            .tls_danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            // The grinder is on the home network: a proxy set for the
            // internet (HTTPS_PROXY, say for the update check) is not for it.
            .no_proxy()
            // The grinder only finds the body with `Content-Length` spelled
            // this way; lower-case header names leave it reading an empty one.
            .http1_title_case_headers()
            // The grinder drops kept-open connections without notice, and a
            // request sent on one fails with "connection closed before message
            // completed". A fresh connection for each request avoids that.
            .pool_max_idle_per_host(0)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("building the HTTP client")?;
        Ok(Self {
            cfg,
            http,
            pairing,
            spare_id,
            current: None,
            answer: None,
            handled: VecDeque::new(),
            first_seen: VecDeque::new(),
            polls: 0,
            problem: None,
            unreachable: false,
            last_failure: None,
            not_set_up: false,
            asked: None,
            pairing_lost: false,
            phase: WifiPhase::Checking,
            phase_since: Instant::now(),
            owner: 0,
            lock,
            holding,
        })
    }

    /// Loads the pairing again and saves it, as [`WifiGrinder::new`] does.
    fn reopen_pairing(&mut self) -> anyhow::Result<()> {
        (self.pairing, self.spare_id) = Pairing::open(&self.cfg)?;
        self.pairing.save(self.cfg.pairing_file.as_deref())
    }

    /// Where the grinder is, as requests go to it.
    pub fn base_url(&self) -> &str {
        &self.cfg.base_url
    }

    /// Runs until the process ends or the link is replaced: pairs when
    /// needed, then serves the grinder's jobs, retrying with backoff on
    /// errors. `owner` and `ids` come from attaching it to the server
    /// (`Server::attach_wifi`).
    pub(super) async fn run(
        mut self,
        server: Arc<Server>,
        owner: u64,
        mut ids: mpsc::Receiver<u32>,
        mut accept: mpsc::Receiver<()>,
    ) {
        self.owner = owner;
        let mut backoff = Backoff::new(self.cfg.min_backoff, self.cfg.max_backoff);
        if self.holding.is_none() {
            // The link this one replaces may still be saving the pairing:
            // start from what it saved, once it has ended.
            self.holding = Some(self.lock.clone().lock_owned().await);
            while let Err(e) = self.reopen_pairing() {
                tracing::warn!("grinder: {e:#}");
                self.status(&server, WifiState::Failing, None, format!("{e:#}"));
                backoff.wait().await;
            }
            backoff.reset();
        }
        // One subscription for the whole run, so a shot reported while a
        // request is being retried is still answered.
        let mut events = server.subscribe();
        self.status(&server, WifiState::Connecting, None, "connecting");
        loop {
            // Said again by `pair` for each application it turns down.
            self.not_set_up = false;
            let outcome = match self.pairing.pairing_id {
                None => self.pair(&server, &mut ids, &mut accept).await,
                Some(id) => self.serve(&server, id, &mut events).await,
            };
            if let Err(e) = outcome {
                // Off or in standby for the night: say so once, not every minute.
                let unreachable = unreachable(&e);
                let message = format!("{e:#}");
                if unreachable && self.unreachable {
                    tracing::debug!("grinder: {e:#}");
                } else if unreachable {
                    tracing::warn!(
                        "grinder: not reachable (off or in standby?), checking again at least \
                         every minute (every 10 s while not paired): {e:#}"
                    );
                } else if self.last_failure.as_deref() == Some(message.as_str()) {
                    tracing::debug!("grinder: {message}");
                } else {
                    tracing::warn!("grinder: {message}");
                }
                self.last_failure = (!unreachable).then(|| message.clone());
                self.unreachable = unreachable;
                self.status(&server, WifiState::Failing, None, format!("{e:#}"));
                tokio::time::sleep(self.retry_delay(&mut backoff)).await;
            } else {
                backoff.reset();
                self.last_failure = None;
            }
        }
    }

    /// How long to wait before trying again after a failed request: the
    /// backoff, but at most `unpaired_backoff` while there is no pairing.
    fn retry_delay(&self, backoff: &mut Backoff) -> Duration {
        let delay = backoff.advance();
        if self.pairing.pairing_id.is_none() {
            delay.min(self.cfg.unpaired_backoff)
        } else {
            delay
        }
    }

    /// Applies for pairing and waits until the person accepts it on the
    /// grinder. `Ok` once paired, or when it should be tried again.
    async fn pair(
        &mut self,
        server: &Server,
        ids: &mut mpsc::Receiver<u32>,
        accept: &mut mpsc::Receiver<()>,
    ) -> anyhow::Result<()> {
        let app = PairingApplication::new(&self.pairing.brew_device_id);
        let reply = self.post(wifi::PATH_PAIRING_APPLICATION, &app).await?;
        if reply.status() == wifi::STATUS_ALREADY_PAIRED {
            return self.wait_for_pairing_id(server, ids).await;
        }
        // It answers but turns the application down: not set up for pairing.
        self.not_set_up = refused(&reply);
        reply.ok().context("asking to pair")?;
        let res: PairingApplicationRes = reply.parse()?;
        let application_id = res
            .application_id
            .context("the grinder sent no application ID")?;
        if self.pairing.refused_pairing_id.take().is_some() {
            // It no longer has this gbs-anywhere paired: that ID is gone.
            self.pairing.save(self.cfg.pairing_file.as_deref())?;
        }
        let code = res.pairing_code;
        let detail = format!(
            "{}on the grinder, under Configuration, tap Start Pairing and \
             accept gbs-anywhere{}",
            if self.pairing_lost {
                "the grinder no longer has the pairing request, so a pairing accepted on it did not \
                 stick: "
            } else {
                ""
            },
            code.map_or(String::new(), |c| format!(" (Pairing Code {c})"))
        );
        tracing::info!("grinder: {detail}");
        // The new code is shown first: a confirmation left from an earlier
        // attempt must not release this one (cf. `wait_for_pairing_id`
        // draining `ids`). A new code starts its own phase age, also after
        // an earlier one in `Accepting`.
        self.phase = WifiPhase::Accepting;
        self.phase_since = Instant::now();
        self.status(server, WifiState::Pairing, code, detail.clone());
        while accept.try_recv().is_ok() {}
        let shown = tokio::time::Instant::now();

        let ask = json!({ "version": API_VERSION, "applicationId": application_id });
        // Left alone after a check got no answer (see `stay_quiet`).
        let mut quiet = false;
        // The check (or the one before the quiet time) came after a
        // confirmation in the app.
        let mut confirmed = false;
        loop {
            // Shows the code and waits for the next check: after the
            // confirmation and a moment, or the slow check without one. After
            // the quiet time, the request is checked again at once.
            if !quiet {
                confirmed = self.await_check(server, accept, code, &detail, shown).await;
            }
            let outcome = self.post(wifi::PATH_PAIRING_STATUS, &ask).await;
            if no_answer(&outcome) {
                self.stay_quiet(server, outcome, !quiet).await;
                quiet = true;
                continue;
            }
            let reply = outcome?;
            if reply.status() == wifi::STATUS_APPLICATION_UNKNOWN {
                if quiet {
                    tracing::info!(
                        "grinder: it no longer has the pairing request (it restarted?), asking \
                         again"
                    );
                    self.pairing_lost = true;
                } else {
                    tracing::info!("grinder: the pairing request expired, asking again");
                }
                return Ok(());
            }
            reply.ok().context("checking the pairing")?;
            let status: PairingStatusRes = reply.parse()?;
            if !status.accepted {
                // Back without a restart, the request still open; or
                // confirmed before the grinder had it accepted. Either way
                // the code is shown again, and only a confirmation from now
                // on counts: the grinder has no accept for one sent before.
                if std::mem::take(&mut quiet) || confirmed {
                    if confirmed {
                        tracing::info!(
                            "grinder: the grinder has not accepted the pairing yet, waiting for \
                             the accept and a new confirmation in the app"
                        );
                    }
                    while accept.try_recv().is_ok() {}
                }
                continue;
            }
            let Some(id) = status.pairing_id else {
                return self.wait_for_pairing_id(server, ids).await;
            };
            self.use_pairing_id(id)?;
            tracing::info!("grinder: paired");
            return Ok(());
        }
    }

    /// Shows the code (phase `Accepting`) and waits for the next pairing
    /// check, for as long as the person takes to confirm in the app. Without
    /// a confirmation, the check is due `unconfirmed_poll` from now. A
    /// confirmation moves to `Holding`: the grinder is sent nothing for
    /// `accept_settle`, then the check is due, in phase `Checking`. Returns
    /// whether the person confirmed. `shown` is when the code was shown, for
    /// the log.
    async fn await_check(
        &mut self,
        server: &Server,
        accept: &mut mpsc::Receiver<()>,
        code: Option<u32>,
        detail: &str,
        shown: tokio::time::Instant,
    ) -> bool {
        // For the log: seconds since the code was shown.
        let after = |at: tokio::time::Instant| at.duration_since(shown).as_secs_f32();
        self.set_phase(WifiPhase::Accepting);
        self.status(server, WifiState::Pairing, code, detail);

        let slow = tokio::time::Instant::now() + self.cfg.unconfirmed_poll;
        let tapped = tokio::select! {
            biased;
            got = accept.recv() => got.is_some(),
            () = tokio::time::sleep_until(slow) => false,
        };
        if !tapped {
            // No confirmation, or the sender is gone (the link is being
            // replaced): the slow check, never sooner.
            tokio::time::sleep_until(slow).await;
            tracing::debug!(
                "grinder: checking the pairing {:.1} s after the code, not confirmed in the app",
                after(tokio::time::Instant::now())
            );
            return false;
        }

        let tap = tokio::time::Instant::now();
        tracing::info!(
            "grinder: confirmed in the app {:.1} s after the code, checking the pairing in {:.1} s",
            after(tap),
            self.cfg.accept_settle.as_secs_f32()
        );
        self.set_phase(WifiPhase::Holding);
        let hold = "accepted: waiting a moment, then checking the pairing";
        self.status(server, WifiState::Pairing, code, hold);
        tokio::time::sleep_until(tap + self.cfg.accept_settle).await;

        let now = tokio::time::Instant::now();
        tracing::info!(
            "grinder: checking the pairing {:.1} s after the code, {:.1} s after the confirmation",
            after(now),
            now.duration_since(tap).as_secs_f32()
        );
        self.set_phase(WifiPhase::Checking);
        // Back to the plain pairing detail for the check and `stay_quiet`.
        self.status(server, WifiState::Pairing, code, detail);
        true
    }

    /// After a pairing check got no answer: sends the grinder nothing for a
    /// while (see [`RESTART_QUIET`]). Shown as still pairing, in phase
    /// `Quiet` and not answering, and logged as a warning the `first` time
    /// in a row.
    async fn stay_quiet(&mut self, server: &Server, outcome: anyhow::Result<Reply>, first: bool) {
        let secs = self.cfg.restart_quiet.as_secs();
        let wait = if secs >= 60 {
            format!("{} min", secs.div_ceil(60))
        } else {
            format!("{secs} s")
        };
        let detail = format!(
            "no answer to the pairing check: nothing is sent to the grinder for {wait}, then \
             gbs-anywhere checks again (the grinder may be restarting)"
        );
        if first {
            tracing::warn!("grinder: {detail}: {}", failure(outcome));
        } else {
            tracing::debug!("grinder: {detail}: {}", failure(outcome));
        }
        self.unreachable = true;
        // Each round starts its own quiet time, also after a quiet one, so
        // `phase_age_ms` says how long until the next check.
        self.phase = WifiPhase::Quiet;
        self.phase_since = Instant::now();
        self.status(server, WifiState::Pairing, None, detail);
        tokio::time::sleep(self.cfg.restart_quiet).await;
    }

    /// The grinder has gbs-anywhere paired, but its pairing ID is not known
    /// here (the grinder restarted before saying it, or the file is lost).
    /// Tries the ID the grinder refused before, then waits for one typed in
    /// the app and checks it with the grinder. `Ok` once one works, or after a
    /// while, to apply again in case the pairing was cleared on the grinder
    /// meanwhile.
    async fn wait_for_pairing_id(
        &mut self,
        server: &Server,
        ids: &mut mpsc::Receiver<u32>,
    ) -> anyhow::Result<()> {
        let mut detail = "paired on the grinder, but the pairing ID is missing here: enter the \
                          Pairing ID the grinder shows under Configuration"
            .to_owned();
        let mut refused_id = None;
        // The grinder has the pairing: whatever restarted, it stuck, and we
        // are no longer waiting for an accept.
        self.pairing_lost = false;
        // Asked before, and back after `ask_window` (or a failed request): the
        // question goes on, with its age and an ID typed meanwhile.
        let again = self.phase == WifiPhase::Asking;
        if !again {
            self.set_phase(WifiPhase::Checking);
            self.asked = None;
        } else if let Some((said, id)) = self.asked.clone() {
            // Still the same question: what it said, and an ID refused in it.
            detail = said;
            refused_id = id;
        }
        // The ID it refused for a while, in case it refused it only then.
        if let Some(id) = self.pairing.refused_pairing_id {
            let reply = self.post(wifi::PATH_BREW_JOBS, &Paired::new(id)).await?;
            if reply.http == StatusCode::FORBIDDEN || refused(&reply) {
                self.pairing.refused_pairing_id = None;
                self.pairing.save(self.cfg.pairing_file.as_deref())?;
                detail = format!(
                    "the grinder no longer knows pairing ID {id}: enter the Pairing ID it shows \
                     under Configuration"
                );
                refused_id = Some(id);
            } else {
                reply
                    .ok()
                    .context("checking the pairing ID the grinder refused before")?;
                self.use_pairing_id(id)?;
                tracing::info!("grinder: it takes the pairing ID it refused before");
                return Ok(());
            }
        }
        let deadline = tokio::time::Instant::now() + self.cfg.ask_window;
        // An ID typed while the link was not asking is stale by now.
        if !again {
            while ids.try_recv().is_ok() {}
        }
        loop {
            // The same question again (a new round of the ask window) at
            // debug, so it doesn't fill the log book.
            if self.asked.as_ref().is_some_and(|(said, _)| *said == detail) {
                tracing::debug!("grinder: {detail} (in the app, or with --grinder-pairing-id)");
            } else {
                tracing::warn!("grinder: {detail} (in the app, or with --grinder-pairing-id)");
            }
            self.asked = Some((detail.clone(), refused_id));
            self.set_phase(WifiPhase::Asking);
            self.ask_for_pairing_id(server, &detail, refused_id);
            let id = tokio::select! {
                _ = tokio::time::sleep_until(deadline) => return Ok(()),
                Some(id) = ids.recv() => id,
            };
            let reply = self.post(wifi::PATH_BREW_JOBS, &Paired::new(id)).await?;
            if reply.http == StatusCode::FORBIDDEN {
                detail = format!(
                    "the grinder does not know pairing ID {id}: enter the Pairing ID it shows \
                     under Configuration"
                );
                refused_id = Some(id);
                continue;
            }
            reply.ok().context("checking the pairing ID")?;
            self.use_pairing_id(id)?;
            tracing::info!("grinder: paired with the given pairing ID");
            return Ok(());
        }
    }

    /// The grinder answered: if it could not be reached before, say it is back.
    fn reached(&mut self) {
        if std::mem::take(&mut self.unreachable) {
            tracing::info!("grinder: reachable again");
        }
    }

    /// Moves the pairing to `phase`, counting its age from now unless it is
    /// there already.
    fn set_phase(&mut self, phase: WifiPhase) {
        if self.phase != phase {
            self.phase = phase;
            self.phase_since = Instant::now();
        }
    }

    fn use_pairing_id(&mut self, id: u32) -> anyhow::Result<()> {
        self.pairing_lost = false;
        self.set_phase(WifiPhase::Checking);
        self.pairing.pairing_id = Some(id);
        self.pairing.refused_pairing_id = None;
        self.pairing.save(self.cfg.pairing_file.as_deref())
    }

    /// Asks for jobs and answers shots until the grinder no longer knows the
    /// pairing (`Ok`: pair again) or a request fails.
    async fn serve(
        &mut self,
        server: &Server,
        pairing_id: u32,
        events: &mut broadcast::Receiver<EventRecord>,
    ) -> anyhow::Result<()> {
        let mut tick = tokio::time::interval(self.cfg.poll);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // Since when every job request was refused (see [`REFUSED_FOR`]). Only
        // refusals in a row count: a failed request in between ends `serve`,
        // and the next one starts over.
        let mut refused_since: Option<Instant> = None;
        loop {
            tokio::select! {
                // A shot reported while a request was failing is taken before
                // the next poll could see its job gone.
                biased;
                event = events.recv() => match event {
                    Ok(rec) => {
                        if self.on_event(pairing_id, &rec.event) {
                            self.send_answer(server).await;
                        }
                    }
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => bail!("the server stopped"),
                },
                _ = tick.tick() => {
                    let reply = self
                        .post(wifi::PATH_BREW_JOBS, &Paired::new(pairing_id))
                        .await?;
                    if reply.http == StatusCode::FORBIDDEN {
                        let since = *refused_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= self.cfg.refused_for {
                            return self.forget_pairing(server);
                        }
                        let detail = "the grinder refused the pairing, checking again";
                        self.status(server, WifiState::Failing, None, detail);
                        continue;
                    }
                    reply.ok().context("asking for brew jobs")?;
                    refused_since = None;
                    // Working again: the same failure later is a new one.
                    self.last_failure = None;
                    if self.pairing.refused_pairing_id.take().is_some() {
                        // The pairing in use works: no fallback needed.
                        self.pairing.save(self.cfg.pairing_file.as_deref())?;
                    }
                    server.note_wifi_poll(self.owner, reply.peer);
                    if server.grinder_link().wifi.is_none_or(|w| w.state != WifiState::Paired) {
                        let idle = "paired, waiting for a Grind-by-Sync grind";
                        self.status(server, WifiState::Paired, None, idle);
                    }
                    let jobs = reply.parse::<BrewJobsRes>()?.brew_jobs;
                    self.on_jobs(server, pairing_id, &jobs).await;
                }
            }
        }
    }

    /// The grinder refuses the pairing (cleared there): try the command
    /// line's ID if it is another one, else pair again, keeping the refused ID
    /// to try once more if the grinder still has gbs-anywhere paired.
    fn forget_pairing(&mut self, server: &Server) -> anyhow::Result<()> {
        self.drop_job(server, AbortCause::GrinderUnpaired);
        self.answer = None;
        // The first refused ID stays: when the command-line ID is refused too,
        // the one saved before is the better guess.
        if let Some(id) = self.pairing.pairing_id.take() {
            self.pairing.refused_pairing_id.get_or_insert(id);
        }
        if let Some(id) = self.spare_id.take() {
            tracing::warn!("grinder: it refuses the saved pairing ID, trying the given one");
            self.pairing.pairing_id = Some(id);
        } else {
            tracing::warn!("grinder: it no longer knows this gbs-anywhere, pairing again");
        }
        self.pairing.save(self.cfg.pairing_file.as_deref())
    }

    /// After each poll: keeps sending a pending answer, else holds the current
    /// job open or takes a new one.
    async fn on_jobs(&mut self, server: &Server, pairing_id: u32, jobs: &[BrewJob]) {
        self.polls += 1;
        for job in jobs {
            if !self.first_seen.iter().any(|(id, _)| *id == job.grind_uuid) {
                if self.first_seen.len() == HANDLED_CAP {
                    self.first_seen.pop_front();
                }
                self.first_seen
                    .push_back((job.grind_uuid.clone(), self.polls));
            }
        }
        let listed = |job: &BrewJob| jobs.iter().any(|j| j.grind_uuid == job.grind_uuid);
        if let Some(answer) = &self.answer {
            let cancel = answer.done.result == BrewResult::Canceled;
            let limit = if cancel { CANCEL_LIMIT } else { ANSWER_LIMIT };
            let gone = !answer.accepted && !listed(&answer.job);
            let expired = gone || answer.since.elapsed() > limit;
            if expired && cancel {
                // Nothing to adjust: the grinder times the job out by itself.
                tracing::info!("grinder: gave up telling it the shot was cancelled");
                self.answer = None;
                self.problem = None;
                self.refresh(server);
            } else if expired {
                self.answer = None;
                self.report_problem(server, "the shot did not reach the grinder in time");
            } else {
                self.send_answer(server).await;
            }
            return;
        }
        if let Some(held) = &self.current {
            let since = self.seen_in(&held.job);
            let newer = jobs
                .iter()
                .any(|j| !self.handled.contains(&j.grind_uuid) && self.seen_in(j) > since);
            let waiting = listed(&held.job);
            if newer {
                // Ground again before pulling: that grind is the shot now. The
                // brew ends here, and its event answers this job as cancelled.
                // If someone reported or aborted it a moment ago, that event
                // answers it instead; only a brew already over leaves it.
                let over = server.with_wifi(self.owner, |m, now| {
                    m.give_up(now, AbortCause::GrinderShotReplaced).is_err()
                        && m.phase(now) == Phase::Ready
                });
                if over == Some(true) {
                    self.current = None;
                }
            } else if waiting {
                // Not taken yet, or handed back out: take it (again).
                self.take_current(server, pairing_id).await;
            } else if let Some(held) = self.current.as_mut() {
                // Not listed: the grinder has it as taken (or forgot it, which
                // the answer finds out).
                held.accepted = true;
            }
            return;
        }
        self.take_job(server, jobs);
        if self.current.is_some() {
            self.take_current(server, pairing_id).await;
        }
    }

    /// The poll a job first showed up in.
    fn seen_in(&self, job: &BrewJob) -> u64 {
        self.first_seen
            .iter()
            .find(|(id, _)| *id == job.grind_uuid)
            .map_or(self.polls, |(_, poll)| *poll)
    }

    /// Starts a brew for the job that showed up last; of jobs that showed up
    /// in the same poll (only right after a start), the last listed one. The
    /// others are left for the grinder to time out.
    fn take_job(&mut self, server: &Server, jobs: &[BrewJob]) {
        let fresh: Vec<&BrewJob> = jobs
            .iter()
            .filter(|j| !self.handled.contains(&j.grind_uuid))
            .collect();
        let Some(job) = fresh
            .iter()
            .copied()
            .enumerate()
            .max_by_key(|(i, j)| (self.seen_in(j), *i))
            .map(|(_, j)| j)
        else {
            return;
        };
        let target_ms = job.target_ms();
        let started = server.with_wifi(self.owner, |m, now| {
            // A shot that is still being shown, or a brew started by hand,
            // goes first; the job waits for the next poll.
            m.on_brew_job(now, grind_result(job), target_ms)
        });
        if started != Some(true) {
            return;
        }
        for other in fresh.iter().filter(|j| j.grind_uuid != job.grind_uuid) {
            tracing::info!("grinder: skipping an older Grind-by-Sync grind");
            self.remember(other.grind_uuid.clone());
        }
        self.remember(job.grind_uuid.clone());
        self.problem = None;
        let detail = match target_ms {
            Some(ms) => format!(
                "waiting for the shot (target {:.1} s)",
                f64::from(ms) / 1000.0
            ),
            None => "waiting for the shot".to_owned(),
        };
        tracing::info!("grinder: Grind-by-Sync grind, {detail}");
        self.status(server, WifiState::Paired, None, detail);
        self.current = Some(Held {
            job: job.clone(),
            accepted: false,
        });
    }

    /// Takes the current job, right after it showed up and whenever the
    /// grinder lists it again, so it stays open until the shot is measured.
    /// A job the grinder refuses ends the brew; when taking it fails
    /// otherwise, it is tried again for as long as it stays listed.
    async fn take_current(&mut self, server: &Server, pairing_id: u32) {
        let Some(held) = &self.current else {
            return;
        };
        if held.accepted {
            tracing::debug!("grinder: it handed the job back out, taking it again");
        }
        let accept = BrewAccept::new(pairing_id, &held.job);
        let accepted = match self.post(wifi::PATH_BREW_ACCEPT, &accept).await {
            Ok(reply) if reply.status() == wifi::STATUS_OK => true,
            Ok(reply) if reply.status() == wifi::STATUS_JOB_UNKNOWN => {
                self.drop_job(server, AbortCause::GrinderShotDropped);
                return;
            }
            Ok(reply) if refused(&reply) => {
                tracing::warn!("grinder: it refused the job: {:?}", reply.status());
                self.drop_job(server, AbortCause::GrinderShotRefused);
                return;
            }
            other => {
                let why = failure(other);
                tracing::warn!("grinder: could not take the job yet, trying again: {why}");
                false
            }
        };
        if let Some(held) = &mut self.current {
            held.accepted = accepted;
        }
    }

    /// Ends the brew of the current job without answering it.
    fn drop_job(&mut self, server: &Server, cause: AbortCause) {
        if self.current.take().is_none() {
            return;
        }
        // Already over when someone reported or aborted it a moment ago.
        let _ = server.with_wifi(self.owner, |m, now| m.give_up(now, cause));
        self.report_problem(server, cause.text());
    }

    /// A reported or aborted shot becomes the answer to the current job.
    /// Returns whether there is one to send.
    fn on_event(&mut self, pairing_id: u32, event: &MachineEvent) -> bool {
        let (result, time_ms, grams, reason) = match event {
            MachineEvent::ShotReported { result, .. } => {
                (BrewResult::Success, result.time_ms, result.weight_g, "")
            }
            MachineEvent::ShotAborted { reason, .. } => (BrewResult::Canceled, 0, 0.0, *reason),
            _ => return false,
        };
        let Some(held) = self.current.take() else {
            return false;
        };
        let event_uuid = uuid::Uuid::new_v4().to_string();
        let done = BrewDone::new(pairing_id, &held.job, result, time_ms, grams, event_uuid);
        let line = match result {
            BrewResult::Success => format!(
                "sent {:.1} s, {:.1} g",
                f64::from(time_ms) / 1000.0,
                done.mass_final_mg as f64 / 1000.0
            ),
            BrewResult::Canceled => format!("told the grinder the shot was cancelled ({reason})"),
        };
        self.answer = Some(Answer {
            job: held.job,
            done,
            accepted: held.accepted,
            since: Instant::now(),
            line,
        });
        true
    }

    /// One try at getting the pending answer to the grinder: take the job if
    /// not yet taken, then send the result. Kept for the next poll unless it
    /// went through or the grinder refuses it; a job the grinder no longer
    /// has ends on the next poll.
    async fn send_answer(&mut self, server: &Server) {
        let Some(mut answer) = self.answer.take() else {
            return;
        };
        if !answer.accepted {
            let accept = BrewAccept::new(answer.done.pairing_id, &answer.job);
            match self.post(wifi::PATH_BREW_ACCEPT, &accept).await {
                Ok(reply) if reply.status() == wifi::STATUS_OK => answer.accepted = true,
                // Not listed: taken already (its reply got lost) or gone. Taken
                // counts until the result says it is gone
                // (`STATUS_ONGOING_JOB_UNKNOWN`); the next poll then ends it.
                Ok(reply) if reply.status() == wifi::STATUS_JOB_UNKNOWN => answer.accepted = true,
                Ok(reply) if refused(&reply) => {
                    let why = format!("the grinder refused the shot: {:?}", reply.status());
                    self.report_problem(server, &why);
                    return;
                }
                other => {
                    self.retry_later(server, answer, other);
                    return;
                }
            }
        }
        match self.post(wifi::PATH_BREW_DONE, &answer.done).await {
            Ok(reply) if reply.status() == wifi::STATUS_OK => {
                tracing::info!("grinder: {}", answer.line);
                self.problem = None;
                self.status(server, WifiState::Paired, None, answer.line);
            }
            // The grinder handed the job back out: take it again next time.
            Ok(reply) if reply.status() == wifi::STATUS_ONGOING_JOB_UNKNOWN => {
                answer.accepted = false;
                self.answer = Some(answer);
            }
            Ok(reply) if refused(&reply) => {
                let why = format!("the grinder refused the shot: {:?}", reply.status());
                self.report_problem(server, &why);
            }
            other => self.retry_later(server, answer, other),
        }
    }

    fn retry_later(&mut self, server: &Server, answer: Answer, outcome: anyhow::Result<Reply>) {
        let why = failure(outcome);
        tracing::warn!("grinder: the shot has not reached the grinder yet, retrying: {why}");
        self.answer = Some(answer);
        self.problem = Some(format!(
            "the shot has not reached the grinder yet, retrying: {why}"
        ));
        self.refresh(server);
    }

    fn report_problem(&mut self, server: &Server, problem: &str) {
        tracing::warn!("grinder: {problem}");
        self.problem = Some(problem.to_owned());
        let idle = "paired, waiting for a Grind-by-Sync grind";
        self.status(server, WifiState::Paired, None, idle);
    }

    /// Any answer, even an error, means the grinder can be reached again.
    async fn post(&mut self, path: &str, body: &impl Serialize) -> anyhow::Result<Reply> {
        let base = &self.cfg.base_url;
        let mut res = self
            .http
            .post(format!("{base}{path}"))
            .json(body)
            .send()
            .await
            .with_context(|| format!("cannot reach the grinder at {base}"))?;
        self.reached();
        let http = res.status();
        let peer = res.remote_addr();
        let bytes = super::read_capped(&mut res, MAX_REPLY_BYTES, "the grinder's reply").await?;
        let text = wifi::decode(&bytes);
        tracing::debug!("grinder {path} -> {http} {text}");
        let body = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).context("unreadable reply from the grinder")?
        };
        Ok(Reply { http, body, peer })
    }

    fn remember(&mut self, grind_uuid: String) {
        if self.handled.len() == HANDLED_CAP {
            self.handled.pop_front();
        }
        self.handled.push_back(grind_uuid);
    }

    fn status(
        &self,
        server: &Server,
        state: WifiState,
        pairing_code: Option<u32>,
        detail: impl Into<String>,
    ) {
        server.set_wifi(self.owner, self.link(state, pairing_code, detail.into()));
    }

    /// The state the link shows first, before it ran.
    pub fn connecting(&self) -> WifiLink {
        self.link(WifiState::Connecting, None, "connecting".to_owned())
    }

    fn link(&self, state: WifiState, pairing_code: Option<u32>, detail: String) -> WifiLink {
        WifiLink {
            grinder: self.cfg.base_url.clone(),
            state,
            state_since: None,
            phase: (state == WifiState::Pairing).then_some(self.phase),
            phase_since: (state == WifiState::Pairing).then_some(self.phase_since),
            pairing_code,
            unknown_pairing_id: None,
            pairing_lost: self.pairing_lost,
            quiet_ms: self.cfg.restart_quiet.as_millis() as u64,
            detail,
            problem: self.problem.clone(),
            pairing_id: self.pairing.pairing_id,
            unreachable: self.unreachable,
            not_set_up: self.not_set_up,
        }
    }

    /// Shows the current problem without changing the rest of the state.
    fn refresh(&self, server: &Server) {
        if let Some(mut link) = server.grinder_link().wifi {
            link.problem = self.problem.clone();
            server.set_wifi(self.owner, link);
        }
    }

    /// Shows the question for the pairing ID (phase `Asking`), with the last
    /// one the grinder did not take.
    fn ask_for_pairing_id(&self, server: &Server, detail: &str, refused_id: Option<u32>) {
        server.set_wifi(
            self.owner,
            WifiLink {
                grinder: self.cfg.base_url.clone(),
                state: WifiState::Pairing,
                state_since: None,
                phase: Some(WifiPhase::Asking),
                phase_since: Some(self.phase_since),
                pairing_code: None,
                unknown_pairing_id: refused_id,
                pairing_lost: false,
                quiet_ms: self.cfg.restart_quiet.as_millis() as u64,
                detail: detail.to_owned(),
                problem: None,
                pairing_id: None,
                unreachable: false,
                not_set_up: false,
            },
        );
    }
}

/// A client error from the grinder, which a retry would only repeat.
fn refused(reply: &Reply) -> bool {
    reply.status().starts_with("ClientError")
}

/// Why a request that is tried again did not go through.
fn failure(outcome: anyhow::Result<Reply>) -> String {
    match outcome.and_then(|r| r.ok()) {
        Err(e) => format!("{e:#}"),
        Ok(()) => "unexpected reply".to_owned(),
    }
}

/// Whether a request got no answer from the grinder: it failed in any way (a
/// grinder that is starting up can fail them in several), or the grinder sent
/// an HTTP error without a status.
fn no_answer(outcome: &anyhow::Result<Reply>) -> bool {
    match outcome {
        Err(_) => true,
        Ok(reply) => reply.status().is_empty() && !reply.http.is_success(),
    }
}

/// Whether the grinder did not answer at all: connecting failed or timed out,
/// as while it is off, in standby, or gone from the network.
fn unreachable(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|c| c.downcast_ref::<reqwest::Error>())
        .any(|r| r.is_connect() || r.is_timeout())
}

/// The job as the grind result the app and the shot rules read (recipe weight).
fn grind_result(job: &BrewJob) -> GrindResult {
    GrindResult {
        target_weight_g: job.target_grams(),
        filter: None,
        raw: serde_json::to_value(job).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests;
