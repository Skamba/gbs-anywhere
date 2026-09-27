//! The face the grinder sees: a Xenia's `/api/v2/*` on port 80.

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

/// Every path goes through one handler so nothing the grinder sends is
/// missed; unknown requests are logged and answered with `{}`.
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
    x.note_request(peer, poll);

    let reply: Value = match &req {
        GrinderRequest::Identity => x.with(|m, _| json!(m.identity())),
        GrinderRequest::PollMako => x.with(|m, now| m.mako_json(now)),
        GrinderRequest::GrindResult(g) => {
            tracing::info!(
                "grinder grind result: beverage {:?} g, filter {:?}",
                g.beverage_weight_g,
                g.filter
            );
            x.with(|m, now| m.on_grind_result(now, g.clone()));
            json!({})
        }
        GrinderRequest::StartBrew(s) => {
            tracing::info!("grinder asks to start a brew (ID {:?})", s.script_id);
            x.with(|m, now| m.on_start_request(now, s.script_id));
            json!({})
        }
        GrinderRequest::Malformed { path, error } => {
            tracing::warn!("unparsable grinder write to {path}: {error}");
            json!({})
        }
        GrinderRequest::Other { method, path } => {
            tracing::info!("unhandled grinder request {method} {path}");
            json!({})
        }
    };

    let text = reply.to_string();
    if poll {
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
