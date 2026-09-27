//! `gbs-anywhere` — lets a Mahlkönig E64 WS grinder use Grind-by-Sync with any
//! espresso machine. It serves what the grinder polls on port 80, plus a phone
//! app where you enter the shot time and weight; the grinder then adjusts its
//! grind setting.
//!
//! Flow: grind with a GbS recipe -> press the grinder knob when it says
//! "Press grinder rotary knob to start brewing." -> start the shot -> enter
//! time and weight in the app (or type `30 36` here) -> the grinder adjusts.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use gbs_anywhere::protocol::machine::EventRecord;
use gbs_anywhere::protocol::{MachineConfig, MachineEvent, ShotResult};
use gbs_anywhere::server::{self, ServeConfig, Server};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Parser)]
#[command(
    name = "gbs-anywhere",
    version,
    about = "Grind-by-Sync for the Mahlkönig E64 WS with any espresso machine"
)]
struct Args {
    /// Grinder-facing ports. The grinder talks to port 80.
    #[arg(short, long, value_delimiter = ',', default_value = "80")]
    ports: Vec<u16>,

    /// Address for the grinder-facing ports.
    #[arg(short, long, default_value = "0.0.0.0")]
    bind: IpAddr,

    /// Control API address (JSON + event stream for an app). `off` disables.
    #[arg(long, default_value = "127.0.0.1:8787")]
    control: String,

    /// Append every grinder request and our reply to this file.
    #[arg(short, long)]
    log: Option<PathBuf>,

    /// Seconds to show ACTIVE FINISHING before going back to ON (>= 1 poll).
    #[arg(long, default_value_t = 6.0)]
    finishing_hold_s: f64,

    /// Abort a brew after this many seconds without a reported result.
    #[arg(long, default_value_t = 180.0)]
    brew_timeout_s: f64,

    /// Serial number to report (MA_SN).
    #[arg(long)]
    serial: Option<String>,

    /// No stdin prompt (for running as a service; use the control API).
    #[arg(long)]
    no_stdin: bool,

    /// Do not serve the app page and control API on the grinder ports.
    #[arg(long)]
    no_app: bool,

    /// Host or IP to print in the app URL, instead of the detected LAN
    /// address (which is the container's own inside Docker).
    #[arg(long)]
    app_host: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    init_tracing();
    let mut config = MachineConfig {
        finishing_hold: Duration::from_secs_f64(args.finishing_hold_s),
        brew_timeout: (args.brew_timeout_s > 0.0)
            .then(|| Duration::from_secs_f64(args.brew_timeout_s)),
        ..MachineConfig::default()
    };
    if let Some(sn) = args.serial {
        config.identity.serial = sn;
    }
    let x = match &args.log {
        Some(p) => Server::with_transcript(config, p).await?,
        None => Server::new(config),
    };
    let control = match args.control.as_str() {
        "off" | "" => None,
        s => Some(s.parse::<SocketAddr>()?),
    };
    let cfg = ServeConfig {
        grinder: args
            .ports
            .iter()
            .map(|p| SocketAddr::new(args.bind, *p))
            .collect(),
        control,
        app_on_grinder_ports: !args.no_app,
    };
    if !args.no_app {
        let host = args.app_host.clone().unwrap_or_else(|| {
            lan_ip().map_or_else(|| "<this-host>".to_owned(), |ip| ip.to_string())
        });
        let port = args.ports.first().copied().unwrap_or(80);
        let url = if port == 80 {
            format!("http://{host}/")
        } else {
            format!("http://{host}:{port}/")
        };
        println!("app: open {url} on your phone (same WiFi)");
    }

    tokio::spawn(print_events(x.clone()));
    if !args.no_stdin {
        println!("{HELP}");
        tokio::spawn(stdin_loop(x.clone()));
    }
    server::serve(x, cfg).await
}

/// The address this host uses on the LAN (no packet is sent).
fn lan_ip() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:80").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

const HELP: &str = "\
commands:  <seconds> [grams]   report the shot (e.g. `30 36`)
           a                   abort the running brew
           s                   start a brew by hand (grinder counts it as a flush)
           set KEY=VALUE ...   change ON-state mako fields, e.g. `set TANK_LEVEL=0`
           ?                   show state      h  this help";

async fn print_events(x: Arc<Server>) {
    let mut rx = x.subscribe();
    loop {
        match rx.recv().await {
            Ok(rec) => println!("{}", describe(&rec)),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => return,
        }
    }
}

fn describe(rec: &EventRecord) -> String {
    match &rec.event {
        MachineEvent::GrindResult { result } => format!(
            "[{}] grinder finished a grind: target beverage {} g, filter {}. \
             Press the grinder knob to start brewing.",
            rec.seq,
            result
                .beverage_weight_g
                .map_or("?".into(), |w| format!("{w}")),
            result.filter.as_deref().unwrap_or("?"),
        ),
        MachineEvent::StartRequested { script_id } => {
            format!(
                "[{}] grinder requests brew start (ID {script_id:?})",
                rec.seq
            )
        }
        MachineEvent::BrewStarted { human_action, .. } => {
            format!("[{}] >>> {human_action}", rec.seq)
        }
        MachineEvent::ShotReported {
            result,
            in_grinder_range,
        } => format!(
            "[{}] showing shot to grinder: {:.1} s, {:.1} ml{}",
            rec.seq,
            result.time_ms as f64 / 1000.0,
            result.volume_ml,
            if *in_grinder_range {
                ""
            } else {
                "  (over 80 s: the grinder will reject it)"
            }
        ),
        MachineEvent::ShotAborted { reason } => format!("[{}] brew aborted: {reason}", rec.seq),
        MachineEvent::Ready => format!("[{}] machine back to ON", rec.seq),
    }
}

async fn stdin_loop(x: Arc<Server>) {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let msg = handle_line(&x, line.trim());
        if !msg.is_empty() {
            println!("{msg}");
        }
    }
}

fn handle_line(x: &Server, line: &str) -> String {
    let mut words = line.split_whitespace();
    let Some(first) = words.next() else {
        return String::new();
    };
    match first {
        "h" | "help" => HELP.into(),
        "?" => serde_json::to_string_pretty(&server::state_json(x)).unwrap_or_default(),
        "a" => result_msg(x.with(|m, now| m.abort(now))),
        "s" => result_msg(x.with(|m, now| m.start_brew(now))),
        "set" => {
            let mut patch = Map::new();
            for kv in words {
                let Some((k, v)) = kv.split_once('=') else {
                    return format!("expected KEY=VALUE, got `{kv}`");
                };
                let v = serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.into()));
                patch.insert(k.into(), v);
            }
            match server::patch_ready(x, &patch) {
                Ok(()) => "ok".into(),
                Err(e) => format!("error: {e}"),
            }
        }
        secs => {
            let Ok(s) = secs.replace(',', ".").parse::<f64>() else {
                return format!("unknown command `{line}` (h for help)");
            };
            if !s.is_finite() || s < 0.0 {
                return "time must be a positive number of seconds".into();
            }
            let grams = words
                .next()
                .and_then(|g| g.replace(',', ".").parse::<f64>().ok())
                .unwrap_or(0.0);
            let shot = ShotResult::from_grams(Duration::from_secs_f64(s), grams);
            result_msg(x.with(|m, now| m.report_shot(now, shot)))
        }
    }
}

fn result_msg(r: Result<(), gbs_anywhere::protocol::MachineError>) -> String {
    match r {
        Ok(()) => String::new(),
        Err(e) => format!("error: {e}"),
    }
}

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("gbs_anywhere=info"));
    fmt().with_env_filter(filter).with_target(false).init();
}
