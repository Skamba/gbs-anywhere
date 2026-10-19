//! The link against a local fake of the grinder's Generic WiFi API (plain
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
use crate::server::wait_for;

/// The pairing ID the fake hands out.
const PAIRING_ID: u32 = 42;

/// [`WifiConfig::restart_quiet`] here.
const RESTART_QUIET_IN_TESTS: Duration = Duration::from_millis(500);

/// [`WifiConfig::refused_for`] here: 30 polls, so a few refusals stay well
/// under it.
const REFUSED_FOR_IN_TESTS: Duration = Duration::from_millis(600);

/// What the fake grinder knows and what it was sent.
#[derive(Default)]
struct Fake {
    /// Every request: path and when it came.
    requests: Vec<(String, Instant)>,
    /// Status checks to answer as for a request that expired.
    forget_requests: u32,
    /// Status checks to fail with HTTP 503, as if the network dropped.
    fail_checks: u32,
    /// Save the pairing on the accept and restart, as the E65W does: the
    /// accept check is not answered, and the request is gone afterwards.
    restart_on_accept: bool,
    /// When it restarted that way.
    restarted_at: Option<Instant>,
    /// Accepts the pairing on the second status check, like a person who
    /// needs a moment.
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
    let mut f = fake.lock().unwrap();
    let path = uri.path();
    f.requests.push((path.to_owned(), Instant::now()));
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
            if f.paired.is_some() {
                return refuse(wifi::STATUS_ALREADY_PAIRED);
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
    let fake = Arc::new(Mutex::new(fake));
    let app = axum::Router::new()
        .fallback(axum::routing::post(handle))
        .with_state(fake.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let server = Server::new(MachineConfig {
        finishing_hold: Duration::from_millis(100),
        ..machine
    });
    let mut cfg = WifiConfig::from_host(&format!("http://{addr}")).unwrap();
    cfg.poll = poll;
    cfg.restart_quiet = RESTART_QUIET_IN_TESTS;
    cfg.refused_for = REFUSED_FOR_IN_TESTS;
    cfg.pairing_file = pairing_file;
    cfg.pairing_id = pairing_id;
    let link = WifiGrinder::new(cfg).unwrap();
    tokio::spawn(link.run(server.clone()));
    (server, fake)
}

fn wifi(server: &Server) -> WifiLink {
    server
        .grinder_link()
        .wifi
        .expect("the link reports its state")
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
    let saved: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    saved["pairing_id"].clone()
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
    let mut cfg = WifiConfig::from_host(&format!("http://{addr}")).unwrap();
    cfg.poll = Duration::from_millis(20);
    tokio::spawn(WifiGrinder::new(cfg).unwrap().run(server.clone()));
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
            .is_some_and(|w| w.needs_pairing_id)
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
        link().is_some_and(|w| w.state == WifiState::Failing)
    })
    .await;
    wait_for("the code again", || {
        link().is_some_and(|w| w.pairing_code == Some(73))
    })
    .await;
    wait_for("pairing", || paired(&server)).await;
    assert_eq!(fake.lock().unwrap().applications(), 1, "no new request");
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
    wait_for("pairing", || paired(&server)).await;
    let f = fake.lock().unwrap();
    let applied = f.requests_to(wifi::PATH_PAIRING_APPLICATION);
    let checks = f.requests_to(wifi::PATH_PAIRING_STATUS);
    assert_eq!(applied.len(), 2, "asked again");
    assert!(applied[1] < checks[0] + RESTART_QUIET_IN_TESTS, "at once");
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
            .is_some_and(|w| w.needs_pairing_id)
    })
    .await;
    assert_eq!(wifi(&server).pairing_id, None, "none in use yet");

    // A wrong one is checked with the grinder, not kept.
    server.use_grinder_pairing_id(PAIRING_ID + 1).unwrap();
    wait_for("the refusal", || {
        wifi(&server).detail.contains("does not know pairing ID 43")
    })
    .await;
    assert!(wifi(&server).needs_pairing_id, "still asking");
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
    let asks = |s: &Server| s.grinder_link().wifi.is_some_and(|w| w.needs_pairing_id);
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
    assert!(!wifi(&server).needs_pairing_id, "nothing to type in");
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
            .is_some_and(|w| w.needs_pairing_id)
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
