//! The face the E64 WS sees: a Xenia's `/api/v2/*` on port 80. (An E65/E80
//! GbS is polled by [`super::wifi_grinder`] instead.)

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::GrinderRequest;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{Method, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::any;
use serde_json::{Value, json};

use crate::server::Server;

/// Where the grinder's requests live (paths come without a trailing `/`).
const GRINDER_PATHS: &str = "/api/v2";

/// Every path goes through one handler so nothing the grinder sends is
/// missed; unknown requests are answered with `{}`. An unknown request under
/// `/api/v2/` is logged and counts as the grinder (a newer firmware may ask
/// for more); anything else comes from other devices on the network looking
/// for their own kind, so it is logged at debug level only.
pub fn grinder_router(state: Arc<Server>) -> axum::Router {
    axum::Router::new().fallback(any(handle)).with_state(state)
}

async fn handle(
    State(x): State<Arc<Server>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> impl IntoResponse {
    let req = GrinderRequest::classify(method.as_str(), uri.path(), &body);
    let poll = matches!(req, GrinderRequest::PollMako);
    let grinder = match &req {
        GrinderRequest::Other { path, .. } => path
            .strip_prefix(GRINDER_PATHS)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/')),
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

    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("access-control-allow-origin", "*"),
        ],
        text,
    )
}

fn unix_time() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}", d.as_secs(), d.subsec_millis())
}
