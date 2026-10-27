//! Where the grinder is: [`WifiConfig`], and the addresses people type.

use std::net::Ipv6Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};

use super::{
    ACCEPT_SETTLE, ASK_WINDOW, MAX_BACKOFF, MIN_BACKOFF, POLL, REFUSED_FOR, RESTART_QUIET,
    UNCONFIRMED_POLL, UNPAIRED_BACKOFF,
};
use crate::protocol::wifi;

/// Where the grinder is and where the pairing is kept.
#[derive(Debug, Clone)]
pub struct WifiConfig {
    /// `https://<grinder>:30001`, or a plain-HTTP fake in tests.
    pub base_url: String,
    /// Keeps the pairing across restarts. Without it, every start pairs anew.
    pub pairing_file: Option<PathBuf>,
    /// The pairing ID the grinder shows. Used when the file has none, or when
    /// the grinder refuses the file's.
    pub pairing_id: Option<u32>,
    /// Between job requests.
    pub poll: Duration,
    /// How long nothing is sent after a pairing check got no answer (see
    /// `RESTART_QUIET`).
    pub restart_quiet: Duration,
    /// How long refused job requests take to count as a cleared pairing (see
    /// `REFUSED_FOR`).
    pub refused_for: Duration,
    /// How long from the confirmation in the app to the pairing check (see
    /// `ACCEPT_SETTLE`).
    pub accept_settle: Duration,
    /// Between pairing checks while nobody has confirmed in the app (see
    /// `UNCONFIRMED_POLL`).
    pub unconfirmed_poll: Duration,
    /// The wait after a failed request, from `min_backoff` doubling up to
    /// `max_backoff`, and at most `unpaired_backoff` while unpaired (see
    /// `UNPAIRED_BACKOFF`).
    pub min_backoff: Duration,
    pub max_backoff: Duration,
    pub unpaired_backoff: Duration,
    /// How long the app is asked for the pairing ID at a time (see
    /// `ASK_WINDOW`).
    pub ask_window: Duration,
}

impl WifiConfig {
    /// From an address as people type it: `192.168.1.30`, `grinder.local`,
    /// `host:port`, or (on the command line, for tests) a full URL. The app
    /// can only set what [`grinder_address`] allows.
    pub fn from_host(host: &str) -> anyhow::Result<Self> {
        let host = host.trim().trim_end_matches('/');
        if host.is_empty() {
            bail!("the grinder address is empty");
        }
        if host.len() > MAX_URL_LEN || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
            bail!("the grinder address has spaces or is too long: {ADDRESS_HINT}");
        }
        let base_url = base_url(host);
        let url = reqwest::Url::parse(&base_url).ok();
        if url.as_ref().and_then(reqwest::Url::host_str).is_none() {
            bail!("`{host}` is not an address: enter the grinder's IP address, e.g. 192.168.1.30");
        }
        Ok(Self {
            base_url,
            pairing_file: None,
            pairing_id: None,
            poll: POLL,
            restart_quiet: RESTART_QUIET,
            refused_for: REFUSED_FOR,
            accept_settle: ACCEPT_SETTLE,
            unconfirmed_poll: UNCONFIRMED_POLL,
            min_backoff: MIN_BACKOFF,
            max_backoff: MAX_BACKOFF,
            unpaired_backoff: UNPAIRED_BACKOFF,
            ask_window: ASK_WINDOW,
        })
    }
}

/// What to type as the grinder's address.
const ADDRESS_HINT: &str = "enter the grinder's IP address, e.g. 192.168.1.30 (optionally :port)";

/// The longest address the app may set: a host name has at most 253
/// characters, then the port.
const MAX_ADDRESS_LEN: usize = 255;

/// The longest address `--grinder` takes (it may be a whole URL).
const MAX_URL_LEN: usize = 2048;

/// The grinder's address as the app may set it: `host`, `host:port` or
/// `[v6]:port`; no scheme, path, login, spaces or control characters, so
/// requests always go to `https://<host>:<port>` on the grinder. Returns it
/// tidied up: trimmed, in lower case, an IPv6 address in brackets, and
/// without an empty port.
pub fn grinder_address(input: &str) -> anyhow::Result<String> {
    let s = input.trim();
    if s.is_empty() || s.len() > MAX_ADDRESS_LEN {
        bail!("{ADDRESS_HINT}");
    }
    if let Ok(ip) = s.parse::<Ipv6Addr>() {
        return Ok(format!("[{ip}]"));
    }
    let (host, port) = match s.strip_prefix('[') {
        Some(rest) => {
            let (v6, after) = rest.split_once(']').context(ADDRESS_HINT)?;
            let ip: Ipv6Addr = v6.parse().ok().context(ADDRESS_HINT)?;
            let port = match after {
                "" => None,
                p => Some(p.strip_prefix(':').context(ADDRESS_HINT)?),
            };
            (format!("[{ip}]"), port)
        }
        None => {
            let (host, port) = match s.split_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (s, None),
            };
            let name_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '.';
            if host.is_empty() || host.starts_with(['-', '.']) || !host.chars().all(name_char) {
                bail!("{ADDRESS_HINT}");
            }
            (host.to_ascii_lowercase(), port)
        }
    };
    match port {
        None | Some("") => Ok(host),
        Some(p) => {
            let port = p
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .context(ADDRESS_HINT)?;
            Ok(format!("{host}:{port}"))
        }
    }
}

/// `https://host:30001` unless the address names a scheme or port.
fn base_url(host: &str) -> String {
    if host.contains("://") {
        return host.to_owned();
    }
    let port = wifi::DEFAULT_PORT;
    let with_port = match host.rsplit_once(':') {
        // `host:port` or `[v6]:port`.
        Some((h, p)) if p.parse::<u16>().is_ok() && (!h.contains(':') || h.ends_with(']')) => {
            host.to_owned()
        }
        // `host:` with the port left out.
        Some((h, "")) if !h.contains(':') => format!("{h}:{port}"),
        _ if host.starts_with('[') => format!("{host}:{port}"),
        // A bare IPv6 address.
        _ if host.contains(':') => format!("[{host}]:{port}"),
        _ => format!("{host}:{port}"),
    };
    format!("https://{with_port}")
}
