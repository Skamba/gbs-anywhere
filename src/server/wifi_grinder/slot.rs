//! The server's side of the E65/E80 link: the one place that starts and
//! stops it, and through which a running link reaches the machine and the
//! app. A link that was stopped may still be finishing a step (an aborted
//! task ends only at its next await); what it does then is ignored.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use super::{PairingIdRefusal, WifiConfig, WifiGrinder, WifiLink, WifiPhase};
use crate::protocol::{AbortCause, Machine};
use crate::server::Server;

/// The running E65/E80 link, kept with the machine (in the server's
/// `Inner`), so that whether a link may still act is decided under the same
/// lock as what it does.
#[derive(Default)]
pub(in crate::server) struct WifiSlot {
    /// Which link may act: counts up with every start and stop, and a link
    /// acts only while it holds the current number.
    owner: u64,
    /// The running link's task.
    task: Option<AbortHandle>,
    /// Pairing IDs typed in the app, for the running link.
    ids: Option<mpsc::Sender<u32>>,
    /// "I accepted on the grinder" from the app, for the running link.
    accept: Option<mpsc::Sender<()>>,
}

/// What a link gets when it starts: its owner number and where the app's
/// pairing IDs and confirmations arrive.
pub(super) struct Attached {
    pub(super) owner: u64,
    pub(super) ids: mpsc::Receiver<u32>,
    pub(super) accept: mpsc::Receiver<()>,
}

impl Server {
    /// The link to the grinder `cfg` names, ready to start: loads (or makes)
    /// the pairing, which the server's links share (see `PairingLock`).
    pub(in crate::server) fn wifi_link(&self, cfg: WifiConfig) -> anyhow::Result<WifiGrinder> {
        WifiGrinder::new(cfg, self.pairing_lock.clone())
    }

    /// Starts `link` as the server's E65/E80 link, stopping the one before.
    /// From now on only it reports its state and gets the pairing IDs and
    /// confirmations from the app; until it reports for itself, it shows as
    /// connecting, so the app never sees an address without a link.
    pub(in crate::server) fn start_wifi_link(self: &Arc<Self>, link: WifiGrinder) {
        self.stop_wifi_link();
        if link.cfg.pairing_file.is_none() {
            tracing::warn!(
                "no --config or --grinder-pairing-file: the pairing lasts until restart"
            );
        }
        tracing::info!(
            "grinder: pairing over Local WiFi with {}",
            link.cfg.base_url
        );
        // Room for one each: only the latest typed ID, and the latest
        // confirmation, matter.
        let (ids_tx, ids) = mpsc::channel(1);
        let (accept_tx, accept) = mpsc::channel(1);
        let shown = link.connecting();
        let mut inner = self.lock();
        let slot = &mut inner.wifi;
        slot.owner += 1;
        slot.ids = Some(ids_tx);
        slot.accept = Some(accept_tx);
        let owner = slot.owner;
        let attached = Attached { owner, ids, accept };
        // Spawned under the lock, so the task is in the slot before anyone
        // can stop it; it waits for the lock before it acts.
        let task = tokio::spawn(link.run(self.clone(), attached));
        inner.wifi.task = Some(task.abort_handle());
        inner.grinder.wifi = Some(WifiLink {
            state_since: Some(Instant::now()),
            ..shown
        });
    }

    /// Stops the running link, if any. A shot it was waiting for (a brew
    /// job) can no longer reach the grinder, so that brew ends; a knob press
    /// of an E64 WS on the same server goes on.
    pub(in crate::server) fn stop_wifi_link(&self) {
        // The task ends at its next await; if it is in the middle of a step
        // right now, what it still reports is ignored once detached, and the
        // next link waits for it to end before it touches the pairing file.
        if let Some(task) = self.detach_wifi() {
            task.abort();
        }
    }

    /// The stop without the abort: the running link may no longer act, and
    /// what it said is forgotten. What an E64 WS told (its address, request
    /// counts) stays; the last poll goes, so the grinder no longer shows as
    /// connected unless an E64 WS polls. Returns the link's task, which runs
    /// on until it is aborted.
    pub(in crate::server) fn detach_wifi(&self) -> Option<AbortHandle> {
        let mut inner = self.lock();
        let task = inner.wifi.task.take()?;
        inner.wifi.owner += 1;
        inner.wifi.ids = None;
        inner.wifi.accept = None;
        inner.grinder.wifi = None;
        inner.grinder.last_poll = None;
        self.apply(&mut inner, |m, now| {
            if m.brewing_for_brew_job(now) {
                let _ = m.give_up(now, AbortCause::GrinderAddressChanged);
            }
        });
        Some(task)
    }

    /// Like [`Server::with`], for the E65/E80 link `owner`: `None`, and the
    /// machine untouched, once that link was stopped. Its task may still
    /// be finishing a step, and must not start or end a brew after that.
    pub(super) fn with_wifi<R>(
        &self,
        owner: u64,
        f: impl FnOnce(&mut Machine, Instant) -> R,
    ) -> Option<R> {
        let mut inner = self.lock();
        (inner.wifi.owner == owner).then(|| self.apply(&mut inner, f))
    }

    /// The link's state, unless that link was stopped meanwhile. The time
    /// it entered its state carries over while the state stays the same
    /// (`state_age_ms`).
    pub(super) fn set_wifi(&self, owner: u64, mut link: WifiLink) {
        let mut inner = self.lock();
        if inner.wifi.owner == owner {
            link.state_since = match &inner.grinder.wifi {
                Some(old) if old.state == link.state => old.state_since,
                _ => None,
            }
            .or_else(|| Some(Instant::now()));
            inner.grinder.wifi = Some(link);
        }
    }

    /// A poll of an E65/E80 grinder went through.
    pub(super) fn note_wifi_poll(&self, owner: u64, peer: Option<SocketAddr>) {
        let mut inner = self.lock();
        if inner.wifi.owner == owner {
            inner.grinder.note(peer, true);
        }
    }

    /// Hands the pairing ID an E65/E80 grinder shows to its link, for when
    /// gbs-anywhere lost it. Taken only while the link asks for one; the link
    /// checks it with the grinder before keeping it.
    pub(in crate::server) fn use_grinder_pairing_id(
        &self,
        id: u32,
    ) -> Result<(), PairingIdRefusal> {
        let (tx, asks, paired) = {
            let inner = self.lock();
            let wifi = inner.grinder.wifi.as_ref();
            let asks = wifi.is_some_and(|w| w.phase == Some(WifiPhase::Asking));
            let paired = wifi.is_some_and(|w| w.pairing_id.is_some());
            (inner.wifi.ids.clone(), asks, paired)
        };
        let tx = tx.ok_or(PairingIdRefusal::NoLink)?;
        if !asks {
            return Err(if paired {
                PairingIdRefusal::NotNeeded
            } else {
                PairingIdRefusal::NotAsking
            });
        }
        tx.try_send(id).map_err(|_| PairingIdRefusal::Busy)
    }

    /// The person confirmed they accepted gbs-anywhere on the grinder, so the
    /// E65/E80 link waits a moment, then checks the pairing as confirmed.
    /// Taken only while the link waits for it.
    pub(in crate::server) fn grinder_accepted(&self) -> Result<(), PairingIdRefusal> {
        let (tx, awaiting) = {
            let inner = self.lock();
            let awaiting = inner
                .grinder
                .wifi
                .as_ref()
                .is_some_and(|w| w.phase == Some(WifiPhase::Accepting));
            (inner.wifi.accept.clone(), awaiting)
        };
        let tx = tx.ok_or(PairingIdRefusal::NoLink)?;
        if !awaiting {
            return Err(PairingIdRefusal::NotAccepting);
        }
        tx.try_send(()).map_err(|_| PairingIdRefusal::Busy)
    }
}
