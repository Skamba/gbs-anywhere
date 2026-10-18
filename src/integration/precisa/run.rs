//! The task: keep the scale connected, and after each knob press tare it,
//! start its timer, watch the cup fill and report time and weight.

use std::time::{Duration, Instant};

use anyhow::{Context, bail};

use super::TITLE;
use super::config::Config;
use super::protocol::{self, Reading};
use super::shot::{End, ShotTracker};
use crate::integration::bluetooth::{Bluetooth, Device, NotFound};
use crate::integration::{Backoff, BoxFuture, Detail, Integration, Link, ReportOutcome, tenths};

/// Without a notification for this long, check the scale is still connected.
const SILENCE: Duration = Duration::from_secs(5);
/// How often a running shot is checked and its status line updated.
const TICK: Duration = Duration::from_millis(250);
/// Give up on a shot whose end is never seen: after this, or a minute past
/// the minimum time if that is later.
const MAX_SHOT: Duration = Duration::from_secs(120);
/// Pause between commands: sent back to back, the scale drops some.
const COMMAND_GAP: Duration = Duration::from_millis(200);
/// If the scale does not report its timer running this long after a start
/// command, the command is sent again ...
const TIMER_RETRY: Duration = Duration::from_millis(800);
/// ... up to this many times in all.
const TIMER_TRIES: u32 = 3;
/// Failed attempts in a row that are the adapter's fault (not just no scale
/// in sight) before Bluetooth is opened again.
const ADAPTER_ERRORS: u32 = 3;
/// Between two double beeps, so four beeps are heard as four.
const BEEP_GAP: Duration = Duration::from_millis(700);
/// Wait before trying again while Bluetooth itself cannot be opened.
const NO_BLUETOOTH: Duration = Duration::from_secs(10);

/// A reported shot: its time and the weight in the cup.
type Shot = (Duration, f64);

/// The status lines, each with a key the app can translate.
fn searching() -> Detail {
    Detail::keyed("precisa.searching", "looking for the scale")
}

/// `last shot: 25.1 s, 38.0 g`.
fn idle_line(last: Option<Shot>) -> Detail {
    let Some((time, grams)) = last else {
        return Detail::keyed("precisa.idle", "ready, waiting for a knob press");
    };
    let (time, grams) = (tenths(time.as_secs_f64()), tenths(grams));
    Detail::keyed(
        "precisa.last_shot",
        format!("last shot: {time:.1} s, {grams:.1} g"),
    )
    .var("time_s", time)
    .var("weight_g", grams)
}

fn countdown_line(delay: Duration) -> Detail {
    let secs = delay.as_secs_f64();
    Detail::keyed(
        "precisa.countdown",
        format!("grinder is waiting · start the machine, measuring in {secs:.0} s"),
    )
    .var("delay_s", secs.round())
}

fn measuring(time: Duration, grams: f64) -> Detail {
    let (time, grams) = (tenths(time.as_secs_f64()), tenths(grams));
    Detail::keyed(
        "precisa.measuring",
        format!("measuring · {time:.1} s, {grams:.1} g"),
    )
    .var("time_s", time)
    .var("weight_g", grams)
}

fn too_short(time: Duration, min: Duration) -> Detail {
    let (time, min) = (tenths(time.as_secs_f64()), min.as_secs_f64().round());
    Detail::keyed(
        "precisa.too_short",
        format!("shot ended after {time:.1} s, under {min:.0} s: not reported"),
    )
    .var("time_s", time)
    .var("min_s", min)
}

/// The integration. See the module docs.
pub struct Precisa {
    pub cfg: Config,
}

impl Integration for Precisa {
    fn run(self: Box<Self>, link: Link) -> BoxFuture {
        Box::pin(run(link, self.cfg))
    }
}

async fn run(link: Link, cfg: Config) {
    // A switched-off scale is the normal case: keep looking at the
    // configured pace, without growing delays, so it reconnects soon after
    // switching on.
    let mut backoff = Backoff::new(cfg.reconnect_every, cfg.reconnect_every);
    let mut adapter_errors = 0;
    let mut last = None;
    loop {
        let bt = match Bluetooth::shared().await {
            Ok(bt) => bt,
            Err(e) => {
                tracing::warn!("{TITLE}: {e:#}");
                link.status.error(format!("{e:#}"));
                // Without Bluetooth nothing changes quickly.
                tokio::time::sleep(NO_BLUETOOTH).await;
                continue;
            }
        };
        link.status.starting(searching());
        match connect(&bt, &cfg).await {
            Ok(mut scale) => {
                backoff.reset();
                adapter_errors = 0;
                tracing::info!("{TITLE}: connected to {}", scale.device.label());
                link.status.subject(scale.device.label());
                link.status.connected(idle_line(last));
                let served = serve(&link, &cfg, &mut scale, &mut last).await;
                scale.device.disconnect().await;
                match served {
                    // The server is gone.
                    Ok(()) => return,
                    Err(e) => {
                        tracing::warn!("{TITLE}: {e:#}");
                        link.status.error(format!("{e:#}"));
                    }
                }
            }
            Err(e) if e.is::<NotFound>() => {
                // Switched off or away: not worth a warning.
                adapter_errors = 0;
                tracing::debug!("{TITLE}: {e:#}");
            }
            Err(e) => {
                adapter_errors += 1;
                tracing::warn!("{TITLE}: {e:#}");
                link.status.error(format!("{e:#}"));
                if adapter_errors >= ADAPTER_ERRORS {
                    adapter_errors = 0;
                    bt.close().await;
                }
            }
        }
        drop(bt);
        backoff.wait().await;
    }
}

/// Runs while the scale stays connected. `Ok` once the server is gone.
async fn serve(
    link: &Link,
    cfg: &Config,
    scale: &mut Scale,
    last: &mut Option<Shot>,
) -> anyhow::Result<()> {
    // Subscribed per connection: knob presses while the scale was away must
    // not start a shot now.
    let mut brews = link.grinder_brews();
    loop {
        tokio::select! {
            pressed = brews.next() => {
                if !pressed {
                    return Ok(());
                }
                // Not for a press whose brew is already over.
                if link.grinder_waiting() {
                    shot(link, cfg, scale, last).await?;
                }
            }
            // Idle: read on, which also notices the scale going away.
            reading = scale.next_reading() => {
                reading?;
            }
        }
    }
}

/// One shot, from the knob press to its end. Leaves the status line saying
/// how it went.
async fn shot(
    link: &Link,
    cfg: &Config,
    scale: &mut Scale,
    last: &mut Option<Shot>,
) -> anyhow::Result<()> {
    tracing::info!("{TITLE}: grinder is waiting, watching the scale");
    if !cfg.start_delay.is_zero() && !countdown(link, cfg, scale).await? {
        link.status.connected(idle_line(*last));
        aborted(cfg, scale).await;
        return Ok(());
    }
    let start = Instant::now();
    scale.send(&protocol::TARE).await.context("cannot tare")?;
    if cfg.drive_timer {
        tokio::time::sleep(COMMAND_GAP).await;
        scale
            .send(&protocol::RESET_TIMER)
            .await
            .context("cannot reset the timer")?;
        tokio::time::sleep(COMMAND_GAP).await;
        scale
            .send(&protocol::START_TIMER)
            .await
            .context("cannot start the timer")?;
    }
    // Whether the scale has confirmed its timer runs; started again if not.
    let mut timer_confirmed = !cfg.drive_timer;
    let mut start_tries = 1;
    let mut last_start = Instant::now();

    let mut rule = cfg.end_rule();
    rule.target_g = target_weight(link, cfg);
    let mut tracker = ShotTracker::new(rule);
    let give_up = MAX_SHOT.max(cfg.min_time + Duration::from_secs(60));
    let mut tick = tokio::time::interval(TICK);
    let mut grams = 0.0;
    let end: End = loop {
        tokio::select! {
            reading = scale.next_reading() => {
                let reading = reading?;
                grams = reading.grams;
                timer_confirmed |= reading.timer_running;
                if let Some(end) = tracker.reading(start.elapsed(), reading) {
                    break end;
                }
            }
            _ = tick.tick() => {
                let at = start.elapsed();
                // The clock runs on while the scale is quiet.
                link.status.watching(measuring(at, grams));
                if !timer_confirmed && last_start.elapsed() >= TIMER_RETRY {
                    if start_tries < TIMER_TRIES {
                        start_tries += 1;
                        last_start = Instant::now();
                        tracing::info!(
                            "{TITLE}: the scale's timer is not running, starting it again \
                             (try {start_tries} of {TIMER_TRIES})"
                        );
                        scale
                            .send(&protocol::START_TIMER)
                            .await
                            .context("cannot start the timer")?;
                    } else {
                        tracing::warn!(
                            "{TITLE}: the scale's timer does not start; measuring the shot \
                             without it"
                        );
                        timer_confirmed = true;
                    }
                }
                if let Some(end) = tracker.tick(at) {
                    break end;
                }
                // Typed on the phone, aborted or timed out: nothing left to
                // measure for.
                if !link.grinder_waiting() {
                    tracing::info!("{TITLE}: the brew ended before the scale saw the shot end");
                    link.status.connected(idle_line(*last));
                    aborted(cfg, scale).await;
                    return Ok(());
                }
                if at > give_up {
                    tracing::info!(
                        "{TITLE}: no end of the shot after {} s, giving up",
                        give_up.as_secs()
                    );
                    link.status.connected(idle_line(*last));
                    aborted(cfg, scale).await;
                    return Ok(());
                }
            }
        }
    };
    finish(link, cfg, scale, end, last).await;
    Ok(())
}

/// The grinder's recipe weight, when stopping there is on and the grinder
/// sent one.
fn target_weight(link: &Link, cfg: &Config) -> Option<f64> {
    if !cfg.stop_at_target {
        return None;
    }
    let target = link
        .server
        .with(|m, _| m.recipe_weight_g())
        .filter(|g| g.is_finite() && *g > 0.0);
    match target {
        Some(g) => tracing::info!("{TITLE}: the shot ends at the recipe weight, {g:.1} g"),
        None => tracing::info!(
            "{TITLE}: no recipe weight from the grinder; the shot ends when the flow stops"
        ),
    }
    target
}

/// A shot that was seen to end: too short ones are dropped (four beeps; the
/// grinder keeps waiting, so it can still be typed in), the rest reported
/// (two beeps). Reaching the target weight always counts, however quick: a
/// fast shot is what the grinder must hear about.
async fn finish(link: &Link, cfg: &Config, scale: &mut Scale, end: End, last: &mut Option<Shot>) {
    let secs = end.time.as_secs_f64();
    if end.at_target {
        tracing::info!(
            "{TITLE}: recipe weight {:.1} g reached after {secs:.1} s",
            end.grams
        );
    }
    if end.time < cfg.min_time && !end.at_target {
        tracing::info!(
            "{TITLE}: shot ended after {secs:.1} s, below the minimum {:.0} s: not reported",
            cfg.min_time.as_secs_f64()
        );
        link.status.connected(too_short(end.time, cfg.min_time));
        aborted(cfg, scale).await;
        return;
    }
    let source = if end.at_target {
        "recipe weight reached on the scale"
    } else {
        "weighed by the scale"
    };
    let outcome = link.report(end.time, Some((tenths(end.grams), source.to_owned())));
    let reported = match outcome {
        ReportOutcome::Reported { grams, .. } => {
            *last = Some((end.time, grams));
            true
        }
        ReportOutcome::NotWaiting | ReportOutcome::Refused(_) => false,
    };
    link.status.connected(idle_line(*last));
    if cfg.drive_timer {
        // Only for the display; the shot is measured already.
        let _ = scale.send(&protocol::STOP_TIMER).await;
        tokio::time::sleep(COMMAND_GAP).await;
    }
    if cfg.beep {
        beep(scale, if reported { 2 } else { 4 }).await;
    }
}

/// Waits `cfg.start_delay` after the knob press, so there is time to start
/// the machine by hand; the shot is timed from the end of it. `false` if
/// the grinder stopped waiting meanwhile.
async fn countdown(link: &Link, cfg: &Config, scale: &mut Scale) -> anyhow::Result<bool> {
    tracing::info!(
        "{TITLE}: measuring starts in {:.1} s",
        cfg.start_delay.as_secs_f64()
    );
    link.status.watching(countdown_line(cfg.start_delay));
    let until = Instant::now() + cfg.start_delay;
    let mut tick = tokio::time::interval(TICK);
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(true);
        }
        if !link.grinder_waiting() {
            tracing::info!("{TITLE}: the brew ended before measuring started");
            return Ok(false);
        }
        tokio::select! {
            reading = scale.next_reading() => {
                reading?;
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
        tokio::time::sleep(COMMAND_GAP).await;
    }
    if cfg.beep {
        beep(scale, 4).await;
    }
}

/// Beeps `times` times (2 or 4): the scale only knows a double beep.
/// Failures are left out, the beeps are only a signal.
async fn beep(scale: &mut Scale, times: u32) {
    for i in 0..times / 2 {
        if i > 0 {
            tokio::time::sleep(BEEP_GAP).await;
        }
        let _ = scale.send(&protocol::BEEP_TWICE).await;
    }
}

// ---------------------------------------------------------------------------
// The scale over Bluetooth
// ---------------------------------------------------------------------------

/// A connected Precisa.
struct Scale {
    device: Device,
}

impl Scale {
    /// Commands are written without response, as the scale requires.
    async fn send(&mut self, command: &[u8]) -> anyhow::Result<()> {
        self.device.write(protocol::COMMAND, command, false).await
    }

    /// The next weight reading. Waits through silence while the scale stays
    /// connected (it may only send on change); fails once it is gone.
    /// Cancel-safe.
    async fn next_reading(&mut self) -> anyhow::Result<Reading> {
        loop {
            let (uuid, value) = self.device.next_notification(SILENCE).await?;
            if uuid == protocol::STATUS
                && let Some(r) = protocol::parse(&value)
            {
                return Ok(r);
            }
        }
    }
}

/// Finds the scale, connects, subscribes to its weight and sets grams.
async fn connect(bt: &Bluetooth, cfg: &Config) -> anyhow::Result<Scale> {
    let seen = bt
        .find(cfg.scan_for, |s| match &cfg.address {
            Some(address) => s.has_address(address),
            None => s.name.starts_with(&cfg.name_prefix),
        })
        .await
        .context("scale")?;
    let mut scale = Scale {
        device: bt.connect(seen).await?,
    };
    match set_up(&mut scale).await {
        Ok(()) => Ok(scale),
        Err(e) => {
            // Half set up: let go of it so the next attempt starts clean.
            let label = scale.device.label().to_owned();
            scale.device.disconnect().await;
            Err(e.context(format!("cannot set up {label}")))
        }
    }
}

async fn set_up(scale: &mut Scale) -> anyhow::Result<()> {
    if !scale.device.has(protocol::STATUS) || !scale.device.has(protocol::COMMAND) {
        bail!("it has no weight service (FFF0); is it a Precisa?");
    }
    scale.device.subscribe(protocol::STATUS).await?;
    scale
        .send(&protocol::UNIT_GRAMS)
        .await
        .context("cannot switch it to grams")
}
