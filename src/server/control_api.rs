//! The face for a person or support app. All JSON.
//!
//! | method | path | what |
//! |---|---|---|
//! | GET | `/` | the support app (one HTML page, phone friendly) |
//! | GET | `/icons/<name>` | an icon from `--icons`, e.g. `la_marzocco.svg` (404 otherwise) |
//! | GET | `/favicon.ico`, `/apple-touch-icon*.png` | 404, so browsers asking for icons don't count as the grinder |
//! | GET | `/api/state` | version, phase, current mako reply, grind gate, last shot/grind, grinder link, integrations |
//! | GET | `/api/integrations/kinds` | the integrations the app can add: title, icon, setup form; whether additions are saved |
//! | POST | `/api/integrations` | `{"kind":"la_marzocco","settings":{...}}` — adds and starts one |
//! | DELETE | `/api/integrations/<id>` | stops and removes one added in the app |
//! | GET | `/api/events?after=N` | events with `seq > N` |
//! | GET | `/api/events/stream` | the same, live, as server-sent events |
//! | POST | `/api/shot/result` | `{"time_s":30,"weight_g":36}` — ends the brew with these numbers; without a weight, the recipe's |
//! | POST | `/api/shot/abort` | ends the brew as a user abort (grinder skips it) |
//! | POST | `/api/shot/start` | starts a brew without a grinder request (grinder sees a flush) |
//! | POST | `/api/machine` | patch the `ON`-state mako fields, e.g. `{"TANK_LEVEL":0}` |
//! | PUT/DELETE | `/api/overrides` | raw keys forced into every mako reply |

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use crate::protocol::{MachineError, MakoState, ShotResult};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::broadcast::error::RecvError;

use crate::integration::{self, ChangeError, Settings};
use crate::server::Server;

pub fn control_router(state: Arc<Server>) -> axum::Router {
    axum::Router::new()
        .route("/", get(app_page))
        .route("/icons/{name}", get(icon))
        .route("/favicon.ico", get(no_icon))
        .route("/apple-touch-icon.png", get(no_icon))
        .route("/apple-touch-icon-precomposed.png", get(no_icon))
        .route("/api/state", get(get_state))
        .route("/api/events", get(get_events))
        .route("/api/events/stream", get(stream_events))
        .route("/api/shot/result", post(shot_result))
        .route("/api/shot/abort", post(shot_abort))
        .route("/api/shot/start", post(shot_start))
        .route("/api/integrations", post(add_integration))
        .route("/api/integrations/kinds", get(integration_kinds))
        .route("/api/integrations/{id}", delete(remove_integration))
        .route("/api/machine", post(patch_machine))
        .route("/api/overrides", put(put_overrides).delete(clear_overrides))
        .with_state(state)
}

/// The full state as JSON; also what the CLI prints.
pub fn state_json(x: &Server) -> Value {
    let link = x.grinder_link();
    let integrations = x.integrations().list();
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
            "integrations": integrations,
            "last_seq": m.last_seq(),
            "version": crate::version(),
            "config": m.config(),
        })
    })
}

async fn app_page() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../../web/index.html"))
}

/// Browsers ask for these by themselves; not answered here, they would reach
/// the grinder handler and be counted as grinder traffic.
async fn no_icon() -> StatusCode {
    StatusCode::NOT_FOUND
}

/// An icon file from the `--icons` directory. Names are restricted to
/// `[a-z0-9_-]+.(svg|png|webp)` so nothing else on disk can be read.
async fn icon(State(x): State<Arc<Server>>, Path(name): Path<String>) -> Response {
    let Some(dir) = x.icons_dir() else {
        return error(StatusCode::NOT_FOUND, "no --icons directory");
    };
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return error(StatusCode::NOT_FOUND, "no such icon");
    };
    let mime = match ext {
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => return error(StatusCode::NOT_FOUND, "no such icon"),
    };
    if stem.is_empty()
        || !stem
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return error(StatusCode::NOT_FOUND, "no such icon");
    }
    match tokio::fs::read(dir.join(&name)).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, "max-age=3600"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => error(StatusCode::NOT_FOUND, "no such icon"),
    }
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

async fn integration_kinds(State(x): State<Arc<Server>>) -> Json<Value> {
    Json(json!({
        "kinds": integration::KINDS,
        "persistent": x.integrations().persistent(),
    }))
}

#[derive(Deserialize)]
struct AddIntegration {
    kind: String,
    #[serde(default)]
    settings: Settings,
}

async fn add_integration(
    State(x): State<Arc<Server>>,
    Json(req): Json<AddIntegration>,
) -> Response {
    let Some(kind) = integration::KINDS
        .iter()
        .copied()
        .find(|k| k.id == req.kind)
    else {
        return error(
            StatusCode::BAD_REQUEST,
            &format!("no integration `{}`", req.kind),
        );
    };
    match x.integrations().add(&x, kind, req.settings) {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Err(e) => change_error(&e),
    }
}

async fn remove_integration(State(x): State<Arc<Server>>, Path(id): Path<String>) -> Response {
    match x.integrations().remove(&id) {
        Ok(()) => Json(json!({ "removed": id })).into_response(),
        Err(e) => change_error(&e),
    }
}

fn change_error(e: &ChangeError) -> Response {
    let code = match e {
        ChangeError::Invalid(_) => StatusCode::BAD_REQUEST,
        ChangeError::NotFound => StatusCode::NOT_FOUND,
        ChangeError::CommandLine => StatusCode::CONFLICT,
        ChangeError::Save(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(code, &e.to_string())
}

/// A measured shot as a person enters it. Time is required; volume may be
/// given as grams from a scale (1 g ≈ 1 ml) or as ml. Without one (or with
/// 0), the machine uses the recipe weight.
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
            (Some(ms), _) => Some(Duration::from_millis(ms.into())),
            (None, Some(s)) => ShotResult::time_from_secs(s),
            (None, None) => return Err("need time_s or time_ms"),
        };
        let time = time
            .filter(|t| *t <= ShotResult::MAX_TIME)
            .ok_or("time must be between 0 and 600 seconds")?;
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
        Err(e @ MachineError::NoWeight) => error(StatusCode::BAD_REQUEST, &e.to_string()),
        Err(e) => error(StatusCode::CONFLICT, &e.to_string()),
    }
}

fn error(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({ "error": msg }))).into_response()
}
