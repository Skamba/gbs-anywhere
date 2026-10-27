//! The face the E64 WS sees: a Xenia's `/api/v2/*` on port 80. (An E65/E80
//! GbS is polled by [`super::wifi_grinder`] instead.)

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::GrinderRequest;
use axum::Extension;
use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use crate::server::browser::{
    REFUSED_GRINDER, UNKNOWN_GRINDER, UNPARSABLE_GRINDER, from_browser, short, tell_once,
};
use crate::server::logbook::printable;
use crate::server::settings::make_private;
use crate::server::{Conn, Server};

/// The grinder's writes are a few dozen bytes; anything much bigger is not
/// from the grinder, and would be kept with the event.
const MAX_BODY_BYTES: usize = 4 * 1024;

/// Where the grinder's requests live (paths come without a trailing `/`).
const GRINDER_PATHS: &str = "/api/v2";

/// Whether requests with browser headers may reach the grinder's routes
/// (`--accept-browser-headers`).
#[derive(Clone, Copy)]
struct AcceptBrowsers(bool);

/// Every path goes through one handler so nothing the grinder sends is
/// missed; unknown requests are answered with `{}`. An unknown request under
/// `/api/v2/` is logged, once per device and path (a newer firmware may ask
/// for more); anything else comes from other devices on the network looking
/// for their own kind, so it is logged at debug level only. Only the
/// grinder's own requests count as the grinder (its address, its polls):
/// any device can send the others. Requests with a browser's headers, and
/// bodies far bigger than the grinder's, are refused, so a web page cannot
/// act as the grinder.
pub fn grinder_router(state: Arc<Server>, accept_browsers: bool) -> axum::Router {
    axum::Router::new()
        .fallback(any(handle))
        .layer(Extension(AcceptBrowsers(accept_browsers)))
        .with_state(state)
}

/// Whether `path` is one of the grinder's (`/api/v2` and below).
fn grinder_path(path: &str) -> bool {
    path.trim_end_matches('/')
        .strip_prefix(GRINDER_PATHS)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

async fn handle(
    State(x): State<Arc<Server>>,
    Extension(AcceptBrowsers(accept)): Extension<AcceptBrowsers>,
    ConnectInfo(Conn { peer, .. }): ConnectInfo<Conn>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    // A refusal on the grinder's own paths that could be a grinder after all
    // (one with a `hint`) is logged where people look, once per device; every
    // refusal is in the transcript.
    let refuse = async |code: StatusCode, why: &str, hint: Option<&str>| {
        match hint {
            Some(hint)
                if grinder_path(uri.path()) && tell_once(REFUSED_GRINDER, Some(peer.ip()), why) =>
            {
                let (method, path) = (short(method.as_str()), short(uri.path()));
                tracing::warn!("{peer}: refused {method} {path}: {why}. {hint}");
            }
            _ => tracing::debug!("{peer} {method} {uri}: refused, {why}"),
        }
        let line = transcript_line(peer, &method, &uri, b"", &format!("refused: {why}"));
        x.write_transcript(&line).await;
        code.into_response()
    };
    if !accept && from_browser(&headers) {
        return refuse(
            StatusCode::FORBIDDEN,
            "browser headers (Origin or Sec-Fetch-Site)",
            Some(
                "They keep web pages from acting as the grinder. If this is your E64 WS, start \
                 gbs-anywhere with --accept-browser-headers, and please report it",
            ),
        )
        .await;
    }
    let body = match read_body(body).await {
        Ok(body) => body,
        Err(BodyError::TooLarge) => {
            return refuse(
                StatusCode::PAYLOAD_TOO_LARGE,
                "a body over 4 KiB",
                Some(
                    "The grinder's are a few dozen bytes; if this is your E64 WS, please report \
                     it",
                ),
            )
            .await;
        }
        // A client that went away mid-body, or sent it garbled: not a sign
        // of a grinder that needs something else.
        Err(BodyError::Broken(e)) => {
            let why = format!("a broken request body ({e})");
            return refuse(StatusCode::BAD_REQUEST, &why, None).await;
        }
    };
    let req = GrinderRequest::classify(method.as_str(), uri.path(), &body);
    let poll = matches!(req, GrinderRequest::PollMako);
    // Only what the grinder asks tells where it is; anything on the network
    // can send an unknown or broken request.
    let grinder = matches!(
        req,
        GrinderRequest::Identity
            | GrinderRequest::PollMako
            | GrinderRequest::GrindResult(_)
            | GrinderRequest::StartBrew(_)
    );
    if grinder {
        x.note_request(Some(peer), poll);
    }

    let reply: Value = match &req {
        GrinderRequest::Identity => x.with(|m, _| json!(m.identity())),
        GrinderRequest::PollMako => x.with(|m, now| m.mako_json(now)),
        GrinderRequest::GrindResult(g) => {
            tracing::debug!(
                "grinder grind result: beverage {:?} g, filter {:?}",
                g.target_weight_g,
                g.filter
            );
            x.with(|m, now| m.on_grind_result(now, g.clone()));
            json!({})
        }
        GrinderRequest::StartBrew(s) => {
            tracing::debug!("grinder asks to start a brew (ID {:?})", s.script_id);
            x.with(|m, now| m.on_start_request(now, s.script_id));
            json!({})
        }
        // Told once per device and path, however often it comes.
        GrinderRequest::Malformed { path, error } => {
            let path = short(path);
            if tell_once(UNPARSABLE_GRINDER, Some(peer.ip()), &path) {
                tracing::warn!("{peer}: unparsable grinder write to {path}: {error}");
            } else {
                tracing::debug!("{peer}: unparsable grinder write to {path}: {error}");
            }
            json!({})
        }
        GrinderRequest::Other { method, path } => {
            let request = format!("{} {}", short(method), short(path));
            if !grinder_path(path) {
                tracing::debug!("{peer}: not a grinder request: {request}");
            } else if tell_once(UNKNOWN_GRINDER, Some(peer.ip()), &request) {
                tracing::info!("{peer}: unknown grinder request {request}, answered with {{}}");
            } else {
                tracing::debug!("{peer}: unknown grinder request {request}");
            }
            json!({})
        }
    };

    let text = reply.to_string();
    // Polls come every 2 s; a grind and a start are logged as the machine's
    // events (see `print_events`), the rest above. Only the identity request
    // is left, which the grinder sends when it connects.
    if matches!(req, GrinderRequest::Identity) {
        tracing::info!("{peer} {method} {} -> {text}", short(uri.path()));
    } else {
        tracing::debug!("{peer} {method} {uri} -> {text}");
    }
    let line = transcript_line(peer, &method, &uri, &body, &text);
    x.write_transcript(&line).await;

    (StatusCode::OK, [("content-type", "application/json")], text).into_response()
}

/// Why a request body was not read.
enum BodyError {
    /// Over [`MAX_BODY_BYTES`].
    TooLarge,
    /// The connection broke, or the body was not valid HTTP.
    Broken(axum::Error),
}

/// The request body, up to [`MAX_BODY_BYTES`]. Counted here rather than by a
/// limit inside the body, so a body that is too large can't be mistaken for
/// a broken one.
async fn read_body(body: Body) -> Result<Vec<u8>, BodyError> {
    let mut chunks = body.into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(BodyError::Broken)?;
        if bytes.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(BodyError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// How big the `--log` file grows before it starts over: the grinder's polls
/// alone write about 10 MB a day.
pub(crate) const TRANSCRIPT_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// How much of a request's URI and body a transcript line keeps, in
/// characters: the grinder's are far shorter.
const TRANSCRIPT_URI_CHARS: usize = 512;
const TRANSCRIPT_BODY_CHARS: usize = 1024;

/// The `--log` file: every E64 WS request and our reply, a line each. Only
/// its owner may read it, and it is kept to a size: past that it moves to
/// `<file>.1`, replacing the one there, and starts over.
pub(crate) struct Transcript {
    path: PathBuf,
    max_bytes: u64,
    open: tokio::sync::Mutex<OpenTranscript>,
}

struct OpenTranscript {
    file: tokio::fs::File,
    len: u64,
    /// The log said that starting over failed.
    warned: bool,
}

impl Transcript {
    /// Opens `path` to append to it, or creates it, readable by its owner
    /// only; it starts over past `max_bytes`.
    pub(crate) async fn open(path: &Path, max_bytes: u64) -> std::io::Result<Self> {
        // A file from an older version may be readable by others.
        make_private(path);
        let file = open_private(path).await?;
        let len = file.metadata().await?.len();
        Ok(Self {
            path: path.to_owned(),
            max_bytes,
            open: tokio::sync::Mutex::new(OpenTranscript {
                file,
                len,
                warned: false,
            }),
        })
    }

    /// Appends `line`. A failed write is only a lost line.
    pub(crate) async fn write(&self, line: &str) {
        let mut open = self.open.lock().await;
        let size = u64::try_from(line.len()).unwrap_or(u64::MAX);
        if open.len > 0 && open.len.saturating_add(size) > self.max_bytes {
            match self.start_over(&mut open).await {
                Ok(()) => open.warned = false,
                Err(e) if !std::mem::replace(&mut open.warned, true) => {
                    tracing::warn!("--log: cannot start {} over: {e}", self.path.display());
                }
                Err(_) => {}
            }
        }
        let written = async {
            open.file.write_all(line.as_bytes()).await?;
            open.file.flush().await
        };
        match written.await {
            Ok(()) => open.len = open.len.saturating_add(size),
            Err(e) => tracing::debug!("--log: cannot write {}: {e}", self.path.display()),
        }
    }

    /// Moves the file to `<file>.1` and opens a new one; if it can't be
    /// moved, empties it.
    async fn start_over(&self, open: &mut OpenTranscript) -> std::io::Result<()> {
        let mut old = self.path.as_os_str().to_owned();
        old.push(".1");
        match tokio::fs::rename(&self.path, PathBuf::from(old)).await {
            Ok(()) => open.file = open_private(&self.path).await?,
            Err(_) => open.file.set_len(0).await?,
        }
        open.len = 0;
        Ok(())
    }
}

/// Opens `path` for appending, created readable by its owner only.
async fn open_private(path: &Path) -> std::io::Result<tokio::fs::File> {
    let mut opts = tokio::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    opts.mode(0o600);
    opts.open(path).await
}

/// A line of the `--log` file: when, who, the request and what came of it
/// (`outcome`). What the sender chose (the URI, the body) is cut short and
/// kept on the line, so it can't make lines of its own.
fn transcript_line(
    peer: SocketAddr,
    method: &Method,
    uri: &Uri,
    body: &[u8],
    outcome: &str,
) -> String {
    let mut line = format!(
        "{} {peer} {} {}",
        unix_time(),
        short(method.as_str()),
        printable(&uri.to_string(), TRANSCRIPT_URI_CHARS)
    );
    if !body.is_empty() {
        line.push_str(" body=");
        let body = String::from_utf8_lossy(body);
        line.push_str(&printable(&body, TRANSCRIPT_BODY_CHARS));
    }
    line.push_str(" -> ");
    line.push_str(&printable(outcome, TRANSCRIPT_BODY_CHARS));
    line.push('\n');
    line
}

fn unix_time() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}", d.as_secs(), d.subsec_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_dir;
    use axum::body::Bytes;

    fn body(chunks: Vec<std::io::Result<Bytes>>) -> Body {
        Body::from_stream(futures_util::stream::iter(chunks))
    }

    fn bytes(n: usize) -> std::io::Result<Bytes> {
        Ok(Bytes::from(vec![b'x'; n]))
    }

    #[test]
    fn a_transcript_line_stays_one_short_line() {
        let peer: SocketAddr = "192.168.1.30:51000".parse().unwrap();
        let long = format!("/api/v2/mako?{}", "q".repeat(60_000));
        let uri: Uri = long.parse().unwrap();
        let body = b"{\"ID\":9}\n1700000000.000 192.168.1.30:1 POST /api/v2/scripts/execute -> {}";
        let line = transcript_line(peer, &Method::POST, &uri, body, "{}");
        assert_eq!(line.matches('\n').count(), 1, "{line}");
        assert!(line.ends_with(" -> {}\n"), "{line}");
        assert!(line.contains(r#"body={"ID":9}\n1700000000.000"#), "{line}");
        assert!(line.len() < 2048, "{} bytes", line.len());
        let refused = transcript_line(peer, &Method::GET, &uri, b"", "refused: a\r\nb");
        assert!(refused.ends_with(" -> refused: a\\r\\nb\n"), "{refused}");
        assert!(!refused.contains("body="));
    }

    #[tokio::test]
    async fn the_transcript_starts_over_past_its_size() {
        let dir = temp_dir("rotate");
        let path = dir.join("grinder.log");
        let old = dir.join("grinder.log.1");
        let t = Transcript::open(&path, 100).await.unwrap();
        let line = |n: usize| format!("{n:039}\n");
        for n in 0..2 {
            t.write(&line(n)).await;
        }
        assert!(!old.exists());
        t.write(&line(2)).await;
        let read = |p: &Path| std::fs::read_to_string(p).unwrap();
        assert_eq!(read(&old), line(0) + &line(1));
        assert_eq!(read(&path), line(2));
        for n in 3..6 {
            t.write(&line(n)).await;
        }
        assert_eq!(read(&old), line(2) + &line(3), "one old file, the last");
        assert_eq!(read(&path), line(4) + &line(5));
        assert!(!dir.join("grinder.log.2").exists());
        drop(t);

        // Opened again, it goes on from its size.
        let t = Transcript::open(&path, 100).await.unwrap();
        t.write(&line(6)).await;
        assert_eq!(read(&old), line(4) + &line(5));
        assert_eq!(read(&path), line(6));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn only_its_owner_may_read_the_transcript() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = temp_dir("private");
        let new = dir.join("new.log");
        let t = Transcript::open(&new, 100).await.unwrap();
        assert_eq!(mode(&new), 0o600);
        // Started over, too.
        for _ in 0..3 {
            t.write(&format!("{:059}\n", 0)).await;
        }
        assert_eq!(mode(&new), 0o600);
        // One an older version left readable by others.
        let old = dir.join("old.log");
        std::fs::write(&old, "x\n").unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o644)).unwrap();
        Transcript::open(&old, 100).await.unwrap();
        assert_eq!(mode(&old), 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn only_a_body_over_the_limit_is_too_large() {
        let fits = read_body(body(vec![bytes(MAX_BODY_BYTES - 1), bytes(1)])).await;
        assert_eq!(fits.ok().map(|b| b.len()), Some(MAX_BODY_BYTES));
        let over = read_body(body(vec![bytes(MAX_BODY_BYTES), bytes(1)])).await;
        assert!(matches!(over, Err(BodyError::TooLarge)));
        // A client that goes away mid-body.
        let cut = std::io::Error::other("connection closed");
        let broken = read_body(body(vec![bytes(10), Err(cut)])).await;
        assert!(matches!(broken, Err(BodyError::Broken(_))));
        // Too large before it broke off is too large.
        let cut = std::io::Error::other("connection closed");
        let both = read_body(body(vec![bytes(MAX_BODY_BYTES + 1), Err(cut)])).await;
        assert!(matches!(both, Err(BodyError::TooLarge)));
    }
}
