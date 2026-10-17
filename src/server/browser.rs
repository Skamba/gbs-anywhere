//! Keeps web pages out. The app and its API have no login, so they are meant
//! for the home network, not for a page that someone on it happens to open.
//! The app's routes answer only to a host name the home network would use
//! (a page that re-points its own name to this address, DNS rebinding, has a
//! public one) and refuse writes from other sites (CSRF). The grinder API
//! refuses whatever a browser sends with its headers, which every write from
//! a page does: the grinder is no browser. A plain read can't be told apart,
//! so the machine state the grinder polls stays readable.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::{MatchedPath, Request, State};
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
    /// From `--allow-host`: names with or without a port, in any case.
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
/// (`Sec-Fetch-Site`), or its `Origin` is not this host.
fn from_other_site(headers: &HeaderMap) -> bool {
    let text = |name| headers.get(name).and_then(|v| v.to_str().ok());
    if let Some(site) = text(header::HeaderName::from_static("sec-fetch-site")) {
        return site != "same-origin" && site != "none";
    }
    match (text(header::ORIGIN), text(header::HOST)) {
        (Some(origin), Some(host)) => origin
            .split_once("://")
            .is_none_or(|(_, authority)| !authority.eq_ignore_ascii_case(host)),
        _ => false,
    }
}

/// Only browsers send these; the grinder never does.
pub fn from_browser(headers: &HeaderMap) -> bool {
    headers.contains_key(header::ORIGIN) || headers.contains_key("sec-fetch-site")
}

/// How many different refusals of one kind are told apart in the log.
const TOLD_APART: usize = 64;

/// Whether `what` is refused for the first time, so the log says so once,
/// however often a page tries. Each kind (the first word of `what`: `host`,
/// `site`, `grinder`) tells up to [`TOLD_APART`] apart, so a page that fills
/// one kind can't hide another, such as a real grinder's refusal. Past that,
/// the log says once that the rest go to the debug log.
pub fn first_refusal(what: &str) -> bool {
    static SEEN: OnceLock<Mutex<HashMap<String, HashSet<String>>>> = OnceLock::new();
    let kind = what.split(' ').next().unwrap_or_default();
    let mut seen = SEEN
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let told = seen.entry(kind.to_owned()).or_default();
    if told.contains(what) {
        return false;
    }
    match told.len().cmp(&TOLD_APART) {
        Ordering::Less => told.insert(what.to_owned()),
        Ordering::Equal => {
            // An empty entry marks that this was said.
            told.insert(String::new());
            tracing::warn!(
                "more than {TOLD_APART} different refused requests ({kind}); further ones go \
                 to the debug log only"
            );
            false
        }
        Ordering::Greater => false,
    }
}

/// Text from a request, cut short for the log.
fn short(text: &str) -> String {
    text.chars().take(64).collect()
}

/// For the app's routes: a host name of the home network, and for writes, no
/// other site.
pub async fn guard(State(hosts): State<AllowedHosts>, req: Request, next: Next) -> Response {
    // Without a Host (HTTP/1.0, some scripts) there is nothing to check; a
    // browser always sends one. HTTP/2 carries it as the URI's authority.
    let host = match req.headers().get(header::HOST) {
        Some(value) => Some(value.to_str().map(host_of).unwrap_or_default()),
        None => req.uri().authority().map(|a| host_of(a.as_str())),
    };
    if let Some(host) = host
        && (host.is_empty() || !hosts.allows(&host))
    {
        let host = short(&host);
        if first_refusal(&format!("host {host}")) {
            tracing::warn!(
                "refused requests for {host:?}: not a name on the home network (to open \
                     the app by that name, start gbs-anywhere with --allow-host {host})"
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
    if !reads && from_other_site(req.headers()) {
        let path = short(req.uri().path());
        // Told apart by route, not by path, so a page can't use up the log's
        // warnings with made-up paths.
        let route = req
            .extensions()
            .get::<MatchedPath>()
            .map_or_else(|| path.clone(), |m| m.as_str().to_owned());
        if first_refusal(&format!("site {} {route}", req.method())) {
            tracing::warn!("refused {} {path} requested by another site", req.method());
        } else {
            tracing::debug!("refused {} {path} requested by another site", req.method());
        }
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_kind_of_refusal_cannot_hide_another() {
        for n in 0..TOLD_APART {
            assert!(first_refusal(&format!("flood {n}")));
        }
        assert!(!first_refusal("flood 3"), "said once");
        assert!(!first_refusal("flood 999"), "past the cap: debug only");
        assert!(!first_refusal("flood 1000"));
        assert!(first_refusal("other kind"), "another kind still warns");
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
        let host = ("host", "192.168.1.20");
        assert!(
            !from_other_site(&headers(&[host])),
            "a script, not a browser"
        );
        assert!(!from_other_site(&headers(&[
            host,
            ("origin", "http://192.168.1.20")
        ])));
        assert!(from_other_site(&headers(&[
            host,
            ("origin", "https://evil.example")
        ])));
        assert!(from_other_site(&headers(&[host, ("origin", "null")])));
        assert!(!from_other_site(&headers(&[
            host,
            ("sec-fetch-site", "same-origin")
        ])));
        assert!(from_other_site(&headers(&[
            host,
            ("sec-fetch-site", "cross-site")
        ])));
        assert!(from_other_site(&headers(&[
            host,
            ("sec-fetch-site", "same-site")
        ])));
        // The browser's own word wins over a Host a proxy rewrote.
        let proxied = ("host", "10.0.0.5:8080");
        assert!(!from_other_site(&headers(&[
            proxied,
            ("origin", "https://gbs.example.net"),
            ("sec-fetch-site", "same-origin"),
        ])));
        assert!(!from_other_site(&headers(&[
            ("host", "192.168.1.20:8080"),
            ("origin", "http://192.168.1.20:8080"),
        ])));
    }
}
