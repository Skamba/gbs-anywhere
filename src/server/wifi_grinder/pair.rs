//! Pairing: the application, the person's accept on the grinder and their
//! confirmation in the app, and the pairing ID when it is missing here.

use std::time::Instant;

use anyhow::Context;
use reqwest::StatusCode;
use serde_json::json;
use tokio::sync::mpsc;

use super::reply::{Reply, failure, no_answer, refused};
use super::{WifiGrinder, WifiPhase, WifiState};
use crate::protocol::wifi::{
    self, API_VERSION, Paired, PairingApplication, PairingApplicationRes, PairingStatusRes,
};
use crate::server::Server;

impl WifiGrinder {
    /// Applies for pairing and waits until the person accepts it on the
    /// grinder. `Ok` once paired, or when it should be tried again.
    pub(super) async fn pair(
        &mut self,
        server: &Server,
        ids: &mut mpsc::Receiver<u32>,
        accept: &mut mpsc::Receiver<()>,
    ) -> anyhow::Result<()> {
        let app = PairingApplication::new(&self.pairing.brew_device_id);
        let reply = self.post(wifi::PATH_PAIRING_APPLICATION, &app).await?;
        if reply.status() == wifi::STATUS_ALREADY_PAIRED {
            return self.wait_for_pairing_id(server, ids).await;
        }
        // It answers but turns the application down: not set up for pairing.
        self.screen.not_set_up = refused(&reply);
        reply.ok().context("asking to pair")?;
        let res: PairingApplicationRes = reply.parse()?;
        let application_id = res
            .application_id
            .context("the grinder sent no application ID")?;
        if self.pairing.refused_pairing_id.take().is_some() {
            // It no longer has this gbs-anywhere paired: that ID is gone.
            self.save_pairing()?;
        }
        let code = res.pairing_code;
        let detail = format!(
            "{}on the grinder, under Configuration, tap Start Pairing and \
             accept gbs-anywhere{}",
            if self.screen.pairing_lost {
                "the grinder no longer has the pairing request, so a pairing accepted on it did not \
                 stick: "
            } else {
                ""
            },
            code.map_or(String::new(), |c| format!(" (Pairing Code {c})"))
        );
        tracing::info!("grinder: {detail}");
        // The new code is shown first: a confirmation left from an earlier
        // attempt must not release this one (cf. `wait_for_pairing_id`
        // draining `ids`). A new code starts its own phase age, also after
        // an earlier one in `Accepting`.
        self.screen.phase = WifiPhase::Accepting;
        self.screen.phase_since = Instant::now();
        self.status(server, WifiState::Pairing, code, detail.clone());
        while accept.try_recv().is_ok() {}
        let shown = tokio::time::Instant::now();

        let ask = json!({ "version": API_VERSION, "applicationId": application_id });
        // Left alone after a check got no answer (see `stay_quiet`).
        let mut quiet = false;
        // The check (or the one before the quiet time) came after a
        // confirmation in the app.
        let mut confirmed = false;
        loop {
            // Shows the code and waits for the next check: after the
            // confirmation and a moment, or the slow check without one. After
            // the quiet time, the request is checked again at once.
            if !quiet {
                confirmed = self.await_check(server, accept, code, &detail, shown).await;
            }
            let outcome = self.post(wifi::PATH_PAIRING_STATUS, &ask).await;
            if no_answer(&outcome) {
                self.stay_quiet(server, outcome, !quiet).await;
                quiet = true;
                continue;
            }
            let reply = outcome?;
            if reply.status() == wifi::STATUS_APPLICATION_UNKNOWN {
                if quiet {
                    tracing::info!(
                        "grinder: it no longer has the pairing request (it restarted?), asking \
                         again"
                    );
                    self.screen.pairing_lost = true;
                } else {
                    tracing::info!("grinder: the pairing request expired, asking again");
                }
                return Ok(());
            }
            reply.ok().context("checking the pairing")?;
            let status: PairingStatusRes = reply.parse()?;
            if !status.accepted {
                // Back without a restart, the request still open; or
                // confirmed before the grinder had it accepted. Either way
                // the code is shown again, and only a confirmation from now
                // on counts: the grinder has no accept for one sent before.
                if std::mem::take(&mut quiet) || confirmed {
                    if confirmed {
                        tracing::info!(
                            "grinder: the grinder has not accepted the pairing yet, waiting for \
                             the accept and a new confirmation in the app"
                        );
                    }
                    while accept.try_recv().is_ok() {}
                }
                continue;
            }
            let Some(id) = status.pairing_id else {
                return self.wait_for_pairing_id(server, ids).await;
            };
            self.use_pairing_id(id)?;
            tracing::info!("grinder: paired");
            return Ok(());
        }
    }

    /// Shows the code (phase `Accepting`) and waits for the next pairing
    /// check, for as long as the person takes to confirm in the app. Without
    /// a confirmation, the check is due `unconfirmed_poll` from now. A
    /// confirmation moves to `Holding`: the grinder is sent nothing for
    /// `accept_settle`, then the check is due, in phase `Checking`. Returns
    /// whether the person confirmed. `shown` is when the code was shown, for
    /// the log.
    async fn await_check(
        &mut self,
        server: &Server,
        accept: &mut mpsc::Receiver<()>,
        code: Option<u32>,
        detail: &str,
        shown: tokio::time::Instant,
    ) -> bool {
        // For the log: seconds since the code was shown.
        let after = |at: tokio::time::Instant| at.duration_since(shown).as_secs_f32();
        self.set_phase(WifiPhase::Accepting);
        self.status(server, WifiState::Pairing, code, detail);

        let slow = tokio::time::Instant::now() + self.cfg.unconfirmed_poll;
        let tapped = tokio::select! {
            biased;
            got = accept.recv() => got.is_some(),
            () = tokio::time::sleep_until(slow) => false,
        };
        if !tapped {
            // No confirmation, or the sender is gone (the link is being
            // replaced): the slow check, never sooner.
            tokio::time::sleep_until(slow).await;
            tracing::debug!(
                "grinder: checking the pairing {:.1} s after the code, not confirmed in the app",
                after(tokio::time::Instant::now())
            );
            return false;
        }

        let tap = tokio::time::Instant::now();
        tracing::info!(
            "grinder: confirmed in the app {:.1} s after the code, checking the pairing in {:.1} s",
            after(tap),
            self.cfg.accept_settle.as_secs_f32()
        );
        self.set_phase(WifiPhase::Holding);
        let hold = "accepted: waiting a moment, then checking the pairing";
        self.status(server, WifiState::Pairing, code, hold);
        tokio::time::sleep_until(tap + self.cfg.accept_settle).await;

        let now = tokio::time::Instant::now();
        tracing::info!(
            "grinder: checking the pairing {:.1} s after the code, {:.1} s after the confirmation",
            after(now),
            now.duration_since(tap).as_secs_f32()
        );
        self.set_phase(WifiPhase::Checking);
        // Back to the plain pairing detail for the check and `stay_quiet`.
        self.status(server, WifiState::Pairing, code, detail);
        true
    }

    /// After a pairing check got no answer: sends the grinder nothing for a
    /// while (see `RESTART_QUIET`). Shown as still pairing, in phase
    /// `Quiet` and not answering, and logged as a warning the `first` time
    /// in a row.
    async fn stay_quiet(&mut self, server: &Server, outcome: anyhow::Result<Reply>, first: bool) {
        let secs = self.cfg.restart_quiet.as_secs();
        let wait = if secs >= 60 {
            format!("{} min", secs.div_ceil(60))
        } else {
            format!("{secs} s")
        };
        let detail = format!(
            "no answer to the pairing check: nothing is sent to the grinder for {wait}, then \
             gbs-anywhere checks again (the grinder may be restarting)"
        );
        if first {
            tracing::warn!("grinder: {detail}: {}", failure(outcome));
        } else {
            tracing::debug!("grinder: {detail}: {}", failure(outcome));
        }
        self.reach.unreachable = true;
        // Each round starts its own quiet time, also after a quiet one, so
        // `phase_age_ms` says how long until the next check.
        self.screen.phase = WifiPhase::Quiet;
        self.screen.phase_since = Instant::now();
        self.status(server, WifiState::Pairing, None, detail);
        tokio::time::sleep(self.cfg.restart_quiet).await;
    }

    /// The grinder has gbs-anywhere paired, but its pairing ID is not known
    /// here (the grinder restarted before saying it, or the file is lost).
    /// Tries the ID the grinder refused before, then waits for one typed in
    /// the app and checks it with the grinder. `Ok` once one works, or after a
    /// while, to apply again in case the pairing was cleared on the grinder
    /// meanwhile.
    async fn wait_for_pairing_id(
        &mut self,
        server: &Server,
        ids: &mut mpsc::Receiver<u32>,
    ) -> anyhow::Result<()> {
        let mut detail = "paired on the grinder, but the pairing ID is missing here: enter the \
                          Pairing ID the grinder shows under Configuration"
            .to_owned();
        let mut refused_id = None;
        // The grinder has the pairing: whatever restarted, it stuck, and we
        // are no longer waiting for an accept.
        self.screen.pairing_lost = false;
        // Asked before, and back after `ask_window` (or a failed request): the
        // question goes on, with its age and an ID typed meanwhile.
        let again = self.screen.phase == WifiPhase::Asking;
        if !again {
            self.set_phase(WifiPhase::Checking);
            self.screen.asked = None;
        } else if let Some((said, id)) = self.screen.asked.clone() {
            // Still the same question: what it said, and an ID refused in it.
            detail = said;
            refused_id = id;
        }
        // The ID it refused for a while, in case it refused it only then.
        if let Some(id) = self.pairing.refused_pairing_id {
            let reply = self.post(wifi::PATH_BREW_JOBS, &Paired::new(id)).await?;
            if reply.http == StatusCode::FORBIDDEN || refused(&reply) {
                self.pairing.refused_pairing_id = None;
                self.save_pairing()?;
                detail = format!(
                    "the grinder no longer knows pairing ID {id}: enter the Pairing ID it shows \
                     under Configuration"
                );
                refused_id = Some(id);
            } else {
                reply
                    .ok()
                    .context("checking the pairing ID the grinder refused before")?;
                self.use_pairing_id(id)?;
                tracing::info!("grinder: it takes the pairing ID it refused before");
                return Ok(());
            }
        }
        let deadline = tokio::time::Instant::now() + self.cfg.ask_window;
        // An ID typed while the link was not asking is stale by now.
        if !again {
            while ids.try_recv().is_ok() {}
        }
        loop {
            // The same question again (a new round of the ask window) at
            // debug, so it doesn't fill the log book.
            if self
                .screen
                .asked
                .as_ref()
                .is_some_and(|(said, _)| *said == detail)
            {
                tracing::debug!("grinder: {detail} (in the app, or with --grinder-pairing-id)");
            } else {
                tracing::warn!("grinder: {detail} (in the app, or with --grinder-pairing-id)");
            }
            self.screen.asked = Some((detail.clone(), refused_id));
            self.set_phase(WifiPhase::Asking);
            self.ask_for_pairing_id(server, &detail, refused_id);
            let id = tokio::select! {
                _ = tokio::time::sleep_until(deadline) => return Ok(()),
                Some(id) = ids.recv() => id,
            };
            let reply = self.post(wifi::PATH_BREW_JOBS, &Paired::new(id)).await?;
            if reply.http == StatusCode::FORBIDDEN {
                detail = format!(
                    "the grinder does not know pairing ID {id}: enter the Pairing ID it shows \
                     under Configuration"
                );
                refused_id = Some(id);
                continue;
            }
            reply.ok().context("checking the pairing ID")?;
            self.use_pairing_id(id)?;
            tracing::info!("grinder: paired with the given pairing ID");
            return Ok(());
        }
    }

    /// Moves the pairing to `phase`, counting its age from now unless it is
    /// there already.
    fn set_phase(&mut self, phase: WifiPhase) {
        if self.screen.phase != phase {
            self.screen.phase = phase;
            self.screen.phase_since = Instant::now();
        }
    }

    fn use_pairing_id(&mut self, id: u32) -> anyhow::Result<()> {
        self.screen.pairing_lost = false;
        self.set_phase(WifiPhase::Checking);
        self.pairing.pairing_id = Some(id);
        self.pairing.refused_pairing_id = None;
        self.save_pairing()
    }
}
