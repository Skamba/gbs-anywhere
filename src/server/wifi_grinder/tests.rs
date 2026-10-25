//! The link against a local fake of the grinder's Local WiFi API (plain
//! HTTP), with the real machine and shot rules behind it.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use super::*;
use crate::protocol::{GrinderRange, MachineConfig, MachineError, Phase, ShotResult};
use crate::server::settings::HostChange;
use crate::server::{SettingsChange, Startup, wait_for};

/// The pairing ID the fake hands out.
const PAIRING_ID: u32 = 42;

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

/// What the fake grinder knows and what it was sent.
#[derive(Default)]
struct Fake {
    /// Every request: path and when it came.
    requests: Vec<(String, Instant)>,
    /// Status checks to answer as for a request that expired.
    forget_requests: u32,
    /// Status checks to fail with HTTP 503, as if the network dropped.
    fail_checks: u32,
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

/// A fake grinder on a free port, and the link to it, running.
async fn start(
    fake: Fake,
    pairing_file: Option<PathBuf>,
    pairing_id: Option<u32>,
) -> (Arc<Server>, Shared) {
    start_with(fake, MachineConfig::default(), pairing_file, pairing_id).await
}

/// Like [`start`], with the machine's settings.
async fn start_with(
    fake: Fake,
    machine: MachineConfig,
    pairing_file: Option<PathBuf>,
    pairing_id: Option<u32>,
) -> (Arc<Server>, Shared) {
    start_polling(
        fake,
        machine,
        pairing_file,
        pairing_id,
        Duration::from_millis(20),
    )
    .await
}

/// Like [`start_with`], polling every `poll`.
async fn start_polling(
    fake: Fake,
    machine: MachineConfig,
    pairing_file: Option<PathBuf>,
    pairing_id: Option<u32>,
    poll: Duration,
) -> (Arc<Server>, Shared) {
    let (base, fake) = fake_grinder(fake).await;
    let server = Server::new(MachineConfig {
        finishing_hold: Duration::from_millis(100),
        ..machine
    });
    let mut cfg = test_config(&base);
    cfg.poll = poll;
    cfg.pairing_file = pairing_file;
    cfg.pairing_id = pairing_id;
    let link = WifiGrinder::new(cfg, server.pairing_lock.clone()).unwrap();
    run(&server, link);
    (server, fake)
}

/// The link's settings for the grinder at `base`, with the short timings of
/// the tests and a fast poll.
fn test_config(base: &str) -> WifiConfig {
    let mut cfg = WifiConfig::from_host(base).unwrap();
    cfg.poll = Duration::from_millis(20);
    cfg.restart_quiet = RESTART_QUIET_IN_TESTS;
    cfg.refused_for = REFUSED_FOR_IN_TESTS;
    cfg.accept_settle = ACCEPT_SETTLE_IN_TESTS;
    cfg.unconfirmed_poll = UNCONFIRMED_POLL_IN_TESTS;
    cfg
}

/// A fake grinder on which the person has accepted already: the first
/// pairing check finds the pairing.
fn accepted_on_the_grinder() -> Fake {
    Fake {
        status_checks: 1,
        ..Fake::default()
    }
}

/// A fake grinder on a free port, and its base URL.
async fn fake_grinder(fake: Fake) -> (String, Shared) {
    let fake = Arc::new(Mutex::new(fake));
    let app = axum::Router::new()
        .fallback(axum::routing::post(handle))
        .with_state(fake.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), fake)
}

/// Runs `link` as the server's E65/E80 link, as the settings do.
fn run(server: &Arc<Server>, link: WifiGrinder) {
    let (owner, ids, accept) = server.attach_wifi();
    server.set_wifi(owner, link.connecting());
    tokio::spawn(link.run(server.clone(), owner, ids, accept));
}

/// A fake grinder on a free port, and the link to it, running with an
/// unconfirmed poll no test reaches, for the accept-gate tests.
async fn start_gated(fake: Fake) -> (Arc<Server>, Shared) {
    start_timed(fake, ACCEPT_SETTLE_IN_TESTS, NO_SLOW_CHECK).await
}

/// Like [`start_gated`], with the given accept-gate timings.
async fn start_timed(
    fake: Fake,
    accept_settle: Duration,
    unconfirmed_poll: Duration,
) -> (Arc<Server>, Shared) {
    let (base, fake) = fake_grinder(fake).await;
    let server = Server::new(MachineConfig::default());
    let mut cfg = test_config(&base);
    cfg.accept_settle = accept_settle;
    cfg.unconfirmed_poll = unconfirmed_poll;
    run(
        &server,
        WifiGrinder::new(cfg, server.pairing_lock.clone()).unwrap(),
    );
    (server, fake)
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

/// The link waits for the person to accept and confirm in the app.
fn accepting(server: &Server) -> bool {
    server
        .grinder_link()
        .wifi
        .is_some_and(|w| w.phase == Some(WifiPhase::Accepting))
}

fn paired(server: &Server) -> bool {
    server
        .grinder_link()
        .wifi
        .is_some_and(|w| w.state == WifiState::Paired)
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
    let shot = ShotResult::from_grams(Duration::from_secs_f64(secs), grams);
    server.with(|m, now| m.report_shot(now, shot)).unwrap();
}

fn job(uuid: &str, weight_mg: u64) -> Value {
    json!({
        "grindUuid": uuid, "doseTypeExpected": 2,
        "durationExpectedMs": 28000, "brewWeightExpectedMg": weight_mg
    })
}

fn temp_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("gbs-anywhere-{name}-{}.json", uuid::Uuid::new_v4()))
}

fn saved_id(file: &Path) -> Value {
    saved(file)["pairing_id"].clone()
}

fn saved(file: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap()
}

#[test]
fn addresses_get_the_api_port() {
    let url = |h: &str| WifiConfig::from_host(h).unwrap().base_url;
    assert_eq!(url("192.168.1.30"), "https://192.168.1.30:30001");
    assert_eq!(url(" grinder.local/ "), "https://grinder.local:30001");
    assert_eq!(url("192.168.1.30:"), "https://192.168.1.30:30001");
    assert_eq!(url("192.168.1.30:4000"), "https://192.168.1.30:4000");
    assert_eq!(url("fe80::1"), "https://[fe80::1]:30001");
    assert_eq!(url("[fe80::1]"), "https://[fe80::1]:30001");
    assert_eq!(url("[fe80::1]:4000"), "https://[fe80::1]:4000");
    assert_eq!(url("http://127.0.0.1:9"), "http://127.0.0.1:9");
    assert!(WifiConfig::from_host("  ").is_err());
    assert!(WifiConfig::from_host("192.168.1.30\n").is_ok(), "trimmed");
    for bad in ["two words", "192.168.1.30\r\nx", "a\tb", &"a".repeat(3000)] {
        assert!(WifiConfig::from_host(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn the_app_sets_only_a_host_and_port() {
    let addr = |s: &str| grinder_address(s).map_err(|e| e.to_string());
    assert_eq!(addr(" 192.168.1.30 ").unwrap(), "192.168.1.30");
    assert_eq!(addr("Grinder.Local").unwrap(), "grinder.local");
    assert_eq!(addr("192.168.1.30:4000").unwrap(), "192.168.1.30:4000");
    assert_eq!(addr("192.168.1.30:").unwrap(), "192.168.1.30");
    assert_eq!(addr("fe80::1").unwrap(), "[fe80::1]");
    assert_eq!(addr("[FE80::1]:4000").unwrap(), "[fe80::1]:4000");
    assert_eq!(addr("[fe80::1]").unwrap(), "[fe80::1]");
    for bad in [
        "",
        "http://192.168.1.30",
        "192.168.1.30/x",
        "me@192.168.1.30",
        "two words",
        "192.168.1.30\nx",
        "192.168.1.30:0",
        "192.168.1.30:70000",
        "192.168.1.30:80:80",
        "[fe80::1",
        "[nope]:1",
        "-x",
        &"a".repeat(256),
    ] {
        let e = addr(bad).unwrap_err();
        assert!(e.contains("e.g. 192.168.1.30"), "{bad:?}: {e}");
    }
    // What the app sets always goes to the grinder over HTTPS.
    let url = WifiConfig::from_host(&addr("[fe80::1]:4000").unwrap()).unwrap();
    assert_eq!(url.base_url, "https://[fe80::1]:4000");
}

fn taken(fake: &Shared) -> usize {
    fake.lock().unwrap().calls_to(wifi::PATH_BREW_ACCEPT).len()
}

#[tokio::test]
async fn pairs_takes_the_job_at_once_then_sends_the_shot() {
    let file = temp_file("pairing");
    let (server, fake) = start(Fake::default(), Some(file.clone()), None).await;

    wait_for("pairing", || paired(&server)).await;
    assert_eq!(saved_id(&file), PAIRING_ID, "kept for the next start");
    assert_eq!(
        wifi(&server).pairing_id,
        Some(PAIRING_ID),
        "shown in the app"
    );
    assert!(
        server.grinder_link().last_poll.is_some(),
        "shown as connected"
    );

    // A Grind-by-Sync grind: the grinder now waits for this shot, and the job
    // is taken right away so the grinder keeps it open.
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));
    wait_for("the brew", || brewing(&server)).await;
    let grind = server.with(|m, _| m.last_grind().cloned()).unwrap();
    assert_eq!(grind.target_weight_g, Some(40.0), "the recipe weight");
    wait_for("the job taken", || taken(&fake) == 1).await;
    assert!(
        fake.lock()
            .unwrap()
            .calls_to(wifi::PATH_BREW_DONE)
            .is_empty(),
        "no result before the shot is measured"
    );

    // An integration or the app reports it.
    report(&server, 27.7, 40.6);
    wait_for("the answer", || {
        wifi(&server).detail == "sent 27.7 s, 40.6 g"
    })
    .await;
    let f = fake.lock().unwrap();
    assert_eq!(f.calls_to(wifi::PATH_BREW_ACCEPT).len(), 1, "taken once");
    let accept = &f.calls_to(wifi::PATH_BREW_ACCEPT)[0];
    assert_eq!(
        (
            &accept["grindUuid"],
            &accept["doseType"],
            &accept["groupHeadId"]
        ),
        (&json!("g-1"), &json!(2), &json!(0))
    );
    let done = &f.calls_to(wifi::PATH_BREW_DONE)[0];
    assert_eq!(done["durationFinalMs"], 27_700);
    assert_eq!(done["massFinalMg"], 40_600);
    assert_eq!(done["result"], "Success");
    assert!(
        f.ongoing.is_empty() && f.jobs.is_empty(),
        "the grinder has it"
    );
    drop(f);
    let _ = std::fs::remove_file(file);
}

#[tokio::test]
async fn a_lost_result_is_sent_again_without_taking_the_job_twice() {
    let fake = Fake {
        fail_done: 1,
        ..Fake::default()
    };
    let (server, fake) = start(fake, None, None).await;
    wait_for("pairing", || paired(&server)).await;
    fake.lock().unwrap().jobs.push(job("g-1", 0));
    wait_for("the brew", || brewing(&server)).await;

    report(&server, 30.0, 38.0);
    wait_for("the answer", || {
        wifi(&server).detail == "sent 30.0 s, 38.0 g"
    })
    .await;
    {
        let f = fake.lock().unwrap();
        assert_eq!(f.calls_to(wifi::PATH_BREW_ACCEPT).len(), 1, "taken once");
        let done = f.calls_to(wifi::PATH_BREW_DONE);
        assert_eq!(done.len(), 2, "the result went out again");
        assert_eq!(done[0]["brewEventUuid"], done[1]["brewEventUuid"]);
    }
    assert_eq!(wifi(&server).problem, None, "the retry fixed it");
}

#[tokio::test]
async fn a_job_handed_back_out_is_taken_again() {
    let fake = Fake {
        hand_back: true,
        ..Fake::default()
    };
    let (server, fake) = start(fake, None, None).await;
    wait_for("pairing", || paired(&server)).await;
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));
    wait_for("the brew", || brewing(&server)).await;

    report(&server, 31.0, 40.0);
    wait_for("the answer", || {
        wifi(&server).detail == "sent 31.0 s, 40.0 g"
    })
    .await;
    let f = fake.lock().unwrap();
    assert_eq!(f.calls_to(wifi::PATH_BREW_ACCEPT).len(), 2);
    let done = f.calls_to(wifi::PATH_BREW_DONE);
    assert_eq!(done.len(), 2);
    assert_eq!(done[0]["brewEventUuid"], done[1]["brewEventUuid"]);
    assert!(f.ongoing.is_empty() && f.jobs.is_empty());
}

#[tokio::test]
async fn a_job_handed_back_while_waiting_is_taken_again() {
    let (server, fake) = start(Fake::default(), None, None).await;
    wait_for("pairing", || paired(&server)).await;
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));
    wait_for("the job taken", || taken(&fake) == 1).await;

    // Tamping takes a while: the grinder hands the job back out twice.
    for n in 2..=3 {
        fake.lock().unwrap().hand_back_taken();
        wait_for("the job taken again", || taken(&fake) == n).await;
        assert!(brewing(&server), "still waiting for the shot");
    }

    report(&server, 29.5, 39.0);
    wait_for("the answer", || {
        wifi(&server).detail == "sent 29.5 s, 39.0 g"
    })
    .await;
    let f = fake.lock().unwrap();
    assert_eq!(f.calls_to(wifi::PATH_BREW_ACCEPT).len(), 3);
    let done = f.calls_to(wifi::PATH_BREW_DONE);
    assert_eq!(done.len(), 1, "the result went straight through");
    assert_eq!(done[0]["result"], "Success");
    assert!(f.ongoing.is_empty() && f.jobs.is_empty());
}

#[tokio::test]
async fn a_job_that_could_not_be_taken_is_tried_again() {
    let fake = Fake {
        fail_accept: 1,
        ..Fake::default()
    };
    let (server, fake) = start(fake, None, None).await;
    wait_for("pairing", || paired(&server)).await;
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));
    wait_for("the job taken", || taken(&fake) == 2).await;
    assert!(brewing(&server), "still waiting for the shot");
    assert!(fake.lock().unwrap().jobs.is_empty(), "the grinder has it");

    report(&server, 28.4, 40.1);
    wait_for("the answer", || {
        wifi(&server).detail == "sent 28.4 s, 40.1 g"
    })
    .await;
    let f = fake.lock().unwrap();
    assert_eq!(
        f.calls_to(wifi::PATH_BREW_ACCEPT).len(),
        2,
        "not taken again"
    );
    assert_eq!(f.calls_to(wifi::PATH_BREW_DONE).len(), 1);
}

#[tokio::test]
async fn a_taken_job_the_grinder_forgot_is_reported_as_not_reached() {
    let (server, fake) = start(Fake::default(), None, None).await;
    wait_for("pairing", || paired(&server)).await;
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));
    wait_for("the job taken", || taken(&fake) == 1).await;

    // The grinder restarts and forgets the job without listing it again.
    fake.lock().unwrap().ongoing.clear();
    report(&server, 27.0, 40.0);
    wait_for("the problem", || {
        wifi(&server).problem.as_deref() == Some("the shot did not reach the grinder in time")
    })
    .await;
    let f = fake.lock().unwrap();
    assert_eq!(
        f.calls_to(wifi::PATH_BREW_DONE).len(),
        1,
        "tried, then given up"
    );
    assert_eq!(f.calls_to(wifi::PATH_BREW_ACCEPT).len(), 1);
}

#[test]
fn only_a_grinder_that_does_not_answer_is_unreachable() {
    let silent = anyhow::Error::new(std::io::Error::other("not a request error"));
    assert!(!unreachable(&silent));
    let odd = Reply {
        http: StatusCode::BAD_REQUEST,
        body: json!({ "status": "ClientErrorJsonSyntaxWrong" }),
        peer: None,
    };
    assert!(!unreachable(&odd.ok().unwrap_err()), "it did answer");
}

#[tokio::test]
async fn a_closed_port_is_unreachable() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let refused = reqwest::Client::new()
        .post("http://127.0.0.1:1/")
        .send()
        .await
        .unwrap_err();
    let e = anyhow::Error::new(refused).context("asking for brew jobs");
    assert!(unreachable(&e));
}

#[tokio::test]
async fn the_app_sees_a_grinder_that_does_not_answer_until_it_does() {
    // A port nobody listens on, like a grinder switched off for the night.
    let addr = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let server = Server::new(MachineConfig::default());
    let cfg = test_config(&format!("http://{addr}"));
    run(&server, WifiGrinder::new(cfg, Arc::default()).unwrap());
    wait_for("the error", || {
        server
            .grinder_link()
            .wifi
            .is_some_and(|w| w.state == WifiState::Failing)
    })
    .await;
    assert!(wifi(&server).unreachable);

    // Switched on: it answers on the next try and pairs.
    let fake: Shared = Arc::default();
    let app = axum::Router::new()
        .fallback(axum::routing::post(handle))
        .with_state(fake);
    let listener = TcpListener::bind(addr)
        .await
        .expect("the port is free again");
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    wait_for("paired", || paired(&server)).await;
    assert!(!wifi(&server).unreachable);
}

#[tokio::test]
async fn the_jobs_target_time_sets_the_shot_window() {
    let (server, fake) = start(Fake::default(), None, None).await;
    wait_for("pairing", || paired(&server)).await;
    // `in_grinder_range` of the last shot reported.
    let last_verdict = || {
        server.with(|m, _| {
            m.events_after(0)
                .filter_map(|e| match e.event {
                    MachineEvent::ShotReported {
                        in_grinder_range, ..
                    } => Some(in_grinder_range),
                    _ => None,
                })
                .last()
        })
    };

    // A 20 s target: within 15 s of it, but over 10 s.
    let mut timed = job("g-1", 40_000);
    timed["durationExpectedMs"] = json!(20_000);
    fake.lock().unwrap().jobs.push(timed);
    wait_for("the brew", || brewing(&server)).await;
    let window = GrinderRange {
        min_ms: 10_000,
        max_ms: 35_000,
    };
    assert_eq!(server.with(|m, _| m.grinder_range()), Some(window));
    // The API shows the job as an E64 WS's grind and knob press: the grind,
    // then the request with the job's target time.
    let events: Vec<Value> = server.with(|m, _| {
        m.events_after(0)
            .map(|e| serde_json::to_value(e).unwrap())
            .collect()
    });
    let kinds: Vec<_> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        [
            "grinder_grind_finished",
            "grinder_shot_requested",
            "shot_started"
        ]
    );
    assert_eq!(events[0]["result"]["target_weight_g"], 40.0);
    assert_eq!(
        (&events[1]["target_time_ms"], &events[1]["script_id"]),
        (&json!(20_000), &Value::Null)
    );
    // Within the E64 WS's 80 s, but more than 15 s over this target.
    report(&server, 36.0, 40.0);
    assert_eq!(last_verdict(), Some(Some(false)));
}

#[tokio::test]
async fn an_aborted_shot_is_cancelled() {
    let (server, fake) = start(Fake::default(), None, None).await;
    wait_for("pairing", || paired(&server)).await;

    fake.lock().unwrap().jobs.push(job("g-1", 40_000));
    wait_for("the brew", || brewing(&server)).await;
    server.with(|m, now| m.abort(now)).unwrap();
    wait_for("the cancel", || {
        !fake
            .lock()
            .unwrap()
            .calls_to(wifi::PATH_BREW_DONE)
            .is_empty()
    })
    .await;
    let f = fake.lock().unwrap();
    assert_eq!(f.calls_to(wifi::PATH_BREW_DONE)[0]["result"], "Canceled");
    assert!(
        f.ongoing.is_empty() && f.jobs.is_empty(),
        "the grinder closed it"
    );
}

#[tokio::test]
async fn no_shot_in_time_cancels_the_job() {
    let machine = MachineConfig {
        shot_window: Some(Duration::from_millis(300)),
        ..MachineConfig::default()
    };
    let (server, fake) = start_with(Fake::default(), machine, None, None).await;
    wait_for("pairing", || paired(&server)).await;
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));
    wait_for("the brew", || brewing(&server)).await;

    // `brewing` ticks the machine, as the server's timer does.
    wait_for("the timeout", || !brewing(&server)).await;
    wait_for("the cancel", || {
        wifi(&server).detail
            == "told the grinder the shot was cancelled (no result reported in time)"
    })
    .await;
    let f = fake.lock().unwrap();
    let done = f.calls_to(wifi::PATH_BREW_DONE);
    assert_eq!(done.len(), 1);
    assert_eq!(done[0]["result"], "Canceled");
    assert!(
        f.ongoing.is_empty() && f.jobs.is_empty(),
        "the grinder closed it"
    );
}

#[tokio::test]
async fn a_job_the_grinder_no_longer_has_ends_the_brew() {
    let fake = Fake {
        lose_jobs: true,
        ..Fake::default()
    };
    let (server, fake) = start(fake, None, None).await;
    wait_for("pairing", || paired(&server)).await;
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));

    wait_for("giving up", || wifi(&server).problem.is_some()).await;
    assert!(!brewing(&server));
    let late = ShotResult::from_grams(Duration::from_secs(30), 40.0);
    assert_eq!(
        server.with(|m, now| m.report_shot(now, late)),
        Err(MachineError::NotRunning),
        "a late shot has nothing to answer"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let f = fake.lock().unwrap();
    assert_eq!(f.calls_to(wifi::PATH_BREW_ACCEPT).len(), 1, "tried once");
    assert!(
        f.calls_to(wifi::PATH_BREW_DONE).is_empty(),
        "nothing to answer"
    );
}

#[tokio::test]
async fn the_newest_job_is_the_shot_being_pulled() {
    let (server, fake) = start(Fake::default(), None, None).await;
    wait_for("pairing", || paired(&server)).await;
    fake.lock()
        .unwrap()
        .jobs
        .extend([job("old", 36_000), job("new", 40_000)]);
    wait_for("the brew", || brewing(&server)).await;
    let grind = server.with(|m, _| m.last_grind().cloned()).unwrap();
    assert_eq!(grind.target_weight_g, Some(40.0));

    report(&server, 29.0, 40.0);
    wait_for("the answer", || {
        wifi(&server).detail == "sent 29.0 s, 40.0 g"
    })
    .await;
    // The older job stays listed, but is not brewed after this one.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!brewing(&server));
    let accepted = fake.lock().unwrap().calls_to(wifi::PATH_BREW_ACCEPT);
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0]["grindUuid"], "new");
}

#[tokio::test]
async fn a_newer_grind_cancels_one_still_waiting() {
    let (server, fake) = start(Fake::default(), None, None).await;
    wait_for("pairing", || paired(&server)).await;
    fake.lock().unwrap().jobs.push(job("first", 36_000));
    wait_for("the first job taken", || taken(&fake) == 1).await;

    // Ground again before pulling the shot.
    fake.lock().unwrap().jobs.push(job("second", 40_000));
    wait_for("the second brew", || {
        brewing(&server)
            && server.with(|m, _| m.last_grind().and_then(|g| g.target_weight_g)) == Some(40.0)
    })
    .await;
    report(&server, 28.0, 40.0);
    wait_for("the answer", || {
        wifi(&server).detail == "sent 28.0 s, 40.0 g"
    })
    .await;
    let f = fake.lock().unwrap();
    let accepted = f.calls_to(wifi::PATH_BREW_ACCEPT);
    assert_eq!(accepted.len(), 2);
    assert_eq!(
        (&accepted[0]["grindUuid"], &accepted[1]["grindUuid"]),
        (&json!("first"), &json!("second"))
    );
    let done = f.calls_to(wifi::PATH_BREW_DONE);
    assert_eq!(
        (&done[0]["grindUuid"], &done[0]["result"]),
        (&json!("first"), &json!("Canceled")),
        "the grinder stops waiting for the first"
    );
    assert_eq!(
        (&done[1]["grindUuid"], &done[1]["result"]),
        (&json!("second"), &json!("Success"))
    );
    assert!(f.ongoing.is_empty() && f.jobs.is_empty());
}

#[tokio::test]
async fn a_short_refusal_keeps_the_pairing_a_long_one_pairs_again() {
    let file = temp_file("refusal");
    let saved = json!({ "brew_device_id": "dev-1", "pairing_id": PAIRING_ID });
    std::fs::write(&file, saved.to_string()).unwrap();
    let fake = Fake {
        paired: Some(PAIRING_ID),
        refuse_jobs: 1,
        ..Fake::default()
    };
    let (server, fake) = start(fake, Some(file.clone()), None).await;
    wait_for("paired", || paired(&server)).await;
    assert_eq!(
        saved_id(&file),
        PAIRING_ID,
        "one refusal is not a cleared pairing"
    );
    assert_eq!(fake.lock().unwrap().applications(), 0);

    // Cleared on the grinder: it refuses from now on, so after a while
    // gbs-anywhere pairs anew.
    fake.lock().unwrap().paired = None;
    wait_for("the new pairing", || fake.lock().unwrap().paired.is_some()).await;
    wait_for("paired again", || paired(&server)).await;
    assert_eq!(fake.lock().unwrap().applications(), 1);
    let saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(saved["brew_device_id"], "dev-1", "same device as before");
    assert!(
        saved.get("refused_pairing_id").is_none(),
        "the refused ID went with the new pairing"
    );
    let _ = std::fs::remove_file(file);
}

#[tokio::test]
async fn the_given_pairing_id_is_tried_when_the_saved_one_is_refused() {
    let file = temp_file("given");
    let stale = json!({ "brew_device_id": "dev-1", "pairing_id": 7 });
    std::fs::write(&file, stale.to_string()).unwrap();
    let fake = Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    };
    let (server, fake) = start(fake, Some(file.clone()), Some(PAIRING_ID)).await;
    wait_for("saved", || saved_id(&file) == PAIRING_ID).await;
    wait_for("paired", || paired(&server)).await;
    let refused_kept = || {
        let saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        saved.get("refused_pairing_id").is_some()
    };
    wait_for("the stale ID let go once the given one works", || {
        !refused_kept()
    })
    .await;
    assert_eq!(
        fake.lock().unwrap().applications(),
        0,
        "no new pairing needed"
    );
    let _ = std::fs::remove_file(file);
}

#[tokio::test]
async fn a_grinder_that_restarts_on_the_accept_is_left_alone_while_it_starts_up() {
    let fake = Fake {
        restart_on_accept: true,
        ..Fake::default()
    };
    let (server, fake) = start(fake, None, None).await;
    wait_for("the question", || {
        server
            .grinder_link()
            .wifi
            .is_some_and(|w| w.phase == Some(WifiPhase::Asking))
    })
    .await;
    {
        let f = fake.lock().unwrap();
        let restarted = f.restarted_at.unwrap();
        let (path, next) = f
            .requests
            .iter()
            .find(|(_, at)| *at > restarted)
            .cloned()
            .unwrap();
        assert!(
            next >= restarted + RESTART_QUIET_IN_TESTS,
            "nothing sent while it starts up"
        );
        assert_eq!(path, wifi::PATH_PAIRING_STATUS, "the request checked first");
        assert_eq!(f.applications(), 2, "then asked once more");
    }
    server.use_grinder_pairing_id(PAIRING_ID).unwrap();
    wait_for("paired", || paired(&server)).await;
}

#[tokio::test]
async fn a_pairing_lost_in_the_restart_is_shown_until_one_sticks() {
    let fake = Fake {
        lose_on_restart: 1,
        ..Fake::default()
    };
    let (server, fake) = start(fake, None, None).await;
    let link = || server.grinder_link().wifi;
    wait_for("the first request", || {
        link().is_some_and(|w| w.state == WifiState::Pairing)
    })
    .await;
    assert!(!wifi(&server).pairing_lost, "nothing lost yet");
    wait_for("the request again, as lost", || {
        link().is_some_and(|w| w.state == WifiState::Pairing && w.pairing_lost)
    })
    .await;
    let w = wifi(&server);
    assert_eq!(w.pairing_code, Some(73));
    assert!(w.detail.contains("did not stick"), "{}", w.detail);
    assert_eq!(fake.lock().unwrap().applications(), 2, "asked again");
    wait_for("paired", || paired(&server)).await;
    assert!(!wifi(&server).pairing_lost, "this one stuck");
}

#[tokio::test]
async fn a_pairing_request_still_open_after_the_quiet_time_is_kept() {
    // No answer once, then the grinder answers with the request still open.
    let fake = Fake {
        fail_checks: 1,
        slow_accept: 10,
        ..Fake::default()
    };
    let (server, fake) = start(fake, None, None).await;
    let link = || server.grinder_link().wifi;
    wait_for("the quiet time", || {
        link().is_some_and(|w| w.phase == Some(WifiPhase::Quiet))
    })
    .await;
    // The code again, still waiting for the confirmation: nobody tapped
    // Accepted, so it is not checking as confirmed.
    wait_for("the code again, accepting", || {
        link().is_some_and(|w| w.pairing_code == Some(73) && w.phase == Some(WifiPhase::Accepting))
    })
    .await;
    wait_for("pairing", || paired(&server)).await;
    assert_eq!(fake.lock().unwrap().applications(), 1, "no new request");
}

#[tokio::test]
async fn the_quiet_after_an_unanswered_check_is_a_phase_of_the_pairing() {
    let fake = Fake {
        fail_checks: 1,
        ..Fake::default()
    };
    let (base, fake) = fake_grinder(fake).await;
    let server = Server::new(MachineConfig::default());
    let mut cfg = test_config(&base);
    // Longer than the test, so the checks below all fall into it.
    cfg.restart_quiet = Duration::from_secs(30);
    run(
        &server,
        WifiGrinder::new(cfg, server.pairing_lock.clone()).unwrap(),
    );
    wait_for("the quiet time", || {
        server
            .grinder_link()
            .wifi
            .is_some_and(|w| w.phase == Some(WifiPhase::Quiet))
    })
    .await;
    // Still pairing, and not answering; no code to show.
    let w = wifi(&server);
    assert_eq!(w.state, WifiState::Pairing);
    assert!(w.unreachable);
    assert_eq!(w.pairing_code, None);
    let state = crate::server::state_json(&server);
    assert_eq!(state["grinder"]["wifi"]["state"], json!("pairing"));
    assert_eq!(state["grinder"]["wifi"]["phase"], json!("quiet"));
    // Neither a confirmation nor a pairing ID is taken meanwhile.
    assert_eq!(
        server.grinder_accepted(),
        Err(PairingIdRefusal::NotAccepting)
    );
    assert_eq!(
        server.use_grinder_pairing_id(PAIRING_ID),
        Err(PairingIdRefusal::NotAsking)
    );
    // And nothing is sent to the grinder, while the phase's age grows.
    let age = |state: &Value| state["grinder"]["wifi"]["phase_age_ms"].as_u64().unwrap();
    let before = age(&state);
    let sent = fake.lock().unwrap().requests.len();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fake.lock().unwrap().requests.len(), sent, "left alone");
    let later = crate::server::state_json(&server);
    assert_eq!(later["grinder"]["wifi"]["phase"], json!("quiet"));
    assert!(
        age(&later) >= before + 200,
        "{} then {}",
        before,
        age(&later)
    );
}

#[tokio::test]
async fn an_expired_pairing_request_is_replaced_at_once() {
    // Not accepted in time, while the grinder kept answering: nothing to
    // wait for.
    let fake = Fake {
        forget_requests: 1,
        ..Fake::default()
    };
    let (server, fake) = start(fake, None, None).await;
    wait_for("the request again", || {
        fake.lock().unwrap().applications() == 2
    })
    .await;
    assert!(!wifi(&server).pairing_lost, "it did not restart");
    wait_for("pairing", || paired(&server)).await;
    let f = fake.lock().unwrap();
    let applied = f.requests_to(wifi::PATH_PAIRING_APPLICATION);
    let checks = f.requests_to(wifi::PATH_PAIRING_STATUS);
    assert_eq!(applied.len(), 2, "asked again");
    assert!(applied[1] < checks[0] + RESTART_QUIET_IN_TESTS, "at once");
}

#[tokio::test]
async fn a_new_pairing_code_starts_its_own_phase_age() {
    // The first request expires while the grinder keeps answering: the new
    // code's phase counts from when it was shown, not from the first one.
    let fake = Fake {
        forget_requests: 1,
        slow_accept: 1_000,
        ..Fake::default()
    };
    let (server, fake) = start(fake, None, None).await;
    wait_for("the request again", || {
        fake.lock().unwrap().applications() == 2 && accepting(&server)
    })
    .await;
    // The reply to the second application, and the code it shows, come a
    // moment after the request (well within the 100 ms between checks).
    tokio::time::sleep(Duration::from_millis(30)).await;
    let since_second = fake
        .lock()
        .unwrap()
        .requests_to(wifi::PATH_PAIRING_APPLICATION)[1]
        .elapsed();
    let age = crate::server::state_json(&server)["grinder"]["wifi"]["phase_age_ms"]
        .as_u64()
        .unwrap();
    assert!(
        u128::from(age) <= since_second.as_millis() + 50,
        "{age} ms for a code shown {since_second:?} ago"
    );
}

#[tokio::test]
async fn asking_again_keeps_a_refused_pairing_id() {
    // The grinder refuses the ID typed in; when the question is asked again
    // after its window, the refusal is still said.
    let (base, fake) = fake_grinder(Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    })
    .await;
    let server = Server::new(MachineConfig::default());
    let mut cfg = test_config(&base);
    cfg.ask_window = Duration::from_millis(200);
    run(
        &server,
        WifiGrinder::new(cfg, server.pairing_lock.clone()).unwrap(),
    );
    let asks = || {
        server
            .grinder_link()
            .wifi
            .is_some_and(|w| w.phase == Some(WifiPhase::Asking))
    };
    wait_for("the question", asks).await;
    server.use_grinder_pairing_id(7).unwrap();
    wait_for("the refusal", || {
        wifi(&server).unknown_pairing_id == Some(7)
    })
    .await;
    let applications = || fake.lock().unwrap().applications();
    let before = applications();
    wait_for("asked again", || applications() > before && asks()).await;
    // The new round shows a moment after its request (within the window).
    tokio::time::sleep(Duration::from_millis(50)).await;
    let w = wifi(&server);
    assert_eq!(w.unknown_pairing_id, Some(7), "still refused");
    assert!(w.detail.contains("pairing ID 7"), "{}", w.detail);
}

#[tokio::test]
async fn a_lost_pairing_id_is_asked_for_and_checked() {
    // Paired on the grinder, but nothing saved here (the grinder restarted
    // before it said the ID).
    let fake = Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    };
    let file = temp_file("lost");
    let (server, _fake) = start(fake, Some(file.clone()), None).await;
    wait_for("the question", || {
        server
            .grinder_link()
            .wifi
            .is_some_and(|w| w.phase == Some(WifiPhase::Asking))
    })
    .await;
    assert_eq!(wifi(&server).pairing_id, None, "none in use yet");
    assert_eq!(
        wifi(&server).state,
        WifiState::Pairing,
        "asking is part of the pairing"
    );
    let state = crate::server::state_json(&server);
    assert_eq!(state["grinder"]["wifi"]["phase"], json!("asking"));
    assert_eq!(
        server.grinder_accepted(),
        Err(PairingIdRefusal::NotAccepting),
        "nothing to confirm while asking"
    );

    // A wrong one is checked with the grinder, not kept.
    server.use_grinder_pairing_id(PAIRING_ID + 1).unwrap();
    wait_for("the refusal", || {
        wifi(&server).detail.contains("does not know pairing ID 43")
    })
    .await;
    assert_eq!(wifi(&server).phase, Some(WifiPhase::Asking), "still asking");
    assert_eq!(
        wifi(&server).unknown_pairing_id,
        Some(PAIRING_ID + 1),
        "the app says so"
    );
    assert_eq!(saved_id(&file), Value::Null, "not saved");

    server.use_grinder_pairing_id(PAIRING_ID).unwrap();
    wait_for("paired", || paired(&server)).await;
    assert_eq!(saved_id(&file), PAIRING_ID);
    assert_eq!(
        server.use_grinder_pairing_id(5),
        Err(PairingIdRefusal::NotNeeded),
        "a working pairing cannot be replaced from the app"
    );
    assert_eq!(PairingIdRefusal::NotNeeded.code(), "grinder_already_paired");
    let _ = std::fs::remove_file(file);
}

#[tokio::test]
async fn a_few_refusals_while_starting_up_keep_a_valid_pairing() {
    let file = temp_file("refusals-kept");
    let saved = json!({ "brew_device_id": "dev-1", "pairing_id": PAIRING_ID });
    std::fs::write(&file, saved.to_string()).unwrap();
    // Still paired, but refusing three polls in a row while it starts up.
    let fake = Fake {
        paired: Some(PAIRING_ID),
        refuse_jobs: 3,
        ..Fake::default()
    };
    let (server, fake) = start(fake, Some(file.clone()), None).await;
    let asks = |s: &Server| {
        s.grinder_link()
            .wifi
            .is_some_and(|w| w.phase == Some(WifiPhase::Asking))
    };
    wait_for("paired, or asking to pair again", || {
        paired(&server) || asks(&server) || fake.lock().unwrap().applications() > 0
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let kept = saved_id(&file);
    let _ = std::fs::remove_file(&file);
    assert_eq!(kept, PAIRING_ID, "the saved pairing ID is kept");
    assert_eq!(fake.lock().unwrap().applications(), 0, "no new pairing");
    assert!(!asks(&server), "the app does not ask for the pairing ID");
}

#[tokio::test]
async fn a_refused_pairing_id_is_tried_again_before_the_app_asks() {
    // Refused for a while, but the grinder still has gbs-anywhere paired.
    let file = temp_file("refused-again");
    let saved =
        json!({ "brew_device_id": "dev-1", "pairing_id": null, "refused_pairing_id": PAIRING_ID });
    std::fs::write(&file, saved.to_string()).unwrap();
    let fake = Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    };
    let (server, fake) = start(fake, Some(file.clone()), None).await;
    wait_for("paired", || paired(&server)).await;
    let saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let _ = std::fs::remove_file(&file);
    assert_eq!(saved["pairing_id"], PAIRING_ID, "taken again");
    assert!(saved.get("refused_pairing_id").is_none(), "no longer kept");
    assert_eq!(
        fake.lock().unwrap().applications(),
        1,
        "asked once, already paired"
    );
    assert_eq!(wifi(&server).phase, None, "nothing to type in");
}

#[tokio::test]
async fn a_refused_pairing_id_the_grinder_still_refuses_is_dropped() {
    let file = temp_file("refused-dropped");
    let saved = json!({ "brew_device_id": "dev-1", "pairing_id": null, "refused_pairing_id": 7 });
    std::fs::write(&file, saved.to_string()).unwrap();
    let fake = Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    };
    let (server, _fake) = start(fake, Some(file.clone()), None).await;
    wait_for("the question", || {
        server
            .grinder_link()
            .wifi
            .is_some_and(|w| w.phase == Some(WifiPhase::Asking))
    })
    .await;
    let saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let _ = std::fs::remove_file(&file);
    assert!(
        saved.get("refused_pairing_id").is_none(),
        "tried once, then dropped"
    );
    assert!(
        wifi(&server)
            .detail
            .contains("no longer knows pairing ID 7"),
        "{}",
        wifi(&server).detail
    );
    assert_eq!(wifi(&server).unknown_pairing_id, Some(7));
}

#[tokio::test]
async fn a_result_after_a_lost_accept_reply_is_retried() {
    // The grinder takes the job but its reply is lost, and the first brewDone
    // fails. A slower poll lets the shot come in before the next poll.
    let fake = Fake {
        drop_accept_reply: 1,
        fail_done: 1,
        ..Fake::default()
    };
    let poll = Duration::from_millis(600);
    let (server, fake) = start_polling(fake, MachineConfig::default(), None, None, poll).await;
    wait_for("pairing", || paired(&server)).await;
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));
    wait_for("the job taken", || taken(&fake) == 1).await;
    assert_eq!(fake.lock().unwrap().ongoing, ["g-1"], "the grinder has it");
    report(&server, 27.0, 40.0);
    wait_for("the shot sent or given up", || {
        let w = wifi(&server);
        w.detail == "sent 27.0 s, 40.0 g" || w.problem.is_some_and(|p| p.contains("did not reach"))
    })
    .await;
    assert_eq!(wifi(&server).problem, None, "the retry delivered the shot");
    assert!(
        fake.lock().unwrap().ongoing.is_empty(),
        "the grinder got it"
    );
}

#[tokio::test]
async fn a_new_link_waits_for_the_one_it_replaces_before_it_touches_the_pairing() {
    let file = temp_file("handover");
    let unpaired = json!({ "brew_device_id": "dev-1", "pairing_id": null });
    std::fs::write(&file, unpaired.to_string()).unwrap();
    let fake = Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    };
    let (base, fake) = fake_grinder(fake).await;
    let server = Server::new(MachineConfig::default());
    // Stands in for a replaced link whose task has not ended yet: aborting
    // it only takes effect at its next await.
    let old = server.pairing_lock.clone().lock_owned().await;
    let mut cfg = WifiConfig::from_host(&base).unwrap();
    cfg.poll = Duration::from_millis(20);
    cfg.pairing_file = Some(file.clone());
    run(
        &server,
        WifiGrinder::new(cfg, server.pairing_lock.clone()).unwrap(),
    );

    // The old link's last step: the pairing went through, and it saves it.
    let accepted = json!({ "brew_device_id": "dev-1", "pairing_id": PAIRING_ID });
    std::fs::write(&file, accepted.to_string()).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(fake.lock().unwrap().requests.is_empty(), "it waits");
    assert_eq!(saved(&file), accepted, "not written over");
    assert_eq!(wifi(&server).state, WifiState::Connecting);

    drop(old);
    wait_for("paired", || paired(&server)).await;
    assert_eq!(
        fake.lock().unwrap().applications(),
        0,
        "it went on with what the old link saved"
    );
    assert_eq!(wifi(&server).pairing_id, Some(PAIRING_ID));
    assert_eq!(saved(&file), accepted);
    let _ = std::fs::remove_file(file);
}

#[tokio::test]
async fn nothing_is_sent_to_the_grinder_until_the_person_accepts() {
    // A slow check the test never reaches, so only the confirmation can
    // release the first check.
    let (server, fake) = start_gated(accepted_on_the_grinder()).await;
    wait_for("awaiting the accept", || accepting(&server)).await;

    // With no confirmation: still nothing sent, and the app and its API keep
    // serving (the state shows the waiting phase).
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        wifi(&server).phase,
        Some(WifiPhase::Accepting),
        "still waiting, not holding yet"
    );
    assert!(
        checks(&fake).is_empty(),
        "no pairing check before the person confirms"
    );
    let state = crate::server::state_json(&server);
    assert_eq!(
        state["grinder"]["wifi"]["phase"],
        json!("accepting"),
        "the state keeps serving while the gate holds"
    );
    assert!(state["grinder"]["wifi"]["grinder"].is_string());
    assert_eq!(
        server.use_grinder_pairing_id(PAIRING_ID),
        Err(PairingIdRefusal::NotAsking),
        "no pairing ID is taken while waiting for the accept"
    );
    assert_eq!(PairingIdRefusal::NotAsking.code(), "grinder_not_asking");

    // Confirmed: the check goes out and it pairs.
    server.grinder_accepted().unwrap();
    wait_for("paired", || paired(&server)).await;
    assert_eq!(
        checks(&fake).len(),
        1,
        "the check went out after the confirmation"
    );
}

#[tokio::test]
async fn a_confirmation_holds_the_check_for_the_settle_after_it() {
    let settle = Duration::from_millis(1_200);
    let (server, fake) = start_timed(accepted_on_the_grinder(), settle, NO_SLOW_CHECK).await;
    wait_for("awaiting the accept", || accepting(&server)).await;

    let tap = Instant::now();
    server.grinder_accepted().unwrap();
    // The button is gone and the hold is shown.
    wait_for("the hold shown", || {
        wifi(&server).phase == Some(WifiPhase::Holding)
    })
    .await;

    wait_for("paired", || paired(&server)).await;
    assert_eq!(wifi(&server).phase, None, "the hold is over");
    let checked = checks(&fake)[0];
    assert!(
        due(checked, tap, settle),
        "checked {:?} after the tap",
        checked - tap
    );
}

#[tokio::test]
async fn without_a_confirmation_it_checks_only_every_unconfirmed_poll() {
    // Nobody accepts or confirms, as for a code left on screen.
    let every = Duration::from_millis(500);
    let fake = Fake {
        slow_accept: 1_000,
        ..Fake::default()
    };
    let (server, fake) = start_timed(fake, ACCEPT_SETTLE_IN_TESTS, every).await;
    wait_for("awaiting the accept", || accepting(&server)).await;
    let applied = fake
        .lock()
        .unwrap()
        .requests_to(wifi::PATH_PAIRING_APPLICATION)[0];

    // Short of the slow check: nothing sent.
    let short = (every - Duration::from_millis(150)).saturating_sub(applied.elapsed());
    tokio::time::sleep(short).await;
    assert!(checks(&fake).is_empty(), "no check before the slow one");

    // Then one check each time, and no more.
    wait_for("three slow checks", || checks(&fake).len() >= 3).await;
    let times = checks(&fake);
    assert!(
        due(times[0], applied, every),
        "the first check {:?} after the application",
        times[0] - applied
    );
    for pair in times.windows(2) {
        assert!(
            due(pair[1], pair[0], every),
            "{:?} between checks",
            pair[1] - pair[0]
        );
    }
    let w = wifi(&server);
    assert_eq!(w.phase, Some(WifiPhase::Accepting), "still waiting for it");
    assert_eq!(w.pairing_code, Some(73), "the code is still shown");
    assert_eq!(fake.lock().unwrap().applications(), 1, "the same request");
}

#[tokio::test]
async fn accepted_on_the_grinder_without_a_confirmation_pairs_at_a_slow_check() {
    let every = Duration::from_millis(400);
    let fake = Fake {
        slow_accept: 1_000,
        ..Fake::default()
    };
    let (server, fake) = start_timed(fake, ACCEPT_SETTLE_IN_TESTS, every).await;
    wait_for("a slow check", || answered(&fake) == 1).await;

    // Accepted on the grinder, but nobody taps in the app: the next slow
    // check finds it.
    fake.lock().unwrap().slow_accept = 0;
    wait_for("paired", || paired(&server)).await;
    let times = checks(&fake);
    assert_eq!(times.len(), 2, "no check in between");
    assert!(
        due(times[1], times[0], every),
        "{:?} between checks",
        times[1] - times[0]
    );
}

#[tokio::test]
async fn a_confirmation_long_after_the_code_checks_after_the_settle() {
    // Slow checks more often than the settle lasts, so a check during the
    // hold would show.
    let every = Duration::from_millis(600);
    let settle = Duration::from_millis(900);
    let fake = Fake {
        slow_accept: 1_000,
        ..Fake::default()
    };
    let (server, fake) = start_timed(fake, settle, every).await;
    wait_for("two slow checks", || checks(&fake).len() >= 2).await;

    // Confirmed well after the code: held for the whole settle all the same.
    let tap = Instant::now();
    server.grinder_accepted().unwrap();
    wait_for("the hold shown", || {
        wifi(&server).phase == Some(WifiPhase::Holding)
    })
    .await;
    assert_eq!(
        server.grinder_accepted(),
        Err(PairingIdRefusal::NotAccepting),
        "taken once"
    );
    fake.lock().unwrap().slow_accept = 0;
    wait_for("paired", || paired(&server)).await;

    let after_tap: Vec<Instant> = checks(&fake).into_iter().filter(|at| *at > tap).collect();
    assert_eq!(after_tap.len(), 1, "nothing sent during the hold");
    assert!(
        due(after_tap[0], tap, settle),
        "checked {:?} after the tap",
        after_tap[0] - tap
    );
}

#[tokio::test]
async fn a_confirmation_before_the_grinder_accepted_goes_back_to_accepting() {
    let every = Duration::from_millis(500);
    let settle = Duration::from_millis(100);
    let fake = Fake {
        slow_accept: 1_000,
        ..Fake::default()
    };
    let (server, fake) = start_timed(fake, settle, every).await;
    wait_for("awaiting the accept", || accepting(&server)).await;

    // Tapped before accepting on the grinder: the check finds no accept.
    let tap = Instant::now();
    server.grinder_accepted().unwrap();
    wait_for("the confirmed check", || checks(&fake).len() == 1).await;
    wait_for("accepting again", || accepting(&server)).await;
    let first = checks(&fake)[0];
    assert!(
        due(first, tap, settle),
        "checked {:?} after the tap",
        first - tap
    );
    assert_eq!(
        wifi(&server).pairing_code,
        Some(73),
        "the code is shown again"
    );

    // Slow checks again from then on.
    wait_for("the slow check", || answered(&fake) == 2).await;
    let slow = checks(&fake)[1];
    assert!(due(slow, first, every), "{:?} later", slow - first);
    assert_eq!(wifi(&server).phase, Some(WifiPhase::Accepting));

    // Accepted on the grinder now, and confirmed again: that tap is taken.
    fake.lock().unwrap().slow_accept = 0;
    let tap = Instant::now();
    server.grinder_accepted().unwrap();
    wait_for("paired", || paired(&server)).await;
    let times = checks(&fake);
    assert_eq!(times.len(), 3, "no other check");
    assert!(
        due(times[2], tap, settle),
        "checked {:?} after the second tap",
        times[2] - tap
    );
}

#[tokio::test]
async fn while_unpaired_a_failing_grinder_is_retried_soon() {
    // The backoff caps: 20 ms doubling to 800 ms, and 50 ms while unpaired.
    let timed = |base: &str, pairing_id: Option<u32>| {
        let mut cfg = test_config(base);
        cfg.min_backoff = Duration::from_millis(20);
        cfg.max_backoff = Duration::from_millis(800);
        cfg.unpaired_backoff = Duration::from_millis(50);
        cfg.pairing_id = pairing_id;
        cfg
    };
    let longest_gap = |times: &[Instant]| {
        times
            .windows(2)
            .map(|w| w[1] - w[0])
            .max()
            .unwrap_or_default()
    };

    // Unpaired: the applications fail eight times in a row.
    let (base, fake) = fake_grinder(Fake {
        fail_applications: 8,
        ..Fake::default()
    })
    .await;
    let server = Server::new(MachineConfig::default());
    let link = WifiGrinder::new(timed(&base, None), server.pairing_lock.clone()).unwrap();
    run(&server, link);
    wait_for("the code after the failures", || accepting(&server)).await;
    let applied = fake
        .lock()
        .unwrap()
        .requests_to(wifi::PATH_PAIRING_APPLICATION);
    assert_eq!(applied.len(), 9);
    let unpaired = longest_gap(&applied);
    assert!(
        unpaired < Duration::from_millis(300),
        "capped: {unpaired:?}"
    );

    // Paired: the job requests fail six times in a row, and the waits grow
    // as before (20, 40, 80, 160, 320, 640 ms).
    let (base, fake) = fake_grinder(Fake {
        paired: Some(PAIRING_ID),
        fail_jobs: 6,
        ..Fake::default()
    })
    .await;
    let server = Server::new(MachineConfig::default());
    let link =
        WifiGrinder::new(timed(&base, Some(PAIRING_ID)), server.pairing_lock.clone()).unwrap();
    run(&server, link);
    wait_for("paired after the failures", || paired(&server)).await;
    let polled = fake.lock().unwrap().requests_to(wifi::PATH_BREW_JOBS);
    let paired_gap = longest_gap(&polled);
    assert!(
        paired_gap >= Duration::from_millis(600),
        "not capped: {paired_gap:?}"
    );
}

#[tokio::test]
async fn asking_again_keeps_the_question_and_an_id_typed_meanwhile() {
    // Paired on the grinder, the pairing ID missing here; asked for 100 ms
    // at a time, so the link applies again often.
    let (base, fake) = fake_grinder(Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    })
    .await;
    let server = Server::new(MachineConfig::default());
    let mut cfg = test_config(&base);
    cfg.ask_window = Duration::from_millis(100);
    run(
        &server,
        WifiGrinder::new(cfg, server.pairing_lock.clone()).unwrap(),
    );
    let asks = || {
        server
            .grinder_link()
            .wifi
            .is_some_and(|w| w.phase == Some(WifiPhase::Asking))
    };
    wait_for("the question", asks).await;
    let first = Instant::now();
    let applications = || fake.lock().unwrap().applications();
    wait_for("asked twice more", || applications() >= 3 && asks()).await;
    // One question all along: its age counts from the first ask.
    let age = crate::server::state_json(&server)["grinder"]["wifi"]["phase_age_ms"]
        .as_u64()
        .unwrap();
    let since_first = u64::try_from(first.elapsed().as_millis()).unwrap();
    assert!(age >= since_first, "{age} ms, asked {since_first} ms ago");
    assert!(age >= 200, "two windows: {age} ms");

    // An ID typed while the link applies again (the question still shows)
    // is taken, not dropped as stale.
    fake.lock().unwrap().application_delay = Duration::from_millis(300);
    let before = applications();
    wait_for("applying again", || applications() > before).await;
    assert!(asks(), "the question still shows");
    server.use_grinder_pairing_id(PAIRING_ID).unwrap();
    wait_for("paired", || paired(&server)).await;
    assert_eq!(wifi(&server).pairing_id, Some(PAIRING_ID));
}

#[tokio::test]
async fn a_confirmation_is_refused_when_the_link_is_not_awaiting() {
    // No link at all.
    let no_link = Server::new(MachineConfig::default());
    assert_eq!(no_link.grinder_accepted(), Err(PairingIdRefusal::NoLink));
    assert_eq!(PairingIdRefusal::NoLink.code(), "grinder_not_set_up");

    // A link waiting for the accept: a second confirmation while the first is
    // still queued is refused as busy. (Nothing awaits between the two calls,
    // so the link cannot take the first yet.)
    let (server, _fake) = start_gated(accepted_on_the_grinder()).await;
    wait_for("awaiting the accept", || accepting(&server)).await;
    assert_eq!(server.grinder_accepted(), Ok(()));
    assert_eq!(
        server.grinder_accepted(),
        Err(PairingIdRefusal::Busy),
        "the confirmation is already queued"
    );
    assert_eq!(PairingIdRefusal::Busy.code(), "grinder_pairing_busy");

    // Once paired, there is nothing to confirm.
    wait_for("paired", || paired(&server)).await;
    assert_eq!(
        server.grinder_accepted(),
        Err(PairingIdRefusal::NotAccepting),
        "not waiting for an accept any more"
    );
    assert_eq!(
        PairingIdRefusal::NotAccepting.code(),
        "grinder_not_accepting"
    );
}

#[tokio::test]
async fn an_accept_for_a_replaced_link_does_not_reach_it() {
    let (server, fake) = start_gated(Fake::default()).await;
    wait_for("awaiting the accept", || accepting(&server)).await;

    // What replacing the link does: the owner changes and the channel is
    // dropped. The old task runs on (as an aborted one does until its next
    // await), but what it reports is ignored.
    server.detach_wifi();
    assert_eq!(
        server.grinder_accepted(),
        Err(PairingIdRefusal::NoLink),
        "a tap from a stale app view does not reach the replaced link"
    );
    // Had it been released, it would check after the short settle. Without
    // its sender it waits for the slow check, which the test never reaches.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        fake.lock()
            .unwrap()
            .requests_to(wifi::PATH_PAIRING_STATUS)
            .is_empty(),
        "the replaced link was not released and sent no check"
    );
    assert!(
        server.grinder_link().wifi.is_none(),
        "the replaced link is gone from the state"
    );
}

#[tokio::test]
async fn a_lost_pairing_re_runs_the_accept_gate() {
    // The grinder restarts on the accept and loses the pairing. gbs-anywhere
    // asks again, and the accept gate runs anew.
    let fake = Fake {
        lose_on_restart: 1,
        ..accepted_on_the_grinder()
    };
    let (server, fake) = start_gated(fake).await;

    // First attempt: the gate waits for the confirmation.
    wait_for("awaiting the accept", || accepting(&server)).await;
    assert!(!wifi(&server).pairing_lost, "nothing lost yet");
    server.grinder_accepted().unwrap();

    // The pairing is lost in the restart; the app says so and the gate runs
    // again, waiting for a fresh confirmation.
    wait_for("the gate again, as lost", || {
        let w = wifi(&server);
        w.state == WifiState::Pairing && w.pairing_lost && w.phase == Some(WifiPhase::Accepting)
    })
    .await;
    assert_eq!(fake.lock().unwrap().applications(), 2, "asked again");

    // Accepted on the grinder and confirmed once more; this time it sticks.
    {
        let mut f = fake.lock().unwrap();
        f.slow_accept = 0;
        f.status_checks = 1;
    }
    server.grinder_accepted().unwrap();
    wait_for("paired", || paired(&server)).await;
    assert!(!wifi(&server).pairing_lost, "this one stuck");
    assert_eq!(wifi(&server).phase, None, "paired, so no phase");
    let state = crate::server::state_json(&server);
    assert_eq!(state["grinder"]["wifi"]["phase_age_ms"], Value::Null);
}

/// Changes only the grinder's address.
fn to(host: HostChange) -> SettingsChange {
    SettingsChange {
        grinder_host: Some(host),
        ..SettingsChange::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changing_the_address_while_pairing_hands_over_at_once() {
    let file = temp_file("relink-pairing");
    // A person who never accepts: the link keeps waiting.
    let fake = Fake {
        slow_accept: u32::MAX / 2,
        ..Fake::default()
    };
    let (base, fake) = fake_grinder(fake).await;
    let server = Server::new(MachineConfig::default());
    let startup = Startup {
        grinder_host: Some(base),
        pairing_file: Some(file.clone()),
        ..Startup::default()
    };
    server.start_settings(startup, None).unwrap();
    wait_for("pairing", || wifi(&server).state == WifiState::Pairing).await;
    let device = saved(&file)["brew_device_id"].clone();

    server
        .change_settings(to(HostChange::Set("127.0.0.1:9".into())))
        .unwrap();
    // Right away, before the new link's task ran: the app sees the new
    // address, and a pairing ID typed now goes to the new link rather than
    // to none.
    let w = wifi(&server);
    assert_eq!(w.grinder, "https://127.0.0.1:9");
    assert_ne!(w.phase, Some(WifiPhase::Asking));
    assert_eq!(
        server.use_grinder_pairing_id(PAIRING_ID),
        Err(PairingIdRefusal::NotAsking),
        "the new link is attached"
    );
    wait_for("the new link's own report", || {
        wifi(&server).state == WifiState::Failing
    })
    .await;
    let sent = fake.lock().unwrap().requests.len();

    // Back to the fake: still the same gbs-anywhere to it, asking anew.
    server.change_settings(to(HostChange::Reset)).unwrap();
    wait_for("pairing again", || {
        wifi(&server).state == WifiState::Pairing
    })
    .await;
    assert!(fake.lock().unwrap().requests.len() > sent);
    assert_eq!(saved(&file)["brew_device_id"], device, "the pairing stayed");
    assert_eq!(saved_id(&file), Value::Null);
    let _ = std::fs::remove_file(file);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changing_the_address_keeps_the_saved_pairing() {
    let file = temp_file("relink-paired");
    let paired_file = json!({ "brew_device_id": "dev-1", "pairing_id": PAIRING_ID });
    std::fs::write(&file, paired_file.to_string()).unwrap();
    let fake = Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    };
    let (base, fake) = fake_grinder(fake).await;
    let server = Server::new(MachineConfig::default());
    let startup = Startup {
        grinder_host: Some(base),
        pairing_file: Some(file.clone()),
        ..Startup::default()
    };
    server.start_settings(startup, None).unwrap();
    wait_for("paired", || paired(&server)).await;

    // Back and forth, faster than the links get going.
    for _ in 0..5 {
        server
            .change_settings(to(HostChange::Set("127.0.0.1:9".into())))
            .unwrap();
        assert_eq!(wifi(&server).pairing_id, Some(PAIRING_ID));
        server.change_settings(to(HostChange::Reset)).unwrap();
    }
    wait_for("paired again", || paired(&server)).await;
    assert_eq!(fake.lock().unwrap().applications(), 0, "no new pairing");
    assert_eq!(saved(&file), paired_file);
    let _ = std::fs::remove_file(file);
}

#[tokio::test]
async fn a_replaced_link_starts_no_brew() {
    let (server, fake) = start(Fake::default(), None, None).await;
    wait_for("paired", || paired(&server)).await;
    // What stopping it does. Its task goes on here, as an aborted one does
    // until its next await.
    server.detach_wifi();
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));
    let polls = || fake.lock().unwrap().requests_to(wifi::PATH_BREW_JOBS).len();
    let seen = polls();
    wait_for("a few more polls", || polls() >= seen + 5).await;
    assert!(!brewing(&server), "the machine is left alone");
    assert_eq!(taken(&fake), 0, "the job is not taken");
}

/// A fake grinder and the link to it, retrying every 20 ms while unpaired.
async fn start_retrying(fake: Fake) -> (Arc<Server>, Shared) {
    let (base, fake) = fake_grinder(fake).await;
    let server = Server::new(MachineConfig::default());
    let mut cfg = test_config(&base);
    cfg.min_backoff = Duration::from_millis(20);
    cfg.max_backoff = Duration::from_millis(20);
    cfg.unpaired_backoff = Duration::from_millis(20);
    run(
        &server,
        WifiGrinder::new(cfg, server.pairing_lock.clone()).unwrap(),
    );
    (server, fake)
}

#[tokio::test]
async fn a_grinder_that_turns_the_application_down_is_not_set_up() {
    // It answers, but not for pairing: the app offers the setup, until the
    // grinder takes an application.
    let fake = Fake {
        turn_down_applications: 10,
        ..Fake::default()
    };
    let (server, _fake) = start_retrying(fake).await;
    wait_for("turned down", || {
        server
            .grinder_link()
            .wifi
            .is_some_and(|w| w.state == WifiState::Failing && w.not_set_up && !w.unreachable)
    })
    .await;
    wait_for("the code", || accepting(&server)).await;
    assert!(!wifi(&server).not_set_up);
}

#[tokio::test]
async fn another_error_while_unpaired_is_no_missing_setup() {
    // An error that is not the grinder turning the application down (here
    // HTTP 503) stays an error, with its detail.
    let fake = Fake {
        fail_applications: 10,
        ..Fake::default()
    };
    let (server, _fake) = start_retrying(fake).await;
    // Checked in the state the wait sees: a later one may already be
    // pairing, where both are false anyway.
    wait_for("the error", || {
        server
            .grinder_link()
            .wifi
            .is_some_and(|w| w.state == WifiState::Failing && !w.not_set_up && !w.unreachable)
    })
    .await;
}

#[tokio::test]
async fn the_state_age_counts_from_the_state_not_the_last_report() {
    // Failing again and again keeps one state age; the code then starts
    // its own.
    let fake = Fake {
        turn_down_applications: 1_000,
        ..Fake::default()
    };
    let (server, fake) = start_retrying(fake).await;
    let age = || {
        crate::server::state_json(&server)["grinder"]["wifi"]["state_age_ms"]
            .as_u64()
            .unwrap()
    };
    let applications = || fake.lock().unwrap().applications();
    wait_for("failing", || {
        server
            .grinder_link()
            .wifi
            .is_some_and(|w| w.state == WifiState::Failing)
    })
    .await;
    let first = age();
    let read = Instant::now();
    let before = applications();
    // Five more reports, at least 80 ms: an age started afresh by each one
    // would be about 20 ms.
    wait_for("turned down five times more", || {
        applications() >= before + 5
    })
    .await;
    let waited = read.elapsed().as_millis() as u64;
    assert!(
        age() + 1 >= first + waited,
        "{} ms, {waited} ms after {first} ms",
        age()
    );
    // The new state can only start once the grinder takes an application,
    // so its age can't exceed the time since then; an age that went on would
    // be at least `first + waited` more.
    let cleared = Instant::now();
    fake.lock().unwrap().turn_down_applications = 0;
    wait_for("the code", || accepting(&server)).await;
    let new_age = age();
    assert!(
        new_age <= cleared.elapsed().as_millis() as u64,
        "{new_age} ms in a new state"
    );
}

#[tokio::test]
async fn not_set_up_ends_with_another_error() {
    // Turned down, then failing another way: no longer "not set up".
    let fake = Fake {
        turn_down_applications: 1_000,
        ..Fake::default()
    };
    let (server, fake) = start_retrying(fake).await;
    wait_for("turned down", || {
        server.grinder_link().wifi.is_some_and(|w| w.not_set_up)
    })
    .await;
    fake.lock().unwrap().fail_applications = 1_000;
    wait_for("another error", || {
        server
            .grinder_link()
            .wifi
            .is_some_and(|w| w.state == WifiState::Failing && !w.not_set_up && !w.unreachable)
    })
    .await;
}
