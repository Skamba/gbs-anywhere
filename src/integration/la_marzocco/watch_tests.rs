use super::*;
use crate::{
    integration::{Settings, Status},
    protocol::{MachineConfig, Phase},
    server::{Server, wait_for},
};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Fake {
    replies: Mutex<VecDeque<Option<Value>>>,
    reads: AtomicUsize,
    hold: bool,
    release: tokio::sync::Notify,
}

async fn coffee(State(fake): State<Arc<Fake>>) -> Response {
    let count = fake.reads.fetch_add(1, Ordering::SeqCst);
    let reply = fake
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or(Some(json!({"output":{"lastCoffees":[]}})));
    if fake.hold && count == 0 {
        fake.release.notified().await;
    }
    match reply {
        Some(value) => Json(value).into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

fn cup(time: i64) -> Option<Value> {
    Some(
        json!({"output":{"lastCoffees":[{"time":time,"extractionSeconds":28.0,"doseValue":30.2,"valid":true}]}}),
    )
}

struct Tasks(Vec<tokio::task::JoinHandle<()>>);
impl Drop for Tasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

async fn start(
    replies: Vec<Option<Value>>,
    hold: bool,
    recipe_follow: bool,
) -> (Link, Arc<Fake>, Tasks) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fake = Arc::new(Fake {
        replies: Mutex::new(replies.into()),
        reads: AtomicUsize::new(0),
        hold,
        release: tokio::sync::Notify::new(),
    });
    let app = Router::new()
        .route(
            "/auth/signin",
            post(|| async { Json(json!({"accessToken":"synthetic", "refreshToken":"synthetic"})) }),
        )
        .route("/things/SYNTHETIC/stats/LAST_COFFEE/1", get(coffee))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let http = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut cfg = Config::from_settings(
        &Settings::new()
            .with("username", Some("synthetic@example.org"))
            .with("password", Some("synthetic"))
            .with("base_url", Some(&base)),
    )
    .unwrap();
    cfg.poll = Duration::from_millis(20);
    cfg.recipe_follow = recipe_follow;
    let client = CloudClient::new(&cfg).unwrap();
    let link = Link {
        server: Server::new(MachineConfig {
            finishing_hold: Duration::ZERO,
            ..MachineConfig::default()
        }),
        status: Status::new("la_marzocco", "la_marzocco", TITLE),
    };
    let task_link = link.clone();
    let task = tokio::spawn(async move {
        watch(
            &task_link,
            &cfg,
            client,
            Machine {
                serial: "SYNTHETIC".into(),
                subject: "Synthetic".into(),
                last: None,
            },
        )
        .await
        .unwrap();
    });
    tokio::task::yield_now().await;
    (link, fake, Tasks(vec![http, task]))
}

fn begin(link: &Link) {
    link.server.with(|m, now| m.on_start_request(now, Some(9)));
}

#[tokio::test]
async fn failed_baseline_skips_the_grind_and_the_next_grind_can_report() {
    let (link, fake, _tasks) = start(vec![None, cup(100), cup(101)], false, false).await;
    begin(&link);
    wait_for("failed baseline", || {
        link.status.snapshot().detail_key == Some("la_marzocco.baseline_failed")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        fake.reads.load(Ordering::SeqCst),
        1,
        "no later old coffee can become this shot"
    );
    assert_eq!(link.status.snapshot().reports, 0);
    link.server.with(|m, now| m.abort(now)).unwrap();
    begin(&link);
    wait_for("fresh cloud-timed result", || {
        link.status.snapshot().reports == 1
    })
    .await;
    assert_eq!(link.status.snapshot().last_report.unwrap().weight_g, 30.2);
}

#[tokio::test]
async fn replacing_a_brew_during_its_baseline_does_not_report_that_response() {
    let (link, fake, _tasks) = start(vec![cup(100), cup(101)], true, false).await;
    begin(&link);
    wait_for("held first read", || fake.reads.load(Ordering::SeqCst) == 1).await;
    link.server.with(|m, now| m.abort(now)).unwrap();
    begin(&link);
    fake.release.notify_one();
    wait_for("new brew has its own baseline", || {
        fake.reads.load(Ordering::SeqCst) >= 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(link.status.snapshot().reports, 0);
    assert!(
        link.server
            .with(|m, now| matches!(m.phase(now), Phase::Running { .. }))
    );
}

#[tokio::test]
async fn cloud_clock_is_used_even_when_far_behind_the_computers_clock() {
    let (link, _, _tasks) = start(vec![cup(10), cup(11)], false, false).await;
    begin(&link);
    wait_for("cloud-time result", || link.status.snapshot().reports == 1).await;
}

#[tokio::test]
async fn recipe_follow_waits_before_the_baseline_read_finishes() {
    let (link, fake, _tasks) = start(vec![cup(100)], true, true).await;
    begin(&link);
    wait_for("held baseline", || fake.reads.load(Ordering::SeqCst) == 1).await;
    assert_eq!(
        link.status.snapshot().detail_key,
        Some("la_marzocco.recipe_reading")
    );
    assert_eq!(link.status.snapshot().reports, 0);
}
