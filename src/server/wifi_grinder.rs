//! The link to an E65/E80 GbS grinder over its "Generic WiFi" option (see
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
//! (`--brew-timeout-s`), after which it is answered as cancelled. Until the
//! grinder has the result, it is retried on every poll, so a short network
//! outage loses no shot.
//!
//! The job that showed up last is the shot being pulled: a newer grind
//! cancels a job still waiting for its shot, and other open jobs are left
//! for the grinder to time out.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::integration::Backoff;
use crate::protocol::machine::EventRecord;
use crate::protocol::wifi::{
    self, API_VERSION, BrewAccept, BrewDone, BrewJob, BrewJobsRes, BrewResult, Paired,
    PairingApplication, PairingApplicationRes, PairingStatusRes,
};
use crate::protocol::{GrindResult, MachineEvent, Phase};
use crate::server::Server;

/// Between job requests (and pairing checks). The grinder shows the machine
/// as active while it asks.
const POLL: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const MIN_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// How long nothing is sent to a grinder that stopped answering while
/// pairing. The E65W freezes and restarts as it saves an accepted pairing
/// (about a minute and a half), and a request while it starts up can make it
/// lose that pairing.
const RESTART_QUIET: Duration = Duration::from_secs(180);
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
    /// Between job requests and pairing checks.
    pub poll: Duration,
    /// How long nothing is sent after a pairing check got no answer (see
    /// `RESTART_QUIET`).
    pub restart_quiet: Duration,
    /// How long refused job requests take to count as a cleared pairing (see
    /// `REFUSED_FOR`).
    pub refused_for: Duration,
}

impl WifiConfig {
    /// From an address as people type it: `192.168.1.30`, `grinder.local`,
    /// `host:port` or a full URL.
    pub fn from_host(host: &str) -> anyhow::Result<Self> {
        let host = host.trim().trim_end_matches('/');
        if host.is_empty() {
            bail!("the grinder address is empty");
        }
        Ok(Self {
            base_url: base_url(host),
            pairing_file: None,
            pairing_id: None,
            poll: POLL,
            restart_quiet: RESTART_QUIET,
            refused_for: REFUSED_FOR,
        })
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

/// The Generic WiFi link as the app shows it (`grinder.wifi` in
/// `GET /api/state`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WifiLink {
    /// The grinder's address, as configured.
    pub grinder: String,
    pub state: WifiState,
    /// While pairing: the number the grinder shows next to gbs-anywhere.
    pub pairing_code: Option<u32>,
    /// The grinder has gbs-anywhere paired but the pairing ID is missing
    /// here: the app asks for the one the grinder shows
    /// (`POST /api/grinder/pairing`).
    pub needs_pairing_id: bool,
    /// While asking: the last pairing ID the grinder did not take, so the
    /// app can say so.
    pub unknown_pairing_id: Option<u32>,
    /// What is going on, in one sentence.
    pub detail: String,
    /// What went wrong with the last shot, until the next one goes through.
    pub problem: Option<String>,
    /// The pairing in use; `None` while unpaired.
    pub pairing_id: Option<u32>,
    /// The grinder does not answer at all (off, in standby or gone from the
    /// network), as opposed to answering with an error. Cleared as soon as
    /// it answers again.
    pub unreachable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WifiState {
    Connecting,
    /// Waiting for the person to accept gbs-anywhere on the grinder.
    Pairing,
    Paired,
    /// A request failed; retrying.
    Error,
}

/// Why a pairing ID from the app was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingIdRefusal {
    /// No `--grinder` link runs.
    NoLink,
    /// The link has a working pairing; it needs no ID.
    NotNeeded,
    /// Another ID is being checked right now.
    Busy,
}

impl std::fmt::Display for PairingIdRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoLink => "no E65/E80 grinder is set up: start gbs-anywhere with --grinder",
            Self::NotNeeded => "gbs-anywhere is paired with the grinder and needs no pairing ID",
            Self::Busy => "another pairing ID is being checked, try again in a moment",
        })
    }
}

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

    fn save(&self, path: Option<&Path>) -> anyhow::Result<()> {
        let Some(path) = path else {
            return Ok(());
        };
        let json = serde_json::to_vec_pretty(self)?;
        // Write next to it and rename, so a crash never leaves half a file.
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
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
    /// The grinder could not be reached last time (it is off or in standby):
    /// further failures are not logged until it answers again, and the app
    /// says so (`WifiLink::unreachable`).
    unreachable: bool,
}

impl WifiGrinder {
    /// Loads the pairing (or makes a device id) and saves it right away, so an
    /// unwritable file fails at startup rather than after pairing.
    pub fn new(cfg: WifiConfig) -> anyhow::Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let file = cfg.pairing_file.as_deref();
        let mut pairing = Pairing::load(file)?;
        let mut spare_id = cfg.pairing_id.filter(|id| pairing.pairing_id != Some(*id));
        if pairing.pairing_id.is_none() {
            pairing.pairing_id = spare_id.take();
        }
        pairing.save(file)?;
        let http = reqwest::Client::builder()
            // The grinder's certificate is self-signed and names no host, so it
            // cannot be checked. Requests only go to the configured address,
            // never on to wherever a reply points.
            .tls_danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
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
        })
    }

    /// Runs until the process ends: pairs when needed, then serves the
    /// grinder's jobs, retrying with backoff on errors.
    pub async fn run(mut self, server: Arc<Server>) {
        // Room for one: only the latest typed ID matters.
        let (tx, mut ids) = mpsc::channel(1);
        server.set_wifi_ids(tx);
        // One subscription for the whole run, so a shot reported while a
        // request is being retried is still answered.
        let mut events = server.subscribe();
        let mut backoff = Backoff::new(MIN_BACKOFF, MAX_BACKOFF);
        self.status(&server, WifiState::Connecting, None, "connecting");
        loop {
            let outcome = match self.pairing.pairing_id {
                None => self.pair(&server, &mut ids).await,
                Some(id) => self.serve(&server, id, &mut events).await,
            };
            if let Err(e) = outcome {
                // Off or in standby for the night: say so once, not every minute.
                let unreachable = unreachable(&e);
                if unreachable && self.unreachable {
                    tracing::debug!("grinder: {e:#}");
                } else if unreachable {
                    tracing::warn!(
                        "grinder: not reachable (off or in standby?), checking again at least \
                         every minute: {e:#}"
                    );
                } else {
                    tracing::warn!("grinder: {e:#}");
                }
                self.unreachable = unreachable;
                self.status(&server, WifiState::Error, None, format!("{e:#}"));
                backoff.wait().await;
            } else {
                backoff.reset();
            }
        }
    }

    /// Applies for pairing and waits until the person accepts it on the
    /// grinder. `Ok` once paired, or when it should be tried again.
    async fn pair(&mut self, server: &Server, ids: &mut mpsc::Receiver<u32>) -> anyhow::Result<()> {
        let app = PairingApplication::new(&self.pairing.brew_device_id);
        let reply = self.post(wifi::PATH_PAIRING_APPLICATION, &app).await?;
        if reply.status() == wifi::STATUS_ALREADY_PAIRED {
            return self.wait_for_pairing_id(server, ids).await;
        }
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
            "on the grinder, open Generic WiFi, Configuration, start pairing and accept \
             gbs-anywhere{}",
            code.map_or(String::new(), |c| format!(" (code {c})"))
        );
        tracing::info!("grinder: {detail}");
        self.status(server, WifiState::Pairing, code, detail.clone());

        let ask = json!({ "version": API_VERSION, "applicationId": application_id });
        // Left alone after a check got no answer (see `stay_quiet`).
        let mut quiet = false;
        loop {
            tokio::time::sleep(self.cfg.poll).await;
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
                } else {
                    tracing::info!("grinder: the pairing request expired, asking again");
                }
                return Ok(());
            }
            reply.ok().context("checking the pairing")?;
            let status: PairingStatusRes = reply.parse()?;
            if !status.accepted {
                if std::mem::take(&mut quiet) {
                    // Back without a restart: the request is still open.
                    self.status(server, WifiState::Pairing, code, detail.clone());
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

    /// After a pairing check got no answer: sends the grinder nothing for a
    /// while (see [`RESTART_QUIET`]). Shown as not answering meanwhile, and
    /// logged as a warning the `first` time in a row.
    async fn stay_quiet(&mut self, server: &Server, outcome: anyhow::Result<Reply>, first: bool) {
        let secs = self.cfg.restart_quiet.as_secs();
        let wait = if secs >= 60 {
            format!("{} min", secs.div_ceil(60))
        } else {
            format!("{secs} s")
        };
        let detail = format!(
            "not answering: if you have just accepted the pairing, it is restarting, so nothing \
             is sent for {wait}"
        );
        if first {
            tracing::warn!("grinder: {detail}: {}", failure(outcome));
        } else {
            tracing::debug!("grinder: {detail}: {}", failure(outcome));
        }
        self.unreachable = true;
        self.status(server, WifiState::Error, None, detail);
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
                          Pairing ID the grinder shows in its gbs-anywhere entry (Configuration)"
            .to_owned();
        let mut unknown = None;
        // The ID it refused for a while, in case it refused it only then.
        if let Some(id) = self.pairing.refused_pairing_id {
            let reply = self.post(wifi::PATH_BREW_JOBS, &Paired::new(id)).await?;
            if reply.http == StatusCode::FORBIDDEN || refused(&reply) {
                self.pairing.refused_pairing_id = None;
                self.pairing.save(self.cfg.pairing_file.as_deref())?;
                detail = format!(
                    "the grinder no longer knows pairing ID {id}: enter the Pairing ID it shows \
                     in its gbs-anywhere entry (Configuration)"
                );
                unknown = Some(id);
            } else {
                reply
                    .ok()
                    .context("checking the pairing ID the grinder refused before")?;
                self.use_pairing_id(id)?;
                tracing::info!("grinder: it takes the pairing ID it refused before");
                return Ok(());
            }
        }
        let deadline = tokio::time::Instant::now() + MAX_BACKOFF;
        // An ID typed while the link was not asking is stale by now.
        while ids.try_recv().is_ok() {}
        loop {
            tracing::warn!("grinder: {detail} (in the app, or with --grinder-pairing-id)");
            self.ask_for_pairing_id(server, &detail, unknown);
            let id = tokio::select! {
                _ = tokio::time::sleep_until(deadline) => return Ok(()),
                Some(id) = ids.recv() => id,
            };
            let reply = self.post(wifi::PATH_BREW_JOBS, &Paired::new(id)).await?;
            if reply.http == StatusCode::FORBIDDEN {
                detail = format!(
                    "the grinder does not know pairing ID {id}: enter the Pairing ID it shows \
                     in its gbs-anywhere entry (Configuration)"
                );
                unknown = Some(id);
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

    fn use_pairing_id(&mut self, id: u32) -> anyhow::Result<()> {
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
                        self.status(server, WifiState::Error, None, detail);
                        continue;
                    }
                    reply.ok().context("asking for brew jobs")?;
                    refused_since = None;
                    if self.pairing.refused_pairing_id.take().is_some() {
                        // The pairing in use works: no fallback needed.
                        self.pairing.save(self.cfg.pairing_file.as_deref())?;
                    }
                    server.note_request(reply.peer, true);
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
        self.drop_job(server, "the grinder no longer knows this gbs-anywhere");
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
                let over = server.with(|m, now| {
                    m.give_up(now, "a newer Grind-by-Sync grind replaced it")
                        .is_err()
                        && m.phase(now) == Phase::Ready
                });
                if over {
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
        let started = server.with(|m, now| {
            // A shot that is still being shown, or a brew started by hand,
            // goes first; the job waits for the next poll.
            m.on_brew_job(now, grind_result(job), target_ms)
        });
        if !started {
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
                self.drop_job(server, "the grinder stopped waiting before the shot came");
                return;
            }
            Ok(reply) if refused(&reply) => {
                tracing::warn!("grinder: it refused the job: {:?}", reply.status());
                self.drop_job(server, "the grinder refused the job");
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
    fn drop_job(&mut self, server: &Server, reason: &'static str) {
        if self.current.take().is_none() {
            return;
        }
        // Already over when someone reported or aborted it a moment ago.
        let _ = server.with(|m, now| m.give_up(now, reason));
        self.report_problem(server, reason);
    }

    /// A reported or aborted shot becomes the answer to the current job.
    /// Returns whether there is one to send.
    fn on_event(&mut self, pairing_id: u32, event: &MachineEvent) -> bool {
        let (result, time_ms, grams, reason) = match event {
            MachineEvent::ShotReported { result, .. } => {
                (BrewResult::Success, result.time_ms, result.volume_ml, "")
            }
            MachineEvent::ShotAborted { reason } => (BrewResult::Canceled, 0, 0.0, *reason),
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
        let mut bytes = Vec::new();
        while let Some(chunk) = res.chunk().await.context("reading the grinder's reply")? {
            if bytes.len() + chunk.len() > MAX_REPLY_BYTES {
                bail!("the grinder's reply is too large");
            }
            bytes.extend_from_slice(&chunk);
        }
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
        server.set_wifi(self.link(state, pairing_code, detail.into()));
    }

    fn link(&self, state: WifiState, pairing_code: Option<u32>, detail: String) -> WifiLink {
        WifiLink {
            grinder: self.cfg.base_url.clone(),
            state,
            pairing_code,
            needs_pairing_id: false,
            unknown_pairing_id: None,
            detail,
            problem: self.problem.clone(),
            pairing_id: self.pairing.pairing_id,
            unreachable: self.unreachable,
        }
    }

    /// Shows the current problem without changing the rest of the state.
    fn refresh(&self, server: &Server) {
        if let Some(mut link) = server.grinder_link().wifi {
            link.problem = self.problem.clone();
            server.set_wifi(link);
        }
    }

    fn ask_for_pairing_id(&self, server: &Server, detail: &str, unknown: Option<u32>) {
        server.set_wifi(WifiLink {
            grinder: self.cfg.base_url.clone(),
            state: WifiState::Error,
            pairing_code: None,
            needs_pairing_id: true,
            unknown_pairing_id: unknown,
            detail: detail.to_owned(),
            problem: None,
            pairing_id: None,
            unreachable: false,
        });
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
        beverage_weight_g: job.target_grams(),
        filter: None,
        raw: serde_json::to_value(job).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests;
