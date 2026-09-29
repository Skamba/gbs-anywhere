//! The task: keep the scale connected, and after each knob press tare it,
//! start its timer, watch the cup fill and report time and weight.

use std::pin::Pin;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use btleplug::api::{
    Central as _, Characteristic, Manager as _, Peripheral as _, ScanFilter, ValueNotification,
    WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::{Stream, StreamExt};
use uuid::Uuid;

use super::TITLE;
use super::config::Config;
use super::precisa::{self, Reading};
use super::shot::{End, ShotTracker};
use crate::integration::{Backoff, BoxFuture, Integration, Link, ReportOutcome};

const MIN_BACKOFF: Duration = Duration::from_secs(2);
/// A switched-off scale is the normal case: look again at least every minute.
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Without a notification for this long, check the scale is still connected.
const SILENCE: Duration = Duration::from_secs(5);
/// Give up on a shot whose end is never seen.
const MAX_SHOT: Duration = Duration::from_secs(120);
/// How often a running shot is checked while the scale is quiet.
const TICK: Duration = Duration::from_millis(250);
const READY: &str = "ready, waiting for a knob press";

/// The integration. See the module docs.
pub struct Precisa {
    pub cfg: Config,
}

impl Integration for Precisa {
    fn run(self: Box<Self>, link: Link) -> BoxFuture {
        Box::pin(run(link, self.cfg))
    }
}

// ---------------------------------------------------------------------------

async fn run(link: Link, cfg: Config) {
    let mut backoff = Backoff::new(MIN_BACKOFF, MAX_BACKOFF);
    loop {
        link.status.starting("looking for the scale");
        match connect(&cfg).await {
            Ok(mut scale) => {
                backoff.reset();
                tracing::info!("{TITLE}: connected to {}", scale.name);
                link.status.subject(&scale.name);
                link.status.connected(READY);
                let served = serve(&link, &cfg, &mut scale).await;
                scale.disconnect().await;
                match served {
                    // The server is gone.
                    Ok(()) => return,
                    Err(e) => {
                        tracing::warn!("{TITLE}: {e:#}");
                        link.status.error(format!("{e:#}"));
                    }
                }
            }
            Err(e) => {
                // Mostly: the scale is switched off. Not worth a warning.
                tracing::debug!("{TITLE}: {e:#}");
                link.status.error(format!("{e:#}"));
            }
        }
        backoff.wait().await;
    }
}

/// Runs while the scale stays connected. `Ok` once the server is gone.
async fn serve(link: &Link, cfg: &Config, scale: &mut Scale) -> anyhow::Result<()> {
    // Subscribed per connection: knob presses while the scale was away must
    // not start a shot now.
    let mut brews = link.grinder_brews();
    loop {
        tokio::select! {
            pressed = brews.next() => {
                if !pressed {
                    return Ok(());
                }
                shot(link, cfg, scale).await?;
            }
            // Idle readings only keep the connection watched.
            reading = scale.next_reading() => {
                reading?;
            }
        }
    }
}

/// One shot, from the knob press to the report.
async fn shot(link: &Link, cfg: &Config, scale: &mut Scale) -> anyhow::Result<()> {
    let start = Instant::now();
    tracing::info!("{TITLE}: grinder is waiting, watching the scale");
    link.status.watching("grinder is waiting · watching the scale");
    scale.send(&precisa::TARE).await.context("tare")?;
    if cfg.drive_timer {
        scale.send(&precisa::RESET_TIMER).await.context("reset timer")?;
        scale.send(&precisa::START_TIMER).await.context("start timer")?;
    }

    let mut tracker = ShotTracker::new(cfg.end_rule());
    let mut tick = tokio::time::interval(TICK);
    let end: End = loop {
        tokio::select! {
            reading = scale.next_reading() => {
                if let Some(end) = tracker.reading(start.elapsed(), reading?) {
                    break end;
                }
            }
            _ = tick.tick() => {
                let at = start.elapsed();
                if let Some(end) = tracker.tick(at) {
                    break end;
                }
                if !link.grinder_waiting() {
                    tracing::info!("{TITLE}: brew ended before the scale saw the shot end");
                    link.status.connected(READY);
                    return Ok(());
                }
                if at > MAX_SHOT {
                    tracing::info!("{TITLE}: no end of the shot after {} s, giving up",
                        MAX_SHOT.as_secs());
                    link.status.connected(READY);
                    return Ok(());
                }
            }
        }
    };
    if cfg.drive_timer {
        // Only for the display; the shot is measured already.
        let _ = scale.send(&precisa::STOP_TIMER).await;
    }

    // The scale reads 0.1 g.
    let grams = (end.grams * 10.0).round() / 10.0;
    let outcome = link.report(end.time, Some((grams, "weighed by the scale".to_owned())));
    let line = match outcome {
        ReportOutcome::Reported { grams, .. } => {
            format!("last shot: {:.1} s, {grams:.1} g", end.time.as_secs_f64())
        }
        ReportOutcome::NotWaiting | ReportOutcome::Refused(_) => READY.to_owned(),
    };
    link.status.connected(line);
    Ok(())
}

// ---------------------------------------------------------------------------
// The scale over Bluetooth
// ---------------------------------------------------------------------------

struct Scale {
    peripheral: Peripheral,
    command: Characteristic,
    notifications: Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
    /// `CFS-9002 · AA:BB:CC:DD:EE:FF`.
    name: String,
}

impl Scale {
    /// Commands are written without response, as the scale requires.
    /// `&mut self` like everything here: the notification stream is `Send`
    /// but not `Sync`, so a shared borrow across an await would make the task
    /// not `Send`.
    async fn send(&mut self, command: &[u8]) -> anyhow::Result<()> {
        self.peripheral
            .write(&self.command, command, WriteType::WithoutResponse)
            .await?;
        Ok(())
    }

    /// The next weight reading. Waits through silence while the scale stays
    /// connected (it may only send on change); errors once it is gone.
    /// Cancel-safe.
    async fn next_reading(&mut self) -> anyhow::Result<Reading> {
        loop {
            match tokio::time::timeout(SILENCE, self.notifications.next()).await {
                Ok(Some(n)) => {
                    if n.uuid == precisa::STATUS
                        && let Some(r) = precisa::parse(&n.value)
                    {
                        return Ok(r);
                    }
                }
                Ok(None) => bail!("scale disconnected"),
                Err(_) => {
                    if !self.peripheral.is_connected().await? {
                        bail!("scale disconnected (switched off or out of range)");
                    }
                }
            }
        }
    }

    async fn disconnect(&mut self) {
        let _ = self.peripheral.disconnect().await;
    }
}

/// Finds the scale, connects, subscribes to its weight and sets grams.
async fn connect(cfg: &Config) -> anyhow::Result<Scale> {
    let manager = Manager::new().await.context("Bluetooth not available")?;
    let adapter = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .context("no Bluetooth adapter")?;

    // No service filter: not every scale advertises FFF0.
    adapter.start_scan(ScanFilter::default()).await?;
    let deadline = Instant::now() + cfg.scan_for;
    let found = loop {
        if let Some(found) = find(&adapter, cfg).await? {
            break found;
        }
        if Instant::now() >= deadline {
            let _ = adapter.stop_scan().await;
            bail!("scale not found (switched on and in range?)");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    let _ = adapter.stop_scan().await;
    let (peripheral, name) = found;

    peripheral.connect().await.context("connecting to the scale")?;
    peripheral.discover_services().await?;
    let characteristics = peripheral.characteristics();
    let pick = |uuid: Uuid| characteristics.iter().find(|c| c.uuid == uuid).cloned();
    let status = pick(precisa::STATUS).context("scale has no FFF1 characteristic")?;
    let command = pick(precisa::COMMAND).context("scale has no FFF2 characteristic")?;
    peripheral.subscribe(&status).await?;
    let notifications = peripheral.notifications().await?;

    let mut scale = Scale {
        peripheral,
        command,
        notifications,
        name,
    };
    scale.send(&precisa::UNIT_GRAMS).await.context("set grams")?;
    Ok(scale)
}

/// The configured scale among what the scan has seen, with a display name.
async fn find(adapter: &Adapter, cfg: &Config) -> anyhow::Result<Option<(Peripheral, String)>> {
    for p in adapter.peripherals().await? {
        let Some(props) = p.properties().await? else {
            continue;
        };
        let address = p.address().to_string();
        let name = props.local_name.unwrap_or_default();
        let wanted = match &cfg.address {
            Some(a) => address.eq_ignore_ascii_case(a),
            None => name.starts_with(&cfg.name_prefix),
        };
        if wanted {
            let label = if name.is_empty() {
                address
            } else {
                format!("{name} · {address}")
            };
            return Ok(Some((p, label)));
        }
    }
    Ok(None)
}
