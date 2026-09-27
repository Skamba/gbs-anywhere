//! The face for a person or support app. All JSON.
//!
//! | method | path | what |
//! |---|---|---|
//! | GET | `/` | the support app (one HTML page, phone friendly) |
//! | GET | `/api/state` | phase, current mako reply, grind gate, last shot/grind, grinder link |
//! | GET | `/api/events?after=N` | events with `seq > N` |
//! | GET | `/api/events/stream` | the same, live, as server-sent events |
//! | POST | `/api/shot/result` | `{"time_s":30,"weight_g":36}` — ends the brew with these numbers |
//! | POST | `/api/shot/abort` | ends the brew as a user abort (grinder skips it) |
//! | POST | `/api/shot/start` | starts a brew without a grinder request (grinder sees a flush) |
//! | POST | `/api/machine` | patch the `ON`-state mako fields, e.g. `{"TANK_LEVEL":0}` |
//! | PUT/DELETE | `/api/overrides` | raw keys forced into every mako reply |

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use crate::protocol::{MachineError, MakoState, ShotResult};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::broadcast::error::RecvError;

use crate::server::Server;

pub fn control_router(state: Arc<Server>) -> axum::Router {
    axum::Router::new()
        .route("/", get(app_page))
        .route("/api/state", get(get_state))
        .route("/api/events", get(get_events))
        .route("/api/events/stream", get(stream_events))
        .route("/api/shot/result", post(shot_result))
        .route("/api/shot/abort", post(shot_abort))
        .route("/api/shot/start", post(shot_start))
        .route("/api/machine", post(patch_machine))
        .route("/api/overrides", put(put_overrides).delete(clear_overrides))
        .with_state(state)
}

/// The full state as JSON; also what the CLI prints.
pub fn state_json(x: &Server) -> Value {
    let link = x.grinder_link();
    x.with(|m, now| {
        let mako = m.mako(now);
        let blocked = mako.grind_blocked().map(|b| {
            json!({ "reason": b, "grinder_message": b.grinder_message() })
        });
        json!({
            "phase": m.phase(now),
            "mako": m.mako_json(now),
            "grind_blocked": blocked,
            "last_shot": m.last_shot(),
            "last_grind": m.last_grind(),
            "overrides": m.overrides,
            "grinder": {
                "peer": link.peer,
                "polls": link.polls,
                "requests": link.requests,
                "last_poll_age_ms": link.last_poll.map(|t| now.saturating_duration_since(t).as_millis() as u64),
            },
            "last_seq": m.last_seq(),
            "config": m.config(),
        })
    })
}

async fn app_page() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../../web/index.html"))
}

async fn get_state(State(x): State<Arc<Server>>) -> Json<Value> {
    Json(state_json(&x))
}

#[derive(Deserialize)]
struct After {
    #[serde(default)]
    after: u64,
}

async fn get_events(State(x): State<Arc<Server>>, Query(q): Query<After>) -> Json<Value> {
    Json(x.with(|m, _| json!(m.events_after(q.after).collect::<Vec<_>>())))
}

async fn stream_events(State(x): State<Arc<Server>>) -> impl IntoResponse {
    let rx = x.subscribe();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(rec) => {
                    let data = serde_json::to_value(&rec).unwrap_or(Value::Null);
                    let kind = data["kind"].as_str().unwrap_or("event").to_owned();
                    let ev = Event::default()
                        .event(kind)
                        .id(rec.seq.to_string())
                        .data(data.to_string());
                    return Some((Ok::<_, Infallible>(ev), rx));
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// A measured shot as a person enters it. Time is required; volume may be
/// given as grams from a scale (1 g ≈ 1 ml) or as ml.
#[derive(Debug, Deserialize)]
pub struct ShotInput {
    pub time_s: Option<f64>,
    pub time_ms: Option<u32>,
    pub weight_g: Option<f64>,
    pub volume_ml: Option<f64>,
}

impl ShotInput {
    pub fn into_result(self) -> Result<ShotResult, &'static str> {
        let time = match (self.time_ms, self.time_s) {
            (Some(ms), _) => Duration::from_millis(ms.into()),
            (None, Some(s)) if s.is_finite() && s >= 0.0 => Duration::from_secs_f64(s),
            _ => return Err("need time_s or time_ms"),
        };
        let volume = self.volume_ml.or(self.weight_g).unwrap_or(0.0);
        Ok(ShotResult::new(time, volume))
    }
}

async fn shot_result(State(x): State<Arc<Server>>, Json(input): Json<ShotInput>) -> Response {
    let result = match input.into_result() {
        Ok(r) => r,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    machine_reply(x.with(|m, now| m.report_shot(now, result)), &x)
}

async fn shot_abort(State(x): State<Arc<Server>>) -> Response {
    machine_reply(x.with(|m, now| m.abort(now)), &x)
}

async fn shot_start(State(x): State<Arc<Server>>) -> Response {
    machine_reply(x.with(|m, now| m.start_brew(now)), &x)
}

/// Merges the given keys (mako wire names) into the `ON`-state reply.
pub fn patch_ready(x: &Server, patch: &Map<String, Value>) -> Result<(), String> {
    x.with(|m, _| {
        let mut v = serde_json::to_value(&m.ready).map_err(|e| e.to_string())?;
        if let Value::Object(obj) = &mut v {
            for (k, val) in patch {
                obj.insert(k.clone(), val.clone());
            }
        }
        m.ready = serde_json::from_value::<MakoState>(v).map_err(|e| e.to_string())?;
        Ok(())
    })
}

async fn patch_machine(
    State(x): State<Arc<Server>>,
    Json(patch): Json<Map<String, Value>>,
) -> Response {
    match patch_ready(&x, &patch) {
        Ok(()) => Json(state_json(&x)).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, &e),
    }
}

async fn put_overrides(
    State(x): State<Arc<Server>>,
    Json(o): Json<Map<String, Value>>,
) -> Json<Value> {
    x.with(|m, _| m.overrides = o);
    Json(state_json(&x))
}

async fn clear_overrides(State(x): State<Arc<Server>>) -> Json<Value> {
    x.with(|m, _| m.overrides.clear());
    Json(state_json(&x))
}

fn machine_reply(r: Result<(), MachineError>, x: &Server) -> Response {
    match r {
        Ok(()) => Json(state_json(x)).into_response(),
        Err(e) => error(StatusCode::CONFLICT, &e.to_string()),
    }
}

fn error(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({ "error": msg }))).into_response()
}
