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
//! after each Grind-by-Sync grind, gbs-anywhere asks it for the shot it
//! waits for and answers with that shot's time and weight.
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
use gbs_anywhere::protocol::{
    E64_POLL_INTERVAL, MAX_FINISHING_HOLD, MIN_FINISHING_HOLD, MachineConfig, MachineEvent,
    ShotResult,
};
use gbs_anywhere::server::settings::{self, MAX_SHOT_WINDOW_S, MIN_SHOT_WINDOW_S};
use gbs_anywhere::server::{self, LogBook, ServeConfig, Server, SettingsFile, Startup};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, BufReader};

// A flag given again wins over the one before, so flags added after the
// image name (Docker) or in `GBS_ANYWHERE_ARGS` (the Debian package) can
// change those the launcher passes (`--control off`). Lists add up.
#[derive(Parser)]
#[command(
    name = "gbs-anywhere",
    version = gbs_anywhere::version(),
    about = "Grind-by-Sync for the Mahlkönig E64 WS and E65/E80 GbS with any espresso machine",
    args_override_self = true
)]
struct Args {
    /// Grinder-facing ports. The grinder talks to port 80.
    #[arg(short, long, value_delimiter = ',', default_value = "80")]
    ports: Vec<u16>,

    /// Address for the grinder-facing ports.
    #[arg(short, long, default_value = "0.0.0.0")]
    bind: IpAddr,

    /// Control API address (JSON + event stream for an app). `off` disables.
    // In full, so clap leaves it to `control_address` to say "off".
    #[arg(
        long,
        default_value = "127.0.0.1:8787",
        value_name = "ADDR|off",
        value_parser = control_address
    )]
    control: ::std::option::Option<SocketAddr>,

    /// Append every E64 WS request and our reply to this file.
    #[arg(short, long)]
    log: Option<PathBuf>,

    /// Seconds to show ACTIVE FINISHING before going back to ON: 3 to 60, so
    /// the E64 WS, polling every 2 s, sees it.
    #[arg(long, default_value_t = 6.0, value_name = "S", value_parser = finishing_hold)]
    finishing_hold_s: f64,

    /// The shot window: abort a shot after this many seconds without a
    /// reported result, 30 to 3600; 0 waits forever [default: 180]. A shot
    /// window set in the app wins. Also taken as --brew-timeout-s, its name
    /// in 0.2.1.
    #[arg(long, value_parser = shot_window, value_name = "S", alias = "brew-timeout-s")]
    shot_window_s: Option<f64>,

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

    /// An E65/E80 GbS grinder to pair with over its Local WiFi (Machine To
    /// Machine → GBS COFFEE MACHINES on the grinder): its address, e.g.
    /// 192.168.1.30. The E64 WS connects to gbs-anywhere
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
    // First, so a `docker stop` during startup is not dropped.
    let shutdown = server::shutdown_signal();
    tracing::info!("gbs-anywhere {}", gbs_anywhere::version());
    let file = args.config.as_deref().map(SettingsFile::open).transpose()?;
    // The shot window comes from the settings (`start_settings` below).
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
    let cfg = ServeConfig {
        grinder: args
            .ports
            .iter()
            .map(|p| SocketAddr::new(args.bind, *p))
            .collect(),
        control: args.control,
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
        shot_window_s: args.shot_window_s,
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
    // Exits rather than returns: a read from stdin cannot be cancelled, and
    // the runtime would wait for it (for Enter) before the process ends.
    if let Err(e) = server::serve(x, cfg, shutdown).await {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
    std::process::exit(0)
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
           a                   abort the running shot
           s                   start a shot by hand (grinder counts it as a flush)
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
        MachineEvent::GrinderGrindFinished { result } => format!(
            "[{}] grinder finished a grind: target weight {} g, filter {}",
            rec.seq,
            result
                .target_weight_g
                .map_or("?".into(), |w| format!("{w}")),
            result.filter.as_deref().unwrap_or("?"),
        ),
        MachineEvent::GrinderShotRequested {
            script_id,
            target_time_ms,
        } => format!(
            "[{}] grinder asks for the shot{}{}",
            rec.seq,
            script_id.map_or(String::new(), |id| format!(" (script ID {id})")),
            target_time_ms.map_or(String::new(), |ms| format!(
                ", target time {} s",
                f64::from(ms) / 1000.0
            )),
        ),
        MachineEvent::ShotStarted { human_action, .. } => {
            format!("[{}] >>> {human_action}", rec.seq)
        }
        MachineEvent::ShotReported {
            result,
            in_grinder_range,
        } => format!(
            "[{}] showing shot to grinder: {:.1} s, {:.1} g{}",
            rec.seq,
            result.time_ms as f64 / 1000.0,
            result.weight_g,
            if *in_grinder_range == Some(false) {
                "  (too short or too long: the grinder will ignore it)"
            } else {
                ""
            }
        ),
        MachineEvent::ShotAborted { reason, .. } => format!("[{}] shot aborted: {reason}", rec.seq),

        MachineEvent::ShotFinished => format!("[{}] machine back to ON", rec.seq),
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
        "s" => result_msg(x.with(|m, now| m.start_shot(now))),
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
            let shot = ShotResult::new(time, grams);
            result_msg(x.with(|m, now| m.report_shot(now, shot)))
        }
    }
}

/// A flag's number of seconds.
fn seconds(s: &str) -> Result<f64, String> {
    s.parse()
        .ok()
        .filter(|secs: &f64| secs.is_finite())
        .ok_or_else(|| format!("`{s}` is not a number"))
}

/// Parses `--finishing-hold-s`: long enough for the E64 WS to see the shot
/// finish in one of its polls, short enough not to hold up the next shot.
fn finishing_hold(s: &str) -> Result<f64, String> {
    let secs = seconds(s)?;
    let (min, max) = (
        MIN_FINISHING_HOLD.as_secs_f64(),
        MAX_FINISHING_HOLD.as_secs_f64(),
    );
    if (min..=max).contains(&secs) {
        Ok(secs)
    } else {
        Err(format!(
            "must be between {min} and {max} seconds: the E64 WS polls every {} s and takes a \
             shot only from a poll that sees it finish",
            E64_POLL_INTERVAL.as_secs_f64()
        ))
    }
}

/// Parses `--shot-window-s`: the shot windows the app can set too.
fn shot_window(s: &str) -> Result<f64, String> {
    let secs = seconds(s)?;
    if settings::shot_window_ok(secs) {
        Ok(secs)
    } else {
        Err(format!(
            "must be 0 (no limit) or between {MIN_SHOT_WINDOW_S} and {MAX_SHOT_WINDOW_S} seconds"
        ))
    }
}

/// Parses `--control`: an address with its port, or `off`.
fn control_address(s: &str) -> Result<Option<SocketAddr>, String> {
    match s.trim() {
        "off" | "" => Ok(None),
        addr => addr.parse().map(Some).map_err(|_| {
            format!("`{addr}` is not an IP address with a port, such as 127.0.0.1:8787, or `off`")
        }),
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
        let hold = |value: &str| parse("--finishing-hold-s", value).map(|a| a.finishing_hold_s);
        let window = |value: &str| parse("--shot-window-s", value).map(|a| a.shot_window_s);
        assert_eq!(hold("3").unwrap(), 3.0);
        assert_eq!(hold("60").unwrap(), 60.0);
        assert_eq!(
            Args::try_parse_from(["gbs-anywhere"])
                .unwrap()
                .finishing_hold_s,
            6.0
        );
        for ok in [0.0, 30.0, 150.6, 3600.0] {
            assert_eq!(window(&ok.to_string()).unwrap(), Some(ok));
        }
        assert_eq!(
            parse("--brew-timeout-s", "90").unwrap().shot_window_s,
            Some(90.0),
            "the name in 0.2.1"
        );
        let none = Args::try_parse_from(["gbs-anywhere"]).unwrap();
        assert_eq!(none.shot_window_s, None, "the default is not a flag");
        for bad in ["-1", "3601", "1e20", "NaN", "inf", "soon"] {
            assert!(hold(bad).is_err(), "{bad}");
            assert!(window(bad).is_err(), "{bad}");
        }
        // Shorter than a poll and a margin, or holding up the next shot.
        for bad in ["0", "0.5", "2", "2.5", "60.1", "3600"] {
            let Err(e) = hold(bad) else {
                panic!("{bad} was taken");
            };
            assert!(e.to_string().contains("between 3 and 60"), "{bad}: {e}");
        }
        // As in the app: a limit shorter than 30 s ends shots before they
        // are pulled.
        for bad in ["0.4", "1", "29", "29.9"] {
            let Err(e) = window(bad) else {
                panic!("{bad} was taken");
            };
            assert!(e.to_string().contains("between 30 and 3600"), "{bad}: {e}");
        }
    }

    #[test]
    fn the_control_address_is_checked_with_its_flag() {
        let parse = |args: &[&str]| Args::try_parse_from(args).map(|a| a.control);
        assert_eq!(
            parse(&["gbs-anywhere"]).unwrap(),
            Some("127.0.0.1:8787".parse().unwrap())
        );
        assert_eq!(parse(&["gbs-anywhere", "--control", "off"]).unwrap(), None);
        assert_eq!(
            parse(&["gbs-anywhere", "--control", "[::1]:8787"]).unwrap(),
            Some("[::1]:8787".parse().unwrap())
        );
        for bad in ["localhost:8787", "8787", "0.0.0.0"] {
            let Err(e) = parse(&["gbs-anywhere", "--control", bad]) else {
                panic!("{bad} was taken");
            };
            let e = e.to_string();
            assert!(e.contains("--control") && e.contains(bad), "{bad}: {e}");
        }
    }

    /// The flags a launcher passes before the person's: the words after the
    /// binary in `line`, up to the first `$` variable.
    fn launcher_flags(line: &str) -> Vec<String> {
        line.split_whitespace()
            .skip(1)
            .take_while(|w| !w.starts_with('$'))
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn the_image_and_the_package_pass_the_same_flags() {
        let docker = include_str!("../Dockerfile")
            .lines()
            .find_map(|l| l.strip_prefix("ENTRYPOINT "))
            .expect("the Dockerfile's ENTRYPOINT");
        let docker: Vec<String> = serde_json::from_str(docker).expect("ENTRYPOINT [...]");
        let service = include_str!("../packaging/deb/gbs-anywhere.service")
            .lines()
            .find_map(|l| l.strip_prefix("ExecStart="))
            .expect("the unit's ExecStart");
        assert!(service.ends_with(" $GBS_ANYWHERE_ARGS"), "{service}");
        assert_eq!(
            docker[1..],
            launcher_flags(service),
            "{docker:?} vs {service}"
        );

        // The person's flags come after those and win.
        let mut args = vec!["gbs-anywhere".to_owned()];
        args.extend(docker[1..].iter().cloned());
        let launched = Args::try_parse_from(&args).unwrap();
        assert_eq!(launched.control, None);
        assert!(launched.no_stdin);
        args.extend(
            [
                "--control",
                "0.0.0.0:8787",
                "--no-stdin",
                "-p",
                "8080",
                "-p",
                "8081",
            ]
            .map(str::to_owned),
        );
        let changed = Args::try_parse_from(&args).unwrap();
        assert_eq!(changed.control, Some("0.0.0.0:8787".parse().unwrap()));
        assert!(changed.no_stdin);
        assert_eq!(changed.ports, [8080, 8081], "lists add up, as before");
    }

    #[test]
    fn a_list_given_again_adds_up() {
        let parse = |args: &[&str]| Args::try_parse_from(args).unwrap();
        let hosts = parse(&[
            "gbs-anywhere",
            "--allow-host",
            "a.example,b.example",
            "--allow-host",
            "c.example",
        ]);
        assert_eq!(hosts.allow_host, ["a.example", "b.example", "c.example"]);
        let ports = parse(&["gbs-anywhere", "-p", "80,8080"]);
        assert_eq!(ports.ports, [80, 8080]);
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
