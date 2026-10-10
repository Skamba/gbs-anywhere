//! The link against a local fake of the grinder's Generic WiFi API (plain
//! HTTP), with the real machine and shot rules behind it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use super::*;
use crate::protocol::{MachineConfig, MachineError, Phase, ShotResult};

/// The pairing ID the fake hands out.
const PAIRING_ID: u32 = 42;

/// What the fake grinder knows and what it was sent.
#[derive(Default)]
struct Fake {
    applications: u32,
    /// Accepts the pairing on the second status check, like a person who
    /// needs a moment.
    status_checks: u32,
    /// The pairing it knows (none until accepted).
    paired: Option<u32>,
    /// Open jobs; accepting one moves it to `ongoing`.
    jobs: Vec<Value>,
    ongoing: Vec<String>,
    /// `brewDone` requests to fail with HTTP 503, as if the network dropped.
    fail_done: u32,
    /// Hand the job back out on the next `brewDone`, as after its timeout.
    hand_back: bool,
    /// Job requests to refuse with 403, as while starting up.
    refuse_jobs: u32,
    /// `(path, body)` of every `brewAccept` and `brewDone`.
    calls: Vec<(String, Value)>,
}

impl Fake {
    fn calls_to(&self, path: &str) -> Vec<Value> {
        self.calls
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, b)| b.clone())
            .collect()
    }
}

type Shared = Arc<Mutex<Fake>>;

async fn handle(State(fake): State<Shared>, uri: Uri, Json(body): Json<Value>) -> Response {
    let mut f = fake.lock().unwrap();
    let path = uri.path();
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
            f.applications += 1;
            if f.paired.is_some() {
                return refuse(wifi::STATUS_ALREADY_PAIRED);
            }
            ok(json!({ "applicationId": 5, "pairingCode": 73 }))
        }
        wifi::PATH_PAIRING_STATUS => {
            f.status_checks += 1;
            if f.status_checks < 2 {
                return ok(json!({ "accepted": false }));
            }
            f.paired = Some(PAIRING_ID);
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
            let uuid = body["grindUuid"].as_str().unwrap_or_default().to_owned();
            let Some(i) = f.jobs.iter().position(|j| j["grindUuid"] == uuid) else {
                return refuse(wifi::STATUS_JOB_UNKNOWN);
            };
            f.jobs.remove(i);
            f.ongoing.push(uuid);
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
    let fake = Arc::new(Mutex::new(fake));
    let app = axum::Router::new()
        .fallback(axum::routing::post(handle))
        .with_state(fake.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let server = Server::new(MachineConfig {
        finishing_hold: Duration::from_millis(100),
        ..MachineConfig::default()
    });
    let mut cfg = WifiConfig::from_host(&format!("http://{addr}")).unwrap();
    cfg.poll = Duration::from_millis(20);
    cfg.pairing_file = pairing_file;
    cfg.pairing_id = pairing_id;
    let link = WifiGrinder::new(cfg).unwrap();
    tokio::spawn(link.run(server.clone()));
    (server, fake)
}

async fn wait_for(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..250 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
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
            Phase::Brewing {
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
}

#[tokio::test]
async fn pairs_then_sends_the_shot_once_it_is_measured() {
    let file = temp_file("pairing");
    let (server, fake) = start(Fake::default(), Some(file.clone()), None).await;

    wait_for("pairing", || paired(&server)).await;
    assert_eq!(saved_id(&file), PAIRING_ID, "kept for the next start");
    assert!(
        server.grinder_link().last_poll.is_some(),
        "shown as connected"
    );

    // A Grind-by-Sync grind: the grinder now waits for this shot.
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));
    wait_for("the brew", || brewing(&server)).await;
    let grind = server.with(|m, _| m.last_grind().cloned()).unwrap();
    assert_eq!(grind.beverage_weight_g, Some(40.0), "the recipe weight");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        fake.lock().unwrap().calls.is_empty(),
        "not accepted before the shot is measured"
    );

    // An integration or the app reports it.
    report(&server, 27.7, 40.6);
    wait_for("the answer", || {
        wifi(&server).detail == "sent 27.7 s, 40.6 g"
    })
    .await;
    let f = fake.lock().unwrap();
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
async fn an_aborted_shot_is_cancelled_and_a_dropped_job_ends_the_brew() {
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
    assert_eq!(
        fake.lock().unwrap().calls_to(wifi::PATH_BREW_DONE)[0]["result"],
        "Canceled"
    );

    // Once the aborted shot stops showing: a job the grinder drops (it timed
    // out) before any shot came.
    wait_for("ready", || {
        server.with(|m, now| matches!(m.phase(now), Phase::Ready))
    })
    .await;
    fake.lock().unwrap().jobs.push(job("g-2", 40_000));
    wait_for("the second brew", || brewing(&server)).await;
    fake.lock().unwrap().jobs.clear();
    wait_for("giving up", || !brewing(&server)).await;
    let late = ShotResult::from_grams(Duration::from_secs(30), 40.0);
    assert_eq!(
        server.with(|m, now| m.report_shot(now, late)),
        Err(MachineError::NotBrewing),
        "a late shot has nothing to answer"
    );
    assert_eq!(
        fake.lock().unwrap().calls_to(wifi::PATH_BREW_ACCEPT).len(),
        1,
        "only the aborted shot was answered"
    );
    assert!(wifi(&server).problem.is_some(), "the app says so");
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
    assert_eq!(grind.beverage_weight_g, Some(40.0));

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
async fn a_newer_grind_replaces_one_still_waiting() {
    let (server, fake) = start(Fake::default(), None, None).await;
    wait_for("pairing", || paired(&server)).await;
    fake.lock().unwrap().jobs.push(job("first", 36_000));
    wait_for("the first brew", || brewing(&server)).await;

    // Ground again before pulling the shot.
    fake.lock().unwrap().jobs.push(job("second", 40_000));
    wait_for("the second brew", || {
        brewing(&server)
            && server.with(|m, _| m.last_grind().and_then(|g| g.beverage_weight_g)) == Some(40.0)
    })
    .await;
    report(&server, 28.0, 40.0);
    wait_for("the answer", || {
        wifi(&server).detail == "sent 28.0 s, 40.0 g"
    })
    .await;
    let accepted = fake.lock().unwrap().calls_to(wifi::PATH_BREW_ACCEPT);
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0]["grindUuid"], "second");
}

#[tokio::test]
async fn one_refusal_keeps_the_pairing_three_pair_again() {
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
    assert_eq!(fake.lock().unwrap().applications, 0);

    // Cleared on the grinder: it refuses from now on, then pairs anew.
    fake.lock().unwrap().paired = None;
    wait_for("the new pairing", || fake.lock().unwrap().paired.is_some()).await;
    wait_for("paired again", || paired(&server)).await;
    assert_eq!(fake.lock().unwrap().applications, 1);
    let saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(saved["brew_device_id"], "dev-1", "same device as before");
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
    assert_eq!(
        fake.lock().unwrap().applications,
        0,
        "no new pairing needed"
    );
    let _ = std::fs::remove_file(file);
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

    // A wrong one is checked with the grinder, not kept.
    server.use_grinder_pairing_id(PAIRING_ID + 1).unwrap();
    wait_for("the refusal", || {
        wifi(&server).detail.contains("does not know pairing ID 43")
    })
    .await;
    assert!(wifi(&server).needs_pairing_id, "still asking");
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
