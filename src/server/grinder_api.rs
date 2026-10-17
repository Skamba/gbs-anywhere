//! The face the E64 WS sees: a Xenia's `/api/v2/*` on port 80. (An E65/E80
//! GbS is polled by [`super::wifi_grinder`] instead.)

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::GrinderRequest;
use axum::Extension;
use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use serde_json::{Value, json};

use crate::server::Server;
use crate::server::browser::{first_refusal, from_browser};

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
/// `/api/v2/` is logged and counts as the grinder (a newer firmware may ask
/// for more); anything else comes from other devices on the network looking
/// for their own kind, so it is logged at debug level only. Requests with a
/// browser's headers, and bodies far bigger than the grinder's, are refused,
/// so a web page cannot act as the grinder.
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
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    // A refusal on the grinder's own paths could be a grinder after all: it
    // is logged where people look, once per device, and in the transcript.
    let refuse = async |code: StatusCode, why: &str, hint: &str| {
        if !grinder_path(uri.path()) {
            tracing::debug!("{peer} {method} {uri}: refused, {why}");
        } else if first_refusal(&format!("grinder {why} {}", peer.ip())) {
            tracing::warn!("{peer}: refused {method} {uri}: {why}. {hint}");
        } else {
            tracing::debug!("{peer} {method} {uri}: refused, {why}");
        }
        let line = format!("{} {peer} {method} {uri} -> refused: {why}\n", unix_time());
        x.write_transcript(&line).await;
        code.into_response()
    };
    if !accept && from_browser(&headers) {
        return refuse(
            StatusCode::FORBIDDEN,
            "browser headers (Origin or Sec-Fetch-Site)",
            "They keep web pages from acting as the grinder. If this is your E64 WS, start \
             gbs-anywhere with --accept-browser-headers, and please report it",
        )
        .await;
    }
    let Ok(body) = axum::body::to_bytes(body, MAX_BODY_BYTES).await else {
        return refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            "a body over 4 KiB",
            "The grinder's are a few dozen bytes; if this is your E64 WS, please report it",
        )
        .await;
    };
    let req = GrinderRequest::classify(method.as_str(), uri.path(), &body);
    let poll = matches!(req, GrinderRequest::PollMako);
    let grinder = match &req {
        GrinderRequest::Other { path, .. } => grinder_path(path),
        _ => true,
    };
    if grinder {
        x.note_request(Some(peer), poll);
    }

    let reply: Value = match &req {
        GrinderRequest::Identity => x.with(|m, _| json!(m.identity())),
        GrinderRequest::PollMako => x.with(|m, now| m.mako_json(now)),
        GrinderRequest::GrindResult(g) => {
            tracing::debug!(
                "grinder grind result: beverage {:?} g, filter {:?}",
                g.beverage_weight_g,
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
        GrinderRequest::Malformed { path, error } => {
            tracing::warn!("unparsable grinder write to {path}: {error}");
            json!({})
        }
        GrinderRequest::Other { method, path } => {
            if grinder {
                tracing::info!("unhandled grinder request {method} {path}");
            } else {
                tracing::debug!("not a grinder request: {method} {path}");
            }
            json!({})
        }
    };

    let text = reply.to_string();
    // A grind and a start are logged as the machine's events (see
    // `print_events`), so their raw requests are only for debugging.
    let described = matches!(
        req,
        GrinderRequest::GrindResult(_) | GrinderRequest::StartBrew(_)
    );
    if poll || !grinder || described {
        tracing::debug!("{peer} {method} {uri} -> {text}");
    } else {
        tracing::info!("{peer} {method} {uri} -> {text}");
    }
    let mut line = format!("{} {peer} {method} {uri}", unix_time());
    if !body.is_empty() {
        line.push_str(&format!(" body={}", String::from_utf8_lossy(&body)));
    }
    line.push_str(&format!(" -> {text}\n"));
    x.write_transcript(&line).await;

    (StatusCode::OK, [("content-type", "application/json")], text).into_response()
}

fn unix_time() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}", d.as_secs(), d.subsec_millis())
}
