use super::*;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const SERIAL: &str = "SYNTHETIC";

fn dashboard(mode: &str, first: f64, second: f64) -> Value {
    json!({
        "serialNumber": SERIAL, "connected": true, "name": "Synthetic machine",
        "widgets": [
            {"code":"CMMachineStatus", "output":{"status":"PoweredOn"}},
            {"code":"CMBrewByWeightDoses", "output":{
                "mode":mode, "scaleConnected":true,
                "doses":{"Dose1":{"dose":first,"other":"preserve"},"Dose2":{"dose":second}},
                "other":"also preserved"
            }},
            {"code":"CMPreBrewing", "output":{"mode":"Disabled","times":{"In":2}}}
        ]
    })
}

struct FakeCloud {
    dashboard: Mutex<Value>,
    reads: AtomicUsize,
    posts: AtomicUsize,
    bodies: Mutex<Vec<Value>>,
    status: AtomicU16,
    dose_status: AtomicU16,
    apply: AtomicBool,
    change_other_dose: AtomicBool,
    change_second_dose: AtomicBool,
    fail_read: AtomicUsize,
    post_delay_ms: AtomicU64,
    dose_delay_ms: AtomicU64,
    read_delay_ms: AtomicU64,
}

impl FakeCloud {
    fn new(value: Value) -> Self {
        Self {
            dashboard: Mutex::new(value),
            reads: AtomicUsize::new(0),
            posts: AtomicUsize::new(0),
            bodies: Mutex::new(Vec::new()),
            status: AtomicU16::new(200),
            dose_status: AtomicU16::new(200),
            apply: AtomicBool::new(true),
            change_other_dose: AtomicBool::new(false),
            change_second_dose: AtomicBool::new(false),
            fail_read: AtomicUsize::new(0),
            post_delay_ms: AtomicU64::new(0),
            dose_delay_ms: AtomicU64::new(0),
            read_delay_ms: AtomicU64::new(0),
        }
    }
}

async fn read(State(cloud): State<Arc<FakeCloud>>) -> Response {
    let number = cloud.reads.fetch_add(1, Ordering::SeqCst) + 1;
    tokio::time::sleep(Duration::from_millis(
        cloud.read_delay_ms.load(Ordering::SeqCst),
    ))
    .await;
    if cloud.fail_read.load(Ordering::SeqCst) == number {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "synthetic private response",
        )
            .into_response();
    }
    Json(cloud.dashboard.lock().unwrap().clone()).into_response()
}

async fn command(
    State(cloud): State<Arc<FakeCloud>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert!(headers.contains_key("authorization"));
    assert!(headers.contains_key("x-request-signature"));
    cloud.posts.fetch_add(1, Ordering::SeqCst);
    cloud.bodies.lock().unwrap().push(body.clone());
    let setting_doses = body.get("doses").is_some();
    if cloud.apply.load(Ordering::SeqCst) {
        let mut value = cloud.dashboard.lock().unwrap();
        if setting_doses {
            for dose in ["Dose1", "Dose2"] {
                value["widgets"][1]["output"]["doses"][dose]["dose"] = body["doses"][dose].clone();
            }
        } else {
            value["widgets"][1]["output"]["mode"] = body["mode"].clone();
            if cloud.change_other_dose.load(Ordering::SeqCst) {
                value["widgets"][1]["output"]["doses"]["Dose1"]["dose"] = json!(21);
            }
            if cloud.change_second_dose.load(Ordering::SeqCst) {
                value["widgets"][1]["output"]["doses"]["Dose2"]["dose"] = json!(31);
            }
        }
    }
    let delay = if setting_doses {
        &cloud.dose_delay_ms
    } else {
        &cloud.post_delay_ms
    };
    tokio::time::sleep(Duration::from_millis(delay.load(Ordering::SeqCst))).await;
    let status = if setting_doses {
        &cloud.dose_status
    } else {
        &cloud.status
    };
    let status = StatusCode::from_u16(status.load(Ordering::SeqCst)).unwrap();
    if status == StatusCode::TEMPORARY_REDIRECT {
        return (
            status,
            [("location", "/repeat")],
            "synthetic private response",
        )
            .into_response();
    }
    (
        status,
        Json(json!({"id":"synthetic-command","private":"do not log"})),
    )
        .into_response()
}

struct ServerTask(tokio::task::JoinHandle<()>);
impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn fake(value: Value) -> (CloudClient, Arc<FakeCloud>, ServerTask) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cloud = Arc::new(FakeCloud::new(value));
    let app = Router::new()
        .route("/things/SYNTHETIC/dashboard", get(read))
        .route(
            "/things/SYNTHETIC/command/CoffeeMachineBrewByWeightChangeMode",
            post(command),
        )
        .route(
            "/things/SYNTHETIC/command/CoffeeMachineBrewByWeightSettingDoses",
            post(command),
        )
        .route("/repeat", post(command))
        .with_state(cloud.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = CloudClient {
        http: reqwest::Client::new(),
        base_url,
        username: "synthetic@example.test".into(),
        password: "synthetic".into(),
        key: InstallationKey::derive("synthetic@example.test", "synthetic"),
        token: Some(Token {
            access: "synthetic".into(),
            refresh: "synthetic".into(),
            expires_at: Instant::now() + TOKEN_LIFETIME,
        }),
    };
    (client, cloud, ServerTask(task))
}

#[tokio::test]
async fn selecting_an_existing_preset_only_posts_its_mode_then_reads_back() {
    let original = dashboard("Continuous", 20.0, 30.0);
    let (mut client, cloud, _task) = fake(original.clone()).await;
    assert_eq!(
        client.select_bbw_target(SERIAL, 30.0, || true).await,
        BbwSelection::Confirmed
    );
    assert_eq!(cloud.reads.load(Ordering::SeqCst), 2);
    assert_eq!(cloud.posts.load(Ordering::SeqCst), 1);
    assert_eq!(*cloud.bodies.lock().unwrap(), vec![json!({"mode":"Dose2"})]);
    let mut expected = original;
    expected["widgets"][1]["output"]["mode"] = json!("Dose2");
    assert_eq!(*cloud.dashboard.lock().unwrap(), expected);

    let (mut client, cloud, _task) = fake(dashboard("Dose2", 37.4000015, 30.0)).await;
    assert_eq!(
        client.select_bbw_target(SERIAL, 37.4, || true).await,
        BbwSelection::Confirmed
    );
    assert_eq!(*cloud.bodies.lock().unwrap(), vec![json!({"mode":"Dose1"})]);
}

#[tokio::test]
async fn an_already_selected_matching_preset_needs_no_write() {
    for first in [20.0, 30.0] {
        let (mut client, cloud, _task) = fake(dashboard("Dose2", first, 30.0)).await;
        assert_eq!(
            client.select_bbw_target(SERIAL, 30.0, || true).await,
            BbwSelection::AlreadySet
        );
        assert_eq!(cloud.posts.load(Ordering::SeqCst), 0);
        assert_eq!(cloud.reads.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn missing_offline_or_ambiguous_state_never_writes() {
    let normal = dashboard("Continuous", 20.0, 30.0);
    let mut cases = vec![dashboard("Continuous", 30.0, 30.0)];
    let mut offline = normal.clone();
    offline["connected"] = json!(false);
    cases.push(offline);
    let mut standby = normal.clone();
    standby["widgets"][0]["output"]["status"] = json!("StandBy");
    cases.push(standby);
    let mut other = normal.clone();
    other["serialNumber"] = json!("OTHER");
    cases.push(other);
    let mut missing_mode = normal.clone();
    missing_mode["widgets"][1]["output"]
        .as_object_mut()
        .unwrap()
        .remove("mode");
    // Historical coffee must never fill a missing current mode.
    missing_mode["lastCoffees"] = json!([{"doseMode":"Dose2"}]);
    cases.push(missing_mode);
    let mut missing_dose = normal.clone();
    missing_dose["widgets"][1]["output"]["doses"]["Dose1"] = Value::Null;
    cases.push(missing_dose);
    let mut invalid_dose = normal.clone();
    invalid_dose["widgets"][1]["output"]["doses"]["Dose2"]["dose"] = json!(-30);
    cases.push(invalid_dose);
    let mut duplicate = normal.clone();
    duplicate["widgets"]
        .as_array_mut()
        .unwrap()
        .push(normal["widgets"][1].clone());
    cases.push(duplicate);
    for value in cases {
        let (mut client, cloud, _task) = fake(value).await;
        assert!(matches!(
            client.select_bbw_target(SERIAL, 30.0, || true).await,
            BbwSelection::NotApplied(_)
        ));
        assert_eq!(cloud.posts.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn brewing_and_disconnected_or_unknown_scales_never_write() {
    for variant in 0..3 {
        let mut value = dashboard("Continuous", 20.0, 30.0);
        match variant {
            0 => value["widgets"][0]["output"]["status"] = json!("Brewing"),
            1 => value["widgets"][1]["output"]["scaleConnected"] = json!(false),
            _ => {
                value["widgets"][1]["output"]
                    .as_object_mut()
                    .unwrap()
                    .remove("scaleConnected");
            }
        }
        let (mut client, cloud, _task) = fake(value).await;
        assert!(matches!(
            client.select_bbw_target(SERIAL, 30.0, || true).await,
            BbwSelection::NotApplied(_)
        ));
        assert_eq!(cloud.posts.load(Ordering::SeqCst), 0);
        assert_eq!(cloud.reads.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn invalid_input_and_stale_brews_never_start_a_request() {
    let (mut client, cloud, _task) = fake(dashboard("Continuous", 20.0, 30.0)).await;
    for grams in [0.0, -1.0, 30.25, 201.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            client.select_bbw_target(SERIAL, grams, || true).await,
            BbwSelection::NotApplied(_)
        ));
    }
    assert!(matches!(
        client.select_bbw_target("../OTHER", 30.0, || true).await,
        BbwSelection::NotApplied(_)
    ));
    assert!(matches!(
        client.select_bbw_target(SERIAL, 30.0, || false).await,
        BbwSelection::NotApplied(_)
    ));
    assert_eq!(cloud.reads.load(Ordering::SeqCst), 0);
    assert_eq!(cloud.posts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_brew_is_checked_after_preflight_and_immediately_before_post() {
    for allowed_checks in [1, 2] {
        let (mut client, cloud, _task) = fake(dashboard("Continuous", 20.0, 30.0)).await;
        let checks = AtomicUsize::new(0);
        let result = client
            .select_bbw_target(SERIAL, 30.0, || {
                checks.fetch_add(1, Ordering::SeqCst) < allowed_checks
            })
            .await;
        assert!(matches!(result, BbwSelection::NotApplied(_)));
        assert_eq!(cloud.reads.load(Ordering::SeqCst), 1);
        assert_eq!(cloud.posts.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn http_failures_and_redirects_never_replay_a_post() {
    for status in [400, 401, 403, 500, 307] {
        let (mut client, cloud, _task) = fake(dashboard("Continuous", 20.0, 30.0)).await;
        cloud.status.store(status, Ordering::SeqCst);
        assert_eq!(
            client.select_bbw_target(SERIAL, 30.0, || true).await,
            BbwSelection::Unknown
        );
        assert_eq!(cloud.posts.load(Ordering::SeqCst), 1, "HTTP {status}");
    }
}

#[tokio::test]
async fn confirmation_must_be_fresh_and_preserve_both_doses() {
    for variant in 0..4 {
        let (mut client, cloud, _task) = fake(dashboard("Continuous", 20.0, 30.0)).await;
        if variant == 0 {
            cloud.apply.store(false, Ordering::SeqCst);
        }
        if variant == 1 {
            cloud.change_other_dose.store(true, Ordering::SeqCst);
        }
        if variant == 2 {
            cloud.fail_read.store(2, Ordering::SeqCst);
        }
        let checks = AtomicUsize::new(0);
        let result = client
            .select_bbw_target(SERIAL, 30.0, || {
                variant != 3 || checks.fetch_add(1, Ordering::SeqCst) < 3
            })
            .await;
        assert_eq!(result, BbwSelection::Unknown);
        assert_eq!(cloud.posts.load(Ordering::SeqCst), 1);
        assert_eq!(cloud.reads.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn a_failed_preflight_never_sends_a_command() {
    let (mut client, cloud, _task) = fake(dashboard("Continuous", 20.0, 30.0)).await;
    cloud.fail_read.store(1, Ordering::SeqCst);
    assert!(matches!(
        client.select_bbw_target(SERIAL, 30.0, || true).await,
        BbwSelection::NotApplied(_)
    ));
    assert_eq!(cloud.posts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_post_timeout_is_unknown_and_never_retried() {
    let (mut client, cloud, _task) = fake(dashboard("Continuous", 20.0, 30.0)).await;
    cloud.post_delay_ms.store(10_000, Ordering::SeqCst);
    // Time already spent reading belongs to the same eight-second budget.
    cloud.read_delay_ms.store(1_000, Ordering::SeqCst);
    let started = Instant::now();
    assert_eq!(
        client.select_bbw_target(SERIAL, 30.0, || true).await,
        BbwSelection::Unknown
    );
    assert!(started.elapsed() < Duration::from_secs(9));
    assert_eq!(cloud.posts.load(Ordering::SeqCst), 1);
    assert_eq!(cloud.reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_preflight_timeout_does_not_write() {
    let (mut client, cloud, _task) = fake(dashboard("Continuous", 20.0, 30.0)).await;
    cloud.read_delay_ms.store(10_000, Ordering::SeqCst);
    let started = Instant::now();
    assert!(matches!(
        client.select_bbw_target(SERIAL, 30.0, || true).await,
        BbwSelection::NotApplied(_)
    ));
    assert!(started.elapsed() < Duration::from_secs(9));
    assert_eq!(cloud.posts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_unmatched_target_sets_dose1_and_preserves_fresh_dose2() {
    let original = dashboard("Continuous", 20.0, 30.7);
    let (mut client, cloud, _task) = fake(original.clone()).await;
    assert_eq!(
        client.select_bbw_target(SERIAL, 40.0, || true).await,
        BbwSelection::Confirmed
    );
    assert_eq!(cloud.reads.load(Ordering::SeqCst), 3);
    assert_eq!(
        *cloud.bodies.lock().unwrap(),
        vec![
            json!({"mode":"Dose1"}),
            json!({"doses":{"Dose1":40.0,"Dose2":30.7}}),
        ]
    );
    let mut expected = original;
    expected["widgets"][1]["output"]["mode"] = json!("Dose1");
    expected["widgets"][1]["output"]["doses"]["Dose1"]["dose"] = json!(40.0);
    assert_eq!(*cloud.dashboard.lock().unwrap(), expected);
}

#[tokio::test]
async fn an_unmatched_target_already_in_dose1_skips_the_mode_post() {
    let (mut client, cloud, _task) = fake(dashboard("Dose1", 20.0, 30.7)).await;
    assert_eq!(
        client.select_bbw_target(SERIAL, 40.0, || true).await,
        BbwSelection::Confirmed
    );
    assert_eq!(cloud.reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        *cloud.bodies.lock().unwrap(),
        vec![json!({"doses":{"Dose1":40.0,"Dose2":30.7}})]
    );
}

#[tokio::test]
async fn a_failed_second_command_is_unknown_and_each_command_is_sent_once() {
    for status in [400, 401, 500, 307] {
        let (mut client, cloud, _task) = fake(dashboard("Dose2", 20.0, 30.0)).await;
        cloud.dose_status.store(status, Ordering::SeqCst);
        assert_eq!(
            client.select_bbw_target(SERIAL, 40.0, || true).await,
            BbwSelection::Unknown
        );
        assert_eq!(cloud.reads.load(Ordering::SeqCst), 2);
        assert_eq!(
            *cloud.bodies.lock().unwrap(),
            vec![
                json!({"mode":"Dose1"}),
                json!({"doses":{"Dose1":40.0,"Dose2":30.0}}),
            ]
        );
        assert_eq!(cloud.posts.load(Ordering::SeqCst), 2, "HTTP {status}");
    }
}

#[tokio::test]
async fn a_manual_dose_change_after_mode_selection_prevents_the_weight_write() {
    for change_dose2 in [false, true] {
        let (mut client, cloud, _task) = fake(dashboard("Continuous", 20.0, 30.0)).await;
        if change_dose2 {
            cloud.change_second_dose.store(true, Ordering::SeqCst);
        } else {
            cloud.change_other_dose.store(true, Ordering::SeqCst);
        }
        assert_eq!(
            client.select_bbw_target(SERIAL, 40.0, || true).await,
            BbwSelection::Unknown
        );
        assert_eq!(cloud.reads.load(Ordering::SeqCst), 2);
        assert_eq!(*cloud.bodies.lock().unwrap(), vec![json!({"mode":"Dose1"})]);
    }
}

#[tokio::test]
async fn a_brew_change_between_commands_is_unknown_without_a_weight_write() {
    let (mut client, cloud, _task) = fake(dashboard("Continuous", 20.0, 30.0)).await;
    let checks = AtomicUsize::new(0);
    assert_eq!(
        client
            .select_bbw_target(SERIAL, 40.0, || checks.fetch_add(1, Ordering::SeqCst) < 4)
            .await,
        BbwSelection::Unknown
    );
    assert_eq!(cloud.reads.load(Ordering::SeqCst), 2);
    assert_eq!(*cloud.bodies.lock().unwrap(), vec![json!({"mode":"Dose1"})]);
}

#[tokio::test]
async fn the_new_target_needs_a_successful_final_readback() {
    let (mut client, cloud, _task) = fake(dashboard("Continuous", 20.0, 30.0)).await;
    cloud.fail_read.store(3, Ordering::SeqCst);
    assert_eq!(
        client.select_bbw_target(SERIAL, 40.0, || true).await,
        BbwSelection::Unknown
    );
    assert_eq!(cloud.reads.load(Ordering::SeqCst), 3);
    assert_eq!(cloud.posts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_second_post_timeout_is_unknown_and_never_replays_either_command() {
    let (mut client, cloud, _task) = fake(dashboard("Continuous", 20.0, 30.0)).await;
    cloud.dose_delay_ms.store(10_000, Ordering::SeqCst);
    let started = Instant::now();
    assert_eq!(
        client.select_bbw_target(SERIAL, 40.0, || true).await,
        BbwSelection::Unknown
    );
    assert!(started.elapsed() < Duration::from_secs(9));
    assert_eq!(cloud.posts.load(Ordering::SeqCst), 2);
    assert_eq!(cloud.reads.load(Ordering::SeqCst), 2);
}
