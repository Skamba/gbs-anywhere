//! The HTTP server: the machine the grinder syncs with.
//!
//! Two HTTP faces over one shared [`crate::protocol::Machine`]:
//!
//! * the **grinder API** ([`grinder_router`]) — what a real Xenia serves on
//!   port 80: `GET /api/v2/machine`, `GET /api/v2/mako`, and the grinder's
//!   writes `POST /api/v2/brewratio` and `POST /api/v2/scripts/execute`;
//! * the **control API** ([`control_router`]) — for a person, a CLI or a
//!   support app: see the state, get "start the shot now" prompts as a
//!   server-sent-event stream, report the measured time and weight.
//!
//! An E65/E80 GbS grinder works the other way round: [`wifi_grinder`] asks it
//! for brew jobs and drives the same machine from them.
//!
//! Around them: `browser` (keeps web pages out), `page` (builds the app
//! page the control API serves), [`settings`] (what people
//! change in the app, and the settings file), [`logbook`] (the server's log
//! for the app), the shot history and [`update_check`].
//!
//! The `gbs-anywhere` binary is a thin wrapper: build a [`Server`], then
//! [`serve`] it or mount the routers in another app.

mod browser;
mod control_api;
mod grinder_api;
pub mod history;
pub mod logbook;
mod page;
mod private_file;
pub mod settings;
#[cfg(test)]
mod tests;
pub mod update_check;
mod utc;
pub mod wifi_grinder;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::integration::Integrations;
use crate::protocol::machine::EventRecord;
use crate::protocol::{Machine, MachineConfig};
use axum::extract::connect_info::Connected;
use axum::serve::IncomingStream;
pub use browser::AllowedHosts;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

pub use control_api::{ShotInput, control_router, patch_ready, state_json};
pub use grinder_api::grinder_router;
pub use history::{History, Shot, ShotOutcome};
pub use logbook::{LogBook, LogLayer, LogLine};
pub use settings::{SettingsChange, SettingsError, SettingsFile, Startup};
pub use update_check::UpdateConfig;
pub use wifi_grinder::{
    PairingIdRefusal, WifiConfig, WifiGrinder, WifiLink, WifiPhase, WifiState, grinder_address,
};

/// Shared state behind both routers.
pub struct Server {
    inner: Mutex<Inner>,
    events: broadcast::Sender<EventRecord>,
    transcript: Option<grinder_api::Transcript>,
    integrations: Integrations,
    icons_dir: OnceLock<PathBuf>,
    /// The shot window and grinder address, and the E65/E80 link.
    settings: Mutex<settings::Live>,
    /// Held by the E65/E80 link that may write the pairing file.
    pairing_lock: wifi_grinder::PairingLock,
    history: History,
    update: Mutex<update_check::UpdateState>,
    /// Wakes the update check when it is turned on or off.
    update_wake: tokio::sync::Notify,
    log: Arc<LogBook>,
    /// New on every start: event and log numbers start over with it.
    boot_id: String,
    /// Set once the server stops: the live streams end.
    stopping: tokio::sync::watch::Sender<bool>,
}

struct Inner {
    machine: Machine,
    published: u64,
    grinder: GrinderLink,
    /// Pairing IDs for the E65/E80 link, while one runs.
    wifi_ids: Option<mpsc::Sender<u32>>,
    /// "I accepted on the grinder" from the app, for the E65/E80 link while
    /// one is waiting for it.
    wifi_accept: Option<mpsc::Sender<()>>,
    /// The E65/E80 link that may report its state: a replaced one may still
    /// be finishing a step.
    wifi_owner: u64,
}

/// What we know about the grinder from its requests (or, for an E65/E80
/// grinder, from our requests to it).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GrinderLink {
    pub peer: Option<SocketAddr>,
    pub polls: u64,
    pub requests: u64,
    #[serde(skip)]
    pub last_poll: Option<Instant>,
    /// Set when gbs-anywhere pairs with an E65/E80 grinder (`--grinder`).
    pub wifi: Option<WifiLink>,
}

impl GrinderLink {
    fn note(&mut self, peer: Option<SocketAddr>, poll: bool) {
        if peer.is_some() {
            self.peer = peer;
        }
        self.requests += 1;
        if poll {
            self.polls += 1;
            self.last_poll = Some(Instant::now());
        }
    }
}

impl Server {
    pub fn new(config: MachineConfig) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                machine: Machine::new(config),
                published: 0,
                grinder: GrinderLink::default(),
                wifi_ids: None,
                wifi_accept: None,
                wifi_owner: 0,
            }),
            events: broadcast::channel(256).0,
            transcript: None,
            integrations: Integrations::new(),
            icons_dir: OnceLock::new(),
            settings: Mutex::default(),
            pairing_lock: wifi_grinder::PairingLock::default(),
            history: History::default(),
            update: Mutex::default(),
            update_wake: tokio::sync::Notify::new(),
            log: LogBook::global(),
            boot_id: uuid::Uuid::new_v4().simple().to_string(),
            stopping: tokio::sync::watch::channel(false).0,
        })
    }

    /// Changes on every start, so an app can tell that event and log numbers
    /// started over.
    pub fn boot_id(&self) -> &str {
        &self.boot_id
    }

    /// Like [`Server::new`], with a log book of its own (tests run side by
    /// side).
    #[cfg(test)]
    fn with_log(config: MachineConfig, log: Arc<LogBook>) -> Arc<Self> {
        let mut me = Arc::into_inner(Self::new(config)).expect("fresh Arc");
        me.log = log;
        Arc::new(me)
    }

    /// Like [`Server::new`], also appending every grinder request and our
    /// reply to `path` (`--log`), readable by its owner only. Past a few MiB
    /// it moves to `<path>.1` and starts over.
    pub async fn with_transcript(config: MachineConfig, path: &Path) -> anyhow::Result<Arc<Self>> {
        use anyhow::Context;
        let transcript = grinder_api::Transcript::open(path, grinder_api::TRANSCRIPT_MAX_BYTES)
            .await
            .with_context(|| format!("--log: cannot open {}", path.display()))?;
        let mut me = Arc::into_inner(Self::new(config)).expect("fresh Arc");
        me.transcript = Some(transcript);
        Ok(Arc::new(me))
    }

    /// Runs `f` on the machine and publishes any events it produced.
    pub fn with<R>(&self, f: impl FnOnce(&mut Machine, Instant) -> R) -> R {
        self.apply(&mut self.lock(), f)
    }

    /// Like [`Server::with`], for the E65/E80 link `owner`: `None`, and the
    /// machine untouched, once that link was replaced. Its task may still
    /// be finishing a step when it is stopped, and must not start or end a
    /// brew after that.
    fn with_wifi<R>(&self, owner: u64, f: impl FnOnce(&mut Machine, Instant) -> R) -> Option<R> {
        let mut inner = self.lock();
        (inner.wifi_owner == owner).then(|| self.apply(&mut inner, f))
    }

    fn apply<R>(&self, inner: &mut Inner, f: impl FnOnce(&mut Machine, Instant) -> R) -> R {
        let r = f(&mut inner.machine, Instant::now());
        let after = inner.published;
        for e in inner.machine.events_after(after) {
            // No subscribers is fine.
            let _ = self.events.send(e.clone());
        }
        inner.published = inner.machine.last_seq();
        r
    }

    /// Live machine events (grind result, start request, shot reported, ...).
    pub fn subscribe(&self) -> broadcast::Receiver<EventRecord> {
        self.events.subscribe()
    }

    /// Like [`Server::subscribe`], with the newest event's `seq` so far:
    /// every event after it comes through the receiver. Taken under the
    /// machine's lock, which events are sent under.
    pub fn subscribe_at(&self) -> (broadcast::Receiver<EventRecord>, u64) {
        let inner = self.lock();
        (self.events.subscribe(), inner.published)
    }

    /// The kept events with `seq > after`, oldest first.
    pub fn events_after(&self, after: u64) -> std::collections::VecDeque<EventRecord> {
        self.lock().machine.events_after(after).cloned().collect()
    }

    pub fn grinder_link(&self) -> GrinderLink {
        self.lock().grinder.clone()
    }

    /// Serves the app's icons from `dir` (`GET /icons/<name>`): put
    /// `<integration id>.svg` or `.png` there to replace a built-in glyph,
    /// e.g. a vendor's official logo you are allowed to use.
    pub fn set_icons_dir(&self, dir: PathBuf) {
        let _ = self.icons_dir.set(dir);
    }

    pub fn icons_dir(&self) -> Option<&Path> {
        self.icons_dir.get().map(PathBuf::as_path)
    }

    /// The running integrations (vendor clouds, scales, hubs): start, add, remove.
    pub fn integrations(&self) -> &Integrations {
        &self.integrations
    }

    /// The server's log as the app shows it.
    pub fn log(&self) -> &Arc<LogBook> {
        &self.log
    }

    /// A grinder request (E64 WS), or a successful request to an E65/E80
    /// grinder; a poll makes the app show the grinder as connected.
    fn note_request(&self, peer: Option<SocketAddr>, poll: bool) {
        self.lock().grinder.note(peer, poll);
    }

    /// A new E65/E80 link starts: it takes the pairing IDs typed in the app
    /// and the "I accepted" confirmations from now on. Returns its owner
    /// number for [`Server::set_wifi`] and where those arrive.
    fn attach_wifi(&self) -> (u64, mpsc::Receiver<u32>, mpsc::Receiver<()>) {
        // Room for one each: only the latest typed ID, and the latest
        // confirmation, matter.
        let (tx, rx) = mpsc::channel(1);
        let (atx, arx) = mpsc::channel(1);
        let mut inner = self.lock();
        inner.wifi_owner += 1;
        inner.wifi_ids = Some(tx);
        inner.wifi_accept = Some(atx);
        (inner.wifi_owner, rx, arx)
    }

    /// The E65/E80 link stopped: forget what it said. What an E64 WS told
    /// (its address, request counts) stays; the last poll goes, so the
    /// grinder no longer shows as connected unless an E64 WS polls.
    fn detach_wifi(&self) {
        let mut inner = self.lock();
        inner.wifi_owner += 1;
        inner.wifi_ids = None;
        inner.wifi_accept = None;
        inner.grinder.wifi = None;
        inner.grinder.last_poll = None;
    }

    /// The link's state, unless that link was replaced meanwhile. The time
    /// it entered its state carries over while the state stays the same
    /// (`state_age_ms`).
    fn set_wifi(&self, owner: u64, mut link: WifiLink) {
        let mut inner = self.lock();
        if inner.wifi_owner == owner {
            link.state_since = match &inner.grinder.wifi {
                Some(old) if old.state == link.state => old.state_since,
                _ => None,
            }
            .or_else(|| Some(Instant::now()));
            inner.grinder.wifi = Some(link);
        }
    }

    /// A poll of an E65/E80 grinder went through.
    fn note_wifi_poll(&self, owner: u64, peer: Option<SocketAddr>) {
        let mut inner = self.lock();
        if inner.wifi_owner == owner {
            inner.grinder.note(peer, true);
        }
    }

    /// Hands the pairing ID an E65/E80 grinder shows to its link, for when
    /// gbs-anywhere lost it. Taken only while the link asks for one; the link
    /// checks it with the grinder before keeping it.
    pub fn use_grinder_pairing_id(&self, id: u32) -> Result<(), PairingIdRefusal> {
        let (tx, asks, paired) = {
            let inner = self.lock();
            let wifi = inner.grinder.wifi.as_ref();
            let asks = wifi.is_some_and(|w| w.phase == Some(WifiPhase::Asking));
            let paired = wifi.is_some_and(|w| w.pairing_id.is_some());
            (inner.wifi_ids.clone(), asks, paired)
        };
        let tx = tx.ok_or(PairingIdRefusal::NoLink)?;
        if !asks {
            return Err(if paired {
                PairingIdRefusal::NotNeeded
            } else {
                PairingIdRefusal::NotAsking
            });
        }
        tx.try_send(id).map_err(|_| PairingIdRefusal::Busy)
    }

    /// The person confirmed they accepted gbs-anywhere on the grinder, so the
    /// E65/E80 link waits a moment, then checks the pairing as confirmed.
    /// Taken only while the link waits for it.
    pub fn grinder_accepted(&self) -> Result<(), PairingIdRefusal> {
        let (tx, awaiting) = {
            let inner = self.lock();
            let awaiting = inner
                .grinder
                .wifi
                .as_ref()
                .is_some_and(|w| w.phase == Some(WifiPhase::Accepting));
            (inner.wifi_accept.clone(), awaiting)
        };
        let tx = tx.ok_or(PairingIdRefusal::NoLink)?;
        if !awaiting {
            return Err(PairingIdRefusal::NotAccepting);
        }
        tx.try_send(()).map_err(|_| PairingIdRefusal::Busy)
    }

    async fn write_transcript(&self, line: &str) {
        if let Some(t) = &self.transcript {
            t.write(line).await;
        }
    }

    /// Ends the live streams (`/api/events/stream`, `/api/log/stream`), so
    /// that their connections close and the server can stop.
    pub fn stop(&self) {
        self.stopping.send_replace(true);
    }

    /// Done once [`Server::stop`] was called.
    fn stopped(&self) -> impl Future<Output = ()> + Send + use<> {
        let mut rx = self.stopping.subscribe();
        async move {
            let _ = rx.wait_for(|stopping| *stopping).await;
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves plain data; keep serving.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Reads a reply's body, but no more than `max` bytes: `what` names the
/// reply in the error ("the grinder's reply").
pub(crate) async fn read_capped(
    res: &mut reqwest::Response,
    max: usize,
    what: &str,
) -> anyhow::Result<Vec<u8>> {
    use anyhow::Context;
    let mut bytes = Vec::new();
    while let Some(chunk) = res
        .chunk()
        .await
        .with_context(|| format!("reading {what}"))?
    {
        if bytes.len() + chunk.len() > max {
            anyhow::bail!("{what} is too large");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Waits until `cond` holds, for up to five seconds.
#[cfg(test)]
pub(crate) async fn wait_for(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..250 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Where to listen.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    /// Grinder-facing listeners. The grinder uses port 80.
    pub grinder: Vec<SocketAddr>,
    /// Extra control API listener (e.g. localhost only).
    pub control: Option<SocketAddr>,
    /// Also serve the control API and the app page on the grinder-facing
    /// ports, so a phone can open `http://<this host>/`. The grinder only
    /// uses `/api/v2/*`, which the control routes do not overlap.
    pub app_on_grinder_ports: bool,
    /// Host names the app answers to besides home-network ones.
    pub allowed_hosts: AllowedHosts,
    /// Let requests with browser headers reach the grinder's routes.
    pub accept_browser_headers: bool,
}

/// How long requests still open when the server stops get to finish.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// Done on Ctrl-C, or on unix on SIGTERM, which `docker stop` sends. The
/// handlers are in place from this call on: call it first thing at startup,
/// since a signal that comes before them (to PID 1 in a container) is
/// dropped. A second signal, while open requests get their few seconds,
/// ends the process at once.
pub fn shutdown_signal() -> impl Future<Output = ()> + Send + 'static {
    let mut signals = Signals::new();
    async move {
        signals.recv().await;
        tokio::spawn(async move {
            signals.recv().await;
            tracing::info!("stopping at once");
            // 130, as shells report an interrupted program.
            std::process::exit(130);
        });
    }
}

/// The signals that stop gbs-anywhere.
struct Signals {
    #[cfg(unix)]
    interrupt: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    terminate: Option<tokio::signal::unix::Signal>,
    #[cfg(windows)]
    ctrl_c: Option<tokio::signal::windows::CtrlC>,
}

impl Signals {
    fn new() -> Self {
        fn listen<T>(what: &str, handler: std::io::Result<T>) -> Option<T> {
            handler
                .inspect_err(|e| tracing::warn!("cannot wait for {what}: {e}"))
                .ok()
        }
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Self {
                interrupt: listen("Ctrl-C", signal(SignalKind::interrupt())),
                terminate: listen("SIGTERM", signal(SignalKind::terminate())),
            }
        }
        #[cfg(windows)]
        Self {
            ctrl_c: listen("Ctrl-C", tokio::signal::windows::ctrl_c()),
        }
    }

    /// Waits for the next one; forever when none could be set up.
    async fn recv(&mut self) {
        #[cfg(unix)]
        {
            async fn next(handler: &mut Option<tokio::signal::unix::Signal>) {
                match handler {
                    Some(s) => _ = s.recv().await,
                    None => std::future::pending().await,
                }
            }
            tokio::select! {
                () = next(&mut self.interrupt) => {}
                () = next(&mut self.terminate) => {}
            }
        }
        #[cfg(windows)]
        match &mut self.ctrl_c {
            Some(s) => _ = s.recv().await,
            None => std::future::pending().await,
        }
    }
}

/// Binds all listeners and serves until `shutdown` is done (say,
/// [`shutdown_signal`]), or until they all fail. Ports that cannot be bound
/// are logged and skipped; it fails if none bind. On `shutdown` it takes no
/// new connections, ends the live streams ([`Server::stop`]) and gives the
/// requests still open up to a few seconds to finish.
pub async fn serve(
    state: Arc<Server>,
    cfg: ServeConfig,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let mut tasks = Vec::new();
    let grinder_app = if cfg.app_on_grinder_ports {
        control_router(state.clone(), cfg.allowed_hosts.clone())
            .merge(grinder_router(state.clone(), cfg.accept_browser_headers))
    } else {
        grinder_router(state.clone(), cfg.accept_browser_headers)
    };
    for addr in cfg.grinder {
        match TcpListener::bind(addr).await {
            Ok(l) => {
                tracing::info!("grinder API on http://{addr}");
                tasks.push(spawn_server(l, grinder_app.clone(), addr, &state));
            }
            Err(e) => tracing::warn!("cannot bind {addr}: {e} (in use or needs admin)"),
        }
    }
    if tasks.is_empty() {
        anyhow::bail!("no grinder-facing port could be bound");
    }
    if let Some(addr) = cfg.control {
        let l = TcpListener::bind(addr)
            .await
            .with_context(|| format!("--control: cannot listen on {addr}"))?;
        tracing::info!("control API on http://{addr}/api/state");
        tasks.push(spawn_server(
            l,
            control_router(state.clone(), cfg.allowed_hosts.clone()),
            addr,
            &state,
        ));
    }

    // Time-based transitions (finishing -> ready, the shot window running out) publish their
    // events even when nobody polls.
    let ticker = state.clone();
    let ticks = tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_millis(250));
        loop {
            iv.tick().await;
            ticker.with(|m, now| m.tick(now));
        }
    });

    let mut servers = std::pin::pin!(futures_util::future::join_all(tasks));
    tokio::select! {
        _ = &mut servers => {
            ticks.abort();
            anyhow::bail!("no server is running any more");
        }
        () = shutdown => {}
    }
    tracing::info!("stopping");
    state.stop();
    if tokio::time::timeout(STOP_GRACE, servers).await.is_err() {
        tracing::info!("stopping without waiting longer for open requests");
    }
    ticks.abort();
    Ok(())
}

/// Both ends of a request's connection, as `ConnectInfo<Conn>`.
#[derive(Debug, Clone, Copy)]
pub struct Conn {
    /// Who sent it.
    pub peer: SocketAddr,
    /// The address it came in on: on the home network, the one the grinder
    /// can reach this server at; in a Docker container, the container's.
    /// `None` when the system could not tell.
    pub local: Option<SocketAddr>,
}

impl Connected<IncomingStream<'_, TcpListener>> for Conn {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        Self {
            peer: *stream.remote_addr(),
            local: stream.io().local_addr().ok(),
        }
    }
}

fn spawn_server(
    listener: TcpListener,
    app: axum::Router,
    addr: SocketAddr,
    state: &Server,
) -> tokio::task::JoinHandle<()> {
    let stopped = state.stopped();
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app.into_make_service_with_connect_info::<Conn>())
            .with_graceful_shutdown(stopped)
            .await
        {
            tracing::warn!("server on {addr} stopped: {e}");
        }
    })
}
