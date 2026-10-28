//! Pairing: applying, a pairing refused or lost, the quiet time after an
//! unanswered check, and the pairing ID asked for in the app.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{
    Fake, PAIRING_ID, RESTART_QUIET_IN_TESTS, Setup, accepting, age_ms, asking, in_phase, in_state,
    paired, pairing_file, polls, saved, saved_id, wifi, wifi_json,
};
use crate::server::wait_for;
use crate::server::wifi_grinder::{PairingIdRefusal, WifiPhase, WifiState};

#[tokio::test]
async fn a_short_refusal_keeps_the_pairing_a_long_one_pairs_again() {
    let file = pairing_file("refusal");
    let stored = json!({ "brew_device_id": "dev-1", "pairing_id": PAIRING_ID });
    std::fs::write(&file, stored.to_string()).unwrap();
    let fake = Fake {
        paired: Some(PAIRING_ID),
        refuse_jobs: 1,
        ..Fake::default()
    };
    let (server, fake) = Setup::new(fake).pairing_file(&file).start().await;
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
    let now = saved(&file);
    assert_eq!(now["brew_device_id"], "dev-1", "same device as before");
    assert!(
        now.get("refused_pairing_id").is_none(),
        "the refused ID went with the new pairing"
    );
    let _ = std::fs::remove_file(file);
}

#[tokio::test]
async fn the_given_pairing_id_is_tried_when_the_saved_one_is_refused() {
    let file = pairing_file("given");
    let stale = json!({ "brew_device_id": "dev-1", "pairing_id": 7 });
    std::fs::write(&file, stale.to_string()).unwrap();
    let fake = Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    };
    let (server, fake) = Setup::new(fake)
        .pairing_file(&file)
        .pairing_id(PAIRING_ID)
        .start()
        .await;
    wait_for("saved", || saved_id(&file) == PAIRING_ID).await;
    wait_for("paired", || paired(&server)).await;
    let refused_kept = || saved(&file).get("refused_pairing_id").is_some();
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
    let (server, fake) = Setup::new(fake).start().await;
    wait_for("the question", || asking(&server)).await;
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
    let (server, fake) = Setup::new(fake).start().await;
    wait_for("the first request", || {
        in_state(&server, WifiState::Pairing)
    })
    .await;
    assert!(!wifi(&server).pairing_lost, "nothing lost yet");
    wait_for("the request again, as lost", || {
        server
            .grinder_link()
            .wifi
            .is_some_and(|w| w.state == WifiState::Pairing && w.pairing_lost)
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
    let (server, fake) = Setup::new(fake).start().await;
    wait_for("the quiet time", || in_phase(&server, WifiPhase::Quiet)).await;
    // The code again, still waiting for the confirmation: nobody tapped
    // Accepted, so it is not checking as confirmed.
    wait_for("the code again, accepting", || {
        server
            .grinder_link()
            .wifi
            .is_some_and(|w| w.pairing_code == Some(73) && w.phase == Some(WifiPhase::Accepting))
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
    // Longer than the test, so the checks below all fall into it.
    let quiet = Duration::from_secs(30);
    let (server, fake) = Setup::new(fake)
        .config(move |cfg| cfg.restart_quiet = quiet)
        .start()
        .await;
    wait_for("the quiet time", || in_phase(&server, WifiPhase::Quiet)).await;
    // Still pairing, and not answering; no code to show.
    let w = wifi(&server);
    assert_eq!(w.state, WifiState::Pairing);
    assert!(w.unreachable);
    assert_eq!(w.pairing_code, None);
    let state = wifi_json(&server);
    assert_eq!(state["state"], json!("pairing"));
    assert_eq!(state["phase"], json!("quiet"));
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
    let before = age_ms(&server, "phase");
    let sent = fake.lock().unwrap().requests.len();
    let wait = Duration::from_millis(200);
    tokio::time::sleep(wait).await;
    assert_eq!(fake.lock().unwrap().requests.len(), sent, "left alone");
    assert_eq!(wifi_json(&server)["phase"], json!("quiet"));
    let later = age_ms(&server, "phase");
    assert!(
        later >= before + wait.as_millis() as u64,
        "{before} then {later}"
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
    let (server, fake) = Setup::new(fake).start().await;
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
    let (server, fake) = Setup::new(fake).start().await;
    wait_for("the request again", || {
        fake.lock().unwrap().applications() == 2 && accepting(&server)
    })
    .await;
    // The code the second application's reply shows comes a moment after
    // the request. An age counted from the first code would stay well past
    // that (by the 100 ms the first one was up), so this holds only once the
    // new code shows with an age of its own.
    let since_second = || {
        fake.lock()
            .unwrap()
            .requests_to(wifi::PATH_PAIRING_APPLICATION)[1]
            .elapsed()
    };
    wait_for("the new code's own phase age", || {
        u128::from(age_ms(&server, "phase")) <= since_second().as_millis() + 50
    })
    .await;
}

#[tokio::test]
async fn asking_again_keeps_a_refused_pairing_id() {
    // The grinder refuses the ID typed in; when the question is asked again
    // after its window, the refusal is still said.
    let fake = Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    };
    let (server, fake) = Setup::new(fake)
        .config(|cfg| cfg.ask_window = Duration::from_millis(200))
        .start()
        .await;
    wait_for("the question", || asking(&server)).await;
    server.use_grinder_pairing_id(7).unwrap();
    wait_for("the refusal", || {
        wifi(&server).unknown_pairing_id == Some(7)
    })
    .await;
    let applications = || fake.lock().unwrap().applications();
    let before = applications();
    // The round after the next one has applied: the next one has been shown
    // by then (it shows right after its application, a window before the
    // following one).
    wait_for("asked again", || {
        applications() >= before + 2 && asking(&server)
    })
    .await;
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
    let file = pairing_file("lost");
    let (server, _fake) = Setup::new(fake).pairing_file(&file).start().await;
    wait_for("the question", || asking(&server)).await;
    assert_eq!(wifi(&server).pairing_id, None, "none in use yet");
    assert_eq!(
        wifi(&server).state,
        WifiState::Pairing,
        "asking is part of the pairing"
    );
    assert_eq!(wifi_json(&server)["phase"], json!("asking"));
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
    let file = pairing_file("refusals-kept");
    let stored = json!({ "brew_device_id": "dev-1", "pairing_id": PAIRING_ID });
    std::fs::write(&file, stored.to_string()).unwrap();
    // Still paired, but refusing three polls in a row while it starts up.
    let fake = Fake {
        paired: Some(PAIRING_ID),
        refuse_jobs: 3,
        ..Fake::default()
    };
    let (server, fake) = Setup::new(fake).pairing_file(&file).start().await;
    wait_for("paired, or asking to pair again", || {
        paired(&server) || asking(&server) || fake.lock().unwrap().applications() > 0
    })
    .await;
    // And it stays that way over the polls after.
    tokio::time::sleep(polls(10)).await;
    let kept = saved_id(&file);
    let _ = std::fs::remove_file(&file);
    assert_eq!(kept, PAIRING_ID, "the saved pairing ID is kept");
    assert_eq!(fake.lock().unwrap().applications(), 0, "no new pairing");
    assert!(!asking(&server), "the app does not ask for the pairing ID");
}

#[tokio::test]
async fn a_refused_pairing_id_is_tried_again_before_the_app_asks() {
    // Refused for a while, but the grinder still has gbs-anywhere paired.
    let file = pairing_file("refused-again");
    let stored =
        json!({ "brew_device_id": "dev-1", "pairing_id": null, "refused_pairing_id": PAIRING_ID });
    std::fs::write(&file, stored.to_string()).unwrap();
    let fake = Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    };
    let (server, fake) = Setup::new(fake).pairing_file(&file).start().await;
    wait_for("paired", || paired(&server)).await;
    let now = saved(&file);
    let _ = std::fs::remove_file(&file);
    assert_eq!(now["pairing_id"], PAIRING_ID, "taken again");
    assert!(now.get("refused_pairing_id").is_none(), "no longer kept");
    assert_eq!(
        fake.lock().unwrap().applications(),
        1,
        "asked once, already paired"
    );
    assert_eq!(wifi(&server).phase, None, "nothing to type in");
}

#[tokio::test]
async fn a_refused_pairing_id_the_grinder_still_refuses_is_dropped() {
    let file = pairing_file("refused-dropped");
    let stored = json!({ "brew_device_id": "dev-1", "pairing_id": null, "refused_pairing_id": 7 });
    std::fs::write(&file, stored.to_string()).unwrap();
    let fake = Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    };
    let (server, _fake) = Setup::new(fake).pairing_file(&file).start().await;
    wait_for("the question", || asking(&server)).await;
    let now = saved(&file);
    let _ = std::fs::remove_file(&file);
    assert!(
        now.get("refused_pairing_id").is_none(),
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
async fn asking_again_keeps_the_question_and_an_id_typed_meanwhile() {
    // Paired on the grinder, the pairing ID missing here; asked for 100 ms
    // at a time, so the link applies again often.
    let fake = Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    };
    let (server, fake) = Setup::new(fake)
        .config(|cfg| cfg.ask_window = Duration::from_millis(100))
        .start()
        .await;
    wait_for("the question", || asking(&server)).await;
    let first = Instant::now();
    let applications = || fake.lock().unwrap().applications();
    wait_for("asked twice more", || {
        applications() >= 3 && asking(&server)
    })
    .await;
    // One question all along: its age counts from the first ask.
    let age = age_ms(&server, "phase");
    let since_first = u64::try_from(first.elapsed().as_millis()).unwrap();
    assert!(age >= since_first, "{age} ms, asked {since_first} ms ago");
    assert!(age >= 200, "two windows: {age} ms");

    // An ID typed while the link applies again (the question still shows)
    // is taken, not dropped as stale.
    fake.lock().unwrap().application_delay = Duration::from_millis(300);
    let before = applications();
    wait_for("applying again", || applications() > before).await;
    assert!(asking(&server), "the question still shows");
    server.use_grinder_pairing_id(PAIRING_ID).unwrap();
    wait_for("paired", || paired(&server)).await;
    assert_eq!(wifi(&server).pairing_id, Some(PAIRING_ID));
}
