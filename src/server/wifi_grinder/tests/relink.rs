//! Stopping and replacing the link: a new grinder address, and a link that
//! is still finishing a step when it is replaced.

use serde_json::{Value, json};

use super::{
    Fake, PAIRING_ID, Setup, brewing, fake_grinder, job, paired, pairing_file, polls, saved,
    saved_id, start_link, taken, wifi,
};
use crate::protocol::{AbortCause, MachineConfig, MachineEvent};
use crate::server::settings::HostChange;
use crate::server::wifi_grinder::{PairingIdRefusal, WifiPhase, WifiState};
use crate::server::{Server, SettingsChange, Startup, wait_for};

#[tokio::test]
async fn a_new_link_waits_for_the_one_it_replaces_before_it_touches_the_pairing() {
    let file = pairing_file("handover");
    let unpaired = json!({ "brew_device_id": "dev-1", "pairing_id": null });
    std::fs::write(&file, unpaired.to_string()).unwrap();
    let fake = Fake {
        paired: Some(PAIRING_ID),
        ..Fake::default()
    };
    let (server, fake, cfg) = Setup::new(fake).pairing_file(&file).build().await;
    // Stands in for a replaced link whose task has not ended yet: aborting
    // it only takes effect at its next await.
    let old = server.pairing_lock.clone().lock_owned().await;
    start_link(&server, cfg);

    // The old link's last step: the pairing went through, and it saves it.
    let accepted = json!({ "brew_device_id": "dev-1", "pairing_id": PAIRING_ID });
    std::fs::write(&file, accepted.to_string()).unwrap();
    tokio::time::sleep(polls(5)).await;
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

/// Changes only the grinder's address.
fn to(host: HostChange) -> SettingsChange {
    SettingsChange {
        grinder_host: Some(host),
        ..SettingsChange::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changing_the_address_while_pairing_hands_over_at_once() {
    let file = pairing_file("relink-pairing");
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
    let file = pairing_file("relink-paired");
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
    let (server, fake) = Setup::new(Fake::default()).start().await;
    wait_for("paired", || paired(&server)).await;
    // What stopping it does, but for the abort: its task goes on here, as
    // an aborted one does until its next await.
    let _still_running = server.detach_wifi().expect("a link was running");
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));
    let polls = || fake.lock().unwrap().requests_to(wifi::PATH_BREW_JOBS).len();
    let seen = polls();
    wait_for("a few more polls", || polls() >= seen + 5).await;
    assert!(!brewing(&server), "the machine is left alone");
    assert_eq!(taken(&fake), 0, "the job is not taken");
}

#[tokio::test]
async fn a_stopped_link_ends_its_brew_and_sends_nothing_more() {
    let (server, fake) = Setup::new(Fake::default()).start().await;
    wait_for("paired", || paired(&server)).await;
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));
    wait_for("the job taken", || taken(&fake) == 1).await;
    assert!(brewing(&server));

    server.stop_wifi_link();
    assert!(!brewing(&server), "the shot it waited for ended");
    let ended = server.with(|m, _| {
        m.events_after(0).any(|e| {
            matches!(
                e.event,
                MachineEvent::ShotAborted {
                    cause: AbortCause::GrinderAddressChanged,
                    ..
                }
            )
        })
    });
    assert!(ended, "as for a new address");
    assert!(server.grinder_link().wifi.is_none(), "gone from the state");
    assert!(
        server.grinder_link().last_poll.is_none(),
        "no longer shown as connected"
    );
    assert_eq!(
        server.use_grinder_pairing_id(PAIRING_ID),
        Err(PairingIdRefusal::NoLink)
    );
    // The task is gone: no request after the one that may have been on its
    // way, and stopping again does nothing.
    tokio::time::sleep(polls(2)).await;
    let sent = fake.lock().unwrap().requests.len();
    tokio::time::sleep(polls(5)).await;
    assert_eq!(fake.lock().unwrap().requests.len(), sent, "nothing sent");
    server.stop_wifi_link();
}
