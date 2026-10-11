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
    /// Answer every `brewAccept` with "no such job", as for a job the
    /// grinder dropped a moment ago.
    lose_jobs: bool,
    /// `brewAccept` requests to fail with HTTP 503, as if the network dropped.
    fail_accept: u32,
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

fn taken(fake: &Shared) -> usize {
    fake.lock().unwrap().calls_to(wifi::PATH_BREW_ACCEPT).len()
}

#[tokio::test]
async fn pairs_takes_the_job_at_once_then_sends_the_shot() {
    let file = temp_file("pairing");
    let (server, fake) = start(Fake::default(), Some(file.clone()), None).await;

    wait_for("pairing", || paired(&server)).await;
    assert_eq!(saved_id(&file), PAIRING_ID, "kept for the next start");
    assert!(
        server.grinder_link().last_poll.is_some(),
        "shown as connected"
    );

    // A Grind-by-Sync grind: the grinder now waits for this shot, and the job
    // is taken right away so the grinder keeps it open.
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));
    wait_for("the brew", || brewing(&server)).await;
    let grind = server.with(|m, _| m.last_grind().cloned()).unwrap();
    assert_eq!(grind.beverage_weight_g, Some(40.0), "the recipe weight");
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
        brew_timeout: Some(Duration::from_millis(300)),
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
        Err(MachineError::NotBrewing),
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
async fn a_newer_grind_cancels_one_still_waiting() {
    let (server, fake) = start(Fake::default(), None, None).await;
    wait_for("pairing", || paired(&server)).await;
    fake.lock().unwrap().jobs.push(job("first", 36_000));
    wait_for("the first job taken", || taken(&fake) == 1).await;

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
