//! The HTTP server: the machine the E64 WS grinder syncs with.
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
//! The `gbs-anywhere` binary is a thin wrapper: build a [`Server`], then
//! [`serve`] it or mount the routers in another app.

mod control_api;
mod grinder_api;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::integration::{Status, StatusSnapshot};
use crate::protocol::machine::EventRecord;
use crate::protocol::{Machine, MachineConfig};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

pub use control_api::{ShotInput, control_router, patch_ready, state_json};
pub use grinder_api::grinder_router;

/// Shared state behind both routers.
pub struct Server {
    inner: Mutex<Inner>,
    events: broadcast::Sender<EventRecord>,
    transcript: Option<tokio::sync::Mutex<tokio::fs::File>>,
    integrations: Mutex<Vec<Status>>,
    icons_dir: OnceLock<PathBuf>,
}

struct Inner {
    machine: Machine,
    published: u64,
    grinder: GrinderLink,
}

/// What we know about the grinder from its requests.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GrinderLink {
    pub peer: Option<SocketAddr>,
    pub polls: u64,
    pub requests: u64,
    #[serde(skip)]
    pub last_poll: Option<Instant>,
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
            integrations: Mutex::new(Vec::new()),
            icons_dir: OnceLock::new(),
        })
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

    /// Makes an integration's status visible in the API and the app.
    pub fn register_integration(&self, status: Status) {
        self.integrations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(status);
    }

    /// Current status of every integration, in registration order.
    pub fn integrations(&self) -> Vec<StatusSnapshot> {
        self.integrations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(Status::snapshot)
            .collect()
    }

    fn note_request(&self, peer: SocketAddr, poll: bool) {
        let mut inner = self.lock();
        let g = &mut inner.grinder;
        g.peer = Some(peer);
        g.requests += 1;
        if poll {
            g.polls += 1;
            g.last_poll = Some(Instant::now());
        }
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
