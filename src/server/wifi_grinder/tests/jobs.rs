//! Brew jobs: each Grind-by-Sync grind starts a brew, and the shot (or its
//! cancellation) goes back to the grinder.

use std::time::Duration;

use serde_json::{Value, json};

use super::PAIRING_ID;
use super::{
    Fake, Setup, brewing, job, paired, pairing_file, polls, report, saved_id, taken, wifi,
};
use crate::protocol::{
    AbortCause, GrinderRange, MachineConfig, MachineError, MachineEvent, ShotResult,
};
use crate::server::wait_for;
use crate::server::wifi_grinder::{ProblemCause, ResultProblem};

#[tokio::test]
async fn pairs_takes_the_job_at_once_then_sends_the_shot() {
    let file = pairing_file("pairing");
    let (server, fake) = Setup::new(Fake::default())
        .pairing_file(&file)
        .start()
        .await;

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
    let (server, fake) = Setup::new(fake).start().await;
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
    let (server, fake) = Setup::new(fake).start().await;
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
    let (server, fake) = Setup::new(Fake::default()).start().await;
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
    let (server, fake) = Setup::new(fake).start().await;
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
    let (server, fake) = Setup::new(Fake::default()).start().await;
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
    assert_eq!(
        wifi(&server).problem_cause,
        Some(ProblemCause::Result(ResultProblem::GrinderResultExpired))
    );
    let f = fake.lock().unwrap();
    assert_eq!(
        f.calls_to(wifi::PATH_BREW_DONE).len(),
        1,
        "tried, then given up"
    );
    assert_eq!(f.calls_to(wifi::PATH_BREW_ACCEPT).len(), 1);
}

#[tokio::test]
async fn the_jobs_target_time_sets_the_shot_window() {
    let (server, fake) = Setup::new(Fake::default()).start().await;
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
    let (server, fake) = Setup::new(Fake::default()).start().await;
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
    let (server, fake) = Setup::new(Fake::default()).machine(machine).start().await;
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
    let (server, fake) = Setup::new(fake).start().await;
    wait_for("pairing", || paired(&server)).await;
    fake.lock().unwrap().jobs.push(job("g-1", 40_000));

    wait_for("giving up", || wifi(&server).problem.is_some()).await;
    assert!(!brewing(&server));
    // The same cause as the brew's, so the app says it once.
    let cause = AbortCause::GrinderShotDropped;
    assert_eq!(wifi(&server).problem_cause, Some(ProblemCause::Shot(cause)));
    let json = serde_json::to_value(wifi(&server)).unwrap();
    assert_eq!(json["problem_cause"], "grinder_shot_dropped");
    let late = ShotResult::new(Duration::from_secs(30), 40.0);
    assert_eq!(
        server.with(|m, now| m.report_shot(now, late)),
        Err(MachineError::NotRunning),
        "a late shot has nothing to answer"
    );
    // Nothing more is sent for it over the next polls.
    tokio::time::sleep(polls(5)).await;
    let f = fake.lock().unwrap();
    assert_eq!(f.calls_to(wifi::PATH_BREW_ACCEPT).len(), 1, "tried once");
    assert!(
        f.calls_to(wifi::PATH_BREW_DONE).is_empty(),
        "nothing to answer"
    );
}

#[tokio::test]
async fn the_newest_job_is_the_shot_being_pulled() {
    let (server, fake) = Setup::new(Fake::default()).start().await;
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
    // The older job stays listed, but is not brewed after this one, also
    // once the 100 ms finishing hold is over.
    tokio::time::sleep(polls(15)).await;
    assert!(!brewing(&server));
    let accepted = fake.lock().unwrap().calls_to(wifi::PATH_BREW_ACCEPT);
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0]["grindUuid"], "new");
}

#[tokio::test]
async fn a_newer_grind_cancels_one_still_waiting() {
    let (server, fake) = Setup::new(Fake::default()).start().await;
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
async fn a_result_after_a_lost_accept_reply_is_retried() {
    // The grinder takes the job but its reply is lost, and the first brewDone
    // fails. A slower poll lets the shot come in before the next poll.
    let fake = Fake {
        drop_accept_reply: 1,
        fail_done: 1,
        ..Fake::default()
    };
    let (server, fake) = Setup::new(fake)
        .config(|cfg| cfg.poll = Duration::from_millis(600))
        .start()
        .await;
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
