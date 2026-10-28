//! What the link tells the app: its [`WifiLink`], as the server shows it.

use super::{WifiGrinder, WifiLink, WifiPhase, WifiState};
use crate::server::Server;

impl WifiGrinder {
    /// Reports the link's state with what it knows now.
    pub(super) fn status(
        &self,
        server: &Server,
        state: WifiState,
        pairing_code: Option<u32>,
        detail: impl Into<String>,
    ) {
        server.set_wifi(self.owner, self.link(state, pairing_code, detail.into()));
    }

    /// The state the link shows first, before it ran.
    pub(super) fn connecting(&self) -> WifiLink {
        self.link(WifiState::Connecting, None, "connecting".to_owned())
    }

    /// The link as the app shows it in `state`, from what it knows now.
    fn link(&self, state: WifiState, pairing_code: Option<u32>, detail: String) -> WifiLink {
        let pairing = state == WifiState::Pairing;
        WifiLink {
            grinder: self.cfg.base_url.clone(),
            state,
            state_since: None,
            phase: pairing.then_some(self.screen.phase),
            phase_since: pairing.then_some(self.screen.phase_since),
            pairing_code,
            unknown_pairing_id: None,
            pairing_lost: self.screen.pairing_lost,
            quiet_ms: self.cfg.restart_quiet.as_millis() as u64,
            detail,
            error: (state == WifiState::Failing)
                .then(|| self.reach.failure.clone())
                .flatten(),
            problem: self.jobs.problem.as_ref().map(|p| p.text.clone()),
            problem_cause: self.jobs.problem.as_ref().map(|p| p.cause),
            pairing_id: self.pairing.pairing_id,
            unreachable: self.reach.unreachable,
            not_set_up: self.screen.not_set_up,
        }
    }

    /// Shows the current problem without changing the rest of the state.
    pub(super) fn refresh(&self, server: &Server) {
        if let Some(mut link) = server.grinder_link().wifi {
            link.problem = self.jobs.problem.as_ref().map(|p| p.text.clone());
            link.problem_cause = self.jobs.problem.as_ref().map(|p| p.cause);
            server.set_wifi(self.owner, link);
        }
    }

    /// Shows the question for the pairing ID (phase `Asking`, which the
    /// caller has set), with the last one the grinder did not take. The
    /// question stands alone: no shot problem, pairing or failure from
    /// before is shown with it.
    pub(super) fn ask_for_pairing_id(
        &self,
        server: &Server,
        detail: &str,
        refused_id: Option<u32>,
    ) {
        debug_assert_eq!(self.screen.phase, WifiPhase::Asking);
        let link = WifiLink {
            unknown_pairing_id: refused_id,
            pairing_lost: false,
            problem: None,
            problem_cause: None,
            pairing_id: None,
            unreachable: false,
            not_set_up: false,
            ..self.link(WifiState::Pairing, None, detail.to_owned())
        };
        server.set_wifi(self.owner, link);
    }
}
