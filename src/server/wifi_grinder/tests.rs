//! The link against a local fake of the grinder's Local WiFi API (plain
//! HTTP), with the real machine and shot rules behind it, started and
//! stopped as the settings do (`Server::start_wifi_link`).
//!
//! By topic: `address` (what people type), `jobs` (brew jobs and their
//! answers), `pairing` (applying, the pairing ID, the quiet time), `accept`
//! (the person's accept and confirmation), `reach` (failures and retries)
//! and `relink` (stopping and replacing the link).

mod accept;
mod address;
mod jobs;
mod pairing;
mod reach;
mod relink;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use super::{WifiConfig, WifiLink, WifiPhase, WifiState};
use crate::protocol::wifi;
use crate::protocol::{MachineConfig, Phase, ShotResult};
use crate::server::Server;

/// The pairing ID the fake hands out.
const PAIRING_ID: u32 = 42;

/// [`WifiConfig::poll`] here.
const POLL_IN_TESTS: Duration = Duration::from_millis(20);

/// [`WifiConfig::restart_quiet`] here.
const RESTART_QUIET_IN_TESTS: Duration = Duration::from_millis(500);

/// [`WifiConfig::refused_for`] here: 30 polls, so a few refusals stay well
/// under it.
const REFUSED_FOR_IN_TESTS: Duration = Duration::from_millis(600);

/// [`WifiConfig::accept_settle`] here: short, so a confirmed gate is quick.
const ACCEPT_SETTLE_IN_TESTS: Duration = Duration::from_millis(40);

/// [`WifiConfig::unconfirmed_poll`] here: short, so a test with no
/// confirmation still pairs quickly.
const UNCONFIRMED_POLL_IN_TESTS: Duration = Duration::from_millis(100);

/// An unconfirmed poll no test reaches: only a confirmation releases a check.
const NO_SLOW_CHECK: Duration = Duration::from_secs(30);

/// How late a pairing check may come after its time in tests: the task's
/// wake-up and the round trip to the fake.
const GATE_SLACK: Duration = Duration::from_millis(300);

/// `n` polls of the link: how long to wait to see that something does not
/// happen, which it would have by then.
fn polls(n: u32) -> Duration {
    POLL_IN_TESTS * n
}

/// What the fake grinder knows and what it was sent.
#[derive(Default)]
struct Fake {
    /// Every request: path and when it came.
    requests: Vec<(String, Instant)>,
    /// Status checks to answer as for a request that expired.
    forget_requests: u32,
    /// Status checks to fail with HTTP 503, as if the network dropped.
    fail_checks: u32,
    /// Status checks to turn down with a client error other than an unknown
    /// request.
    refuse_checks: u32,
    /// Save the pairing on the accept and restart, as a grinder may: the
    /// accept check is not answered, and the request is gone afterwards.
    restart_on_accept: bool,
    /// When it restarted that way.
    restarted_at: Option<Instant>,
    /// Restarts on the accept that lose the pairing, as a grinder may:
    /// the accept check is not answered, and afterwards the grinder has
    /// neither the pairing nor the request. The next accept takes longer.
    lose_on_restart: u32,
    /// Accepts the pairing on the second status check, like a person who
    /// needs a moment (see [`accepted_on_the_grinder`]).
    status_checks: u32,
    /// Status checks to wait on top of that, like a person who needs longer.
    slow_accept: u32,
    /// The pairing it knows (none until accepted).
    paired: Option<u32>,
    /// Open jobs; accepting one moves it to `ongoing`.
    jobs: Vec<Value>,
    ongoing: Vec<String>,
    /// `brewDone` requests to fail with HTTP 503, as if the network dropped.
    fail_done: u32,
    /// Hand the job back out on the next `brewDone`, as after its timeout.
    hand_back: bool,
    /// Answer every `brewAccept` with "no such job", as for a job the
    /// grinder dropped a moment ago.
    lose_jobs: bool,
    /// `brewAccept` requests to fail with HTTP 503, as if the network dropped.
    fail_accept: u32,
    /// Job requests to refuse with 403, as while starting up.
    refuse_jobs: u32,
    /// `(path, body)` of every `brewAccept` and `brewDone`.
    calls: Vec<(String, Value)>,
    /// `brewAccept` requests to take (the job moves to `ongoing`) but answer
    /// with HTTP 503, as if the reply got lost.
    drop_accept_reply: u32,
    /// Pairing applications to fail with HTTP 503.
    fail_applications: u32,
    /// Pairing applications to turn down with a client error, as a grinder
    /// not set up for pairing would.
    turn_down_applications: u32,
    /// Job requests to fail with HTTP 503.
    fail_jobs: u32,
    /// How long a pairing application takes to answer.
    application_delay: Duration,
}

impl Fake {
    fn applications(&self) -> usize {
        self.requests_to(wifi::PATH_PAIRING_APPLICATION).len()
    }

    /// When each request to `path` came.
    fn requests_to(&self, path: &str) -> Vec<Instant> {
        self.requests
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, at)| *at)
            .collect()
    }

    fn calls_to(&self, path: &str) -> Vec<Value> {
        self.calls
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, b)| b.clone())
            .collect()
    }

    /// Lists every taken job again, as the grinder does about every 50 s
    /// while no result comes.
    fn hand_back_taken(&mut self) {
        for uuid in std::mem::take(&mut self.ongoing) {
            self.jobs.push(job(&uuid, 40_000));
        }
    }
}

type Shared = Arc<Mutex<Fake>>;

async fn handle(State(fake): State<Shared>, uri: Uri, Json(body): Json<Value>) -> Response {
    let path = uri.path();
    let delay = {
        let mut f = fake.lock().unwrap();
        f.requests.push((path.to_owned(), Instant::now()));
        if path == wifi::PATH_PAIRING_APPLICATION {
            f.application_delay
        } else {
            Duration::ZERO
        }
    };
    tokio::time::sleep(delay).await;
    let mut f = fake.lock().unwrap();
    let ok = |extra: Value| {
        let mut reply = json!({ "status": "Ok" });
        let extra = extra.as_object().cloned().unwrap_or_default();
        reply.as_object_mut().unwrap().extend(extra);
        Json(reply).into_response()
    };
    let refuse =
        |status: &str| (StatusCode::BAD_REQUEST, Json(json!({ "status": status }))).into_response();
    match path {
        wifi::PATH_PAIRING_APPLICATION => {
            if f.fail_applications > 0 {
                f.fail_applications -= 1;
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            if f.paired.is_some() {
                return refuse(wifi::STATUS_ALREADY_PAIRED);
            }
            if f.turn_down_applications > 0 {
                f.turn_down_applications -= 1;
                // A made-up client error: which status a grinder not set up
                // for pairing sends is not known.
                return refuse("ClientErrorPairingNotAllowed");
            }
            ok(json!({ "applicationId": 5, "pairingCode": 73 }))
        }
        wifi::PATH_PAIRING_STATUS => {
            if f.restarted_at.is_some() || f.forget_requests > 0 {
                f.forget_requests = f.forget_requests.saturating_sub(1);
                return refuse(wifi::STATUS_APPLICATION_UNKNOWN);
            }
            if f.fail_checks > 0 {
                f.fail_checks -= 1;
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            if f.refuse_checks > 0 {
                f.refuse_checks -= 1;
                return refuse("ClientErrorPairingNotAllowed");
            }
            f.status_checks += 1;
            if f.status_checks < 2 + f.slow_accept {
                return ok(json!({ "accepted": false }));
            }
            if f.lose_on_restart > 0 {
                f.lose_on_restart -= 1;
                f.forget_requests += 1;
                f.status_checks = 0;
                f.slow_accept = 10;
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            f.paired = Some(PAIRING_ID);
            if f.restart_on_accept {
                f.restarted_at = Some(Instant::now());
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            ok(json!({ "accepted": true, "pairingId": PAIRING_ID }))
        }
        _ if f.paired.is_none() || body["pairingId"] != json!(f.paired) => {
            StatusCode::FORBIDDEN.into_response()
        }
        wifi::PATH_BREW_JOBS => {
            if f.fail_jobs > 0 {
                f.fail_jobs -= 1;
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            if f.refuse_jobs > 0 {
                f.refuse_jobs -= 1;
                return StatusCode::FORBIDDEN.into_response();
            }
            ok(json!({ "brewJobs": f.jobs.clone() }))
        }
        wifi::PATH_BREW_ACCEPT => {
            f.calls.push((path.to_owned(), body.clone()));
            if f.fail_accept > 0 {
                f.fail_accept -= 1;
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            let uuid = body["grindUuid"].as_str().unwrap_or_default().to_owned();
            if f.lose_jobs {
                f.jobs.retain(|j| j["grindUuid"] != uuid);
            }
            let Some(i) = f.jobs.iter().position(|j| j["grindUuid"] == uuid) else {
                return refuse(wifi::STATUS_JOB_UNKNOWN);
            };
            f.jobs.remove(i);
            f.ongoing.push(uuid);
            if f.drop_accept_reply > 0 {
                f.drop_accept_reply -= 1;
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            ok(json!({}))
        }
        wifi::PATH_BREW_DONE => {
            f.calls.push((path.to_owned(), body.clone()));
            if f.fail_done > 0 {
                f.fail_done -= 1;
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            let uuid = body["grindUuid"].as_str().unwrap_or_default().to_owned();
            if std::mem::take(&mut f.hand_back) {
                f.ongoing.retain(|u| *u != uuid);
                f.jobs.push(job(&uuid, 40_000));
            }
            let Some(i) = f.ongoing.iter().position(|u| *u == uuid) else {
                return refuse(wifi::STATUS_ONGOING_JOB_UNKNOWN);
            };
            f.ongoing.remove(i);
            ok(json!({}))
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A fake grinder on a free port, and its base URL.
async fn fake_grinder(fake: Fake) -> (String, Shared) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (format!("http://{addr}"), serve_fake(listener, fake))
}

/// Serves a fake grinder on `listener`.
fn serve_fake(listener: TcpListener, fake: Fake) -> Shared {
    let fake = Arc::new(Mutex::new(fake));
    let app = axum::Router::new()
        .fallback(axum::routing::post(handle))
        .with_state(fake.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    fake
}

/// The link's settings for the grinder at `base`, with the short timings of
/// the tests and a fast poll.
fn test_config(base: &str) -> WifiConfig {
    let mut cfg = WifiConfig::from_host(base).unwrap();
    cfg.poll = POLL_IN_TESTS;
    cfg.restart_quiet = RESTART_QUIET_IN_TESTS;
    cfg.refused_for = REFUSED_FOR_IN_TESTS;
    cfg.accept_settle = ACCEPT_SETTLE_IN_TESTS;
    cfg.unconfirmed_poll = UNCONFIRMED_POLL_IN_TESTS;
    cfg
}

/// A change a test makes to the link's settings.
type ConfigChange = Box<dyn FnOnce(&mut WifiConfig)>;

/// A server with a fake grinder and a link to it: `Setup::new(fake)`, the
/// changes a test needs, then `start()`.
struct Setup {
    fake: Fake,
    machine: MachineConfig,
    /// Changes to [`test_config`], in order.
    config: Vec<ConfigChange>,
}

impl Setup {
    fn new(fake: Fake) -> Self {
        Self {
            fake,
            // Short, so the next shot need not wait for the last one.
            machine: MachineConfig {
                finishing_hold: Duration::from_millis(100),
                ..MachineConfig::default()
            },
            config: Vec::new(),
        }
    }

    /// The machine's settings, besides the short finishing hold.
    fn machine(mut self, machine: MachineConfig) -> Self {
        self.machine = MachineConfig {
            finishing_hold: self.machine.finishing_hold,
            ..machine
        };
        self
    }

    /// Changes the link's settings.
    fn config(mut self, change: impl FnOnce(&mut WifiConfig) + 'static) -> Self {
        self.config.push(Box::new(change));
        self
    }

    fn pairing_file(self, file: &Path) -> Self {
        let file = file.to_owned();
        self.config(move |cfg| cfg.pairing_file = Some(file))
    }

    fn pairing_id(self, id: u32) -> Self {
        self.config(move |cfg| cfg.pairing_id = Some(id))
    }

    /// The accept gate with these timings.
    fn gate(self, accept_settle: Duration, unconfirmed_poll: Duration) -> Self {
        self.config(move |cfg| {
            cfg.accept_settle = accept_settle;
            cfg.unconfirmed_poll = unconfirmed_poll;
        })
    }

    /// An unconfirmed poll no test reaches, so only a confirmation releases
    /// a pairing check.
    fn gated(self) -> Self {
        self.gate(ACCEPT_SETTLE_IN_TESTS, NO_SLOW_CHECK)
    }

    /// Every failed request is tried again after one poll's time.
    fn retrying(self) -> Self {
        self.config(|cfg| {
            cfg.min_backoff = POLL_IN_TESTS;
            cfg.max_backoff = POLL_IN_TESTS;
            cfg.unpaired_backoff = POLL_IN_TESTS;
        })
    }

    /// The server and the fake grinder, and the link's settings, for a test
    /// that does something before it starts the link.
    async fn build(self) -> (Arc<Server>, Shared, WifiConfig) {
        let (base, fake) = fake_grinder(self.fake).await;
        let server = Server::new(self.machine);
        let mut cfg = test_config(&base);
        for change in self.config {
            change(&mut cfg);
        }
        (server, fake, cfg)
    }

    /// The server and the fake grinder, with the link running.
    async fn start(self) -> (Arc<Server>, Shared) {
        let (server, fake, cfg) = self.build().await;
        start_link(&server, cfg);
        (server, fake)
    }
}

/// Starts the link for `cfg` as the server's E65/E80 link, as the settings
/// do.
fn start_link(server: &Arc<Server>, cfg: WifiConfig) {
    let link = server.wifi_link(cfg).unwrap();
    server.start_wifi_link(link);
}

/// A fake grinder on which the person has accepted already: the first
/// pairing check finds the pairing.
fn accepted_on_the_grinder() -> Fake {
    Fake {
        status_checks: 1,
        ..Fake::default()
    }
}

/// When each pairing check came.
fn checks(fake: &Shared) -> Vec<Instant> {
    fake.lock().unwrap().requests_to(wifi::PATH_PAIRING_STATUS)
}

/// How many pairing checks the fake has answered. A check is in `checks`
/// as soon as it comes, before its answer: wait for this one before
/// changing what the next answer says.
fn answered(fake: &Shared) -> u32 {
    fake.lock().unwrap().status_checks
}

/// Whether `at` came `after` `from`, within the slack of the tests.
fn due(at: Instant, from: Instant, after: Duration) -> bool {
    at >= from + after && at <= from + after + GATE_SLACK
}

fn wifi(server: &Server) -> WifiLink {
    server
        .grinder_link()
        .wifi
        .expect("the link reports its state")
}

/// `grinder.wifi` in `GET /api/state`.
fn wifi_json(server: &Server) -> Value {
    crate::server::state_json(server)["grinder"]["wifi"].clone()
}

/// The link's `<field>_age_ms` as the API shows it now.
fn age_ms(server: &Server, field: &str) -> u64 {
    wifi_json(server)[format!("{field}_age_ms")]
        .as_u64()
        .unwrap_or_else(|| panic!("{field}_age_ms"))
}

/// The link is in pairing phase `phase`.
fn in_phase(server: &Server, phase: WifiPhase) -> bool {
    server
        .grinder_link()
        .wifi
        .is_some_and(|w| w.phase == Some(phase))
}

/// The link waits for the person to accept and confirm in the app.
fn accepting(server: &Server) -> bool {
    in_phase(server, WifiPhase::Accepting)
}

/// The link asks for the pairing ID.
fn asking(server: &Server) -> bool {
    in_phase(server, WifiPhase::Asking)
}

fn in_state(server: &Server, state: WifiState) -> bool {
    server.grinder_link().wifi.is_some_and(|w| w.state == state)
}

fn paired(server: &Server) -> bool {
    in_state(server, WifiState::Paired)
}

fn brewing(server: &Server) -> bool {
    server.with(|m, now| {
        matches!(
            m.phase(now),
            Phase::Running {
                requested_by_grinder: true,
                ..
            }
        )
    })
}

fn report(server: &Server, secs: f64, grams: f64) {
    let shot = ShotResult::new(Duration::from_secs_f64(secs), grams);
    server.with(|m, now| m.report_shot(now, shot)).unwrap();
}

fn job(uuid: &str, weight_mg: u64) -> Value {
    json!({
        "grindUuid": uuid, "doseTypeExpected": 2,
        "durationExpectedMs": 28000, "brewWeightExpectedMg": weight_mg
    })
}

/// How many times the link took a job.
fn taken(fake: &Shared) -> usize {
    fake.lock().unwrap().calls_to(wifi::PATH_BREW_ACCEPT).len()
}

/// A pairing file of its own for a test, not created.
fn pairing_file(name: &str) -> PathBuf {
    crate::test_support::temp_path(name).with_extension("json")
}

fn saved_id(file: &Path) -> Value {
    saved(file)["pairing_id"].clone()
}

fn saved(file: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap()
}
