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
use crate::integration::{
    Backoff, BoxFuture, BrewStart, Integration, Link, Live, ReportOutcome,
};

/// How often a running scan looks at what it has found.
const SCAN_POLL: Duration = Duration::from_millis(250);
/// Without a notification for this long, check the scale is still connected.
const SILENCE: Duration = Duration::from_secs(5);
/// Give up on a shot whose end is never seen.
const MAX_SHOT: Duration = Duration::from_secs(120);
const READY: &str = "ready, waiting for a knob press";
/// Pause between commands: sent back to back, the scale drops some.
const COMMAND_GAP: Duration = Duration::from_millis(200);
/// If the scale does not report its timer running this long after a start
/// command, the command is sent again ...
const TIMER_RETRY: Duration = Duration::from_millis(800);
/// ... up to this many times in all.
const TIMER_TRIES: u32 = 3;
/// Between two double beeps, so four beeps are heard as four.
const BEEP_GAP: Duration = Duration::from_millis(700);

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
    // A switched-off scale is the normal case: keep looking at the configured
    // pace, without growing delays, so it reconnects soon after switching on.
    let mut backoff = Backoff::new(cfg.reconnect_every, cfg.reconnect_every);
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
    // Subscribed per connection: brews started while the scale was away must
    // not start a shot now.
    let mut brews = link.brews();
    // The last measured shot's time, shown until the next shot starts.
    let mut last_time = None;
    loop {
        tokio::select! {
            started = brews.next() => {
                let Some(started) = started else {
                    return Ok(());
                };
                last_time = shot(link, cfg, scale, started).await?;
            }
            // Idle: show what is on the scale.
            reading = scale.next_reading() => {
                show(link, cfg, reading?.grams, last_time, false);
            }
        }
    }
}

/// One shot, from the brew start to its end. Returns the shot's time once the
/// scale saw it end.
///
/// A knob press is reported to the grinder. A manual start is a test: the
/// shot is measured the same way and ends the manual brew with the scale's
/// numbers, as typing them in would (the grinder sees a flush).
async fn shot(
    link: &Link,
    cfg: &Config,
    scale: &mut Scale,
    started: BrewStart,
) -> anyhow::Result<Option<Duration>> {
    let test = started == BrewStart::Manual;
    if test {
        tracing::info!("{TITLE}: test brew started by hand, watching the scale");
        link.status.watching("test · watching the scale, the grinder sees a flush");
    } else {
        tracing::info!("{TITLE}: grinder is waiting, watching the scale");
        link.status.watching("grinder is waiting · watching the scale");
    }
    if !cfg.start_delay.is_zero() && !countdown(link, cfg, scale, test).await? {
        aborted(cfg, scale).await;
        return Ok(None);
    }
    let start = Instant::now();
    scale.send(&precisa::TARE).await.context("tare")?;
    if cfg.drive_timer {
        tokio::time::sleep(COMMAND_GAP).await;
        scale.send(&precisa::RESET_TIMER).await.context("reset timer")?;
        tokio::time::sleep(COMMAND_GAP).await;
        scale.send(&precisa::START_TIMER).await.context("start timer")?;
    }
    // Whether the scale has confirmed its timer runs; retried if not.
    let mut timer_confirmed = !cfg.drive_timer;
    let mut start_tries = 1;
    let mut last_start = Instant::now();

    let mut tracker = ShotTracker::new(cfg.end_rule());
    let mut tick = tokio::time::interval(cfg.live_every);
    let mut grams_now = 0.0;
    let end: End = loop {
        tokio::select! {
            reading = scale.next_reading() => {
                let reading = reading?;
                let at = start.elapsed();
                grams_now = reading.grams;
                timer_confirmed |= reading.timer_running;
                show(link, cfg, grams_now, Some(at), true);
                if let Some(end) = tracker.reading(at, reading) {
                    break end;
                }
            }
            _ = tick.tick() => {
                let at = start.elapsed();
                // The clock runs on while the scale is quiet.
                show(link, cfg, grams_now, Some(at), true);
                if !timer_confirmed && last_start.elapsed() >= TIMER_RETRY {
                    if start_tries < TIMER_TRIES {
                        start_tries += 1;
                        last_start = Instant::now();
                        tracing::info!("{TITLE}: scale timer not running, start again \
                            (try {start_tries} of {TIMER_TRIES})");
                        scale.send(&precisa::START_TIMER).await.context("start timer")?;
                    } else {
                        tracing::warn!("{TITLE}: the scale's timer does not start; \
                            measuring the shot without it");
                        timer_confirmed = true;
                    }
                }
                if let Some(end) = tracker.tick(at) {
                    break end;
                }
                // Someone answered or aborted the brew (the app, a timeout):
                // nothing left to measure for.
                if !brew_still_on(link, test) {
                    tracing::info!("{TITLE}: brew ended before the scale saw the shot end");
                    link.status.connected(READY);
                    aborted(cfg, scale).await;
                    return Ok(None);
                }
                if at > MAX_SHOT {
                    tracing::info!("{TITLE}: no end of the shot after {} s, giving up",
                        MAX_SHOT.as_secs());
                    link.status.connected(READY);
                    aborted(cfg, scale).await;
                    return Ok(None);
                }
            }
        }
    };
    if cfg.drive_timer {
        // Only for the display; the shot is measured already.
        let _ = scale.send(&precisa::STOP_TIMER).await;
    }

    let grams = tenth(end.grams);
    let secs = end.time.as_secs_f64();
    show(link, cfg, end.grams, Some(end.time), false);
    let outcome = if test {
        link.finish_manual(end.time, grams, "weighed by the scale (test)")
    } else {
        link.report(end.time, Some((grams, "weighed by the scale".to_owned())))
    };
    let line = match &outcome {
        ReportOutcome::Reported { grams, .. } if test => {
            format!("test shot: {secs:.1} s, {grams:.1} g")
        }
        ReportOutcome::Reported { grams, .. } => format!("last shot: {secs:.1} s, {grams:.1} g"),
        ReportOutcome::NotWaiting | ReportOutcome::Refused(_) => READY.to_owned(),
    };
    link.status.connected(line);
    if cfg.beep {
        let times = if matches!(outcome, ReportOutcome::Reported { .. }) { 2 } else { 4 };
        beep(scale, times).await;
    }
    Ok(Some(end.time))
}

/// Waits `cfg.start_delay` after the brew start, so there is time to start
/// the machine by hand; the shot is timed from the end of it. Meanwhile the
/// app's shot time stays at 0. `false` if the brew ended meanwhile.
async fn countdown(
    link: &Link,
    cfg: &Config,
    scale: &mut Scale,
    test: bool,
) -> anyhow::Result<bool> {
    let secs = cfg.start_delay.as_secs_f64();
    tracing::info!("{TITLE}: measuring starts in {secs:.1} s");
    let what = if test { "test" } else { "grinder is waiting" };
    link.status
        .watching(format!("{what} · start the machine, measuring in {secs:.0} s"));
    let until = Instant::now() + cfg.start_delay;
    let mut tick = tokio::time::interval(cfg.live_every);
    let mut grams = 0.0;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            link.status.watching(format!("{what} · watching the scale"));
            return Ok(true);
        }
        link.status.live(Some(Live {
            grams: tenth(grams),
            // Held at 0 until the delay is over and the shot is timed.
            shot_s: Some(0.0),
            measuring: true,
            refresh_ms: refresh_ms(cfg),
        }));
        if !brew_still_on(link, test) {
            tracing::info!("{TITLE}: brew ended before measuring started");
            link.status.connected(READY);
            return Ok(false);
        }
        tokio::select! {
            reading = scale.next_reading() => {
                grams = reading?.grams;
            }
            _ = tick.tick() => {}
            () = tokio::time::sleep(left) => {}
        }
    }
}

/// A shot that ended without a result: stop the scale's timer and beep four
/// times.
async fn aborted(cfg: &Config, scale: &mut Scale) {
    if cfg.drive_timer {
        let _ = scale.send(&precisa::STOP_TIMER).await;
    }
    if cfg.beep {
        beep(scale, 4).await;
    }
}

/// Beeps `times` times (2 or 4): the scale only knows a double beep. Failures
/// are ignored, the beeps are only a signal.
async fn beep(scale: &mut Scale, times: u32) {
    for i in 0..times / 2 {
        if i > 0 {
            tokio::time::sleep(BEEP_GAP).await;
        }
        let _ = scale.send(&precisa::BEEP_TWICE).await;
    }
}

/// Whether the brew a shot is measured for still runs: the grinder still
/// waits, or for a test, the manual brew still runs.
fn brew_still_on(link: &Link, test: bool) -> bool {
    if test {
        link.brewing() == Some(BrewStart::Manual)
    } else {
        link.grinder_waiting()
    }
}

fn refresh_ms(cfg: &Config) -> u32 {
    u32::try_from(cfg.live_every.as_millis()).unwrap_or(u32::MAX)
}

/// Puts the scale's weight and a shot's time into the status for the app's
/// live display: the running time while `measuring`, else the last shot's.
fn show(link: &Link, cfg: &Config, grams: f64, shot: Option<Duration>, measuring: bool) {
    link.status.live(Some(Live {
        grams: tenth(grams),
        shot_s: shot.map(|d| tenth(d.as_secs_f64())),
        measuring,
        refresh_ms: refresh_ms(cfg),
    }));
}

/// The scale reads 0.1 g; times are shown to 0.1 s.
fn tenth(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
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
                    tracing::trace!("{TITLE}: {} {:02X?}", n.uuid, n.value);
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
        tokio::time::sleep(SCAN_POLL).await;
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
