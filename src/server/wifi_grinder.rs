//! The link to an E65/E80 GbS grinder over its Local WiFi option (see
//! [`crate::protocol::wifi`]). gbs-anywhere pairs once, then asks the grinder
//! for brew jobs every couple of seconds, which also keeps it shown as active
//! there. A job (one per Grind-by-Sync grind) starts a brew of the shared
//! machine, like the E64 WS's knob press, so the app and the integrations
//! report the shot exactly as they do there.
//!
//! The grinder drops a job nobody takes within about a minute, and hands a
//! taken job back out about every 50 s while no result follows. So a job is
//! taken as soon as it shows up, and again whenever it is listed again: that
//! keeps it open for as long as the machine waits for the shot
//! (`--shot-window-s`), after which it is answered as cancelled. Until the
//! grinder has the result, it is retried on every poll, so a short network
//! outage loses no shot.
//!
//! The job that showed up last is the shot being pulled: a newer grind
//! cancels a job still waiting for its shot, and other open jobs are left
//! for the grinder to time out.
//!
//! The parts: `config` (where the grinder is), `pairing_file` (the pairing
//! kept across restarts), `reply` (requests and what the replies say),
//! `pair` (pairing), `jobs` (the brew jobs and their answers), `status`
//! (what the app is shown) and `slot` (the server's side: starting and
//! stopping the link).

mod config;
mod jobs;
mod pair;
mod pairing_file;
mod reply;
mod slot;
mod status;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::Serialize;
use tokio::sync::OwnedMutexGuard;

pub(crate) use config::{WifiConfig, grinder_address};
use jobs::Jobs;
use pairing_file::Pairing;
pub(crate) use pairing_file::PairingLock;
use reply::unreachable;
use slot::Attached;
pub(super) use slot::WifiSlot;

use crate::integration::Backoff;
use crate::protocol::AbortCause;
use crate::server::Server;

/// Between job requests. The grinder shows the machine as active while it
/// asks.
const POLL: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const MIN_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// The longest wait after a failed request while unpaired: someone is
/// pairing, and a Pairing Code follows soon after the grinder answers again
/// (say, after it restarted).
const UNPAIRED_BACKOFF: Duration = Duration::from_secs(10);
/// How long the app is asked for the pairing ID before gbs-anywhere applies
/// to pair again, in case the pairing was cleared on the grinder meanwhile.
const ASK_WINDOW: Duration = Duration::from_secs(60);
/// How long nothing is sent to a grinder that stopped answering while
/// pairing. The grinder may freeze and restart after the person accepts
/// (about a minute and a half until it answers again); gbs-anywhere leaves
/// it alone until it is back. The API shows it as `quiet_ms`.
const RESTART_QUIET: Duration = Duration::from_secs(180);
/// How long from the person's confirmation in the app to the pairing check,
/// however long after the code it comes. Nothing is sent to the grinder
/// meanwhile, so it is left alone right after the accept; the app and its
/// API keep serving.
pub(super) const ACCEPT_SETTLE: Duration = Duration::from_secs(30);
/// Between pairing checks while the code is shown and nobody has confirmed
/// in the app. The grinder keeps the pairing request open for a long time,
/// so the confirmation is waited for as long as it takes; this slow check
/// only finds an accept nobody confirms. Long enough that a person who
/// accepts on the grinder and then taps in the app normally gets the quiet
/// moment of `ACCEPT_SETTLE` first.
pub(super) const UNCONFIRMED_POLL: Duration = Duration::from_secs(40);
/// Replies are a few hundred bytes; anything this big is not the grinder.
const MAX_REPLY_BYTES: usize = 64 * 1024;
/// How long the grinder must refuse every job request before the pairing
/// counts as cleared there. It also refuses for a few seconds while it starts
/// up (after a power cut or standby), with the pairing still valid.
const REFUSED_FOR: Duration = Duration::from_secs(60);
/// How long a measured shot is retried before it is given up.
const ANSWER_LIMIT: Duration = Duration::from_secs(120);
/// How long a cancelled shot is retried. The grinder times the job out by
/// itself anyway, and a newer grind waits until it is given up.
const CANCEL_LIMIT: Duration = Duration::from_secs(20);
/// Jobs remembered as handled or seen, so a late copy of one is never brewed
/// again and the newest is known.
const HANDLED_CAP: usize = 16;

/// The Local WiFi link as the app shows it (`grinder.wifi` in
/// `GET /api/state`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct WifiLink {
    /// The grinder's address, as configured.
    pub(crate) grinder: String,
    pub(crate) state: WifiState,
    /// Since when `state` holds, set by the server as the link reports it
    /// (see `Server::set_wifi`); the API shows it as `state_age_ms`.
    #[serde(skip)]
    pub(crate) state_since: Option<Instant>,
    /// Where in the pairing the link is, while `state` is `pairing`; `None`
    /// otherwise.
    pub(crate) phase: Option<WifiPhase>,
    /// Since when `phase` holds; the API shows it as `phase_age_ms`.
    #[serde(skip)]
    pub(crate) phase_since: Option<Instant>,
    /// While pairing: the Pairing Code the grinder shows next to
    /// gbs-anywhere for this attempt (`pairingCode`).
    pub(crate) pairing_code: Option<u32>,
    /// While asking: the last pairing ID the grinder did not take, so the
    /// app can say so.
    pub(crate) unknown_pairing_id: Option<u32>,
    /// The grinder went quiet while pairing and came back without the
    /// pairing request: a pairing accepted before did not stick, and the
    /// app asks to accept again. Cleared once paired,
    /// or once the grinder turns out to have the pairing after all.
    pub(crate) pairing_lost: bool,
    /// How long gbs-anywhere sends the grinder nothing after a pairing check
    /// went unanswered (phase `quiet`), so an app can count down to the next
    /// check.
    pub(crate) quiet_ms: u64,
    /// What is going on, in one sentence.
    pub(crate) detail: String,
    /// While `failing`: why, in English words, without the steps that led
    /// there (`detail` has them all), e.g. `operation timed out`.
    pub(crate) failure: Option<String>,
    /// What went wrong with the last shot, in English words, until the next
    /// one goes through.
    pub(crate) problem: Option<String>,
    /// The same as a fixed code, for the app to match on.
    pub(crate) problem_cause: Option<ProblemCause>,
    /// The Pairing ID in use, the pairing's lasting number (`pairingId`);
    /// `None` while unpaired.
    pub(crate) pairing_id: Option<u32>,
    /// The grinder does not answer at all (off, in standby or gone from the
    /// network), as opposed to answering with an error. Cleared as soon as
    /// it answers again.
    pub(crate) unreachable: bool,
    /// Not paired, and the grinder answered the pairing application with a
    /// client error, as one not set up for pairing would: the app says it is
    /// not set up rather than failing. Cleared once it takes one, or fails
    /// some other way.
    pub(crate) not_set_up: bool,
}

/// Why the last shot did not go through to the grinder
/// (`grinder.wifi.problem_cause`), as a fixed code: the [`AbortCause`] of a
/// brew that ended without an answer, or what kept the answer from the
/// grinder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum ProblemCause {
    /// The brew ended for this cause; the grinder got no answer.
    Shot(AbortCause),
    /// The shot was measured, but its answer did not reach the grinder.
    Result(ResultProblem),
}

/// What kept a measured shot from the grinder. Codes read
/// `grinder_result_<what happened>`, as [`AbortCause`]'s do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
// The variants are named as their codes, which share the prefix on purpose.
#[allow(clippy::enum_variant_names)]
pub(crate) enum ResultProblem {
    /// The grinder did not take it in time, and it was given up.
    GrinderResultExpired,
    /// The grinder refused it.
    GrinderResultRefused,
    /// It has not reached the grinder yet, and is tried again.
    GrinderResultDelayed,
}

impl ProblemCause {
    /// Every problem the link reports, one entry per case, for the tests
    /// that check the page words each.
    #[cfg(test)]
    pub(crate) const ALL: [Self; 6] = [
        Self::Shot(AbortCause::GrinderShotDropped),
        Self::Shot(AbortCause::GrinderShotRefused),
        Self::Shot(AbortCause::GrinderUnpaired),
        Self::Result(ResultProblem::GrinderResultExpired),
        Self::Result(ResultProblem::GrinderResultRefused),
        Self::Result(ResultProblem::GrinderResultDelayed),
    ];
}

/// What went wrong with the last shot (`WifiLink::problem` and
/// `problem_cause`).
#[derive(Debug, Clone, PartialEq)]
struct Problem {
    cause: ProblemCause,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WifiState {
    Connecting,
    /// Pairing with the grinder; `WifiLink::phase` says where in it.
    Pairing,
    Paired,
    /// A request failed; retrying.
    Failing,
}

/// Where in the pairing the link is (`grinder.wifi.phase`), set only while
/// its state is [`WifiState::Pairing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WifiPhase {
    /// The code is shown: gbs-anywhere waits for the person to accept it on
    /// the grinder and confirm that in the app (`POST /api/grinder/accepted`
    /// is taken only now), for as long as it takes. Meanwhile it checks the
    /// pairing only every so often (see `UNCONFIRMED_POLL`).
    Accepting,
    /// Confirmed: gbs-anywhere waits a moment before it checks the pairing
    /// (see `ACCEPT_SETTLE`), and sends the grinder nothing meanwhile.
    Holding,
    /// Confirmed in the app: gbs-anywhere checks with the grinder whether the
    /// pairing was accepted. If not, it is back to `Accepting`.
    Checking,
    /// A pairing check went unanswered: gbs-anywhere sends the grinder
    /// nothing for a while (see `RESTART_QUIET`), then checks again.
    Quiet,
    /// The grinder has gbs-anywhere paired but the pairing ID is missing
    /// here: the app asks for the one the grinder shows
    /// (`POST /api/grinder/pairing` is taken only now).
    Asking,
}

/// Why a pairing ID or a confirmation from the app was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairingIdRefusal {
    /// No `--grinder` link runs.
    NoLink,
    /// The link has a working pairing; it needs no ID.
    NotNeeded,
    /// The link is not paired, but not asking for an ID either: it is
    /// connecting or in another phase of the pairing.
    NotAsking,
    /// The link is not waiting for an accept right now, so there is nothing
    /// to confirm.
    NotAccepting,
    /// The last ID or confirmation is still being handled.
    Busy,
}

impl std::fmt::Display for PairingIdRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoLink => {
                "no E65/E80 grinder is set up: enter its address in the app (Set up grinder), or \
                 start gbs-anywhere with --grinder"
            }
            Self::NotNeeded => "gbs-anywhere is paired with the grinder and needs no pairing ID",
            Self::NotAsking => {
                "gbs-anywhere is not asking for a pairing ID right now: it takes one only when the \
                 grinder has it paired but the ID is missing here"
            }
            Self::NotAccepting => {
                "gbs-anywhere is not waiting for you to accept it on the grinder right now"
            }
            Self::Busy => "the last request is still being handled, try again in a moment",
        })
    }
}

impl PairingIdRefusal {
    /// As a fixed code, for `code` in the API's errors.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::NoLink => "grinder_not_set_up",
            Self::NotNeeded => "grinder_already_paired",
            Self::NotAsking => "grinder_not_asking",
            Self::NotAccepting => "grinder_not_accepting",
            Self::Busy => "grinder_pairing_busy",
        }
    }
}

/// The task. Build it with `Server::wifi_link`, then start it with
/// `Server::start_wifi_link`.
pub(crate) struct WifiGrinder {
    cfg: WifiConfig,
    http: reqwest::Client,
    pairing: Pairing,
    /// A pairing ID from the command line to fall back on when the grinder
    /// refuses the saved one.
    spare_id: Option<u32>,
    /// The grinder's jobs, the shot being answered and its problem.
    jobs: Jobs,
    /// What the app is shown of the pairing.
    screen: Screen,
    /// Whether the grinder answers, and why the link fails.
    reach: Reach,
    /// Which link this is to the server: what an older link that was
    /// replaced (new grinder address) still reports is ignored.
    owner: u64,
    /// Who may write the pairing file (see [`PairingLock`]).
    lock: PairingLock,
    /// Held while this link may write it.
    holding: Option<OwnedMutexGuard<()>>,
}

/// What the app is shown of the pairing, besides the link's state.
struct Screen {
    /// The phase shown while the link reports [`WifiState::Pairing`]:
    /// `Accepting` until the person confirms in the app (`Holding` while the
    /// check is then held), `Checking` while that check goes, `Quiet` while
    /// `stay_quiet` waits. `Asking` is shown by `ask_for_pairing_id`.
    phase: WifiPhase,
    /// Since when `phase` holds (for `Quiet`, the current round), for
    /// `WifiLink::phase_since`.
    phase_since: Instant,
    /// The grinder came back from a restart without the pairing request (see
    /// `WifiLink::pairing_lost`).
    pairing_lost: bool,
    /// The last pairing application was turned down (`WifiLink::not_set_up`).
    not_set_up: bool,
    /// The question for the pairing ID as last asked: what it said and the ID
    /// the grinder refused, carried into the next round of `ask_window`.
    asked: Option<(String, Option<u32>)>,
}

/// Whether the grinder answers, and why the link fails.
#[derive(Default)]
struct Reach {
    /// The grinder could not be reached last time (it is off or in standby):
    /// further failures are not logged until it answers again, and the app
    /// says so (`WifiLink::unreachable`).
    unreachable: bool,
    /// The last failure logged as a warning, so the same one again (every
    /// 10 s while not paired) goes to debug instead of filling the log book.
    last_failure: Option<String>,
    /// Why the link fails, for `WifiLink::failure` while it does.
    failure: Option<String>,
}

impl WifiGrinder {
    /// Loads the pairing (or makes a device id) and saves it right away, so an
    /// unwritable file fails at startup rather than after pairing. While
    /// another link still holds the file (it is being replaced), the pairing
    /// is only read here, and loaded and saved again once that link has
    /// ended (see [`PairingLock`]).
    fn new(cfg: WifiConfig, lock: PairingLock) -> anyhow::Result<Self> {
        // Taken before the read: a link that ends in between may have saved.
        let holding = lock.clone().try_lock_owned().ok();
        let (pairing, spare_id) = Pairing::open(&cfg)?;
        if holding.is_some() {
            pairing.save(cfg.pairing_file.as_deref())?;
        }
        let http = crate::http::builder()
            // The grinder's certificate is self-signed and names no host, so it
            // cannot be checked. Requests only go to the configured address,
            // never on to wherever a reply points.
            .tls_danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            // The grinder is on the home network: a proxy set for the
            // internet (HTTPS_PROXY, say for the update check) is not for it.
            .no_proxy()
            // The grinder only finds the body with `Content-Length` spelled
            // this way; lower-case header names leave it reading an empty one.
            .http1_title_case_headers()
            // The grinder drops kept-open connections without notice, and a
            // request sent on one fails with "connection closed before message
            // completed". A fresh connection for each request avoids that.
            .pool_max_idle_per_host(0)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("building the HTTP client")?;
        Ok(Self {
            cfg,
            http,
            pairing,
            spare_id,
            jobs: Jobs::new(),
            screen: Screen {
                phase: WifiPhase::Checking,
                phase_since: Instant::now(),
                pairing_lost: false,
                not_set_up: false,
                asked: None,
            },
            reach: Reach::default(),
            owner: 0,
            lock,
            holding,
        })
    }

    /// Loads the pairing again and saves it, as [`WifiGrinder::new`] does.
    fn reopen_pairing(&mut self) -> anyhow::Result<()> {
        (self.pairing, self.spare_id) = Pairing::open(&self.cfg)?;
        self.save_pairing()
    }

    /// Saves the pairing to the pairing file, if there is one.
    fn save_pairing(&self) -> anyhow::Result<()> {
        self.pairing.save(self.cfg.pairing_file.as_deref())
    }

    /// Runs until the process ends or the link is stopped: pairs when
    /// needed, then serves the grinder's jobs, retrying with backoff on
    /// errors. `attached` comes from starting it (`Server::start_wifi_link`).
    async fn run(mut self, server: Arc<Server>, attached: Attached) {
        let Attached {
            owner,
            mut ids,
            mut accept,
        } = attached;
        self.owner = owner;
        let mut backoff = Backoff::new(self.cfg.min_backoff, self.cfg.max_backoff);
        if self.holding.is_none() {
            // The link this one replaces may still be saving the pairing:
            // start from what it saved, once it has ended.
            self.holding = Some(self.lock.clone().lock_owned().await);
            while let Err(e) = self.reopen_pairing() {
                tracing::warn!("grinder: {e:#}");
                self.reach.failure = Some(e.root_cause().to_string());
                self.status(&server, WifiState::Failing, None, format!("{e:#}"));
                backoff.wait().await;
            }
            backoff.reset();
        }
        // One subscription for the whole run, so a shot reported while a
        // request is being retried is still answered.
        let mut events = server.subscribe();
        self.status(&server, WifiState::Connecting, None, "connecting");
        loop {
            // Said again by `pair` for each application it turns down.
            self.screen.not_set_up = false;
            let outcome = match self.pairing.pairing_id {
                None => self.pair(&server, &mut ids, &mut accept).await,
                Some(id) => self.serve(&server, id, &mut events).await,
            };
            if let Err(e) = outcome {
                // Off or in standby for the night: say so once, not every minute.
                let unreachable = unreachable(&e);
                let message = format!("{e:#}");
                if unreachable && self.reach.unreachable {
                    tracing::debug!("grinder: {e:#}");
                } else if unreachable {
                    tracing::warn!(
                        "grinder: not reachable (off or in standby?), checking again at least \
                         every minute (every 10 s while not paired): {e:#}"
                    );
                } else if self.reach.last_failure.as_deref() == Some(message.as_str()) {
                    tracing::debug!("grinder: {message}");
                } else {
                    tracing::warn!("grinder: {message}");
                }
                self.reach.last_failure = (!unreachable).then(|| message.clone());
                self.reach.unreachable = unreachable;
                self.reach.failure = Some(e.root_cause().to_string());
                self.status(&server, WifiState::Failing, None, message);
                tokio::time::sleep(self.retry_delay(&mut backoff)).await;
            } else {
                backoff.reset();
                self.reach.last_failure = None;
            }
        }
    }

    /// How long to wait before trying again after a failed request: the
    /// backoff, but at most `unpaired_backoff` while there is no pairing.
    fn retry_delay(&self, backoff: &mut Backoff) -> Duration {
        let delay = backoff.advance();
        if self.pairing.pairing_id.is_none() {
            delay.min(self.cfg.unpaired_backoff)
        } else {
            delay
        }
    }
}
