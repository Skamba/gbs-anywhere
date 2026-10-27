//! Serving the grinder's brew jobs once paired: each new job starts a brew
//! of the machine, is held open on the grinder while the shot is pulled, and
//! gets the shot (or its cancellation) as its answer.

use std::collections::VecDeque;
use std::time::Instant;

use anyhow::{Context, bail};
use reqwest::StatusCode;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::time::MissedTickBehavior;

use super::reply::{Reply, failure, refused};
use super::{
    ANSWER_LIMIT, CANCEL_LIMIT, HANDLED_CAP, Problem, ProblemCause, ResultProblem, WifiGrinder,
    WifiState,
};
use crate::protocol::machine::EventRecord;
use crate::protocol::wifi::{self, BrewAccept, BrewDone, BrewJob, BrewJobsRes, BrewResult, Paired};
use crate::protocol::{AbortCause, GrindResult, MachineEvent, Phase};
use crate::server::Server;

/// What the link knows of the grinder's jobs and the shot it answers.
pub(super) struct Jobs {
    /// The job whose shot is being brewed.
    current: Option<Held>,
    /// A measured (or cancelled) shot still to reach the grinder.
    answer: Option<Answer>,
    /// Jobs brewed or skipped, so a late copy of one is never brewed again.
    handled: Recent<String>,
    /// Each job's grind id with the poll it first showed up in, so the
    /// newest is known.
    first_seen: Recent<(String, u64)>,
    /// Job requests that went through.
    polls: u64,
    /// What went wrong with the last shot (shown in the app).
    pub(super) problem: Option<Problem>,
}

impl Jobs {
    pub(super) fn new() -> Self {
        Self {
            current: None,
            answer: None,
            handled: Recent::new(HANDLED_CAP),
            first_seen: Recent::new(HANDLED_CAP),
            polls: 0,
            problem: None,
        }
    }

    /// The poll a job first showed up in.
    fn seen_in(&self, job: &BrewJob) -> u64 {
        self.first_seen
            .iter()
            .find(|(id, _)| *id == job.grind_uuid)
            .map_or(self.polls, |(_, poll)| *poll)
    }
}

/// The newest few of something, the oldest dropped to make room.
struct Recent<T> {
    items: VecDeque<T>,
    cap: usize,
}

impl<T> Recent<T> {
    fn new(cap: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(cap),
            cap,
        }
    }

    fn push(&mut self, item: T) {
        if self.items.len() == self.cap {
            self.items.pop_front();
        }
        self.items.push_back(item);
    }

    fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }
}

impl<T: PartialEq> Recent<T> {
    fn contains(&self, item: &T) -> bool {
        self.items.contains(item)
    }
}

/// The job whose shot is being brewed, held open on the grinder.
struct Held {
    job: BrewJob,
    /// The grinder has it as taken: it is not listed. A listed job is still
    /// (or again) waiting to be taken.
    accepted: bool,
}

/// A measured (or cancelled) shot on its way to the grinder.
struct Answer {
    job: BrewJob,
    /// Built once, so every retry carries the same event id.
    done: BrewDone,
    /// The grinder took the job; only the result is still to send.
    accepted: bool,
    since: Instant,
    /// The status line once it went through.
    line: String,
}

impl WifiGrinder {
    /// Asks for jobs and answers shots until the grinder no longer knows the
    /// pairing (`Ok`: pair again) or a request fails.
    pub(super) async fn serve(
        &mut self,
        server: &Server,
        pairing_id: u32,
        events: &mut broadcast::Receiver<EventRecord>,
    ) -> anyhow::Result<()> {
        let mut tick = tokio::time::interval(self.cfg.poll);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // Since when every job request was refused (see `REFUSED_FOR`). Only
        // refusals in a row count: a failed request in between ends `serve`,
        // and the next one starts over.
        let mut refused_since: Option<Instant> = None;
        loop {
            tokio::select! {
                // A shot reported while a request was failing is taken before
                // the next poll could see its job gone.
                biased;
                event = events.recv() => match event {
                    Ok(rec) => {
                        if self.on_event(pairing_id, &rec.event) {
                            self.send_answer(server).await;
                        }
                    }
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => bail!("the server stopped"),
                },
                _ = tick.tick() => {
                    let reply = self
                        .post(wifi::PATH_BREW_JOBS, &Paired::new(pairing_id))
                        .await?;
                    if reply.http == StatusCode::FORBIDDEN {
                        let since = *refused_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= self.cfg.refused_for {
                            return self.forget_pairing(server);
                        }
                        let detail = "the grinder refused the pairing, checking again";
                        self.reach.failure = Some(detail.to_owned());
                        self.status(server, WifiState::Failing, None, detail);
                        continue;
                    }
                    reply.ok().context("asking for brew jobs")?;
                    refused_since = None;
                    // Working again: the same failure later is a new one.
                    self.reach.last_failure = None;
                    if self.pairing.refused_pairing_id.take().is_some() {
                        // The pairing in use works: no fallback needed.
                        self.save_pairing()?;
                    }
                    server.note_wifi_poll(self.owner, reply.peer);
                    if server.grinder_link().wifi.is_none_or(|w| w.state != WifiState::Paired) {
                        let idle = "paired, waiting for a Grind-by-Sync grind";
                        self.status(server, WifiState::Paired, None, idle);
                    }
                    let jobs = reply.parse::<BrewJobsRes>()?.brew_jobs;
                    self.on_jobs(server, pairing_id, &jobs).await;
                }
            }
        }
    }

    /// The grinder refuses the pairing (cleared there): try the command
    /// line's ID if it is another one, else pair again, keeping the refused ID
    /// to try once more if the grinder still has gbs-anywhere paired.
    fn forget_pairing(&mut self, server: &Server) -> anyhow::Result<()> {
        self.drop_job(server, AbortCause::GrinderUnpaired);
        self.jobs.answer = None;
        // The first refused ID stays: when the command-line ID is refused too,
        // the one saved before is the better guess.
        if let Some(id) = self.pairing.pairing_id.take() {
            self.pairing.refused_pairing_id.get_or_insert(id);
        }
        if let Some(id) = self.spare_id.take() {
            tracing::warn!("grinder: it refuses the saved pairing ID, trying the given one");
            self.pairing.pairing_id = Some(id);
        } else {
            tracing::warn!("grinder: it no longer knows this gbs-anywhere, pairing again");
        }
        self.save_pairing()
    }

    /// After each poll: keeps sending a pending answer, else holds the current
    /// job open or takes a new one.
    async fn on_jobs(&mut self, server: &Server, pairing_id: u32, jobs: &[BrewJob]) {
        self.jobs.polls += 1;
        for job in jobs {
            if !self
                .jobs
                .first_seen
                .iter()
                .any(|(id, _)| *id == job.grind_uuid)
            {
                let poll = self.jobs.polls;
                self.jobs.first_seen.push((job.grind_uuid.clone(), poll));
            }
        }
        let listed = |job: &BrewJob| jobs.iter().any(|j| j.grind_uuid == job.grind_uuid);
        if let Some(answer) = &self.jobs.answer {
            let cancel = answer.done.result == BrewResult::Canceled;
            let limit = if cancel { CANCEL_LIMIT } else { ANSWER_LIMIT };
            let gone = !answer.accepted && !listed(&answer.job);
            let expired = gone || answer.since.elapsed() > limit;
            if expired && cancel {
                // Nothing to adjust: the grinder times the job out by itself.
                tracing::info!("grinder: gave up telling it the shot was cancelled");
                self.jobs.answer = None;
                self.jobs.problem = None;
                self.refresh(server);
            } else if expired {
                self.jobs.answer = None;
                self.report_problem(
                    server,
                    ProblemCause::Result(ResultProblem::GrinderResultExpired),
                    "the shot did not reach the grinder in time",
                );
            } else {
                self.send_answer(server).await;
            }
            return;
        }
        if let Some(held) = &self.jobs.current {
            let since = self.jobs.seen_in(&held.job);
            let newer = jobs.iter().any(|j| {
                !self.jobs.handled.contains(&j.grind_uuid) && self.jobs.seen_in(j) > since
            });
            let waiting = listed(&held.job);
            if newer {
                // Ground again before pulling: that grind is the shot now. The
                // brew ends here, and its event answers this job as cancelled.
                // If someone reported or aborted it a moment ago, that event
                // answers it instead; only a brew already over leaves it.
                let over = server.with_wifi(self.owner, |m, now| {
                    m.give_up(now, AbortCause::GrinderShotReplaced).is_err()
                        && m.phase(now) == Phase::Ready
                });
                if over == Some(true) {
                    self.jobs.current = None;
                }
            } else if waiting {
                // Not taken yet, or handed back out: take it (again).
                self.take_current(server, pairing_id).await;
            } else if let Some(held) = self.jobs.current.as_mut() {
                // Not listed: the grinder has it as taken (or forgot it, which
                // the answer finds out).
                held.accepted = true;
            }
            return;
        }
        self.take_job(server, jobs);
        if self.jobs.current.is_some() {
            self.take_current(server, pairing_id).await;
        }
    }

    /// Starts a brew for the job that showed up last; of jobs that showed up
    /// in the same poll (only right after a start), the last listed one. The
    /// others are left for the grinder to time out.
    fn take_job(&mut self, server: &Server, jobs: &[BrewJob]) {
        let fresh: Vec<&BrewJob> = jobs
            .iter()
            .filter(|j| !self.jobs.handled.contains(&j.grind_uuid))
            .collect();
        let Some(job) = fresh
            .iter()
            .copied()
            .enumerate()
            .max_by_key(|(i, j)| (self.jobs.seen_in(j), *i))
            .map(|(_, j)| j)
        else {
            return;
        };
        let target_ms = job.target_ms();
        let started = server.with_wifi(self.owner, |m, now| {
            // A shot that is still being shown, or a brew started by hand,
            // goes first; the job waits for the next poll.
            m.on_brew_job(now, grind_result(job), target_ms)
        });
        if started != Some(true) {
            return;
        }
        for other in fresh.iter().filter(|j| j.grind_uuid != job.grind_uuid) {
            tracing::info!("grinder: skipping an older Grind-by-Sync grind");
            self.jobs.handled.push(other.grind_uuid.clone());
        }
        self.jobs.handled.push(job.grind_uuid.clone());
        self.jobs.problem = None;
        let detail = match target_ms {
            Some(ms) => format!(
                "waiting for the shot (target {:.1} s)",
                f64::from(ms) / 1000.0
            ),
            None => "waiting for the shot".to_owned(),
        };
        tracing::info!("grinder: Grind-by-Sync grind, {detail}");
        self.status(server, WifiState::Paired, None, detail);
        self.jobs.current = Some(Held {
            job: job.clone(),
            accepted: false,
        });
    }

    /// Takes the current job, right after it showed up and whenever the
    /// grinder lists it again, so it stays open until the shot is measured.
    /// A job the grinder refuses ends the brew; when taking it fails
    /// otherwise, it is tried again for as long as it stays listed.
    async fn take_current(&mut self, server: &Server, pairing_id: u32) {
        let Some(held) = &self.jobs.current else {
            return;
        };
        if held.accepted {
            tracing::debug!("grinder: it handed the job back out, taking it again");
        }
        let accept = BrewAccept::new(pairing_id, &held.job);
        let accepted = match self.post(wifi::PATH_BREW_ACCEPT, &accept).await {
            Ok(reply) if reply.status() == wifi::STATUS_OK => true,
            Ok(reply) if reply.status() == wifi::STATUS_JOB_UNKNOWN => {
                self.drop_job(server, AbortCause::GrinderShotDropped);
                return;
            }
            Ok(reply) if refused(&reply) => {
                tracing::warn!("grinder: it refused the job: {:?}", reply.status());
                self.drop_job(server, AbortCause::GrinderShotRefused);
                return;
            }
            other => {
                let why = failure(other);
                tracing::warn!("grinder: could not take the job yet, trying again: {why}");
                false
            }
        };
        if let Some(held) = &mut self.jobs.current {
            held.accepted = accepted;
        }
    }

    /// Ends the brew of the current job without answering it.
    fn drop_job(&mut self, server: &Server, cause: AbortCause) {
        if self.jobs.current.take().is_none() {
            return;
        }
        // Already over when someone reported or aborted it a moment ago.
        let _ = server.with_wifi(self.owner, |m, now| m.give_up(now, cause));
        self.report_problem(server, ProblemCause::Shot(cause), cause.text());
    }

    /// A reported or aborted shot becomes the answer to the current job.
    /// Returns whether there is one to send.
    fn on_event(&mut self, pairing_id: u32, event: &MachineEvent) -> bool {
        let (result, time_ms, grams, reason) = match event {
            MachineEvent::ShotReported { result, .. } => {
                (BrewResult::Success, result.time_ms, result.weight_g, "")
            }
            MachineEvent::ShotAborted { reason, .. } => (BrewResult::Canceled, 0, 0.0, *reason),
            _ => return false,
        };
        let Some(held) = self.jobs.current.take() else {
            return false;
        };
        let event_uuid = uuid::Uuid::new_v4().to_string();
        let done = BrewDone::new(pairing_id, &held.job, result, time_ms, grams, event_uuid);
        let line = match result {
            BrewResult::Success => format!(
                "sent {:.1} s, {:.1} g",
                f64::from(time_ms) / 1000.0,
                done.mass_final_mg as f64 / 1000.0
            ),
            BrewResult::Canceled => format!("told the grinder the shot was cancelled ({reason})"),
        };
        self.jobs.answer = Some(Answer {
            job: held.job,
            done,
            accepted: held.accepted,
            since: Instant::now(),
            line,
        });
        true
    }

    /// One try at getting the pending answer to the grinder: take the job if
    /// not yet taken, then send the result. Kept for the next poll unless it
    /// went through or the grinder refuses it; a job the grinder no longer
    /// has ends on the next poll.
    async fn send_answer(&mut self, server: &Server) {
        let Some(mut answer) = self.jobs.answer.take() else {
            return;
        };
        if !answer.accepted {
            let accept = BrewAccept::new(answer.done.pairing_id, &answer.job);
            match self.post(wifi::PATH_BREW_ACCEPT, &accept).await {
                Ok(reply) if reply.status() == wifi::STATUS_OK => answer.accepted = true,
                // Not listed: taken already (its reply got lost) or gone. Taken
                // counts until the result says it is gone
                // (`STATUS_ONGOING_JOB_UNKNOWN`); the next poll then ends it.
                Ok(reply) if reply.status() == wifi::STATUS_JOB_UNKNOWN => answer.accepted = true,
                Ok(reply) if refused(&reply) => {
                    let why = format!("the grinder refused the shot: {:?}", reply.status());
                    let cause = ProblemCause::Result(ResultProblem::GrinderResultRefused);
                    self.report_problem(server, cause, &why);
                    return;
                }
                other => {
                    self.retry_later(server, answer, other);
                    return;
                }
            }
        }
        match self.post(wifi::PATH_BREW_DONE, &answer.done).await {
            Ok(reply) if reply.status() == wifi::STATUS_OK => {
                tracing::info!("grinder: {}", answer.line);
                self.jobs.problem = None;
                self.status(server, WifiState::Paired, None, answer.line);
            }
            // The grinder handed the job back out: take it again next time.
            Ok(reply) if reply.status() == wifi::STATUS_ONGOING_JOB_UNKNOWN => {
                answer.accepted = false;
                self.jobs.answer = Some(answer);
            }
            Ok(reply) if refused(&reply) => {
                let why = format!("the grinder refused the shot: {:?}", reply.status());
                let cause = ProblemCause::Result(ResultProblem::GrinderResultRefused);
                self.report_problem(server, cause, &why);
            }
            other => self.retry_later(server, answer, other),
        }
    }

    fn retry_later(&mut self, server: &Server, answer: Answer, outcome: anyhow::Result<Reply>) {
        let why = failure(outcome);
        tracing::warn!("grinder: the shot has not reached the grinder yet, retrying: {why}");
        self.jobs.answer = Some(answer);
        self.jobs.problem = Some(Problem {
            cause: ProblemCause::Result(ResultProblem::GrinderResultDelayed),
            text: format!("the shot has not reached the grinder yet, retrying: {why}"),
        });
        self.refresh(server);
    }

    fn report_problem(&mut self, server: &Server, cause: ProblemCause, problem: &str) {
        tracing::warn!("grinder: {problem}");
        self.jobs.problem = Some(Problem {
            cause,
            text: problem.to_owned(),
        });
        let idle = "paired, waiting for a Grind-by-Sync grind";
        self.status(server, WifiState::Paired, None, idle);
    }
}

/// The job as the grind result the app and the shot rules read (recipe weight).
fn grind_result(job: &BrewJob) -> GrindResult {
    GrindResult {
        target_weight_g: job.target_grams(),
        filter: None,
        raw: serde_json::to_value(job).unwrap_or_default(),
    }
}
