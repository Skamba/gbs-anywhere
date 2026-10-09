//! The task: stay connected to the scale and, after each knob press, watch
//! its readings for the shot.

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
use crate::integration::{Backoff, BoxFuture, Integration, Link};

const MIN_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The scale sends 10 readings a second; this much silence means it is gone.
const SILENCE: Duration = Duration::from_secs(5);

const SETTLING: &str = "grinder is waiting · waiting for a steady weight";
const READY: &str = "grinder is waiting · watching for the first drops";

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

async fn run(link: Link, cfg: Config) {
    let mut backoff = Backoff::new(MIN_BACKOFF, MAX_BACKOFF);
    let mut last = None;
    link.status.subject(&cfg.host);
    loop {
        link.status.starting(format!("connecting to {}", cfg.host));
        let result = match connect(&cfg.host).await {
            Ok(ws) => {
                backoff.reset();
                tracing::info!("{TITLE}: connected to {}", cfg.host);
                watch(&link, &cfg, ws, &mut last).await
            }
            Err(e) => Err(e),
        };
        match result {
            // The server is shutting down.
            Ok(()) => return,
            Err(e) => {
                tracing::warn!("{TITLE}: {e:#}");
                link.status.error(format!("{e:#}"));
            }
        }
        tracing::info!("{TITLE}: retrying in {} s", backoff.peek().as_secs());
        backoff.wait().await;
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

/// Reads the scale until the connection fails (an error) or the server is
/// gone (`Ok`). After a knob press, feeds a fresh detector until it finds
/// the shot or the grinder stops waiting.
async fn watch(
    link: &Link,
    cfg: &Config,
    mut ws: Socket,
    last: &mut Option<Shot>,
) -> anyhow::Result<()> {
    // Subscribe before saying "connected", so no knob press slips between.
    let mut brews = link.grinder_brews();
    link.status.connected(idle_line(*last));
    let mut detector: Option<Detector> = None;
    loop {
        let text = tokio::select! {
            pressed = brews.next() => {
                if !pressed {
                    return Ok(());
                }
                tracing::info!("{TITLE}: grinder is waiting, taring the scale");
                ws.send(Message::text(r#"{"command":"tare"}"#))
                    .await
                    .context("could not tare the scale")?;
                detector = Some(Detector::after_tare());
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
                link.status.subject(format!("{} · firmware {fw}", cfg.host));
                continue;
            }
            FromScale::Other => continue,
        };
        let Some(d) = detector.as_mut() else {
            continue;
        };
        if !link.grinder_waiting() {
            // Typed on the phone, aborted or timed out.
            detector = None;
            link.status.connected(idle_line(*last));
            continue;
        }
        match d.push(r) {
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
                detector = None;
                link.status.connected(idle_line(*last));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::Instant;

    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
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

    /// A scale on a local port with a 150 g cup on it: readings until `go`,
    /// then an 18 s shot at 2 g/s all at once, then quiet (connection kept
    /// open). It reads 150 g until it gets a tare, 0 g after; `tared` says
    /// whether one came.
    async fn fake_scale(
        listener: TcpListener,
        mut go: oneshot::Receiver<()>,
        tared: Arc<AtomicBool>,
    ) {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let first = ws.next().await.unwrap().unwrap();
        assert_eq!(first.to_text().unwrap(), r#"{"rate_hz":10}"#);
        let mut ms = 60_000;
        loop {
            tokio::select! {
                _ = &mut go => break,
                msg = ws.next() => {
                    let msg = msg.unwrap().unwrap();
                    if msg.to_text().unwrap() == r#"{"command":"tare"}"# {
                        tared.store(true, Ordering::SeqCst);
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    ms += 100;
                    let g = if tared.load(Ordering::SeqCst) { 0.0 } else { 150.0 };
                    ws.send(reading(ms, g)).await.unwrap();
                }
            }
        }
        for r in readings(ms + 100, 32.0, |t| espresso(t, 18.0, 2.0)) {
            ws.send(reading(r.ms, r.grams)).await.unwrap();
        }
        tokio::time::sleep(Duration::from_secs(4)).await;
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

    #[tokio::test]
    async fn reports_a_shot_from_a_fake_scale() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let (go, went) = oneshot::channel();
        let tared = Arc::new(AtomicBool::new(false));
        tokio::spawn(fake_scale(listener, went, tared.clone()));
        let l = link();
        tokio::spawn(run(l.clone(), Config { host }));

        wait_for("connected", || {
            l.status.snapshot().health == Health::Connected
        })
        .await;
        assert!(
            !tared.load(Ordering::SeqCst),
            "no tare before the knob press"
        );
        knob_press(&l);
        wait_for("the tare", || tared.load(Ordering::SeqCst)).await;
        go.send(()).unwrap();
        wait_for("the report", || l.status.snapshot().reports == 1).await;

        let shot = l.status.snapshot().last_report.unwrap();
        assert!((i64::from(shot.time_ms) - 18_000).abs() <= 400, "{shot:?}");
        assert!((shot.volume_ml - 36.3).abs() <= 0.4, "{shot:?}");
        let snap = l.status.snapshot();
        assert_eq!(snap.health, Health::Connected);
        assert!(snap.detail.starts_with("last shot: "), "{}", snap.detail);
    }

    #[tokio::test]
    async fn typed_shot_stops_the_watch() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let (go, went) = oneshot::channel();
        let tared = Arc::new(AtomicBool::new(false));
        tokio::spawn(fake_scale(listener, went, tared.clone()));
        let l = link();
        tokio::spawn(run(l.clone(), Config { host }));

        wait_for("connected", || {
            l.status.snapshot().health == Health::Connected
        })
        .await;
        knob_press(&l);
        wait_for("the tare", || tared.load(Ordering::SeqCst)).await;
        // Someone typed the shot on the phone first.
        l.report(Duration::from_secs(25), Some((38.0, "typed".into())));
        go.send(()).unwrap();
        wait_for("idle again", || {
            l.status.snapshot().health == Health::Connected
        })
        .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(l.status.snapshot().reports, 1, "only the typed one");
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
        tokio::spawn(run(l.clone(), Config { host }));
        wait_for("an error", || l.status.snapshot().health == Health::Error).await;
        let err = l.status.snapshot().last_error.unwrap();
        assert!(err.contains("closed the connection"), "{err}");
        // MIN_BACKOFF later it tries again.
        wait_for("a second attempt", || accepts.load(Ordering::SeqCst) >= 2).await;
    }
}
