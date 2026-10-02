//! The task: keep the scale connected, and after each knob press tare it,
//! start its timer, watch the cup fill and report time and weight.
//!
//! With a pump sensor integration connected (none included yet), the pump times
//! the shot instead: it starts when the pump starts and ends when the pump
//! stops; the scale then weighs the cup once the last drops have landed.

use std::pin::Pin;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use btleplug::api::{
    Central as _, Characteristic, Manager as _, Peripheral as _, ScanFilter, ValueNotification,
    WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::{Stream, StreamExt};
use tokio::sync::watch;
use uuid::Uuid;

use super::TITLE;
use super::config::Config;
use super::protocol::{self, Reading};
use super::shot::{End, ShotTracker, TargetWatch};
use crate::integration::{
    Backoff, BoxFuture, BrewStart, Integration, Link, Live, Pump, ReportOutcome,
};

/// How often a running scan looks at what it has found.
const SCAN_POLL: Duration = Duration::from_millis(250);
/// Without a notification for this long, check the scale is still connected.
const SILENCE: Duration = Duration::from_secs(5);
/// Give up on a shot whose end is never seen: after this, or a minute past
/// the minimum time if that is later.
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
/// After the pump stops, the weight is taken once it has not risen for the
/// "seconds without a rise", or after that plus this at the latest.
const SETTLE_EXTRA: Duration = Duration::from_secs(5);

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
    // One D-Bus connection for the whole run. Each `Manager` opens its own
    // and keeps it; one per attempt used up the system bus's per-user limit
    // ("maximum number of active connections for UID 0").
    let bt = loop {
        link.status.starting("opening Bluetooth");
        match Bluetooth::open().await {
            Ok(bt) => break bt,
            Err(e) => {
                tracing::warn!("{TITLE}: {e:#}");
                link.status.error(format!("{e:#}"));
                // Slower here: without Bluetooth nothing changes quickly.
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    };
    // Held for the whole run: a pump sensor sees someone listens and leaves
    // reporting to this integration.
    let mut pump = link.pump();
    loop {
        link.status.starting("looking for the scale");
        match connect(&bt.adapter, &cfg).await {
            Ok(mut scale) => {
                backoff.reset();
                tracing::info!("{TITLE}: connected to {}", scale.name);
                link.status.subject(&scale.name);
                link.status.connected(READY);
                let served = serve(&link, &cfg, &mut scale, &mut pump).await;
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
async fn serve(
    link: &Link,
    cfg: &Config,
    scale: &mut Scale,
    pump: &mut watch::Receiver<Pump>,
) -> anyhow::Result<()> {
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
                last_time = shot(link, cfg, scale, started, pump).await?;
            }
            // Idle: show what is on the scale.
            reading = scale.next_reading() => {
                show(link, cfg, reading?.grams, last_time, false);
            }
        }
    }
}

/// One shot, from the brew start to its end. Returns the shot's time once it
/// was seen to end.
///
/// A knob press is reported to the grinder. A manual start is a test: the
/// shot is measured the same way and ends the manual brew with the scale's
/// numbers, as typing them in would (the grinder sees a flush).
async fn shot(
    link: &Link,
    cfg: &Config,
    scale: &mut Scale,
    started: BrewStart,
    pump: &mut watch::Receiver<Pump>,
) -> anyhow::Result<Option<Duration>> {
    let test = started == BrewStart::Manual;
    let sensed = pump.borrow_and_update().sensed;
    let what = if test { "test" } else { "grinder is waiting" };
    tracing::info!(
        "{TITLE}: {what}, timing by {}",
        if sensed { "the pump sensor" } else { "the scale" }
    );
    if sensed {
        link.status.watching(format!("{what} · waiting for the pump"));
        return by_pump(link, cfg, scale, test, pump).await;
    }
    link.status.watching(format!("{what} · watching the scale"));
    by_scale(link, cfg, scale, test).await
}

/// Timed by the scale: after the start delay, until the weight stops rising
/// or the scale's timer is stopped.
async fn by_scale(
    link: &Link,
    cfg: &Config,
    scale: &mut Scale,
    test: bool,
) -> anyhow::Result<Option<Duration>> {
    if !cfg.start_delay.is_zero() && !countdown(link, cfg, scale, test).await? {
        aborted(cfg, scale).await;
        return Ok(None);
    }
    let start = Instant::now();
    scale.send(&protocol::TARE).await.context("tare")?;
    if cfg.drive_timer {
        tokio::time::sleep(COMMAND_GAP).await;
        scale.send(&protocol::RESET_TIMER).await.context("reset timer")?;
        tokio::time::sleep(COMMAND_GAP).await;
        scale.send(&protocol::START_TIMER).await.context("start timer")?;
    }
    // Whether the scale has confirmed its timer runs; retried if not.
    let mut timer_confirmed = !cfg.drive_timer;
    let mut start_tries = 1;
    let mut last_start = Instant::now();

    let mut rule = cfg.end_rule();
    rule.target_g = target_weight(link, cfg);
    let mut tracker = ShotTracker::new(rule);
    let give_up = MAX_SHOT.max(cfg.min_time + Duration::from_secs(60));
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
                        scale.send(&protocol::START_TIMER).await.context("start timer")?;
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
                if at > give_up {
                    tracing::info!("{TITLE}: no end of the shot after {} s, giving up",
                        give_up.as_secs());
                    link.status.connected(READY);
                    aborted(cfg, scale).await;
                    return Ok(None);
                }
            }
        }
    };
    finish(link, cfg, scale, test, end.time, end.grams, end.at_target).await;
    Ok(Some(end.time))
}

/// Timed by the pump sensor: from the pump starting to it stopping. The
/// scale is tared at once (the cup is on it) and weighs the cup when the last
/// drops have landed. If the sensor goes away before the pump starts, the
/// scale times the shot after all.
async fn by_pump(
    link: &Link,
    cfg: &Config,
    scale: &mut Scale,
    test: bool,
    pump: &mut watch::Receiver<Pump>,
) -> anyhow::Result<Option<Duration>> {
    let what = if test { "test" } else { "grinder is waiting" };
    let give_up = MAX_SHOT.max(cfg.min_time + Duration::from_secs(60));
    let waiting = Instant::now();
    let mut tick = tokio::time::interval(cfg.live_every);
    let mut grams = 0.0;
    scale.send(&protocol::TARE).await.context("tare")?;

    // 1. Until the pump starts (it may already run).
    while !pump.borrow_and_update().running {
        tokio::select! {
            changed = pump.changed() => {
                changed.context("pump sensor gone")?;
                if !pump.borrow().sensed {
                    tracing::info!("{TITLE}: pump sensor lost before the pump started; \
                        timing by the scale");
                    link.status.watching(format!("{what} · watching the scale"));
                    return by_scale(link, cfg, scale, test).await;
                }
            }
            reading = scale.next_reading() => {
                grams = reading?.grams;
                show(link, cfg, grams, Some(Duration::ZERO), true);
            }
            _ = tick.tick() => {
                show(link, cfg, grams, Some(Duration::ZERO), true);
                if !brew_still_on(link, test) {
                    tracing::info!("{TITLE}: brew ended before the pump started");
                    link.status.connected(READY);
                    aborted(cfg, scale).await;
                    return Ok(None);
                }
                if waiting.elapsed() > give_up {
                    tracing::info!("{TITLE}: the pump did not start, giving up");
                    link.status.connected(READY);
                    aborted(cfg, scale).await;
                    return Ok(None);
                }
            }
        }
    }
    let start = Instant::now();
    link.status.watching(format!("{what} · pump running"));
    if cfg.drive_timer {
        scale.send(&protocol::RESET_TIMER).await.context("reset timer")?;
        tokio::time::sleep(COMMAND_GAP).await;
        scale.send(&protocol::START_TIMER).await.context("start timer")?;
    }

    // Reaching the recipe weight ends the shot at that moment, pump running
    // or not.
    let mut watch = target_weight(link, cfg).map(TargetWatch::new);

    // 2. Until the pump stops. Its run time comes from the sensor's clock.
    let time = loop {
        tokio::select! {
            changed = pump.changed() => {
                changed.context("pump sensor gone")?;
                let p = *pump.borrow_and_update();
                if !p.sensed {
                    tracing::warn!("{TITLE}: pump sensor lost during the shot; \
                        the time ends here");
                    break start.elapsed();
                }
                if !p.running {
                    break p.last_run.unwrap_or_else(|| start.elapsed());
                }
            }
            reading = scale.next_reading() => {
                grams = reading?.grams;
                let at = start.elapsed();
                show(link, cfg, grams, Some(at), true);
                if let Some(target) = target_reached(&mut watch, at, grams) {
                    return at_target(link, cfg, scale, test, target).await;
                }
            }
            _ = tick.tick() => {
                show(link, cfg, grams, Some(start.elapsed()), true);
                if !brew_still_on(link, test) {
                    tracing::info!("{TITLE}: brew ended while the pump ran");
                    link.status.connected(READY);
                    aborted(cfg, scale).await;
                    return Ok(None);
                }
                if start.elapsed() > give_up {
                    tracing::info!("{TITLE}: the pump did not stop, giving up");
                    link.status.connected(READY);
                    aborted(cfg, scale).await;
                    return Ok(None);
                }
            }
        }
    };
    if cfg.drive_timer {
        let _ = scale.send(&protocol::STOP_TIMER).await;
    }

    // 3. The last drops: weigh once the weight stops rising.
    link.status.watching(format!("{what} · pump stopped, weighing"));
    let settling = Instant::now();
    let mut peak = grams;
    let mut last_rise = Instant::now();
    while last_rise.elapsed() < cfg.stable_for && settling.elapsed() < cfg.stable_for + SETTLE_EXTRA {
        tokio::select! {
            reading = scale.next_reading() => {
                grams = reading?.grams;
                if grams >= peak + 0.3 {
                    peak = grams;
                    last_rise = Instant::now();
                }
                if let Some(target) = target_reached(&mut watch, start.elapsed(), grams) {
                    return at_target(link, cfg, scale, test, target).await;
                }
            }
            _ = tick.tick() => {}
        }
        show(link, cfg, grams, Some(time), true);
        if !brew_still_on(link, test) {
            tracing::info!("{TITLE}: brew ended while weighing");
            link.status.connected(READY);
            aborted(cfg, scale).await;
            return Ok(None);
        }
    }
    finish(link, cfg, scale, test, time, grams, false).await;
    Ok(Some(time))
}

/// The moment and weight at which the cup reached the target, if it now has.
fn target_reached(
    watch: &mut Option<TargetWatch>,
    at: Duration,
    grams: f64,
) -> Option<(Duration, f64)> {
    let watch = watch.as_mut()?;
    let time = watch.reading(at, grams)?;
    Some((time, watch.target))
}

/// Pump mode: the cup reached the target, the shot ends there.
async fn at_target(
    link: &Link,
    cfg: &Config,
    scale: &mut Scale,
    test: bool,
    (time, grams): (Duration, f64),
) -> anyhow::Result<Option<Duration>> {
    if cfg.drive_timer {
        let _ = scale.send(&protocol::STOP_TIMER).await;
    }
    finish(link, cfg, scale, test, time, grams, true).await;
    Ok(Some(time))
}

/// The grinder's recipe weight, when stopping there is on and the grinder
/// sent one with its last grind.
fn target_weight(link: &Link, cfg: &Config) -> Option<f64> {
    if !cfg.stop_at_target {
        return None;
    }
    let target = link
        .server
        .with(|m, _| m.last_grind().and_then(|g| g.beverage_weight_g))
        .filter(|g| g.is_finite() && *g > 0.0);
    match target {
        Some(g) => tracing::info!("{TITLE}: the shot ends at the recipe weight, {g:.1} g"),
        None => tracing::info!("{TITLE}: no recipe weight from the grinder; the shot ends \
            when the flow stops"),
    }
    target
}

/// A shot that was seen to end: too short ones are dropped (four beeps, the
/// brew keeps running for entering it by hand), the rest reported (two
/// beeps). Reaching the target weight always counts, however quick: a fast
/// shot is what the grinder must hear about. Shows the result.
async fn finish(
    link: &Link,
    cfg: &Config,
    scale: &mut Scale,
    test: bool,
    time: Duration,
    grams: f64,
    at_target: bool,
) {
    let secs = time.as_secs_f64();
    show(link, cfg, grams, Some(time), false);
    if at_target {
        tracing::info!("{TITLE}: recipe weight {grams:.1} g reached after {secs:.1} s");
    }
    if time < cfg.min_time && !at_target {
        let min = cfg.min_time.as_secs_f64();
        tracing::info!("{TITLE}: shot ended after {secs:.1} s, below the minimum \
            {min:.0} s: not reported");
        link.status
            .connected(format!("shot ended after {secs:.1} s, under {min:.0} s: not reported"));
        aborted(cfg, scale).await;
        return;
    }
    if cfg.drive_timer {
        // Only for the display; the shot is measured already.
        let _ = scale.send(&protocol::STOP_TIMER).await;
    }

    let grams = tenth(grams);
    let source = if at_target {
        "recipe weight reached on the scale"
    } else {
        "weighed by the scale"
    };
    let outcome = if test {
        link.finish_manual(time, grams, &format!("{source} (test)"))
    } else {
        link.report(time, Some((grams, source.to_owned())))
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
        let _ = scale.send(&protocol::STOP_TIMER).await;
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
        let _ = scale.send(&protocol::BEEP_TWICE).await;
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
                    if n.uuid == protocol::STATUS
                        && let Some(r) = protocol::parse(&n.value)
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
/// The Bluetooth stack, opened once: the manager holds the D-Bus connection.
struct Bluetooth {
    // Kept alive with the adapter, which works through its connection.
    _manager: Manager,
    adapter: Adapter,
}

impl Bluetooth {
    async fn open() -> anyhow::Result<Self> {
        let manager = Manager::new().await.context("Bluetooth not available")?;
        let adapter = manager
            .adapters()
            .await?
            .into_iter()
            .next()
            .context("no Bluetooth adapter")?;
        Ok(Self {
            _manager: manager,
            adapter,
        })
    }
}

async fn connect(adapter: &Adapter, cfg: &Config) -> anyhow::Result<Scale> {
    start_scan(adapter).await?;
    // Whatever happens while searching, the scan is stopped again: a scan
    // left running makes the next start fail with "operation already in
    // progress".
    let found = search(adapter, cfg).await;
    let _ = adapter.stop_scan().await;
    let (peripheral, name) = found?;

    peripheral.connect().await.context("connecting to the scale")?;
    match set_up(peripheral.clone(), name).await {
        Ok(scale) => Ok(scale),
        Err(e) => {
            // Half set up: let go of it so the next attempt starts clean.
            let _ = peripheral.disconnect().await;
            Err(e)
        }
    }
}

/// Starts a scan; one still running (from an earlier search, or another
/// program using Bluetooth) is fine too.
async fn start_scan(adapter: &Adapter) -> anyhow::Result<()> {
    // No service filter: not every scale advertises FFF0.
    match adapter.start_scan(ScanFilter::default()).await {
        Ok(()) => Ok(()),
        Err(e) if e.to_string().to_lowercase().contains("in progress") => {
            tracing::debug!("{TITLE}: scan already running ({e})");
            Ok(())
        }
        Err(e) => Err(e).context("starting a Bluetooth scan"),
    }
}

/// Looks for the scale in the running scan for up to `cfg.scan_for`.
async fn search(adapter: &Adapter, cfg: &Config) -> anyhow::Result<(Peripheral, String)> {
    let deadline = Instant::now() + cfg.scan_for;
    loop {
        if let Some(found) = find(adapter, cfg).await? {
            return Ok(found);
        }
        if Instant::now() >= deadline {
            bail!("scale not found (switched on and in range?)");
        }
        tokio::time::sleep(SCAN_POLL).await;
    }
}

/// Subscribes to a connected scale's weight and sets grams.
async fn set_up(peripheral: Peripheral, name: String) -> anyhow::Result<Scale> {
    peripheral.discover_services().await?;
    let characteristics = peripheral.characteristics();
    let pick = |uuid: Uuid| characteristics.iter().find(|c| c.uuid == uuid).cloned();
    let status = pick(protocol::STATUS).context("scale has no FFF1 characteristic")?;
    let command = pick(protocol::COMMAND).context("scale has no FFF2 characteristic")?;
    peripheral.subscribe(&status).await?;
    let notifications = peripheral.notifications().await?;

    let mut scale = Scale {
        peripheral,
        command,
        notifications,
        name,
    };
    scale.send(&protocol::UNIT_GRAMS).await.context("set grams")?;
    Ok(scale)
}

/// The configured scale among what the scan has seen, with a display name.
async fn find(adapter: &Adapter, cfg: &Config) -> anyhow::Result<Option<(Peripheral, String)>> {
    for p in adapter.peripherals().await? {
        // A device can vanish between listing and asking (BlueZ drops stale
        // ones during a scan): skip it rather than fail the search.
        let Ok(Some(props)) = p.properties().await else {
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
