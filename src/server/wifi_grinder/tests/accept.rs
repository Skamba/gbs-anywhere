//! The accept gate: the code is shown, the person accepts on the grinder
//! and confirms in the app, and only then is the pairing checked (or at the
//! slow check, without a confirmation).

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{
    ACCEPT_SETTLE_IN_TESTS, Fake, NO_SLOW_CHECK, PAIRING_ID, Setup, accepted_on_the_grinder,
    accepting, answered, checks, due, paired, wifi, wifi_json,
};
use crate::server::wait_for;
use crate::server::wifi_grinder::{PairingIdRefusal, WifiPhase, WifiState};

/// Long enough for a check released without a confirmation to have gone
/// out: five times the settle after one.
const NO_RELEASE: Duration = Duration::from_millis(5 * ACCEPT_SETTLE_IN_TESTS.as_millis() as u64);

#[tokio::test]
async fn nothing_is_sent_to_the_grinder_until_the_person_accepts() {
    // A slow check the test never reaches, so only the confirmation can
    // release the first check.
    let (server, fake) = Setup::new(accepted_on_the_grinder()).gated().start().await;
    wait_for("awaiting the accept", || accepting(&server)).await;

    // With no confirmation: still nothing sent, and the app and its API keep
    // serving (the state shows the waiting phase).
    tokio::time::sleep(NO_RELEASE).await;
    assert_eq!(
        wifi(&server).phase,
        Some(WifiPhase::Accepting),
        "still waiting, not holding yet"
    );
    assert!(
        checks(&fake).is_empty(),
        "no pairing check before the person confirms"
    );
    let state = wifi_json(&server);
    assert_eq!(
        state["phase"],
        json!("accepting"),
        "the state keeps serving while the gate holds"
    );
    assert!(state["grinder"].is_string());
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
    let (server, fake) = Setup::new(accepted_on_the_grinder())
        .gate(settle, NO_SLOW_CHECK)
        .start()
        .await;
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
    let (server, fake) = Setup::new(fake)
        .gate(ACCEPT_SETTLE_IN_TESTS, every)
        .start()
        .await;
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
    let (server, fake) = Setup::new(fake)
        .gate(ACCEPT_SETTLE_IN_TESTS, every)
        .start()
        .await;
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
    let (server, fake) = Setup::new(fake).gate(settle, every).start().await;
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
    let (server, fake) = Setup::new(fake).gate(settle, every).start().await;
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
async fn a_confirmation_is_refused_when_the_link_is_not_awaiting() {
    // No link at all.
    let no_link = crate::server::Server::new(crate::protocol::MachineConfig::default());
    assert_eq!(no_link.grinder_accepted(), Err(PairingIdRefusal::NoLink));
    assert_eq!(PairingIdRefusal::NoLink.code(), "grinder_not_set_up");

    // A link waiting for the accept: a second confirmation while the first is
    // still queued is refused as busy. (Nothing awaits between the two calls,
    // so the link cannot take the first yet.)
    let (server, _fake) = Setup::new(accepted_on_the_grinder()).gated().start().await;
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
    let (server, fake) = Setup::new(Fake::default()).gated().start().await;
    wait_for("awaiting the accept", || accepting(&server)).await;

    // What stopping the link does, but for the abort: the owner changes and
    // the channel is dropped. The old task runs on (as an aborted one does
    // until its next await), but what it reports is ignored.
    let _still_running = server.detach_wifi().expect("a link was running");
    assert_eq!(
        server.grinder_accepted(),
        Err(PairingIdRefusal::NoLink),
        "a tap from a stale app view does not reach the replaced link"
    );
    // Had it been released, it would check after the short settle. Without
    // its sender it waits for the slow check, which the test never reaches.
    tokio::time::sleep(NO_RELEASE).await;
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
    let (server, fake) = Setup::new(fake).gated().start().await;

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
    assert_eq!(wifi_json(&server)["phase_age_ms"], Value::Null);
}
