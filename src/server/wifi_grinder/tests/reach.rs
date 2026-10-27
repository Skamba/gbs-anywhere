//! A grinder that does not answer, or answers with errors: what the app is
//! shown, and how soon it is tried again.

use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde_json::json;
use tokio::net::TcpListener;

use super::{
    Fake, PAIRING_ID, Setup, accepting, age_ms, fake_grinder, in_state, paired, serve_fake,
    start_link, test_config, wifi,
};
use crate::protocol::MachineConfig;
use crate::server::wifi_grinder::WifiState;
use crate::server::wifi_grinder::reply::{Reply, unreachable};
use crate::server::{Server, wait_for};

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
    let refused = crate::http::client()
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
    start_link(&server, test_config(&format!("http://{addr}")));
    wait_for("the error", || in_state(&server, WifiState::Failing)).await;
    let w = wifi(&server);
    assert!(w.unreachable);
    // Why, without the steps before it: the address is in `detail`.
    let failure = w.failure.expect("why it fails");
    assert!(
        w.detail.starts_with("cannot reach the grinder at"),
        "{}",
        w.detail
    );
    assert!(w.detail.ends_with(&failure) && !failure.contains("cannot reach"));

    // Switched on: it answers on the next try and pairs.
    let listener = TcpListener::bind(addr)
        .await
        .expect("the port is free again");
    serve_fake(listener, Fake::default());
    wait_for("paired", || paired(&server)).await;
    assert!(!wifi(&server).unreachable);
    assert_eq!(wifi(&server).failure, None, "only while failing");
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
    start_link(&server, timed(&base, None));
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
    start_link(&server, timed(&base, Some(PAIRING_ID)));
    wait_for("paired after the failures", || paired(&server)).await;
    let polled = fake.lock().unwrap().requests_to(wifi::PATH_BREW_JOBS);
    let paired_gap = longest_gap(&polled);
    assert!(
        paired_gap >= Duration::from_millis(600),
        "not capped: {paired_gap:?}"
    );
}

#[tokio::test]
async fn a_grinder_that_turns_the_application_down_is_not_set_up() {
    // It answers, but not for pairing: the app offers the setup, until the
    // grinder takes an application.
    let fake = Fake {
        turn_down_applications: 10,
        ..Fake::default()
    };
    let (server, _fake) = Setup::new(fake).retrying().start().await;
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
    let (server, _fake) = Setup::new(fake).retrying().start().await;
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
    let (server, fake) = Setup::new(fake).retrying().start().await;
    let age = || age_ms(&server, "state");
    let applications = || fake.lock().unwrap().applications();
    wait_for("failing", || in_state(&server, WifiState::Failing)).await;
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
    let (server, fake) = Setup::new(fake).retrying().start().await;
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
