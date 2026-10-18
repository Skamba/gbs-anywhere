//! The face for a person or support app. All JSON.
//!
//! | method | path | what |
//! |---|---|---|
//! | GET | `/` | the support app (one HTML page, phone friendly) |
//! | GET | `/icons/<name>` | an icon from `--icons`, e.g. `la_marzocco.svg` (404 otherwise) |
//! | GET | `/favicon.ico`, `/apple-touch-icon*.png` | 404, so browsers asking for icons don't count as the grinder |
//! | GET | `/api/state` | version, phase, current mako reply, grind gate, last shot/grind, grinder link, integrations (each status line also as `detail_key` and `detail_vars`, for wording it in another language), settings, update check; `boot_id` changes on every start, so an app can tell its event and log numbers start over |
//! | PUT | `/api/settings` | `{"shot_window_s":120,"grinder_host":"192.168.1.30","update_check":false}` (any of them) — changes and saves them; they win over flags from then on |
//! | GET | `/api/integrations/kinds` | the integrations the app can add: title, icon, setup form; whether additions are saved |
//! | POST | `/api/integrations` | `{"kind":"la_marzocco","settings":{...}}` — adds and starts one |
//! | GET | `/api/integrations/<id>/settings` | the settings of one added in the app, without passwords (`secrets_set` names the ones that are set) |
//! | PUT | `/api/integrations/<id>` | `{"settings":{...}}` — changes one added in the app and restarts it; a password left out or empty stays |
//! | POST | `/api/integrations/<id>/adopt` | moves one set on the command line into the app (needs a settings file) |
//! | DELETE | `/api/integrations/<id>` | stops and removes one added in the app |
//! | GET | `/api/history` | `shots`: the last 200 shots the grinder asked for, newest first; `capacity` (200); `persistent`: whether they survive a restart |
//! | GET | `/api/log?after=N` | `lines`: the server's last 1000 log lines (INFO and up) with `seq > N`; `last_seq`: the newest line's number; `capacity` (1000) |
//! | GET | `/api/log/stream` | new log lines, live, as server-sent events (`log`); `Last-Event-ID` (or else `?after=N`) sends the missed ones first |
//! | GET | `/api/events?after=N` | events with `seq > N`; `age_ms` says how long ago each happened |
//! | GET | `/api/events/stream` | the same, live, as server-sent events |
//! | POST | `/api/shot/result` | `{"time_s":30,"weight_g":36}` — ends the brew with these numbers; without a weight, the recipe's |
//! | POST | `/api/shot/abort` | ends the brew as a user abort (grinder skips it) |
//! | POST | `/api/shot/start` | starts a brew without a grinder request (an E64 WS sees a flush; an E65/E80 nothing) |
//! | POST | `/api/machine` | patch the `ON`-state mako fields, e.g. `{"TANK_LEVEL":0}` |
//! | POST | `/api/grinder/pairing` | `{"pairing_id":1234}` — the pairing ID an E65/E80 grinder shows, only while the link asks for it |
//! | PUT/DELETE | `/api/overrides` | raw keys forced into every mako reply |

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::protocol::machine::EventRecord;
use crate::protocol::{MachineError, MakoState, ShotResult};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::broadcast::error::RecvError;

use crate::integration::{self, ChangeError, Settings};
use crate::server::logbook::{self, LogLine};
use crate::server::{Server, SettingsChange, history};

pub fn control_router(state: Arc<Server>) -> axum::Router {
    axum::Router::new()
        .route("/", get(app_page))
        .route("/icons/{name}", get(icon))
        .route("/favicon.ico", get(no_icon))
        .route("/apple-touch-icon.png", get(no_icon))
        .route("/apple-touch-icon-precomposed.png", get(no_icon))
        .route("/api/state", get(get_state))
        .route("/api/settings", put(put_settings))
        .route("/api/history", get(get_history))
        .route("/api/log", get(get_log))
        .route("/api/log/stream", get(stream_log))
        .route("/api/events", get(get_events))
        .route("/api/events/stream", get(stream_events))
        .route("/api/shot/result", post(shot_result))
        .route("/api/shot/abort", post(shot_abort))
        .route("/api/shot/start", post(shot_start))
        .route("/api/integrations", post(add_integration))
        .route("/api/integrations/kinds", get(integration_kinds))
        .route(
            "/api/integrations/{id}",
            delete(remove_integration).put(update_integration),
        )
        .route("/api/integrations/{id}/settings", get(integration_settings))
        .route("/api/integrations/{id}/adopt", post(adopt_integration))
        .route("/api/machine", post(patch_machine))
        .route("/api/grinder/pairing", post(grinder_pairing))
        .route("/api/overrides", put(put_overrides).delete(clear_overrides))
        .with_state(state)
}

/// The full state as JSON; also what the CLI prints.
pub fn state_json(x: &Server) -> Value {
    let link = x.grinder_link();
    let integrations = x.integrations().list();
    let settings = x.settings_json();
    let update = x.update_json();
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
                "wifi": link.wifi,
            },
            "integrations": integrations,
            "last_seq": m.last_seq(),
            "boot_id": x.boot_id(),
            "version": crate::version(),
            "config": m.config(),
            "settings": settings,
            "update": update,
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
    Json(x.with(|m, now| {
        Value::Array(
            m.events_after(q.after)
                .map(|r| event_json(r, now))
                .collect(),
        )
    }))
}

/// An event as the API sends it, with `age_ms`: how long before `now` it
/// happened. The age, not a time of day, so the machine stays clock-free and
/// an app can place the event on its own clock.
fn event_json(rec: &EventRecord, now: Instant) -> Value {
    let mut v = serde_json::to_value(rec).unwrap_or(Value::Null);
    if let Value::Object(o) = &mut v {
        let age = now.saturating_duration_since(rec.at).as_millis();
        o.insert(
            "age_ms".into(),
            json!(u64::try_from(age).unwrap_or(u64::MAX)),
        );
    }
    v
}

async fn stream_events(State(x): State<Arc<Server>>) -> impl IntoResponse {
    let rx = x.subscribe();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(rec) => {
                    let data = event_json(&rec, Instant::now());
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

async fn put_settings(State(x): State<Arc<Server>>, Json(body): Json<Value>) -> Response {
    let change = match SettingsChange::parse(&body) {
        Ok(c) => c,
        Err(e) => return error(StatusCode::BAD_REQUEST, &e),
    };
    match x.change_settings(change) {
        Ok(settings) => Json(settings).into_response(),
        // The file could not be written, or the pairing file read.
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn get_history(State(x): State<Arc<Server>>) -> Json<Value> {
    Json(json!({
        "shots": x.history().list(),
        "capacity": history::CAPACITY,
        "persistent": x.history().persistent(),
    }))
}

async fn get_log(State(x): State<Arc<Server>>, Query(q): Query<After>) -> Json<Value> {
    let (lines, last_seq) = x.log().after(q.after);
    Json(json!({
        "lines": lines,
        "last_seq": last_seq,
        "capacity": x.log().capacity(),
    }))
}

/// Live log lines. With the `Last-Event-ID` a browser sends when it
/// reconnects, or else `?after=N`, the lines after `N` that are still kept
/// come first;
/// an `N` beyond the last line (the server restarted since) sends them all.
async fn stream_log(
    State(x): State<Arc<Server>>,
    Query(q): Query<AfterMaybe>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // A reconnecting browser sends the same URL again, so its
    // `Last-Event-ID` is newer than the query.
    let resume = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
        .or(q.after);
    // Subscribed first, so nothing falls between the backlog and the feed.
    let (rx, now) = x.log().subscribe_at();
    let after = match resume {
        Some(after) if after <= now => after,
        // From before a restart (the count started again): all kept lines.
        Some(_) => 0,
        None => now,
    };
    let start = LogStream {
        rx,
        pending: x.log().after(after).0.into(),
        last: after,
        book: x.log().clone(),
    };
    let stream = futures_util::stream::unfold(start, |mut s| async move {
        let line = s.next().await?;
        let ev = Event::default()
            .event("log")
            .id(line.seq.to_string())
            .data(serde_json::to_string(&line).unwrap_or_default());
        Some((Ok::<_, Infallible>(ev), s))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
struct AfterMaybe {
    after: Option<u64>,
}

/// The log lines for one stream, in order and without gaps: when the live
/// feed falls behind, the missed lines are read back from the log book.
struct LogStream {
    book: Arc<logbook::LogBook>,
    rx: tokio::sync::broadcast::Receiver<LogLine>,
    pending: std::collections::VecDeque<LogLine>,
    /// The last `seq` sent (or skipped as already known).
    last: u64,
}

impl LogStream {
    async fn next(&mut self) -> Option<LogLine> {
        loop {
            if let Some(line) = self.pending.pop_front() {
                if line.seq > self.last {
                    self.last = line.seq;
                    return Some(line);
                }
                continue;
            }
            match self.rx.recv().await {
                Ok(line) => self.pending.push_back(line),
                Err(RecvError::Lagged(_)) => {
                    self.pending = self.book.after(self.last).0.into();
                }
                Err(RecvError::Closed) => return None,
            }
        }
    }
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

async fn integration_settings(State(x): State<Arc<Server>>, Path(id): Path<String>) -> Response {
    match x.integrations().form(&id) {
        Ok(form) => Json(form).into_response(),
        Err(e) => change_error(&e),
    }
}

#[derive(Deserialize)]
struct UpdateIntegration {
    #[serde(default)]
    settings: Settings,
}

async fn update_integration(
    State(x): State<Arc<Server>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateIntegration>,
) -> Response {
    match x.integrations().update(&x, &id, req.settings) {
        Ok(()) => Json(json!({ "id": id })).into_response(),
        Err(e) => change_error(&e),
    }
}

async fn adopt_integration(State(x): State<Arc<Server>>, Path(id): Path<String>) -> Response {
    match x.integrations().adopt(&id) {
        Ok(view) => Json(view).into_response(),
        Err(e) => change_error(&e),
    }
}

async fn remove_integration(State(x): State<Arc<Server>>, Path(id): Path<String>) -> Response {
    match x.integrations().remove(&id) {
        Ok(()) => Json(json!({ "removed": id })).into_response(),
        Err(e) => change_error(&e),
    }
}

#[derive(Deserialize)]
struct PairingInput {
    pairing_id: u32,
}

/// The pairing ID an E65/E80 grinder shows for gbs-anywhere (a number up to
/// 65535, as `--grinder-pairing-id` takes).
async fn grinder_pairing(
    State(x): State<Arc<Server>>,
    Json(input): Json<PairingInput>,
) -> Response {
    let id = input.pairing_id;
    if !(1..=u32::from(u16::MAX)).contains(&id) {
        return error(
            StatusCode::BAD_REQUEST,
            "the pairing ID is a number from 1 to 65535",
        );
    }
    match x.use_grinder_pairing_id(id) {
        Ok(()) => Json(json!({ "pairing_id": id })).into_response(),
        Err(e) => error(StatusCode::CONFLICT, &e.to_string()),
    }
}

fn change_error(e: &ChangeError) -> Response {
    let code = match e {
        ChangeError::Invalid(_) => StatusCode::BAD_REQUEST,
        ChangeError::NotFound => StatusCode::NOT_FOUND,
        ChangeError::CommandLine | ChangeError::InApp | ChangeError::NotPersistent => {
            StatusCode::CONFLICT
        }
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

#[cfg(test)]
mod tests {
    //! The app's texts (the `STRINGS` catalog in web/index.html), checked
    //! without a JavaScript engine: every English text is also German, with
    //! the same placeholders, and every key the page uses exists.

    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    const PAGE: &str = include_str!("../../web/index.html");

    /// One language's entries: the `"key": value` lines of `    <lang>: {`,
    /// each on one line.
    fn catalog(lang: &str) -> BTreeMap<&'static str, &'static str> {
        let lines = entries(lang);
        let map: BTreeMap<_, _> = lines.iter().copied().collect();
        assert_eq!(map.len(), lines.len(), "a key twice in {lang}");
        map
    }

    fn entries(lang: &str) -> Vec<(&'static str, &'static str)> {
        let start = PAGE
            .find(&format!("\n    {lang}: {{"))
            .unwrap_or_else(|| panic!("no {lang} catalog"));
        let body = &PAGE[start + 1..];
        let end = body.find("\n    }").expect("end of the catalog");
        body[..end]
            .lines()
            .skip(1)
            .filter_map(|line| {
                let (key, value) = line.strip_prefix("      \"")?.split_once("\":")?;
                Some((key, value.trim()))
            })
            .collect()
    }

    /// The `{name}` placeholders in a value (all forms of a plural).
    fn placeholders(value: &str) -> BTreeSet<&str> {
        value
            .split('{')
            .skip(1)
            .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
            .filter(|name| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
            .collect()
    }

    /// Texts an integration sends in English: only in `de`.
    fn from_server(key: &str) -> bool {
        key.starts_with("kind.") || key.starts_with("integ.detail.")
    }

    #[test]
    fn every_english_text_is_also_german() {
        let (en, de) = (catalog("en"), catalog("de"));
        assert!(en.len() > 100, "the catalog was found");
        let missing: Vec<_> = en.keys().filter(|k| !de.contains_key(*k)).collect();
        assert!(missing.is_empty(), "missing in de: {missing:?}");
        let extra: Vec<_> = de
            .keys()
            .filter(|k| !en.contains_key(*k) && !from_server(k))
            .collect();
        assert!(extra.is_empty(), "only in de: {extra:?}");
    }

    #[test]
    fn both_grinder_wordings_are_there() {
        // tv() picks `<key>.e64` or `<key>.wifi`; one without the other
        // would show the bare key to that grinder's owners.
        let en = catalog("en");
        for key in en.keys() {
            for (this, other) in [(".e64", ".wifi"), (".wifi", ".e64")] {
                if let Some(base) = key.strip_suffix(this) {
                    let twin = format!("{base}{other}");
                    assert!(en.contains_key(twin.as_str()), "{key} without {twin}");
                }
            }
        }
    }

    #[test]
    fn german_texts_keep_the_placeholders() {
        let (en, de) = (catalog("en"), catalog("de"));
        for (key, value) in &en {
            let Some(german) = de.get(key) else { continue };
            assert_eq!(
                placeholders(value),
                placeholders(german),
                "placeholders of {key}"
            );
        }
    }

    #[test]
    fn keys_the_page_uses_exist() {
        let en = catalog("en");
        // A key may get a variant appended (`.e64`, `.wifi`), and one ending
        // in `.` is completed at run time.
        let known = |key: &str| {
            en.contains_key(key) || {
                let prefix = if key.ends_with('.') {
                    key.to_owned()
                } else {
                    format!("{key}.")
                };
                en.keys().any(|k| k.starts_with(&prefix))
            }
        };
        let script = &PAGE[PAGE.rfind("<script>").expect("the app's script")..];
        let markup = &PAGE[PAGE.find("<body").expect("body")..PAGE.rfind("<script>").unwrap()];
        let mut used = BTreeSet::new();
        // t("…") and tv("…") with a literal key.
        for call in ["t(\"", "tv(\""] {
            for (i, _) in script.match_indices(call) {
                let before = script[..i].chars().next_back();
                if before
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.')
                {
                    continue;
                }
                let rest = &script[i + call.len()..];
                let key: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                    .collect();
                if rest[key.len()..].starts_with('"') && !key.is_empty() {
                    used.insert(key);
                }
            }
        }
        // data-i18n="key" and data-i18n-attr="attr:key;…" in the markup.
        for (i, _) in markup.match_indices("data-i18n=\"") {
            let rest = &markup[i + "data-i18n=\"".len()..];
            used.insert(rest[..rest.find('"').unwrap()].to_owned());
        }
        for (i, _) in markup.match_indices("data-i18n-attr=\"") {
            let rest = &markup[i + "data-i18n-attr=\"".len()..];
            for pair in rest[..rest.find('"').unwrap()].split(';') {
                if let Some((_, key)) = pair.split_once(':') {
                    used.insert(key.trim().to_owned());
                }
            }
        }
        assert!(used.len() > 100, "the page's keys were found");
        let unknown: Vec<_> = used.iter().filter(|k| !known(k)).collect();
        assert!(unknown.is_empty(), "keys not in the catalog: {unknown:?}");
    }

    #[test]
    fn integration_texts_are_german() {
        let de = catalog("de");
        let (mut missing, mut wanted) = (Vec::new(), BTreeSet::new());
        let mut need = |key: String| {
            if !de.contains_key(key.as_str()) {
                missing.push(key.clone());
            }
            wanted.insert(key);
        };
        for kind in crate::integration::KINDS {
            need(format!("kind.{}.summary", kind.id));
            for field in kind.fields {
                need(format!("kind.{}.field.{}.label", kind.id, field.key));
                if !field.help.is_empty() {
                    need(format!("kind.{}.field.{}.help", kind.id, field.key));
                }
            }
        }
        // Status lines: every `Detail::keyed("<id>.<state>", …)` of a known kind.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/integration");
        let mut keys = BTreeSet::new();
        for kind in crate::integration::KINDS {
            for entry in std::fs::read_dir(dir.join(kind.id)).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let code = std::fs::read_to_string(&path).unwrap();
                for (i, _) in code.match_indices("Detail::keyed(") {
                    let rest = code[i..].split_once('"').map(|(_, r)| r).unwrap_or("");
                    if let Some((key, _)) = rest.split_once('"') {
                        assert!(
                            key.starts_with(&format!("{}.", kind.id)),
                            "{}: status key {key} does not start with {}.",
                            path.display(),
                            kind.id
                        );
                        keys.insert(key.to_owned());
                    }
                }
            }
        }
        assert!(!keys.is_empty(), "status keys were found");
        for key in keys {
            need(format!("integ.detail.{key}"));
        }
        assert!(missing.is_empty(), "no German for: {missing:?}");
        // And none left behind by a field or status line that is gone (a
        // field's placeholder may have its own wording). A kind left out of
        // this build (one that needs the `bluetooth` feature) keeps its own.
        let field_of = |k: &str| k.strip_suffix(".placeholder").map(|f| format!("{f}.label"));
        let left_out = |k: &str| {
            let id = k
                .strip_prefix("kind.")
                .or_else(|| k.strip_prefix("integ.detail."))
                .and_then(|rest| rest.split_once('.'))
                .map_or("", |(id, _)| id);
            !id.is_empty()
                && dir.join(id).is_dir()
                && !crate::integration::KINDS.iter().any(|kind| kind.id == id)
        };
        let unused: Vec<_> = de
            .keys()
            .filter(|k| from_server(k) && !wanted.contains(**k))
            .filter(|k| field_of(k).is_none_or(|label| !wanted.contains(&label)))
            .filter(|k| !left_out(k))
            .collect();
        assert!(unused.is_empty(), "German for nothing: {unused:?}");
    }
}
