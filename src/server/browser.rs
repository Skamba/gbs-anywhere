//! Keeps web pages out. The app and its API have no login, so they are meant
//! for the home network, not for a page that someone on it happens to open.
//! The app's routes answer only to a host name the home network would use
//! (a page that re-points its own name to this address, DNS rebinding, has a
//! public one) and refuse writes from other sites (CSRF). The grinder API
//! refuses whatever a browser sends with its headers, which every write from
//! a page does: the grinder is no browser. A plain read can't be told apart,
//! so the machine state the grinder polls stays readable.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Names home networks give their devices, besides IP addresses, `localhost`
/// and names without a dot. None of them resolves in public DNS, except that
/// `fritz.box` is a registered name: AVM holds it, and FRITZ!Box routers
/// answer it on the home network.
const HOME_SUFFIXES: [&str; 8] = [
    ".local",
    ".lan",
    ".home",
    ".home.arpa",
    ".internal",
    ".localdomain",
    ".localhost",
    ".fritz.box",
];

/// Host names the app answers to besides the home-network ones
/// (`--allow-host`); `*` lets any name through.
#[derive(Debug, Clone, Default)]
pub struct AllowedHosts(Arc<Vec<String>>);

impl AllowedHosts {
    /// From `--allow-host`: names with or without a port, in any case. Only
    /// `*` alone is a wildcard; the binary refuses a name with a `*` in it
    /// at startup.
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        Self(Arc::new(
            names
                .into_iter()
                .map(|n| {
                    if n.trim() == "*" {
                        "*".to_owned()
                    } else {
                        host_of(&n)
                    }
                })
                .filter(|n| !n.is_empty())
                .collect(),
        ))
    }

    fn allows(&self, host: &str) -> bool {
        self.0.iter().any(|n| n == "*" || n == host) || home_name(host)
    }
}

/// The host of a `Host` header: lowercase, without port, brackets or a
/// trailing dot.
fn host_of(value: &str) -> String {
    let v = value.trim().to_ascii_lowercase();
    let host = match v.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => match v.rsplit_once(':') {
            Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => h,
            _ => &v,
        },
    };
    host.trim_end_matches('.').to_owned()
}

fn home_name(host: &str) -> bool {
    host.parse::<IpAddr>().is_ok()
        || host == "localhost"
        || !host.contains('.')
        || HOME_SUFFIXES.iter().any(|s| host.ends_with(s))
}

/// A request a web page made from another site: the browser says so
/// (`Sec-Fetch-Site`), or its `Origin` is not the host it asked for.
/// `authority` is that host as the request names it (the `Host` header, or
/// over HTTP/2 the URI's authority), port included, so another site on the
/// same computer stays another site.
fn from_other_site(headers: &HeaderMap, authority: Option<&str>) -> bool {
    if let Some(site) = headers.get("sec-fetch-site") {
        return site != "same-origin" && site != "none";
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        // A script, not a browser.
        return false;
    };
    // A browser always names the host, so an `Origin` with nothing to
    // compare it to is not this site.
    match (origin.to_str(), authority) {
        (Ok(origin), Some(host)) => origin
            .split_once("://")
            .is_none_or(|(_, a)| !a.eq_ignore_ascii_case(host)),
        _ => true,
    }
}

/// Only browsers send these; the grinder never does.
pub fn from_browser(headers: &HeaderMap) -> bool {
    headers.contains_key(header::ORIGIN) || headers.contains_key("sec-fetch-site")
}

/// How many different refusals of one kind to one device the log tells
/// apart.
const TOLD_APART: usize = 64;

/// How many devices the log tells apart for each kind of refusal.
const DEVICES: usize = 64;

/// The refusals the log has told, so that it tells each once.
#[derive(Default)]
struct Told {
    /// By kind, then by device. Requests from an unknown address (a router
    /// served without its peers' addresses) share one entry.
    kinds: HashMap<&'static str, Devices>,
}

#[derive(Default)]
struct Devices {
    seen: HashMap<Option<IpAddr>, Seen>,
    /// The log said that refusals to further devices go to the debug log.
    full: bool,
}

#[derive(Default)]
struct Seen {
    what: HashSet<String>,
    /// The log said that further refusals to this device go to the debug
    /// log.
    full: bool,
}

impl Told {
    fn first(&mut self, kind: &'static str, peer: Option<IpAddr>, what: &str) -> bool {
        // Kept short, however long the request made it.
        let what = short(what);
        let devices = self.kinds.entry(kind).or_default();
        let known = devices.seen.len();
        let seen = match devices.seen.entry(peer) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) if known < DEVICES => e.insert(Seen::default()),
            Entry::Vacant(_) => {
                if !std::mem::replace(&mut devices.full, true) {
                    tracing::warn!(
                        "refused requests ({kind}) from more than {DEVICES} devices; those from \
                         further devices go to the debug log only"
                    );
                }
                return false;
            }
        };
        if seen.what.contains(&what) {
            return false;
        }
        if seen.what.len() < TOLD_APART {
            seen.what.insert(what);
            return true;
        }
        if !std::mem::replace(&mut seen.full, true) {
            let from = peer.map_or_else(|| "an unknown address".to_owned(), |ip| ip.to_string());
            tracing::warn!(
                "more than {TOLD_APART} different refused requests ({kind}) from {from}; further \
                 ones from it go to the debug log only"
            );
        }
        false
    }
}

/// Whether `what` is refused to the device at `peer` for the first time, so
/// the log says so once, however often a page tries. Each kind (`host`,
/// `site`, `grinder`) tells up to [`TOLD_APART`] apart for each device, so a
/// page on one device can't hide what is refused to another, and a page that
/// fills one kind can't hide another, such as a real grinder's refusal. Up
/// to [`DEVICES`] devices are told apart per kind, which keeps the memory
/// small. Past either limit, the log says once that the rest go to the debug
/// log. A device is an IP address: behind a reverse proxy, every device
/// counts as the proxy.
pub fn first_refusal(kind: &'static str, peer: Option<IpAddr>, what: &str) -> bool {
    static TOLD: OnceLock<Mutex<Told>> = OnceLock::new();
    TOLD.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .first(kind, peer, what)
}

/// Text from a request, cut short for the log.
fn short(text: &str) -> String {
    text.chars().take(64).collect()
}

/// For the app's routes: a host name of the home network, and for writes, no
/// other site.
pub async fn guard(State(hosts): State<AllowedHosts>, req: Request, next: Next) -> Response {
    // Unknown when the router is served without its peers' addresses.
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    // The host as the request names it. Without a Host (HTTP/1.0, some
    // scripts) there is nothing to check; a browser always sends one. HTTP/2
    // carries it as the URI's authority.
    let authority = match req.headers().get(header::HOST) {
        Some(value) => Some(value.to_str().unwrap_or_default()),
        None => req.uri().authority().map(|a| a.as_str()),
    };
    if let Some(host) = authority.map(host_of)
        && (host.is_empty() || !hosts.allows(&host))
    {
        let host = short(&host);
        if first_refusal("host", peer, &host) {
            tracing::warn!(
                "refused requests for {host:?}: not a name on the home network. --allow-host \
                 lets the app answer to more names, but a web page can also cause this \
                 refusal, so add a name with it only if you open the app by that name yourself"
            );
        } else {
            tracing::debug!("refused a request for {host:?}");
        }
        return (
            StatusCode::MISDIRECTED_REQUEST,
            "gbs-anywhere answers only to its IP address or a home-network name; see \
                 --allow-host",
        )
            .into_response();
    }
    let reads = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if !reads && from_other_site(req.headers(), authority) {
        // A method can be any word, as long as the request allows.
        let method = short(req.method().as_str());
        let path = short(req.uri().path());
        // Told apart by route, not by path, so a page can't use up the log's
        // warnings with made-up paths.
        let route = req
            .extensions()
            .get::<MatchedPath>()
            .map_or_else(|| path.clone(), |m| m.as_str().to_owned());
        if first_refusal("site", peer, &format!("{method} {route}")) {
            tracing::warn!("refused {method} {path} requested by another site");
        } else {
            tracing::debug!("refused {method} {path} requested by another site");
        }
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: Option<IpAddr> = Some(IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 50)));
    const LAPTOP: Option<IpAddr> = Some(IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 51)));

    #[test]
    fn one_kind_of_refusal_cannot_hide_another() {
        let mut told = Told::default();
        for n in 0..TOLD_APART {
            assert!(told.first("host", PHONE, &format!("r{n}.attacker.example")));
        }
        assert!(
            !told.first("host", PHONE, "r3.attacker.example"),
            "said once"
        );
        assert!(
            !told.first("host", PHONE, "r999.attacker.example"),
            "past the cap: debug only"
        );
        assert!(!told.first("host", PHONE, "r1000.attacker.example"));
        assert!(
            told.first("grinder", PHONE, "a body over 4 KiB"),
            "another kind still warns"
        );
        assert!(told.first("site", PHONE, "POST /api/shot/start"));

        // What a request makes long is kept short.
        let long = format!("{} /api/shot/start", "M".repeat(100_000));
        assert!(told.first("site", LAPTOP, &long));
        assert!(!told.first("site", LAPTOP, &long), "said once");
        assert!(
            told.kinds["site"].seen[&LAPTOP]
                .what
                .iter()
                .all(|w| w.len() <= 64)
        );
    }

    #[test]
    fn a_page_on_one_device_cannot_silence_another() {
        let mut told = Told::default();
        // A rebinding page on the phone fills the phone's share.
        for n in 0..=TOLD_APART {
            told.first("host", PHONE, &format!("r{n}.attacker.example"));
        }
        assert!(!told.first("host", PHONE, "gbs.tail1234.ts.net"));
        // The laptop opening the app by its Tailscale name is still told.
        assert!(told.first("host", LAPTOP, "gbs.tail1234.ts.net"));
        assert!(
            told.first("host", None, "gbs.tail1234.ts.net"),
            "unknown addresses share their own entry"
        );

        // The devices told apart are capped too; the ones known stay.
        let device = |n: usize| {
            let n = u16::try_from(n).unwrap();
            Some(IpAddr::V6(std::net::Ipv6Addr::new(
                0x2001, 0xdb8, 0, 0, 0, 0, 0, n,
            )))
        };
        let mut told = Told::default();
        for n in 0..DEVICES {
            assert!(told.first("host", device(n), "attacker.example"));
        }
        assert!(
            !told.first("host", PHONE, "attacker.example"),
            "one device too many"
        );
        assert!(!told.first("host", LAPTOP, "attacker.example"));
        assert!(
            told.first("host", device(0), "other.example"),
            "a known device"
        );
        assert!(
            told.first("grinder", PHONE, "a body over 4 KiB"),
            "another kind"
        );
    }

    #[test]
    fn home_network_names_pass_public_ones_do_not() {
        let hosts = AllowedHosts::new(["gbs.example.org".to_owned()]);
        for ok in [
            "192.168.1.20",
            "192.168.1.20:8080",
            "[fe80::1]:80",
            "localhost:8089",
            "kitchen-pi",
            "gbs.local",
            "nas.fritz.box",
            "pi.home.arpa",
            "gbs.lan.",
            "GBS.LOCAL",
            "gbs.example.org",
            "gbs.example.org:8080",
        ] {
            assert!(hosts.allows(&host_of(ok)), "{ok}");
        }
        for refused in [
            "attacker.example",
            "192.168.1.20.nip.io",
            "local.example.com",
        ] {
            assert!(!hosts.allows(&host_of(refused)), "{refused}");
        }
        assert!(AllowedHosts::new(["*".to_owned()]).allows("attacker.example"));
        let given = AllowedHosts::new(["GBS.Example.NET.:8443".to_owned()]);
        assert!(
            given.allows("gbs.example.net"),
            "port, case and dot dropped"
        );
    }

    #[test]
    fn other_sites_are_told_apart() {
        let headers = |pairs: &[(&'static str, &str)]| {
            let mut h = HeaderMap::new();
            for (k, v) in pairs {
                h.insert(*k, v.parse().unwrap());
            }
            h
        };
        let host = Some("192.168.1.20");
        assert!(
            !from_other_site(&headers(&[]), host),
            "a script, not a browser"
        );
        assert!(!from_other_site(&headers(&[]), None));
        assert!(!from_other_site(
            &headers(&[("origin", "http://192.168.1.20")]),
            host
        ));
        assert!(from_other_site(
            &headers(&[("origin", "https://evil.example")]),
            host
        ));
        assert!(from_other_site(&headers(&[("origin", "null")]), host));
        assert!(
            from_other_site(&headers(&[("origin", "http://192.168.1.20")]), None),
            "an Origin, but no host to compare it to"
        );
        assert!(
            from_other_site(&headers(&[("origin", "http://192.168.1.20:8080")]), host),
            "another port is another site"
        );
        assert!(!from_other_site(
            &headers(&[("sec-fetch-site", "same-origin")]),
            host
        ));
        assert!(from_other_site(
            &headers(&[("sec-fetch-site", "cross-site")]),
            host
        ));
        assert!(from_other_site(
            &headers(&[("sec-fetch-site", "same-site")]),
            host
        ));
        // The browser's own word wins over a Host a proxy rewrote.
        assert!(!from_other_site(
            &headers(&[
                ("origin", "https://gbs.example.net"),
                ("sec-fetch-site", "same-origin"),
            ]),
            Some("10.0.0.5:8080")
        ));
        assert!(!from_other_site(
            &headers(&[("origin", "http://GBS.local:8080")]),
            Some("gbs.local:8080")
        ));
    }
}
