//! The task: after each knob press, connect to the scale, watch its
//! readings for the shot, and let go again. A connected app keeps the scale
//! from switching itself off, so it is only connected while the grinder
//! waits.

use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::TITLE;
use super::config::Config;
use super::shot::{Detector, Progress, Reading, Shot};
use crate::integration::{Backoff, BoxFuture, GrinderBrews, Integration, Link};

const MIN_BACKOFF: Duration = Duration::from_secs(2);
/// Short, so a scale switched on after the knob press is found in time.
const MAX_BACKOFF: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The scale sends 10 readings a second; this much silence means it is gone.
const SILENCE: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

const SETTLING: &str = "grinder is waiting · waiting for a steady weight";
const READY: &str = "grinder is waiting · watching for the first drops";
/// When the scale could not be reached at startup.
const AWAY: &str = "scale is off or out of reach; looked for at each knob press";

/// The integration. See the module docs.
pub struct HalfDecent {
    pub cfg: Config,
}

impl Integration for HalfDecent {
    fn run(self: Box<Self>, link: Link) -> BoxFuture {
        Box::pin(run(link, self.cfg))
    }
}

/// A message from the scale, as far as this integration cares.
#[derive(Debug, PartialEq)]
pub enum FromScale {
    /// A weight reading (these have no `type`).
    Reading(Reading),
    /// The firmware version, from the answer to `status`.
    Firmware(String),
    Other,
}

#[derive(Deserialize)]
struct Wire {
    #[serde(rename = "type")]
    kind: Option<String>,
    grams: Option<f64>,
    ms: Option<u64>,
    firmware_version: Option<String>,
}

pub fn parse(text: &str) -> FromScale {
    let Ok(w) = serde_json::from_str::<Wire>(text) else {
        return FromScale::Other;
    };
    match (w.kind, w.grams, w.ms, w.firmware_version) {
        (None, Some(grams), Some(ms), _) if grams.is_finite() => {
            FromScale::Reading(Reading { ms, grams })
        }
        (Some(_), _, _, Some(fw)) => {
            FromScale::Firmware(fw.trim_start_matches("FW:").trim().to_owned())
        }
        _ => FromScale::Other,
    }
}

/// `last shot: 25.1 s, 38.0 g`.
fn idle_line(last: Option<Shot>) -> String {
    match last {
        Some(s) => format!("last shot: {:.1} s, {:.1} g", s.time.as_secs_f64(), s.grams),
        None => "ready for the next shot".to_owned(),
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The grinder's recipe weight, which tells a pause from the end of a shot.
fn recipe_weight(link: &Link) -> Option<f64> {
    link.server.with(|m, _| m.recipe_weight_g())
}

async fn run(link: Link, cfg: Config) {
    let mut last = None;
    link.status.subject(&cfg.host);
    // Subscribe first, so no knob press slips by.
    let mut brews = link.grinder_brews();
    if !link.grinder_waiting() {
        link.status.starting(format!("connecting to {}", cfg.host));
        match first_look(&link, &cfg).await {
            Ok(()) => link.status.connected(idle_line(last)),
            Err(e) => {
                tracing::info!("{TITLE}: {e:#}");
                link.status.connected(AWAY);
            }
        }
    }
    loop {
        // A press seen now, not one already waiting: the cup is on and the
        // shot has not started, so the scale can be tared.
        let fresh = !link.grinder_waiting();
        if fresh {
            if !brews.next().await {
                return;
            }
            if !link.grinder_waiting() {
                // A press from a brew that is already over.
                continue;
            }
        }
        if !brew(&link, &cfg, &mut brews, fresh, &mut last).await {
            return;
        }
    }
}

/// Connects once, to check the address and show the firmware version, then
/// lets go so the scale can still switch itself off.
async fn first_look(link: &Link, cfg: &Config) -> anyhow::Result<()> {
    let mut ws = connect(&cfg.host).await?;
    let firmware = tokio::time::timeout(SILENCE, async {
        while let Some(Ok(msg)) = ws.next().await {
            if let Message::Text(t) = msg
                && let FromScale::Firmware(fw) = parse(&t)
            {
                return Some(fw);
            }
        }
        None
    })
    .await;
    close(ws).await;
    let Ok(Some(fw)) = firmware else {
        bail!("the scale at {} did not answer", cfg.host);
    };
    tracing::info!("{TITLE}: found the scale at {}, firmware {fw}", cfg.host);
    link.status.subject(format!("{} · v{fw}", cfg.host));
    Ok(())
}

/// Watches one brew: connects, retrying while the grinder waits, and
/// lets go of the scale once the shot is reported or the grinder stops
/// waiting. `false` once the server is gone.
async fn brew(
    link: &Link,
    cfg: &Config,
    brews: &mut GrinderBrews,
    mut tare: bool,
    last: &mut Option<Shot>,
) -> bool {
    let mut backoff = Backoff::new(MIN_BACKOFF, MAX_BACKOFF);
    loop {
        link.status.starting(format!("connecting to {}", cfg.host));
        let result = match connect(&cfg.host).await {
            Ok(ws) => watch(link, cfg, ws, brews, tare, last).await,
            Err(e) => Err(e),
        };
        match result {
            Ok(true) => {
                link.status.connected(idle_line(*last));
                return true;
            }
            // The server is shutting down.
            Ok(false) => return false,
            Err(e) => {
                tracing::warn!("{TITLE}: {e:#}");
                link.status.error(format!("{e:#}"));
            }
        }
        // Once the shot may be running, a tare would hide coffee.
        tare = false;
        tracing::info!("{TITLE}: retrying in {} s", backoff.peek().as_secs());
        backoff.wait().await;
        if !link.grinder_waiting() {
            link.status.connected(idle_line(*last));
            return true;
        }
    }
}

async fn connect(host: &str) -> anyhow::Result<Socket> {
    let url = format!("ws://{host}/snapshot");
    let (mut ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url))
        .await
        .map_err(|_| anyhow!("no answer"))
        .and_then(|r| r.map_err(anyhow::Error::from))
        .with_context(|| {
            format!(
                "could not reach the scale at {host}; is it on, with WiFi on in its \
                 Setup > Connections menu?"
            )
        })?;
    ws.send(Message::text(r#"{"rate_hz":10}"#))
        .await
        .context("could not ask the scale for 10 readings a second")?;
    ws.send(Message::text("status"))
        .await
        .context("could not ask the scale for its status")?;
    Ok(ws)
}

/// Says goodbye to the scale. A scale that is already gone is fine.
async fn close(mut ws: Socket) {
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, ws.close(None)).await;
}

async fn send_tare(ws: &mut Socket) -> anyhow::Result<()> {
    tracing::info!("{TITLE}: grinder is waiting, taring the scale");
    ws.send(Message::text(r#"{"command":"tare"}"#))
        .await
        .context("could not tare the scale")
}

/// Reads the scale for one brew, tared first if `tare`. `Ok(true)` once the
/// shot is reported or the grinder stops waiting, `Ok(false)` when the
/// server is gone; either way the connection is closed. An error when the
/// connection fails.
async fn watch(
    link: &Link,
    cfg: &Config,
    mut ws: Socket,
    brews: &mut GrinderBrews,
    tare: bool,
    last: &mut Option<Shot>,
) -> anyhow::Result<bool> {
    let mut detector = if tare {
        send_tare(&mut ws).await?;
        Detector::after_tare()
    } else {
        // Pressed while the scale was away: the shot may already be running.
        Detector::new()
    }
    .expecting(recipe_weight(link));
    link.status.watching(SETTLING);
    let ended = loop {
        let text = tokio::select! {
            pressed = brews.next() => {
                if !pressed {
                    break false;
                }
                // Pressed again: a new brew.
                send_tare(&mut ws).await?;
                detector = Detector::after_tare().expecting(recipe_weight(link));
                link.status.watching(SETTLING);
                continue;
            }
            msg = tokio::time::timeout(SILENCE, ws.next()) => match msg {
                Err(_) => bail!("the scale stopped sending readings"),
                Ok(None) | Ok(Some(Ok(Message::Close(_)))) => {
                    bail!("the scale closed the connection")
                }
                Ok(Some(Err(e))) => {
                    return Err(e).context("lost the connection to the scale");
                }
                Ok(Some(Ok(Message::Text(t)))) => t,
                Ok(Some(Ok(_))) => continue,
            },
        };
        let r = match parse(&text) {
            FromScale::Reading(r) => r,
            FromScale::Firmware(fw) => {
                link.status.subject(format!("{} · v{fw}", cfg.host));
                continue;
            }
            FromScale::Other => continue,
        };
        if !link.grinder_waiting() {
            // Typed on the phone, aborted or timed out.
            break true;
        }
        match detector.push(r) {
            Progress::Settling => link.status.watching(SETTLING),
            Progress::Ready => link.status.watching(READY),
            Progress::Flowing { grams } => {
                link.status.watching(format!("flowing · {grams:.1} g"));
            }
            Progress::Done(shot) => {
                link.report(
                    shot.time,
                    Some((shot.grams, "weighed by the scale".to_owned())),
                );
                *last = Some(shot);
                break true;
            }
        }
    };
    close(ws).await;
    Ok(ended)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::Instant;

    use futures_util::{SinkExt, StreamExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::tungstenite::Message;

    use super::*;
    use crate::integration::half_decent::shot::synth::{espresso, readings};
    use crate::integration::{Health, Status};
    use crate::protocol::MachineConfig;
    use crate::server::Server;

    #[test]
    fn parses_what_the_scale_sends() {
        assert_eq!(
            parse(r#"{"grams":25.66,"ms":12345}"#),
            FromScale::Reading(Reading {
                ms: 12345,
                grams: 25.66
            })
        );
        assert_eq!(
            parse(
                r#"{"type":"status","status":"ok","firmware_version":"FW: 3.1.14","grams":0.00,"ms":68898}"#
            ),
            FromScale::Firmware("3.1.14".into())
        );
        assert_eq!(
            parse(r#"{"type":"rate","status":"ok","hz":10}"#),
            FromScale::Other
        );
        assert_eq!(parse("not json"), FromScale::Other);
        assert_eq!(parse(r#"{"grams":1.0}"#), FromScale::Other);
    }

    fn reading(ms: u64, grams: f64) -> Message {
        Message::text(format!(r#"{{"grams":{grams:.2},"ms":{ms}}}"#))
    }

    /// What the fake scale saw, and when to pour.
    #[derive(Default)]
    struct Fake {
        /// Connections accepted so far, and how many are open now.
        connects: AtomicU32,
        open: AtomicU32,
        /// Whether a tare came.
        tared: AtomicBool,
        /// Set to pour the shot on the open connection.
        go: AtomicBool,
    }

    /// A scale on a local port with a 150 g cup on it. Each connection gets
    /// the firmware version, then readings: 150 g until a tare, 0 g after.
    /// Once `go` is set: an 18 s shot at 2 g/s all at once, then the cup's
    /// final weight.
    async fn fake_scale(listener: TcpListener, fake: Arc<Fake>) {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            fake.connects.fetch_add(1, Ordering::SeqCst);
            fake.open.fetch_add(1, Ordering::SeqCst);
            let fake = fake.clone();
            tokio::spawn(async move {
                serve(tcp, &fake).await;
                fake.open.fetch_sub(1, Ordering::SeqCst);
            });
        }
    }

    async fn serve(tcp: TcpStream, fake: &Fake) {
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let first = ws.next().await.unwrap().unwrap();
        assert_eq!(first.to_text().unwrap(), r#"{"rate_hz":10}"#);
        let status = r#"{"type":"status","status":"ok","firmware_version":"FW: 3.1.14"}"#;
        if ws.send(Message::text(status)).await.is_err() {
            return;
        }
        let mut ms = 60_000;
        let mut poured = None;
        loop {
            tokio::select! {
                msg = ws.next() => match msg {
                    Some(Ok(Message::Text(t))) => {
                        if t.as_str() == r#"{"command":"tare"}"# {
                            fake.tared.store(true, Ordering::SeqCst);
                        }
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                    Some(Ok(_)) => {}
                },
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if fake.go.load(Ordering::SeqCst) && poured.is_none() {
                        for r in readings(ms + 100, 32.0, |t| espresso(t, 18.0, 2.0)) {
                            if ws.send(reading(r.ms, r.grams)).await.is_err() {
                                return;
                            }
                            (ms, poured) = (r.ms, Some(r.grams));
                        }
                        continue;
                    }
                    ms += 100;
                    let g = match poured {
                        Some(g) => g,
                        None if fake.tared.load(Ordering::SeqCst) => 0.0,
                        None => 150.0,
                    };
                    if ws.send(reading(ms, g)).await.is_err() {
                        return;
                    }
                }
            }
        }
    }

    async fn wait_for(what: &str, cond: impl Fn() -> bool) {
        for _ in 0..250 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for {what}");
    }

    fn link() -> Link {
        Link {
            server: Server::new(MachineConfig::default()),
            status: Status::new("half_decent", "half_decent", TITLE),
        }
    }

    fn knob_press(link: &Link) {
        let later = Instant::now() + Duration::from_secs(30);
        link.server.with(|m, _| m.on_start_request(later, Some(9)));
    }

    /// A fake scale and the integration reading it, idle and disconnected
    /// after its first look at the scale.
    async fn start() -> (Link, String, Arc<Fake>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let fake = Arc::new(Fake::default());
        tokio::spawn(fake_scale(listener, fake.clone()));
        let l = link();
        tokio::spawn(run(l.clone(), Config { host: host.clone() }));
        wait_for("connected", || {
            l.status.snapshot().health == Health::Connected
        })
        .await;
        wait_for("the first look to end", || {
            fake.connects.load(Ordering::SeqCst) == 1 && fake.open.load(Ordering::SeqCst) == 0
        })
        .await;
        (l, host, fake)
    }

    #[tokio::test]
    async fn reports_a_shot_from_a_fake_scale() {
        let (l, host, fake) = start().await;
        assert!(
            !fake.tared.load(Ordering::SeqCst),
            "no tare before the knob press"
        );
        // Short enough to fit the card at phone width.
        assert_eq!(l.status.snapshot().subject, format!("{host} · v3.1.14"));
        knob_press(&l);
        wait_for("the tare", || fake.tared.load(Ordering::SeqCst)).await;
        fake.go.store(true, Ordering::SeqCst);
        wait_for("the report", || l.status.snapshot().reports == 1).await;

        let shot = l.status.snapshot().last_report.unwrap();
        assert!((i64::from(shot.time_ms) - 18_000).abs() <= 400, "{shot:?}");
        assert!((shot.volume_ml - 36.3).abs() <= 0.4, "{shot:?}");
        let snap = l.status.snapshot();
        assert_eq!(snap.health, Health::Connected);
        assert!(snap.detail.starts_with("last shot: "), "{}", snap.detail);
    }

    #[tokio::test]
    async fn the_scale_is_only_connected_while_the_grinder_waits() {
        // Connected apps keep the scale from switching itself off.
        let (l, _, fake) = start().await;
        knob_press(&l);
        wait_for("the tare", || fake.tared.load(Ordering::SeqCst)).await;
        assert_eq!(fake.open.load(Ordering::SeqCst), 1);
        fake.go.store(true, Ordering::SeqCst);
        wait_for("the report", || l.status.snapshot().reports == 1).await;
        wait_for("the connection to close", || {
            fake.open.load(Ordering::SeqCst) == 0
        })
        .await;
        assert_eq!(fake.connects.load(Ordering::SeqCst), 2);
        assert_eq!(l.status.snapshot().health, Health::Connected);
    }

    #[tokio::test]
    async fn a_knob_press_before_the_scale_connects_is_watched() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let fake = Arc::new(Fake::default());
        let l = link();
        // The knob is pressed while the scale is still asleep.
        knob_press(&l);
        tokio::spawn(fake_scale(listener, fake.clone()));
        tokio::spawn(run(l.clone(), Config { host }));

        wait_for("watching", || {
            l.status.snapshot().health == Health::Watching
        })
        .await;
        fake.go.store(true, Ordering::SeqCst);
        wait_for("the report", || l.status.snapshot().reports == 1).await;
        // Coffee may already be in the cup: no tare this late.
        assert!(!fake.tared.load(Ordering::SeqCst));
        let shot = l.status.snapshot().last_report.unwrap();
        assert!((i64::from(shot.time_ms) - 18_000).abs() <= 400, "{shot:?}");
    }

    #[tokio::test]
    async fn typed_shot_stops_the_watch() {
        let (l, _, fake) = start().await;
        knob_press(&l);
        wait_for("the tare", || fake.tared.load(Ordering::SeqCst)).await;
        // Someone typed the shot on the phone first.
        l.report(Duration::from_secs(25), Some((38.0, "typed".into())));
        wait_for("the connection to close", || {
            fake.open.load(Ordering::SeqCst) == 0
        })
        .await;
        fake.go.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(500)).await;
        let snap = l.status.snapshot();
        assert_eq!(snap.reports, 1, "only the typed one");
        assert_eq!(snap.health, Health::Connected);
    }

    #[tokio::test]
    async fn a_closed_connection_is_an_error_and_retried() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let accepts = Arc::new(AtomicU32::new(0));
        let count = accepts.clone();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                count.fetch_add(1, Ordering::SeqCst);
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                // Connected and set up (rate, status), then the scale goes.
                for _ in 0..2 {
                    let _ = ws.next().await;
                }
                let _ = ws.close(None).await;
            }
        });
        let l = link();
        knob_press(&l);
        tokio::spawn(run(l.clone(), Config { host: host.clone() }));
        wait_for("an error", || l.status.snapshot().health == Health::Error).await;
        let err = l.status.snapshot().last_error.unwrap();
        assert!(err.contains("closed the connection"), "{err}");
        // MIN_BACKOFF later it tries again.
        wait_for("a second attempt", || accepts.load(Ordering::SeqCst) >= 2).await;

        // Once the grinder stops waiting, it stops trying.
        l.report(Duration::from_secs(25), Some((38.0, "typed".into())));
        wait_for("idle", || l.status.snapshot().health == Health::Connected).await;
        let tries = accepts.load(Ordering::SeqCst);
        tokio::time::sleep(MIN_BACKOFF + Duration::from_millis(500)).await;
        assert_eq!(accepts.load(Ordering::SeqCst), tries);
    }
}
