//! The face for a person or support app. All JSON. Codes and field names
//! follow one pattern: an event or cause is `<group>_<object>_<what
//! happened>` (`grinder_shot_requested`, `shot_aborted`), a state is one word
//! (`running`, `failing`), units are in the name (`_ms`, `_s`, `_g`), a point
//! in time ends in `_at` and an age in `age_ms`. The groups are those of
//! `/api/state`: `shot`, `grinder`, `machine`, `integrations`, `settings`,
//! `update`, `server`.
//!
//! An error replies `{"error":"<sentence>","code":"<code>"}`: the words are
//! for people, the code (`shot_not_running`, `integration_not_found`, …) for
//! apps to match on. That holds for every route below, also for a body or
//! query it can't read (`request_invalid`) and for a request refused as one a
//! web page made (`request_host_refused`, `request_from_other_site`). Only a
//! path or method not in the table gets a bare 404 or 405.
//!
//! | method | path | what |
//! |---|---|---|
//! | GET | `/` | the support app (one HTML page, phone friendly) |
//! | GET | `/icons/<name>` | an icon from `--icons`, e.g. `la_marzocco.svg` (404 otherwise) |
//! | GET | `/favicon.ico`, `/apple-touch-icon*.png` | 404, so browsers asking for icons don't count as the grinder |
//! | GET | `/api/state` | everything, by group. `shot`: its `phase` (`ready`, `running`, `finishing`), `requested_by_grinder` while running, `result` and `aborted` while finishing, and the `last` result. `grinder`: the link (`peer`, `polls`, `requests`, `last_poll_age_ms`, `wifi` for an E65/E80: its `grinder` address, its `state` (`connecting`, `pairing`, `paired`, `failing`) and, only while `pairing`, its `phase`: `accepting` while gbs-anywhere shows the code and waits for the person to accept it on the grinder and confirm that in the app, for as long as it takes (meanwhile it checks the pairing with the grinder every 40 s), `holding` while it waits 30 s after the confirmation and sends the grinder nothing, `checking` while it checks the pairing with the grinder after that (if the grinder has not accepted it yet, the phase is `accepting` again), `quiet` while gbs-anywhere sends the grinder nothing for a while after a pairing check went unanswered (the grinder may be restarting), `asking` while the grinder has it paired but the pairing ID is missing here; `phase_age_ms`, how long the link has been in that phase (for `quiet`, in the current round), and `quiet_ms`, how long a `quiet` round lasts; with `pairing_code`, `pairing_id`, `unknown_pairing_id` (the last pairing ID the grinder did not take, while asking), `pairing_lost`, `unreachable`, `detail` and `problem`. The two numbers carry the grinder's own names: `pairing_code` is the "Pairing Code" it shows for one attempt (`pairingCode` in its protocol), `pairing_id` the "Pairing ID", the pairing's lasting number (`pairingId`)), `last_grind` (`target_weight_g`, `filter`, `raw`), `grind_blocked` (`cause`, e.g. `machine_tank_empty`, and the grinder's `grinder_message`), and `shot_range`: the shot times the grinder adjusts from for its current or last request, over `min_ms` and up to `max_ms`, as `in_grinder_range` checks them: 10 000 and 80 000 after an E64 WS knob press, 15 000 below (but at least 10 000) and 15 000 above the target time after an E65/E80 brew job; `null` before the grinder's first request and after a brew job without a target time. `machine`: the current `mako` reply, the `overrides`, and its `config` (`shot_window_s`, 0 for no limit, `finishing_hold_s`, …). `integrations`: each with its `health` (`starting`, `connected`, `watching`, `failing`, `stopped`) and `health_age_ms`, and its status line also as `detail_key` and `detail_vars`, for wording it in another language. `settings`: with each one's `source`, the `start` value it goes back to, and `shot_window_range`: the shot windows from `min_s` to `max_s` seconds, 30 and 3600, besides 0 for no limit. `update`: the update check. `server`: `version`, `boot_id` (changes on every start, so an app can tell its event and log numbers start over) and `last_event_seq` |
//! | PUT | `/api/settings` | `{"shot_window_s":120,"grinder_host":"192.168.1.30","update_check":false}` (any of them) — changes and saves them; they win over flags from then on. `null` goes back to the start setting (the flag or environment variable, else the default); `"grinder_host":""` removes the address (without a start address, the same as `null`). Replies with the settings as in `/api/state` |
//! | GET | `/api/integrations/kinds` | the integrations the app can add: title, icon, setup form; whether additions are saved |
//! | POST | `/api/integrations` | `{"kind":"la_marzocco","settings":{...}}` — adds and starts one |
//! | GET | `/api/integrations/<id>/settings` | the settings of one added in the app, without passwords (`secrets_set` names the ones that are set) |
//! | PUT | `/api/integrations/<id>` | `{"settings":{...}}` — changes one added in the app and restarts it; a password left out or empty stays |
//! | POST | `/api/integrations/<id>/adopt` | moves one set on the command line into the app (needs a settings file) |
//! | DELETE | `/api/integrations/<id>` | stops and removes one added in the app; replies `{"id":…}`, as adding and changing do |
//! | GET | `/api/history` | `shots`: the last 200 shots the grinder asked for, newest first: how each ended as `outcome` (`succeeded`, `aborted` or `failed`), `started_at` and `ended_at` (UTC, whole seconds; `ended_at` as the clock read when the shot ended, `started_at` counted back from it by the shot's length on a clock that does not jump), `time_ms` and `weight_g` of each that succeeded, with `in_grinder_range` as on `shot_reported` (`null` when the shot's window was not known, see `grinder.shot_range`, and on shots saved by earlier builds), `target_time_ms` and `target_weight_g` when the grinder said, each other with `reason` and `cause` as on `shot_aborted` (or `shot_end_missed`: still open when the next one started); `capacity` (200); `persistent`: whether they survive a restart (false while saving fails) |
//! | GET | `/api/log?after=N` | `lines`: the server's last 1000 log lines (`at`, `level`: `info`, `warn` or `error`, `text`) with `seq > N`; `last_seq`: the newest line's number; `capacity` (1000) |
//! | GET | `/api/log/stream` | new log lines, live, as server-sent events (`log`); `Last-Event-ID` (or else `?after=N`) sends the missed ones first |
//! | GET | `/api/events?after=N` | events with `seq > N`, each with its `kind`: `grinder_grind_finished` (`result`: the grind's `target_weight_g`, `filter`, `raw`), `grinder_shot_requested` (an E64 WS's `script_id`, an E65/E80's `target_time_ms`), `shot_started`, `shot_reported` (`result`: `time_ms`, `weight_g`; `in_grinder_range`: whether the grinder adjusts from its time, by the window of the grinder's request, see `grinder.shot_range`, or `null` when that is not known), `shot_aborted` and `shot_finished`; `age_ms` says how long ago each happened. `shot_aborted` says why as `cause`, a fixed code to match on (`shot_aborted_by_user`, `shot_timed_out`, `grinder_shot_dropped`, … see `AbortCause`), and in English words as `reason` |
//! | GET | `/api/events/stream` | the same, live, as server-sent events named by `kind` |
//! | POST | `/api/shot/result` | `{"time_s":30,"weight_g":36}` (or `time_ms`) — ends the shot with these numbers; without a weight, the recipe's |
//! | POST | `/api/shot/abort` | ends the shot as a user abort (grinder skips it) |
//! | POST | `/api/shot/start` | starts a shot without a grinder request (an E64 WS sees a flush; an E65/E80 nothing) |
//! | POST | `/api/machine` | patch the `ON`-state mako fields, e.g. `{"TANK_LEVEL":0}` |
//! | POST | `/api/grinder/pairing` | `{"pairing_id":1234}` — the pairing ID an E65/E80 grinder shows, only while the link asks for it (`phase` `asking`); refused with `grinder_already_paired` while paired, `grinder_not_asking` otherwise |
//! | POST | `/api/grinder/accepted` | the person confirms they accepted gbs-anywhere on the grinder: it checks the pairing 30 s later, and sends the grinder nothing meanwhile (`phase` `holding`); only while the link waits for it (`phase` `accepting`), empty body; refused with `grinder_not_accepting` otherwise, `grinder_not_set_up` without an E65/E80 and `grinder_pairing_busy` while the last one is handled |
//! | PUT/DELETE | `/api/overrides` | raw keys forced into every mako reply |

use std::convert::Infallible;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use crate::protocol::machine::EventRecord;
use crate::protocol::{MachineError, MakoState, ShotResult};
use axum::Json;
use axum::extract::{FromRequest, FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio::sync::broadcast::error::RecvError;

use crate::integration::{self, ChangeError, Settings};
use crate::server::browser::{self, AllowedHosts};
use crate::server::logbook::{self, LogLine};
use crate::server::{Server, SettingsChange, history};

/// The app's routes, answering only to home-network host names (`hosts`
/// adds more) and refusing writes from other sites.
pub fn control_router(state: Arc<Server>, hosts: AllowedHosts) -> axum::Router {
    // The page is built now, so a broken template fails when the server
    // starts, not on someone's first visit.
    page();
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
        .route("/api/grinder/accepted", post(grinder_accepted))
        .route("/api/overrides", put(put_overrides).delete(clear_overrides))
        .route_layer(axum::middleware::from_fn_with_state(hosts, browser::guard))
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
        let blocked = mako
            .grind_blocked()
            .map(|b| json!({ "cause": b, "grinder_message": b.grinder_message() }));
        let mut shot = serde_json::to_value(m.phase(now)).unwrap_or(Value::Null);
        if let Value::Object(o) = &mut shot {
            o.insert("last".into(), json!(m.last_shot()));
        }
        json!({
            "shot": shot,
            "grinder": {
                "peer": link.peer,
                "polls": link.polls,
                "requests": link.requests,
                "last_poll_age_ms": link.last_poll.map(|t| now.saturating_duration_since(t).as_millis() as u64),
                "wifi": link.wifi.as_ref().map(|w| {
                    let mut wifi = serde_json::to_value(w).unwrap_or(Value::Null);
                    if let Value::Object(o) = &mut wifi {
                        let age = w.phase_since.map(|t| now.saturating_duration_since(t).as_millis() as u64);
                        o.insert("phase_age_ms".into(), json!(age));
                    }
                    wifi
                }),
                "last_grind": m.last_grind(),
                "grind_blocked": blocked,
                "shot_range": m.grinder_range(),
            },
            "machine": {
                "mako": m.mako_json(now),
                "overrides": m.overrides,
                "config": m.config(),
            },
            "integrations": integrations,
            "settings": settings,
            "update": update,
            "server": {
                "version": crate::version(),
                "boot_id": x.boot_id(),
                "last_event_seq": m.last_seq(),
            },
        })
    })
}

async fn app_page() -> impl IntoResponse {
    (
        [
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        axum::response::Html(page()),
    )
}

// The app page as a template: `index.html` names where the styles and the
// script go, and `app.js` where the texts and the version go. `page()` fills
// them in, so the page comes in one response with everything inline.
const PAGE_HTML: &str = include_str!("../../web/index.html");
const PAGE_CSS: &str = include_str!("../../web/app.css");
const PAGE_JS: &str = include_str!("../../web/app.js");
const CSS_SLOT: &str = "/* web/app.css */";
const JS_SLOT: &str = "/* web/app.js */";
const STRINGS_SLOT: &str = "/* web/strings/<lang>.json */";
/// `PAGE_VERSION` in app.js: the version the page comes with, so a page from
/// the browser's cache can tell that another version answers.
const VERSION_SLOT: &str = "/* version */";

/// The app's languages and their texts; the page takes its list from these.
/// What else a new language needs is under "Web app" in AGENTS.md.
const LANGS: &[(&str, &str)] = &[
    ("en", include_str!("../../web/strings/en.json")),
    ("de", include_str!("../../web/strings/de.json")),
];

/// The page with its slots filled, built once.
pub(super) fn page() -> &'static str {
    static PAGE: LazyLock<String> = LazyLock::new(|| {
        // `STRINGS` in app.js: {"en": {…}, "de": {…}}.
        let strings = LANGS
            .iter()
            .map(|(lang, json)| format!("{}: {}", json!(lang), script_safe(json.trim())))
            .collect::<Vec<_>>()
            .join(",\n");
        let version = script_safe(&json!(crate::version()).to_string());
        let script = fill(
            PAGE_JS.trim_end(),
            &[
                (STRINGS_SLOT, strings.as_str()),
                (VERSION_SLOT, version.as_str()),
            ],
        );
        let styles = PAGE_CSS.trim_end();
        fill(PAGE_HTML, &[(CSS_SLOT, styles), (JS_SLOT, script.as_str())])
    });
    &PAGE
}

/// `template` with each slot replaced by its text, in order; what is filled
/// in is not searched again.
fn fill(template: &str, slots: &[(&str, &str)]) -> String {
    let mut out = String::new();
    let mut rest = template;
    for (slot, text) in slots {
        let (before, after) = rest
            .split_once(slot)
            .unwrap_or_else(|| panic!("the app page has no {slot}"));
        out.push_str(before);
        out.push_str(text);
        rest = after;
    }
    out.push_str(rest);
    out
}

/// JSON to put inside a `<script>` element: `</` as `<\/` and `<!--` as
/// `<\u0021--`, so no text can end the element early. Still the same JSON.
fn script_safe(json: &str) -> String {
    json.replace("</", "<\\/").replace("<!--", "<\\u0021--")
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
        return error(
            StatusCode::NOT_FOUND,
            "icon_not_found",
            "no --icons directory",
        );
    };
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return error(StatusCode::NOT_FOUND, "icon_not_found", "no such icon");
    };
    let mime = match ext {
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => return error(StatusCode::NOT_FOUND, "icon_not_found", "no such icon"),
    };
    if stem.is_empty()
        || !stem
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return error(StatusCode::NOT_FOUND, "icon_not_found", "no such icon");
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
        Err(_) => error(StatusCode::NOT_FOUND, "icon_not_found", "no such icon"),
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

async fn get_events(State(x): State<Arc<Server>>, Params(q): Params<After>) -> Json<Value> {
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
    // Ends when the server stops, so it can close the connection.
    let stream = futures_util::StreamExt::take_until(stream, x.stopped());
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn put_settings(State(x): State<Arc<Server>>, Body(body): Body<Value>) -> Response {
    let change = match SettingsChange::parse(&body) {
        Ok(c) => c,
        Err(e) => return error(StatusCode::BAD_REQUEST, "settings_invalid", &e),
    };
    match x.change_settings(change) {
        Ok(settings) => Json(settings).into_response(),
        // The file could not be written, or the pairing file read.
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.code(), &e.to_string()),
    }
}

async fn get_history(State(x): State<Arc<Server>>) -> Json<Value> {
    Json(json!({
        "shots": x.history().list(),
        "capacity": history::CAPACITY,
        "persistent": x.history().persistent(),
    }))
}

async fn get_log(State(x): State<Arc<Server>>, Params(q): Params<After>) -> Json<Value> {
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
    Params(q): Params<AfterMaybe>,
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
    // Ends when the server stops, so it can close the connection.
    let stream = futures_util::StreamExt::take_until(stream, x.stopped());
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
    Body(req): Body<AddIntegration>,
) -> Response {
    let Some(kind) = integration::KINDS
        .iter()
        .copied()
        .find(|k| k.id == req.kind)
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "integration_kind_unknown",
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
    Body(req): Body<UpdateIntegration>,
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
        Ok(()) => Json(json!({ "id": id })).into_response(),
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
    Body(input): Body<PairingInput>,
) -> Response {
    let id = input.pairing_id;
    if !(1..=u32::from(u16::MAX)).contains(&id) {
        return error(
            StatusCode::BAD_REQUEST,
            "grinder_pairing_id_invalid",
            "the pairing ID is a number from 1 to 65535",
        );
    }
    match x.use_grinder_pairing_id(id) {
        Ok(()) => Json(json!({ "pairing_id": id })).into_response(),
        Err(e) => error(StatusCode::CONFLICT, e.code(), &e.to_string()),
    }
}

/// The person accepted gbs-anywhere on the grinder: the link waits a moment,
/// then checks the pairing. Only while it waits for this. The body is ignored.
async fn grinder_accepted(State(x): State<Arc<Server>>) -> Response {
    match x.grinder_accepted() {
        Ok(()) => Json(json!({ "accepted": true })).into_response(),
        Err(e) => error(StatusCode::CONFLICT, e.code(), &e.to_string()),
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
    error(code, e.code(), &e.to_string())
}

/// A measured shot as a person enters it. Time is required; the weight is
/// in grams. Without one (or with 0), the machine uses the recipe weight.
#[derive(Debug, Deserialize)]
pub struct ShotInput {
    pub time_s: Option<f64>,
    pub time_ms: Option<u32>,
    pub weight_g: Option<f64>,
    /// `weight_g`'s name in 0.2.1 and before, so a client that still sends
    /// it does not fall back to the recipe weight. `weight_g` wins when both
    /// are there.
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
        let weight = self.weight_g.or(self.volume_ml).unwrap_or(0.0);
        Ok(ShotResult::new(time, weight))
    }
}

async fn shot_result(State(x): State<Arc<Server>>, Body(input): Body<ShotInput>) -> Response {
    let result = match input.into_result() {
        Ok(r) => r,
        Err(e) => return error(StatusCode::BAD_REQUEST, "shot_result_invalid", e),
    };
    machine_reply(x.with(|m, now| m.report_shot(now, result)), &x)
}

async fn shot_abort(State(x): State<Arc<Server>>) -> Response {
    machine_reply(x.with(|m, now| m.abort(now)), &x)
}

async fn shot_start(State(x): State<Arc<Server>>) -> Response {
    machine_reply(x.with(|m, now| m.start_shot(now)), &x)
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
    Body(patch): Body<Map<String, Value>>,
) -> Response {
    match patch_ready(&x, &patch) {
        Ok(()) => Json(state_json(&x)).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, "machine_patch_invalid", &e),
    }
}

async fn put_overrides(
    State(x): State<Arc<Server>>,
    Body(o): Body<Map<String, Value>>,
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
        Err(e @ MachineError::NoWeight) => error(StatusCode::BAD_REQUEST, e.code(), &e.to_string()),
        Err(e) => error(StatusCode::CONFLICT, e.code(), &e.to_string()),
    }
}

/// An error reply: `error` in words for people, `code` for apps to match on
/// (`<group>_<what is wrong>`, e.g. `shot_not_running`).
pub(super) fn error(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(json!({ "error": msg, "code": code }))).into_response()
}

/// A JSON body, as `Json` reads it, but one it can't read (malformed, of the
/// wrong shape, without `Content-Type: application/json`) gets the API's
/// error reply with `request_invalid`.
struct Body<T>(T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for Body<T> {
    type Rejection = Response;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Response> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(body)) => Ok(Self(body)),
            Err(e) => Err(error(e.status(), "request_invalid", &e.body_text())),
        }
    }
}

/// The query string, as `Query` reads it, but one it can't read gets the
/// API's error reply with `request_invalid`.
struct Params<T>(T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequestParts<S> for Params<T> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Response> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(params)) => Ok(Self(params)),
            Err(e) => Err(error(e.status(), "request_invalid", &e.body_text())),
        }
    }
}

#[cfg(test)]
mod tests {
    //! The app page and its texts (`web/strings/<lang>.json`), checked
    //! without a JavaScript engine: every English text is in every language,
    //! with the same placeholders, every key the page uses exists, and the
    //! page comes with its styles, script, texts and version filled in.

    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
    use serde_json::Value;

    use super::{
        CSS_SLOT, JS_SLOT, LANGS, PAGE_CSS, PAGE_HTML, PAGE_JS, STRINGS_SLOT, VERSION_SLOT, page,
        script_safe,
    };

    /// One language's texts by key: a text, or a plural
    /// (`{"one": …, "other": …}`).
    fn catalog(lang: &str) -> BTreeMap<String, Value> {
        let (_, json) = LANGS
            .iter()
            .find(|(l, _)| *l == lang)
            .unwrap_or_else(|| panic!("no {lang} catalog"));
        let Entries(entries) =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("web/strings/{lang}.json: {e}"));
        let count = entries.len();
        let map: BTreeMap<_, _> = entries.into_iter().collect();
        assert_eq!(map.len(), count, "a key twice in {lang}");
        map
    }

    /// The languages besides English.
    fn translations() -> impl Iterator<Item = &'static str> {
        LANGS
            .iter()
            .map(|(lang, _)| *lang)
            .filter(|lang| *lang != "en")
    }

    /// A JSON object's entries in file order, a key that comes twice kept
    /// twice (a map would keep only the last).
    struct Entries(Vec<(String, Value)>);

    impl<'de> Deserialize<'de> for Entries {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct Each;
            impl<'de> Visitor<'de> for Each {
                type Value = Entries;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("an object of texts")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Entries, A::Error> {
                    let mut entries = Vec::new();
                    while let Some(entry) = map.next_entry()? {
                        entries.push(entry);
                    }
                    Ok(Entries(entries))
                }
            }
            d.deserialize_map(Each)
        }
    }

    /// The `{name}` placeholders in a value (all forms of a plural).
    fn placeholders(value: &Value) -> BTreeSet<&str> {
        let texts: Vec<&str> = match value {
            Value::Object(forms) => forms.values().filter_map(Value::as_str).collect(),
            other => other.as_str().into_iter().collect(),
        };
        texts
            .into_iter()
            .flat_map(|text| text.split('{').skip(1))
            .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
            .filter(|name| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
            .collect()
    }

    /// Texts an integration sends in English: not in `en`.
    fn from_server(key: &str) -> bool {
        key.starts_with("kind.") || key.starts_with("integ.detail.")
    }

    #[test]
    fn every_english_text_is_in_every_language() {
        let en = catalog("en");
        assert!(en.len() > 100, "the catalog was found");
        assert!(translations().any(|lang| lang == "de"), "German is there");
        for lang in translations() {
            let other = catalog(lang);
            let missing: Vec<_> = en.keys().filter(|k| !other.contains_key(*k)).collect();
            assert!(missing.is_empty(), "missing in {lang}: {missing:?}");
            let extra: Vec<_> = other
                .keys()
                .filter(|k| !en.contains_key(*k) && !from_server(k))
                .collect();
            assert!(extra.is_empty(), "only in {lang}: {extra:?}");
        }
    }

    #[test]
    fn every_entry_is_a_dotted_key_and_a_text() {
        // t() picks a plural's form by the language's plural rules, else
        // `other`; serverText() takes only a text.
        const FORMS: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];
        for (lang, _) in LANGS {
            for (key, value) in catalog(lang) {
                // What the key scan below reads; and a key such as
                // `__proto__` would mean something else in the page's script.
                let dotted = key.starts_with(|c: char| c.is_ascii_alphabetic())
                    && key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
                assert!(dotted, "{lang} {key:?}: not a dotted key");
                match &value {
                    Value::String(_) => {}
                    Value::Object(forms) if !from_server(&key) => {
                        assert!(forms.contains_key("other"), "{lang} {key}: no `other`");
                        for (form, text) in forms {
                            assert!(FORMS.contains(&form.as_str()), "{lang} {key}: {form}?");
                            assert!(text.is_string(), "{lang} {key}.{form}: not a text");
                        }
                    }
                    _ => panic!("{lang} {key}: not a text (or, as the app's own, a plural)"),
                }
            }
        }
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
    fn translations_keep_the_placeholders() {
        let en = catalog("en");
        for lang in translations() {
            let other = catalog(lang);
            for (key, value) in &en {
                let Some(text) = other.get(key) else { continue };
                assert_eq!(
                    placeholders(value),
                    placeholders(text),
                    "placeholders of {key} in {lang}"
                );
            }
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
        let script = PAGE_JS;
        let markup = &PAGE_HTML[PAGE_HTML.find("<body").expect("body")..];
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
    fn the_page_knows_every_abort_cause() {
        use crate::protocol::AbortCause;
        // The script's `CAUSES` table, one `    <code>: { … },` line per cause.
        let start = PAGE_JS
            .find("\n  const CAUSES = {\n")
            .expect("the CAUSES table");
        let table = &PAGE_JS[start..];
        let table = &table[..table.find("\n  };").expect("end of CAUSES")];
        let rows: BTreeMap<_, _> = table
            .lines()
            .filter_map(|line| line.strip_prefix("    ")?.split_once(": {"))
            .collect();
        let en = catalog("en");
        let mut codes = BTreeSet::new();
        for cause in AbortCause::ALL {
            let code = serde_json::to_value(cause).unwrap();
            let code = code.as_str().unwrap().to_owned();
            let row = rows
                .get(code.as_str())
                .unwrap_or_else(|| panic!("{code} is not in CAUSES"));
            let on_purpose = format!("onPurpose: {}", cause.on_purpose());
            assert!(row.contains(&on_purpose), "{code}: not {on_purpose}");
            // A timeout's texts say how long it waited, and the history's
            // own cause never reaches the Start tab.
            if cause != AbortCause::ShotTimedOut {
                assert!(row.contains("title: \""), "{code}: no history title");
                if cause != AbortCause::ShotEndMissed {
                    assert!(row.contains("text: \""), "{code}: no Start tab text");
                }
            }
            for key in row.split('"').skip(1).step_by(2) {
                assert!(en.contains_key(key), "{code}: {key} is not in the catalog");
            }
            codes.insert(code);
        }
        let stale: Vec<_> = rows.keys().filter(|k| !codes.contains(**k)).collect();
        assert!(stale.is_empty(), "not a cause: {stale:?}");
    }

    #[test]
    fn the_page_knows_every_shot_kind() {
        use crate::server::history::ShotOutcome;
        let all = [
            ShotOutcome::Succeeded,
            ShotOutcome::Aborted,
            ShotOutcome::Failed,
        ];
        for kind in all {
            // No wildcard, so a new kind has to be named here, next to
            // `all`, as a reminder to add it there too.
            match kind {
                ShotOutcome::Succeeded | ShotOutcome::Aborted | ShotOutcome::Failed => {}
            }
        }
        let en = catalog("en");
        for kind in all {
            let code = serde_json::to_value(kind).unwrap();
            let code = code.as_str().unwrap();
            assert!(
                PAGE_CSS.contains(&format!("\n.k-{code} {{")),
                "no .k-{code} colours"
            );
            let key = format!("history.kind.{code}");
            assert!(en.contains_key(key.as_str()), "{key} is not in the catalog");
        }
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
        // field's placeholder may have its own wording).
        let field_of = |k: &str| k.strip_suffix(".placeholder").map(|f| format!("{f}.label"));
        let unused: Vec<_> = de
            .keys()
            .filter(|k| from_server(k) && !wanted.contains(k.as_str()))
            .filter(|k| field_of(k).is_none_or(|label| !wanted.contains(&label)))
            .collect();
        assert!(unused.is_empty(), "German for nothing: {unused:?}");
    }

    #[test]
    fn the_page_comes_with_its_styles_script_and_texts() {
        let slots = [
            ("index.html", PAGE_HTML, CSS_SLOT),
            ("index.html", PAGE_HTML, JS_SLOT),
            ("app.js", PAGE_JS, STRINGS_SLOT),
            ("app.js", PAGE_JS, VERSION_SLOT),
        ];
        for (file, text, slot) in slots {
            assert_eq!(text.matches(slot).count(), 1, "{slot} in {file}");
        }
        assert!(PAGE_HTML.find(CSS_SLOT) < PAGE_HTML.find(JS_SLOT));
        assert!(PAGE_JS.find(STRINGS_SLOT) < PAGE_JS.find(VERSION_SLOT));
        let langs: BTreeSet<_> = LANGS.iter().map(|(lang, _)| lang).collect();
        assert_eq!(langs.len(), LANGS.len(), "a language twice in LANGS");
        // Nothing in them ends their element early.
        assert!(!PAGE_CSS.to_ascii_lowercase().contains("</style"));
        let js = PAGE_JS.to_ascii_lowercase();
        assert!(!js.contains("</script") && !js.contains("<!--"));

        let page = page();
        for (_, _, slot) in slots {
            assert!(!page.contains(slot), "{slot} left in the page");
        }
        let css = format!("<style>\n{}\n</style>", PAGE_CSS.trim_end());
        assert!(page.contains(&css), "the styles");
        let (head, rest) = PAGE_JS.split_once(STRINGS_SLOT).unwrap();
        let (middle, tail) = rest.split_once(VERSION_SLOT).unwrap();
        assert!(page.contains(&format!("<script>\n{head}")), "the script");
        // The version the page comes with, as a JSON string.
        let version = serde_json::to_string(crate::version()).unwrap();
        let script_end = format!("{middle}{version}{}\n</script>", tail.trim_end());
        assert!(page.contains(&script_end), "the rest of the script");
        assert!(page.contains(&format!("const PAGE_VERSION = {version};")));
        for (lang, json) in LANGS {
            let texts = format!("\"{lang}\": {}", script_safe(json.trim()));
            assert!(page.contains(&texts), "the {lang} texts");
        }
        // One document, with nothing more to load.
        assert!(page.starts_with("<!doctype html>") && page.ends_with("</html>\n"));
        assert_eq!(page.matches("<style>").count(), 1);
        assert!(!page.contains("<script src") && !page.contains("rel=\"stylesheet\""));
        // And with the texts in, still only the theme script's and the app
        // script's end, and the styles' end.
        let lower = page.to_ascii_lowercase();
        assert_eq!(
            lower.matches("</script").count(),
            2,
            "</script> in the page"
        );
        assert_eq!(lower.matches("</style").count(), 1, "</style> in the page");
        let at = page.find(&format!("<script>\n{head}")).unwrap();
        assert!(!page[at..].contains("<!--"), "<!-- in the app's script");
    }

    #[test]
    fn every_language_has_its_button() {
        // Status → Settings → Language: Auto, and a button per language.
        let attr = "data-lang-choice=\"";
        let buttons: BTreeSet<&str> = PAGE_HTML
            .match_indices(attr)
            .filter_map(|(i, _)| PAGE_HTML[i + attr.len()..].split_once('"'))
            .map(|(code, _)| code)
            .filter(|code| *code != "auto")
            .collect();
        let langs: BTreeSet<&str> = LANGS.iter().map(|(lang, _)| *lang).collect();
        assert_eq!(buttons, langs, "the language buttons and LANGS");
    }

    #[test]
    fn texts_cannot_end_the_script_early() {
        let json = r#"{"a": "</script><!-- b"}"#;
        let safe = script_safe(json);
        assert_eq!(safe, r#"{"a": "<\/script><\u0021-- b"}"#);
        let (a, b): (Value, Value) = (
            serde_json::from_str(json).unwrap(),
            serde_json::from_str(&safe).unwrap(),
        );
        assert_eq!(a, b, "still the same JSON");
    }
}
