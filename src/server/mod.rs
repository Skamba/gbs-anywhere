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
//! The `gbs-anywhere` binary is a thin wrapper: build a [`Server`], then
//! [`serve`] it or mount the routers in another app.

mod control_api;
mod grinder_api;
#[cfg(test)]
mod tests;
pub mod wifi_grinder;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::integration::Integrations;
use crate::protocol::machine::EventRecord;
use crate::protocol::{Machine, MachineConfig};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

pub use control_api::{ShotInput, control_router, patch_ready, state_json};
pub use grinder_api::grinder_router;
pub use wifi_grinder::{PairingIdRefusal, WifiConfig, WifiGrinder, WifiLink, WifiState};

/// Shared state behind both routers.
pub struct Server {
    inner: Mutex<Inner>,
    events: broadcast::Sender<EventRecord>,
    transcript: Option<tokio::sync::Mutex<tokio::fs::File>>,
    integrations: Integrations,
    icons_dir: OnceLock<PathBuf>,
    /// Pairing IDs for the E65/E80 link, when one runs.
    wifi_ids: OnceLock<tokio::sync::mpsc::Sender<u32>>,
    /// New on every start: event numbers start over with it.
    boot_id: String,
}

struct Inner {
    machine: Machine,
    published: u64,
    grinder: GrinderLink,
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

impl Server {
    pub fn new(config: MachineConfig) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                machine: Machine::new(config),
                published: 0,
                grinder: GrinderLink::default(),
            }),
            events: broadcast::channel(256).0,
            transcript: None,
            integrations: Integrations::new(),
            icons_dir: OnceLock::new(),
            wifi_ids: OnceLock::new(),
            boot_id: uuid::Uuid::new_v4().simple().to_string(),
        })
    }

    /// Changes on every start, so an app can tell that event numbers
    /// started over.
    pub fn boot_id(&self) -> &str {
        &self.boot_id
    }

    /// Like [`Server::new`], also appending every grinder request and our
    /// reply to `path`.
    pub async fn with_transcript(config: MachineConfig, path: &Path) -> anyhow::Result<Arc<Self>> {
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        let mut me = Arc::into_inner(Self::new(config)).expect("fresh Arc");
        me.transcript = Some(tokio::sync::Mutex::new(file));
        Ok(Arc::new(me))
    }

    /// Runs `f` on the machine and publishes any events it produced.
    pub fn with<R>(&self, f: impl FnOnce(&mut Machine, Instant) -> R) -> R {
        let mut inner = self.lock();
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

    /// A grinder request (E64 WS), or a successful request to an E65/E80
    /// grinder; a poll makes the app show the grinder as connected.
    fn note_request(&self, peer: Option<SocketAddr>, poll: bool) {
        let mut inner = self.lock();
        let g = &mut inner.grinder;
        if peer.is_some() {
            g.peer = peer;
        }
        g.requests += 1;
        if poll {
            g.polls += 1;
            g.last_poll = Some(Instant::now());
        }
    }

    fn set_wifi(&self, link: WifiLink) {
        self.lock().grinder.wifi = Some(link);
    }

    fn set_wifi_ids(&self, tx: tokio::sync::mpsc::Sender<u32>) {
        let _ = self.wifi_ids.set(tx);
    }

    /// Hands the pairing ID an E65/E80 grinder shows to its link, for when
    /// gbs-anywhere lost it. Taken only while the link asks for one; the link
    /// checks it with the grinder before keeping it.
    pub fn use_grinder_pairing_id(&self, id: u32) -> Result<(), PairingIdRefusal> {
        let tx = self.wifi_ids.get().ok_or(PairingIdRefusal::NoLink)?;
        let asks = self.grinder_link().wifi.is_some_and(|w| w.needs_pairing_id);
        if !asks {
            return Err(PairingIdRefusal::NotNeeded);
        }
        tx.try_send(id).map_err(|_| PairingIdRefusal::Busy)
    }

    async fn write_transcript(&self, line: &str) {
        if let Some(t) = &self.transcript {
            let _ = t.lock().await.write_all(line.as_bytes()).await;
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves plain data; keep serving.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
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
}

/// Binds all listeners and serves until one fails fatally or all stop. Ports
/// that cannot be bound are logged and skipped; it fails if none bind.
pub async fn serve(state: Arc<Server>, cfg: ServeConfig) -> anyhow::Result<()> {
    let mut tasks = Vec::new();
    let grinder_app = if cfg.app_on_grinder_ports {
        control_router(state.clone()).merge(grinder_router(state.clone()))
    } else {
        grinder_router(state.clone())
    };
    for addr in cfg.grinder {
        match TcpListener::bind(addr).await {
            Ok(l) => {
                tracing::info!("grinder API on http://{addr}");
                tasks.push(spawn_server(l, grinder_app.clone(), addr));
            }
            Err(e) => tracing::warn!("cannot bind {addr}: {e} (in use or needs admin)"),
        }
    }
    if tasks.is_empty() {
        anyhow::bail!("no grinder-facing port could be bound");
    }
    if let Some(addr) = cfg.control {
        let l = TcpListener::bind(addr).await?;
        tracing::info!("control API on http://{addr}/api/state");
        tasks.push(spawn_server(l, control_router(state.clone()), addr));
    }

    // Time-based transitions (finishing -> ready, brew timeout) publish their
    // events even when nobody polls.
    let ticker = state.clone();
    tasks.push(tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_millis(250));
        loop {
            iv.tick().await;
            ticker.with(|m, now| m.tick(now));
        }
    }));

    futures_util::future::join_all(tasks).await;
    Ok(())
}

fn spawn_server(
    listener: TcpListener,
    app: axum::Router,
    addr: SocketAddr,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        {
            tracing::warn!("server on {addr} stopped: {e}");
        }
    })
}
