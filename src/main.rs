//! `gbs-anywhere` — lets a Mahlkönig E64 WS or E65/E80 GbS grinder use
//! Grind-by-Sync with any espresso machine. It serves what the E64 WS polls on
//! port 80 (or polls an E65/E80 GbS, see below), plus a phone app where you
//! enter the shot time and weight; the grinder then adjusts its grind setting.
//!
//! Flow: grind with a GbS recipe -> press the grinder knob when it says
//! "Press grinder rotary knob to start brewing." -> start the shot -> enter
//! time and weight in the app (or type `30 36` here) -> the grinder adjusts.
//!
//! Integrations ([`gbs_anywhere::integration`]) can report the shot instead:
//! add them in the app (saved in `--config`) or with their flags, e.g.
//! `--lm-username` for the La Marzocco cloud.
//!
//! An E65/E80 GbS grinder is paired with `--grinder <its address>` instead:
//! gbs-anywhere then asks it for each grind's brew job and answers with the
//! shot.
//!
//! Settings changed in the app (shot window, grinder address) are saved in
//! `--config` and win over their flags from then on.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use gbs_anywhere::integration;
use gbs_anywhere::protocol::machine::EventRecord;
use gbs_anywhere::protocol::{MachineConfig, MachineEvent, ShotResult};
use gbs_anywhere::server::{self, LogBook, ServeConfig, Server, SettingsFile, Startup};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Parser)]
#[command(
    name = "gbs-anywhere",
    version = gbs_anywhere::version(),
    about = "Grind-by-Sync for the Mahlkönig E64 WS and E65/E80 GbS with any espresso machine"
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

    /// Append every E64 WS request and our reply to this file.
    #[arg(short, long)]
    log: Option<PathBuf>,

    /// Seconds to show ACTIVE FINISHING before going back to ON (>= 1 poll).
    #[arg(long, default_value_t = 6.0, value_parser = up_to_an_hour)]
    finishing_hold_s: f64,

    /// Abort a brew after this many seconds without a reported result; 0
    /// waits forever [default: 180]. A shot window set in the app wins.
    #[arg(long, value_parser = up_to_an_hour, value_name = "S")]
    brew_timeout_s: Option<f64>,

    /// Serial number to report (MA_SN).
    #[arg(long)]
    serial: Option<String>,

    /// No stdin prompt (for running as a service; use the control API).
    #[arg(long)]
    no_stdin: bool,

    /// Do not serve the app page and control API on the grinder ports.
    #[arg(long)]
    no_app: bool,

    /// Also open the app by this host name, given in full (comma-separated
    /// for several), besides IP addresses and home-network names such as
    /// `localhost`, a name without a dot or one ending in `.local`, `.lan`,
    /// `.home.arpa` or `.fritz.box`. `*` alone accepts any name, even that of
    /// a web page that re-points its own name to this address.
    #[arg(
        long,
        env = "ALLOW_HOST",
        value_delimiter = ',',
        value_name = "NAME",
        value_parser = full_host_name
    )]
    allow_host: Vec<String>,

    /// Let requests with browser headers (Origin, Sec-Fetch-Site) reach the
    /// E64 WS's routes. They are refused so that a web page cannot act as
    /// the grinder; only needed if the log says your E64 WS was refused (and
    /// then please report it).
    #[arg(long)]
    accept_browser_headers: bool,

    /// Directory with your own icons for the app: `<integration>.svg` or
    /// `.png` (e.g. la_marzocco.svg) replaces the built-in glyph.
    #[arg(long, env = "ICONS_DIR", value_name = "DIR")]
    icons: Option<PathBuf>,

    /// Settings file for the integrations added in the app (created if missing;
    /// holds their passwords). Without it they last until the next restart.
    #[arg(long, env = "CONFIG_FILE", value_name = "FILE")]
    config: Option<PathBuf>,

    /// An E65/E80 GbS grinder to pair with over its "Generic WiFi" option:
    /// its address, e.g. 192.168.1.30. The E64 WS connects to gbs-anywhere
    /// instead and needs nothing here. An address set in the app wins.
    #[arg(long, env = "GRINDER_HOST", value_name = "HOST")]
    grinder: Option<String>,

    /// Keeps the pairing with `--grinder` across restarts [default:
    /// grinder-pairing.json next to `--config`].
    #[arg(long, env = "GRINDER_PAIRING_FILE", value_name = "FILE")]
    grinder_pairing_file: Option<PathBuf>,

    /// The pairing ID the grinder shows for gbs-anywhere: used when the pairing
    /// file has none, or when the grinder refuses the file's.
    #[arg(
        long,
        env = "GRINDER_PAIRING_ID",
        value_name = "ID",
        value_parser = clap::value_parser!(u32).range(1..=65535)
    )]
    grinder_pairing_id: Option<u32>,

    /// Look for a newer release on GitHub once a day, to show in the app (on
    /// unless set; a choice made in the app wins).
    #[arg(
        long,
        env = "UPDATE_CHECK",
        value_parser = clap::builder::BoolishValueParser::new(),
        value_name = "ON|OFF"
    )]
    update_check: Option<bool>,

    #[command(flatten)]
    integrations: integration::CliArgs,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    init_tracing();
    tracing::info!("gbs-anywhere {}", gbs_anywhere::version());
    let file = args.config.as_deref().map(SettingsFile::open).transpose()?;
    // The brew timeout follows the shot window (`start_settings` below).
    let mut config = MachineConfig {
        finishing_hold: Duration::from_secs_f64(args.finishing_hold_s),
        ..MachineConfig::default()
    };
    if let Some(sn) = args.serial {
        config.identity.serial = sn;
    }
    let x = match &args.log {
        Some(p) => Server::with_transcript(config, p).await?,
        None => Server::new(config),
    };
    if let Some(dir) = &args.icons {
        if !dir.is_dir() {
            anyhow::bail!("--icons: {} is not a directory", dir.display());
        }
        x.set_icons_dir(dir.clone());
    }
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
        allowed_hosts: server::AllowedHosts::new(args.allow_host.clone()),
        accept_browser_headers: args.accept_browser_headers,
    };
    if !args.no_app {
        // Inside a container the detected address is the container's own,
        // which the phone cannot reach.
        let host = match lan_ip() {
            Some(ip) if !in_container() => ip.to_string(),
            _ => "<this computer's IP>".to_owned(),
        };
        let port = args.ports.first().copied().unwrap_or(80);
        let url = if port == 80 {
            format!("http://{host}/")
        } else {
            format!("http://{host}:{port}/")
        };
        println!("app: open {url} on your phone (same WiFi)");
    }

    tokio::spawn(print_events(x.clone()));
    x.record_history(
        args.config
            .as_deref()
            .map(|c| c.with_file_name("history.json"))
            .as_deref(),
    )?;
    let startup = Startup {
        shot_window_s: args.brew_timeout_s,
        grinder_host: args.grinder.clone(),
        pairing_file: args.grinder_pairing_file.clone().or_else(|| {
            args.config
                .as_deref()
                .map(|c| c.with_file_name("grinder-pairing.json"))
        }),
        pairing_id: args.grinder_pairing_id,
        update_check: args.update_check,
    };
    x.start_settings(startup, file.clone())?;
    let data = file.as_ref().map(|f| f.data()).unwrap_or_default();
    for (kind, settings) in args.integrations.configured() {
        if let Some(moved) = data.moved(kind.id) {
            if kind.same_settings(&moved.settings, &settings) {
                tracing::info!(
                    "{}: moved into the app; its command-line settings are ignored and can be \
                     removed",
                    kind.title
                );
            } else {
                tracing::warn!(
                    "{}: moved into the app; its command-line settings differ from the app's \
                     and are ignored (remove them, or delete it in the app and restart to use \
                     them)",
                    kind.title
                );
            }
            continue;
        }
        x.integrations().start_cli(&x, kind, &settings)?;
    }
    if let Some(file) = file {
        let path = file.path().to_owned();
        let n = x.integrations().load(&x, file, integration::KINDS)?;
        tracing::info!("settings in {}: {n} added in the app", path.display());
    }
    // It sends nothing while the check is off (see the settings).
    x.start_update_check(server::UpdateConfig::default());
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

fn in_container() -> bool {
    std::path::Path::new("/.dockerenv").exists()
        || std::path::Path::new("/run/.containerenv").exists()
}

const HELP: &str = "\
commands:  <seconds> [grams]   report the shot (e.g. `30 36`)
           a                   abort the running brew
           s                   start a brew by hand (grinder counts it as a flush)
           set KEY=VALUE ...   change ON-state mako fields, e.g. `set TANK_LEVEL=0`
           ?                   show state      h  this help";

/// Says what the machine does, in the log (and so in the app's log view).
async fn print_events(x: Arc<Server>) {
    let mut rx = x.subscribe();
    loop {
        match rx.recv().await {
            Ok(rec) => tracing::info!("{}", describe(&rec)),
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
        MachineEvent::BrewJob {
            result,
            target_time_ms,
        } => format!(
            "[{}] grinder finished a Grind-by-Sync grind: target {}{}",
            rec.seq,
            target_time_ms.map_or("?".into(), |ms| format!("{} s", f64::from(ms) / 1000.0)),
            result
                .beverage_weight_g
                .map_or(String::new(), |w| format!(", {w} g")),
        ),
        MachineEvent::StartRequested { script_id } => format!(
            "[{}] grinder requests brew start{}",
            rec.seq,
            script_id.map_or(String::new(), |id| format!(" (ID {id})")),
        ),
        MachineEvent::BrewStarted { human_action, .. } => {
            format!("[{}] >>> {human_action}", rec.seq)
        }
        MachineEvent::ShotReported {
            result,
            in_grinder_range,
        } => format!(
            "[{}] showing shot to grinder: {:.1} s, {:.1} g{}",
            rec.seq,
            result.time_ms as f64 / 1000.0,
            result.volume_ml,
            if *in_grinder_range {
                ""
            } else {
                "  (over 80 s: the grinder will reject it)"
            }
        ),
        MachineEvent::ShotAborted { reason, .. } => format!("[{}] brew aborted: {reason}", rec.seq),
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
            let Some(time) = ShotResult::time_from_secs(s) else {
                return "time must be between 0 and 600 seconds".into();
            };
            let grams = words
                .next()
                .and_then(|g| g.replace(',', ".").parse::<f64>().ok())
                .unwrap_or(0.0);
            let shot = ShotResult::from_grams(time, grams);
            result_msg(x.with(|m, now| m.report_shot(now, shot)))
        }
    }
}

/// Parses a flag in seconds, from 0 to an hour.
fn up_to_an_hour(s: &str) -> Result<f64, String> {
    let secs: f64 = s.parse().map_err(|_| format!("`{s}` is not a number"))?;
    if (0.0..=3600.0).contains(&secs) {
        Ok(secs)
    } else {
        Err("must be between 0 and 3600 seconds".to_owned())
    }
}

/// Parses a name for `--allow-host`: `*` alone is a wildcard, anything else
/// is compared as given, so a pattern such as `*.ts.net` would never match.
fn full_host_name(s: &str) -> Result<String, String> {
    if s.contains('*') && s.trim() != "*" {
        return Err(
            "give the name in full, e.g. gbs.tail1234.ts.net, or `*` alone to allow any name"
                .to_owned(),
        );
    }
    Ok(s.to_owned())
}

fn result_msg(r: Result<(), gbs_anywhere::protocol::MachineError>) -> String {
    match r {
        Ok(()) => String::new(),
        Err(e) => format!("error: {e}"),
    }
}

/// The console log, and a copy of its INFO and higher lines for the app
/// (`GET /api/log`).
fn init_tracing() {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{EnvFilter, fmt};
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("gbs_anywhere=info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false))
        .with(server::LogLayer::new(LogBook::global()))
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_is_well_formed() {
        use clap::CommandFactory;
        Args::command().debug_assert();
    }

    #[test]
    fn second_flags_reject_what_would_not_fit() {
        let parse = |flag: &str, value: &str| Args::try_parse_from(["gbs-anywhere", flag, value]);
        assert_eq!(
            parse("--finishing-hold-s", "2.5").unwrap().finishing_hold_s,
            2.5
        );
        assert_eq!(
            parse("--brew-timeout-s", "0").unwrap().brew_timeout_s,
            Some(0.0)
        );
        let none = Args::try_parse_from(["gbs-anywhere"]).unwrap();
        assert_eq!(none.brew_timeout_s, None, "the default is not a flag");
        for bad in ["-1", "3601", "1e20", "NaN", "inf", "soon"] {
            assert!(parse("--finishing-hold-s", bad).is_err(), "{bad}");
            assert!(parse("--brew-timeout-s", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_update_check_can_be_turned_off() {
        let parse = |args: &[&str]| Args::try_parse_from(args).map(|a| a.update_check);
        assert_eq!(parse(&["gbs-anywhere"]).unwrap(), None, "on unless set");
        for (value, on) in [
            ("off", false),
            ("OFF", false),
            ("on", true),
            ("false", false),
        ] {
            assert_eq!(
                parse(&["gbs-anywhere", "--update-check", value]).unwrap(),
                Some(on),
                "{value}"
            );
        }
        assert!(parse(&["gbs-anywhere", "--update-check", "maybe"]).is_err());
    }

    #[test]
    fn allowed_hosts_are_full_names() {
        let parse = |value: &str| Args::try_parse_from(["gbs-anywhere", "--allow-host", value]);
        assert_eq!(
            parse("gbs.tail1234.ts.net,*").unwrap().allow_host,
            ["gbs.tail1234.ts.net", "*"]
        );
        for pattern in ["*.ts.net", "gbs.*.ts.net", "**", "gbs.local,*.example"] {
            let Err(e) = parse(pattern) else {
                panic!("{pattern} was taken");
            };
            assert!(
                e.to_string().contains("give the name in full"),
                "{pattern}: {e}"
            );
        }
    }
}
