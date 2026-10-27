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
//! | GET | `/api/state` | everything, by group. `shot`: its `phase` (`ready`, `running`, `finishing`), `requested_by_grinder` and `phase_age_ms` (how long it has run) while running, `result` and `aborted` while finishing, and the `last` result. `grinder`: the link (`peer`, `polls`, `requests`, `last_poll_age_ms`, `wifi` for an E65/E80: its `grinder` address, its `state` (`connecting`, `pairing`, `paired`, `failing`) and, only while `pairing`, its `phase`: `accepting` while gbs-anywhere shows the code and waits for the person to accept it on the grinder and confirm that in the app, for as long as it takes (meanwhile it checks the pairing with the grinder every 40 s), `holding` while it waits 30 s after the confirmation and sends the grinder nothing, `checking` while it checks the pairing with the grinder after that (if the grinder has not accepted it yet, the phase is `accepting` again), `quiet` while gbs-anywhere sends the grinder nothing for a while after a pairing check went unanswered (the grinder may be restarting), `asking` while the grinder has it paired but the pairing ID is missing here; `phase_age_ms`, how long the link has been in that phase (for `quiet`, in the current round), and `quiet_ms`, how long a `quiet` round lasts; `state_age_ms`, how long it has been in its `state` (from the start of the link: a restart or a new address starts it over); with `pairing_code`, `pairing_id`, `unknown_pairing_id` (the last pairing ID the grinder did not take, while asking), `pairing_lost`, `unreachable`, `not_set_up` (not paired, and the grinder answered the pairing application with a client error, as one not set up for pairing would), `detail`, `failure` (while `failing`: why, in English, without the steps before it that `detail` lists, e.g. `operation timed out`), `problem` (what went wrong with the last shot, in English, until the next one goes through) and `problem_cause`, the same as a code: the `AbortCause` of a brew that ended without an answer (`grinder_shot_dropped`, `grinder_shot_refused`, `grinder_unpaired`), or `grinder_result_expired` (the result did not reach the grinder in time), `grinder_result_refused` or `grinder_result_delayed` (not yet, still trying). The two numbers carry the grinder's own names: `pairing_code` is the "Pairing Code" it shows for one attempt (`pairingCode` in its protocol), `pairing_id` the "Pairing ID", the pairing's lasting number (`pairingId`)), `last_grind` (`target_weight_g`, `filter`, `raw`), `grind_blocked` (`cause`, e.g. `machine_tank_empty`, and `grinder_message`, what the grinder shows, in English), and `shot_range`: the shot times the grinder adjusts from for its current or last request, over `min_ms` and up to `max_ms`, as `in_grinder_range` checks them: 10 000 and 80 000 after an E64 WS knob press, 15 000 below (but at least 10 000) and 15 000 above the target time after an E65/E80 brew job; `null` before the grinder's first request and after a brew job without a target time. `machine`: the current `mako` reply, the `overrides`, and its `config` (`shot_window_s`, 0 for no limit, `finishing_hold_s`, …). `integrations`: each with its `health` (`starting`, `connected`, `watching`, `failing`, `stopped`) and `health_age_ms`, and its status line also as `detail_key` and `detail_vars`, for wording it in another language. `settings`: with each one's `source`, the `start` value it goes back to, and `shot_window_range`: the shot windows from `min_s` to `max_s` seconds, 30 and 3600, besides 0 for no limit. `update`: the update check. `server`: `version`, `boot_id` (changes on every start, so an app can tell its event and log numbers start over) and `last_event_seq`; on this route also `ip`, the IPv4 address the request came in on, which an E64 WS can use as its Xenia hostname (`null` over loopback, a link-local or shared (Tailscale) address, or IPv6), and `ip_in_docker_range`: whether it is in 172.17.0.0 to 172.31.255.255, where Docker most likely gave it to its container, so the grinder can't reach it |
//! | PUT | `/api/settings` | `{"shot_window_s":120,"grinder_host":"192.168.1.30","update_check":false}` (any of them) — changes and saves them; they win over flags from then on. `null` goes back to the start setting (the flag or environment variable, else the default); `"grinder_host":""` removes the address (without a start address, the same as `null`). Replies with the settings as in `/api/state` |
//! | GET | `/api/integrations/kinds` | the integrations the app can add: title, icon, setup form; whether additions are saved |
//! | POST | `/api/integrations` | `{"kind":"la_marzocco","settings":{...}}` — adds and starts one |
//! | GET | `/api/integrations/<id>/settings` | the settings of one added in the app, without passwords (`secrets_set` names the ones that are set) |
//! | PUT | `/api/integrations/<id>` | `{"settings":{...}}` — changes one added in the app and restarts it; a password left out or empty stays |
//! | POST | `/api/integrations/<id>/adopt` | moves one set on the command line into the app (needs a settings file) |
//! | DELETE | `/api/integrations/<id>` | stops and removes one added in the app; replies `{"id":…}`, as adding and changing do |
//! | GET | `/api/history` | `shots`: the last 200 shots the grinder asked for, newest first: how each ended as `outcome` (`succeeded`, `aborted` or `failed`), `started_at` and `ended_at` (UTC, whole seconds; `ended_at` as the clock read when the shot ended, `started_at` counted back from it by the shot's length on a clock that does not jump), `running_ms` (how long it ran, from the grinder's request to its end, on that clock; for shots saved by earlier builds, `ended_at` less `started_at`), `time_ms` and `weight_g` of each that succeeded, with `in_grinder_range` and `integration_id` as on `shot_reported` (`in_grinder_range` is `null` when the shot's window was not known, see `grinder.shot_range`; both are `null` on shots saved by earlier builds), `target_time_ms` and `target_weight_g` when the grinder said, each other with `reason` and `cause` as on `shot_aborted` (or `shot_end_missed`: still open when the next one started); `capacity` (200); `persistent`: whether they survive a restart (false while saving fails) |
//! | GET | `/api/log?after=N` | `lines`: the server's last 1000 log lines (`at`, the server's clock, which may not be set; `age_ms`, how long ago; `level`: `info`, `warn` or `error`; `text`) with `seq > N`; `last_seq`: the newest line's number; `capacity` (1000) |
//! | GET | `/api/log/stream` | new log lines, live, as server-sent events (`log`), each as in `/api/log` with its `age_ms` as it is sent; `Last-Event-ID` (or else `?after=N`) sends the missed ones first |
//! | GET | `/api/events?after=N` | events with `seq > N`, each with its `kind`: `grinder_grind_finished` (`result`: the grind's `target_weight_g`, `filter`, `raw`), `grinder_shot_requested` (an E64 WS's `script_id`, an E65/E80's `target_time_ms`), `shot_started`, `shot_reported` (`result`: `time_ms`, `weight_g`; `in_grinder_range`: whether the grinder adjusts from its time, by the window of the grinder's request, see `grinder.shot_range`, or `null` when that is not known; `integration_id`: the `id` of the integration that reported it, `null` when it was typed in the app or sent through this API), `shot_aborted` and `shot_finished`; `shot_reported` and `shot_aborted` say how long the brew ran until then as `running_ms`; `age_ms` says how long ago each happened. `shot_aborted` says why as `cause`, a fixed code to match on (`shot_aborted_by_user`, `shot_timed_out`, `grinder_shot_dropped`, … see `AbortCause`), and in English words as `reason` |
//! | GET | `/api/events/stream` | the same, live, as server-sent events named by `kind`, each with its `seq` as the event `id`, none skipped; `Last-Event-ID` (or else `?after=N`) sends the missed ones still kept first, all of them for an `N` from before a restart |
//! | POST | `/api/shot/result` | `{"time_s":30,"weight_g":36}` (or `time_ms`) — ends the shot with these numbers; without a weight, the recipe's |
//! | POST | `/api/shot/abort` | ends the shot as a user abort (grinder skips it) |
//! | POST | `/api/shot/start` | starts a shot without a grinder request (an E64 WS sees a flush; an E65/E80 nothing) |
//! | POST | `/api/machine` | patch the `ON`-state mako fields, e.g. `{"TANK_LEVEL":0}` |
//! | POST | `/api/grinder/pairing` | `{"pairing_id":1234}` — the pairing ID an E65/E80 grinder shows, only while the link asks for it (`phase` `asking`); refused with `grinder_already_paired` while paired, `grinder_not_asking` otherwise |
//! | POST | `/api/grinder/accepted` | the person confirms they accepted gbs-anywhere on the grinder: it checks the pairing 30 s later, and sends the grinder nothing meanwhile (`phase` `holding`); only while the link waits for it (`phase` `accepting`), empty body; refused with `grinder_not_accepting` otherwise, `grinder_not_set_up` without an E65/E80 and `grinder_pairing_busy` while the last one is handled |
//! | PUT/DELETE | `/api/overrides` | raw keys forced into every mako reply |

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use crate::protocol::machine::EventRecord;
use crate::protocol::{MachineError, MakoState, Phase, ShotResult};
use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio::sync::broadcast::error::RecvError;

use crate::integration::{self, ChangeError, Settings};
use crate::server::browser::{self, AllowedHosts};
use crate::server::logbook::{self, LogLine};
use crate::server::{Conn, Server, SettingsChange, history};

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
        let phase = m.phase(now);
        let mut shot = serde_json::to_value(phase).unwrap_or(Value::Null);
        if let Value::Object(o) = &mut shot {
            o.insert("last".into(), json!(m.last_shot()));
            // How long the shot has run, for a timer that does not depend
            // on the phone's clock.
            if let Phase::Running { since, .. } = phase {
                let age = now.saturating_duration_since(since).as_millis();
                o.insert("phase_age_ms".into(), json!(age as u64));
            }
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
                        let age = |t: Instant| now.saturating_duration_since(t).as_millis() as u64;
                        o.insert("phase_age_ms".into(), json!(w.phase_since.map(age)));
                        o.insert("state_age_ms".into(), json!(w.state_since.map(age)));
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

async fn get_state(
    State(x): State<Arc<Server>>,
    conn: Option<Extension<ConnectInfo<Conn>>>,
) -> Json<Value> {
    let mut state = state_json(&x);
    let ip = conn
        .and_then(|Extension(ConnectInfo(c))| c.local)
        .and_then(|a| grinder_ip(a.ip()));
    if let Some(server) = state["server"].as_object_mut() {
        server.insert("ip".into(), json!(ip));
        server.insert(
            "ip_in_docker_range".into(),
            json!(ip.is_some_and(in_docker_range)),
        );
    }
    Json(state)
}

/// The address the grinder can reach this server at, from the one a request
/// came in on: an IPv4 address that is not loopback, link-local (what a
/// computer takes when no router gave it one) or shared (100.64.0.0/10, as
/// Tailscale uses), else `None`.
fn grinder_ip(local: IpAddr) -> Option<Ipv4Addr> {
    match local.to_canonical() {
        IpAddr::V4(ip)
            if !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() && !shared(ip) =>
        {
            Some(ip)
        }
        _ => None,
    }
}

/// 100.64.0.0/10 (`Ipv4Addr::is_shared` is not stable yet).
fn shared(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && b & 0xc0 == 64
}

/// Whether `ip` is in the part of Docker's built-in network pools that home
/// networks don't use: 172.17.0.0 to 172.31.255.255. There, it is most
/// likely a container's own address, which the grinder can't reach. (The
/// pools also take subnets of 192.168.0.0/16, which says nothing.)
fn in_docker_range(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 172 && (17..=31).contains(&b)
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

/// Live events, as `/api/log/stream` sends log lines: with the
/// `Last-Event-ID` a browser sends when it reconnects, or else `?after=N`,
/// the events after `N` that are still kept come first; an `N` beyond the
/// last event (the server restarted since) sends them all.
async fn stream_events(
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
    let (rx, now) = x.subscribe_at();
    let after = match resume {
        Some(after) if after <= now => after,
        // From before a restart (the count started again): all kept events.
        Some(_) => 0,
        None => now,
    };
    let start = EventStream {
        pending: x.events_after(after),
        server: x.clone(),
        rx,
        last: after,
    };
    let stream = futures_util::stream::unfold(start, |mut s| async move {
        let rec = s.next().await?;
        let data = event_json(&rec, Instant::now());
        let kind = data["kind"].as_str().unwrap_or("event").to_owned();
        let ev = Event::default()
            .event(kind)
            .id(rec.seq.to_string())
            .data(data.to_string());
        Some((Ok::<_, Infallible>(ev), s))
    });
    // Ends when the server stops, so it can close the connection.
    let stream = futures_util::StreamExt::take_until(stream, x.stopped());
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// The events for one stream, in order and without gaps: when the live
/// feed falls behind, the missed events are read back from the machine.
struct EventStream {
    server: Arc<Server>,
    rx: tokio::sync::broadcast::Receiver<EventRecord>,
    pending: std::collections::VecDeque<EventRecord>,
    /// The last `seq` sent (or skipped as already known).
    last: u64,
}

impl EventStream {
    async fn next(&mut self) -> Option<EventRecord> {
        loop {
            if let Some(rec) = self.pending.pop_front() {
                if rec.seq > self.last {
                    self.last = rec.seq;
                    return Some(rec);
                }
                continue;
            }
            match self.rx.recv().await {
                Ok(rec) => self.pending.push_back(rec),
                Err(RecvError::Lagged(_)) => self.pending = self.server.events_after(self.last),
                Err(RecvError::Closed) => return None,
            }
        }
    }
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
    let now = Instant::now();
    Json(json!({
        "lines": lines.iter().map(|l| log_json(l, now)).collect::<Vec<_>>(),
        "last_seq": last_seq,
        "capacity": x.log().capacity(),
    }))
}

/// A log line as the API sends it, with `age_ms`: how long before `now` it
/// was logged, as [`event_json`] does for events. Its `at` is the server's
/// wall clock, which may not be set.
fn log_json(line: &LogLine, now: Instant) -> Value {
    let mut v = serde_json::to_value(line).unwrap_or(Value::Null);
    if let Value::Object(o) = &mut v {
        let age = now.saturating_duration_since(line.logged).as_millis();
        o.insert(
            "age_ms".into(),
            json!(u64::try_from(age).unwrap_or(u64::MAX)),
        );
    }
    v
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
            .data(log_json(&line, Instant::now()).to_string());
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
        CSS_SLOT, JS_SLOT, LANGS, PAGE_CSS, PAGE_HTML, PAGE_JS, STRINGS_SLOT, VERSION_SLOT,
        grinder_ip, in_docker_range, page, script_safe,
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
        let markup = &PAGE_HTML[PAGE_HTML.find("<body").expect("body")..];
        // Every quoted string in the script that reads as a key of one of
        // the catalog's groups: in t() and tv(), and in the tables and
        // ternaries the script picks its keys from. A key is never made by
        // adding to a prefix, except for the prefixes in DYNAMIC, whose
        // keys other tests check.
        const DYNAMIC: [&str; 4] = [
            "kind.",         // kindKey(): integration texts, integration_texts_are_german
            "integ.detail.", // integDetail(): the same
            "history.kind.", // kindLabel(): the_page_knows_every_shot_kind
            "error.",        // errorText(): the_page_words_every_error_code
        ];
        let de = catalog("de");
        let groups: BTreeSet<&str> = en
            .keys()
            .chain(de.keys())
            .filter_map(|k| k.split('.').next())
            .collect();
        let mut used = BTreeSet::new();
        let mut prefixes = BTreeSet::new();
        for literal in js_strings(PAGE_JS) {
            let dotted = literal.contains('.')
                && literal.starts_with(|c: char| c.is_ascii_lowercase())
                && literal
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
            let group = literal.split('.').next().unwrap_or("");
            if !dotted || !groups.contains(group) {
                continue;
            }
            if literal.ends_with('.') {
                prefixes.insert(literal);
            } else {
                used.insert(literal.to_owned());
            }
        }
        let dynamic: BTreeSet<&str> = DYNAMIC.into_iter().collect();
        assert_eq!(prefixes, dynamic, "keys made from a prefix");
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
        assert!(used.len() > 300, "the page's keys were found");
        // Keys picked through tables, not t("…"), are in the scan too.
        for key in [
            "badge.pressKnob",
            "setup.pill.restart",
            "grinder.state.problem",
        ] {
            assert!(used.contains(key), "{key} was found");
        }
        let unknown: Vec<_> = used.iter().filter(|k| !known(k)).collect();
        assert!(unknown.is_empty(), "keys not in the catalog: {unknown:?}");
    }

    /// The contents of the double- and single-quoted strings in `script`,
    /// with comments and template strings left out.
    fn js_strings(script: &str) -> Vec<&str> {
        let bytes = script.as_bytes();
        let mut found = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    i = script[i..].find('\n').map_or(bytes.len(), |n| i + n);
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    i = script[i + 2..]
                        .find("*/")
                        .map_or(bytes.len(), |n| i + n + 4);
                }
                quote @ (b'"' | b'\'' | b'`') => {
                    let start = i + 1;
                    i = start;
                    while i < bytes.len() && bytes[i] != quote {
                        i += if bytes[i] == b'\\' { 2 } else { 1 };
                    }
                    assert!(i < bytes.len(), "a string that does not end at {start}");
                    if quote != b'`' {
                        found.push(&script[start..i]);
                    }
                    i += 1;
                }
                _ => i += 1,
            }
        }
        found
    }

    /// The keys of the script's table `const <name> = { … };`, one
    /// `    <key>: …` line each.
    fn js_table(name: &str) -> Vec<&'static str> {
        let head = format!("\n  const {name} = {{\n");
        let start = PAGE_JS
            .find(&head)
            .unwrap_or_else(|| panic!("the {name} table"));
        let table = &PAGE_JS[start + head.len()..];
        let table = &table[..table
            .find("\n  };")
            .unwrap_or_else(|| panic!("end of {name}"))];
        table
            .lines()
            .filter_map(|line| line.strip_prefix("    ")?.split_once(": "))
            .map(|(key, _)| key)
            .collect()
    }

    #[test]
    fn the_page_knows_every_integration_health() {
        use crate::integration::Health;
        let all = [
            Health::Starting,
            Health::Connected,
            Health::Watching,
            Health::Failing,
            Health::Stopped,
        ];
        for health in all {
            // No wildcard: a new health has to be named here, next to `all`.
            match health {
                Health::Starting
                | Health::Connected
                | Health::Watching
                | Health::Failing
                | Health::Stopped => {}
            }
        }
        let names: BTreeSet<String> = all
            .iter()
            .map(|h| {
                serde_json::to_value(h)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let table: BTreeSet<String> = js_table("HEALTH").into_iter().map(str::to_owned).collect();
        assert_eq!(table, names, "HEALTH in app.js and the server's Health");
    }

    #[test]
    fn the_page_knows_every_shot_phase() {
        use crate::protocol::{Phase, ShotResult};
        use std::time::Instant;
        let all = [
            Phase::Ready,
            Phase::Running {
                since: Instant::now(),
                requested_by_grinder: true,
            },
            Phase::Finishing {
                until: Instant::now(),
                result: ShotResult {
                    time_ms: 25_000,
                    weight_g: 36.0,
                },
                aborted: false,
            },
        ];
        for phase in all {
            // No wildcard: a new phase has to be named here, next to `all`.
            match phase {
                Phase::Ready | Phase::Running { .. } | Phase::Finishing { .. } => {}
            }
        }
        let names: BTreeSet<String> = all
            .iter()
            .map(|p| {
                serde_json::to_value(p).unwrap()["phase"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let table: BTreeSet<String> = js_table("GROUP").into_iter().map(str::to_owned).collect();
        assert_eq!(table, names, "GROUP in app.js and the server's Phase");
    }

    #[test]
    fn the_page_listens_for_every_event() {
        use crate::protocol::{AbortCause, GrindResult, MachineEvent, ShotResult};
        let result = ShotResult {
            time_ms: 25_000,
            weight_g: 36.0,
        };
        let all = [
            MachineEvent::GrinderGrindFinished {
                result: GrindResult {
                    target_weight_g: Some(36.0),
                    filter: None,
                    raw: serde_json::Value::Null,
                },
            },
            MachineEvent::GrinderShotRequested {
                script_id: Some(9),
                target_time_ms: None,
            },
            MachineEvent::ShotStarted {
                requested_by_grinder: true,
                human_action: "",
            },
            MachineEvent::ShotReported {
                result,
                in_grinder_range: None,
                running_ms: 25_000,
                integration_id: None,
            },
            MachineEvent::shot_aborted(
                AbortCause::ShotTimedOut,
                std::time::Duration::from_secs(180),
            ),
            MachineEvent::ShotFinished,
        ];
        for event in &all {
            // No wildcard: a new event has to be named here, next to `all`.
            match event {
                MachineEvent::GrinderGrindFinished { .. }
                | MachineEvent::GrinderShotRequested { .. }
                | MachineEvent::ShotStarted { .. }
                | MachineEvent::ShotReported { .. }
                | MachineEvent::ShotAborted { .. }
                | MachineEvent::ShotFinished => {}
            }
        }
        let kinds: BTreeSet<String> = all
            .iter()
            .map(|e| {
                serde_json::to_value(e).unwrap()["kind"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        // The stream names every event by its kind (stream_events), so the
        // page listens for exactly these.
        let start = PAGE_JS.find("const EVENT_KINDS = [").expect("EVENT_KINDS");
        let list = &PAGE_JS[start..];
        let list = &list[..list.find("];").expect("end of EVENT_KINDS")];
        let listened: BTreeSet<String> = js_strings(list).into_iter().map(str::to_owned).collect();
        assert_eq!(listened, kinds, "EVENT_KINDS in app.js and MachineEvent");
    }

    #[test]
    fn the_theme_colour_is_the_background() {
        // --bg of a palette in app.css: `--bg: #…;` in the block after `head`.
        let bg = |head: &str| {
            let block = &PAGE_CSS[PAGE_CSS.find(head).unwrap_or_else(|| panic!("{head}"))..];
            let block = &block[..block.find('}').unwrap()];
            let at = block.find("--bg:").expect("--bg") + "--bg:".len();
            block[at..].split(';').next().unwrap().trim().to_owned()
        };
        let light = bg(":root {");
        let dark = bg(":root[data-theme=\"dark\"] {");
        // The meta tag the page starts with, and the head script's choice.
        assert!(
            PAGE_HTML.contains(&format!("<meta name=\"theme-color\" content=\"{light}\">")),
            "the theme-color meta tag is {light}"
        );
        assert!(
            PAGE_HTML.contains(&format!("dark ? \"{dark}\" : \"{light}\"")),
            "the head script sets {dark} and {light}"
        );
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

    /// The script's table `const <name> = { … };` of `    <code>: "<key>",`
    /// lines, as code and catalog key, checked against `codes`: one row per
    /// code, each key in the catalog.
    fn check_key_table(name: &str, codes: BTreeSet<String>) {
        let head = format!("\n  const {name} = {{\n");
        let start = PAGE_JS.find(&head).unwrap_or_else(|| panic!("{name}"));
        let table = &PAGE_JS[start + head.len()..];
        let table = &table[..table.find("\n  };").expect("end of the table")];
        let en = catalog("en");
        let mut rows = BTreeSet::new();
        for line in table.lines() {
            let (code, key) = line
                .trim()
                .split_once(": ")
                .unwrap_or_else(|| panic!("{name}: {line}"));
            let key = key.trim_end_matches(',').trim_matches('"');
            assert!(
                en.contains_key(key),
                "{name}.{code}: {key} is not in the catalog"
            );
            rows.insert(code.to_owned());
        }
        assert_eq!(rows, codes, "{name} in app.js and the server's codes");
    }

    /// The JSON string `value` serializes to.
    fn code_of(value: impl serde::Serialize) -> String {
        serde_json::to_value(value)
            .unwrap()
            .as_str()
            .expect("a code")
            .to_owned()
    }

    #[test]
    fn the_page_words_every_link_problem() {
        use crate::protocol::AbortCause;
        use crate::server::wifi_grinder::{ProblemCause, ResultProblem};
        for cause in ProblemCause::ALL {
            // No wildcard: a new problem has to be named here, next to `ALL`.
            match cause {
                ProblemCause::Shot(
                    AbortCause::GrinderShotDropped
                    | AbortCause::GrinderShotRefused
                    | AbortCause::GrinderUnpaired,
                )
                | ProblemCause::Result(
                    ResultProblem::GrinderResultExpired
                    | ResultProblem::GrinderResultRefused
                    | ResultProblem::GrinderResultDelayed,
                ) => {}
                ProblemCause::Shot(other) => panic!("{other:?} is not a link problem"),
            }
        }
        let codes = ProblemCause::ALL.into_iter().map(code_of).collect();
        check_key_table("PROBLEMS", codes);
    }

    #[test]
    fn the_page_words_every_grind_block() {
        use crate::protocol::wire::GrindBlocked;
        for b in GrindBlocked::ALL {
            // No wildcard: a new cause has to be named here, next to `ALL`.
            match b {
                GrindBlocked::ShotRunning
                | GrindBlocked::MachineInStandby
                | GrindBlocked::MachineInEco
                | GrindBlocked::MachineTesting
                | GrindBlocked::MachineFaulty
                | GrindBlocked::MachineWarmingUp
                | GrindBlocked::MachineTankEmpty => {}
            }
        }
        let codes = GrindBlocked::ALL.into_iter().map(code_of).collect();
        check_key_table("BLOCKED", codes);
    }

    /// Every `code` an error reply can carry: the codes of the error types,
    /// and the literal ones in `error(<status>, "<code>", …)` anywhere in
    /// `src/` (read from the source, so a new one can't be left out).
    fn error_codes() -> BTreeSet<String> {
        use crate::integration::ChangeError;
        use crate::protocol::MachineError;
        use crate::server::SettingsError;
        use crate::server::wifi_grinder::PairingIdRefusal;
        let machine = [
            MachineError::NotRunning,
            MachineError::AlreadyRunning,
            MachineError::NoWeight,
        ];
        let settings = [
            SettingsError::Save(String::new()),
            SettingsError::Grinder(String::new()),
        ];
        let change = [
            ChangeError::Invalid(String::new()),
            ChangeError::NotFound,
            ChangeError::CommandLine,
            ChangeError::InApp,
            ChangeError::NotPersistent,
            ChangeError::Save(String::new()),
        ];
        let pairing = [
            PairingIdRefusal::NoLink,
            PairingIdRefusal::NotNeeded,
            PairingIdRefusal::NotAsking,
            PairingIdRefusal::NotAccepting,
            PairingIdRefusal::Busy,
        ];
        // No wildcards: a new error has to be named here, next to its list.
        for e in &machine {
            match e {
                MachineError::NotRunning
                | MachineError::AlreadyRunning
                | MachineError::NoWeight => {}
            }
        }
        for e in &settings {
            match e {
                SettingsError::Save(_) | SettingsError::Grinder(_) => {}
            }
        }
        for e in &change {
            match e {
                ChangeError::Invalid(_)
                | ChangeError::NotFound
                | ChangeError::CommandLine
                | ChangeError::InApp
                | ChangeError::NotPersistent
                | ChangeError::Save(_) => {}
            }
        }
        for e in pairing {
            match e {
                PairingIdRefusal::NoLink
                | PairingIdRefusal::NotNeeded
                | PairingIdRefusal::NotAsking
                | PairingIdRefusal::NotAccepting
                | PairingIdRefusal::Busy => {}
            }
        }
        let mut codes: BTreeSet<String> = machine
            .iter()
            .map(MachineError::code)
            .chain(settings.iter().map(SettingsError::code))
            .chain(change.iter().map(ChangeError::code))
            .chain(pairing.iter().map(PairingIdRefusal::code))
            .map(str::to_owned)
            .collect();
        let mut dirs = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut literal = BTreeSet::new();
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap();
                for (i, _) in source.match_indices("error(") {
                    let before = source[..i].chars().next_back();
                    let line = &source[source[..i].rfind('\n').map_or(0, |n| n + 1)..i];
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
                        || line.trim_start().starts_with("//")
                    {
                        continue;
                    }
                    // The status, then the code as a literal.
                    let args = &source[i + "error(".len()..];
                    let Some((_, rest)) = args.split_once(',') else {
                        continue;
                    };
                    let Some(rest) = rest.trim_start().strip_prefix('"') else {
                        continue;
                    };
                    let code = &rest[..rest.find('"').unwrap()];
                    let snake = code.contains('_')
                        && code.chars().all(|c| c.is_ascii_lowercase() || c == '_');
                    assert!(snake, "{}: error code {code:?}", path.display());
                    literal.insert(code.to_owned());
                }
            }
        }
        for code in [
            "request_invalid",
            "request_host_refused",
            "settings_invalid",
        ] {
            assert!(literal.contains(code), "{code} was found in the source");
        }
        codes.extend(literal);
        codes
    }

    #[test]
    fn the_page_words_every_error_code() {
        // The page's own: no answer came, or one without the API's body.
        let page = ["request_failed", "request_status"];
        for code in page {
            assert!(PAGE_JS.contains(&format!("\"{code}\"")), "{code} in app.js");
        }
        let mut codes = error_codes();
        codes.extend(page.map(str::to_owned));
        let en = catalog("en");
        let missing: Vec<_> = codes
            .iter()
            .filter(|c| !en.contains_key(format!("error.{c}").as_str()))
            .collect();
        assert!(missing.is_empty(), "codes without words: {missing:?}");
        // And none for a code that is gone; `error.<code>.<field>` words
        // one for a field the page sent.
        let stale: Vec<_> = en
            .keys()
            .filter_map(|k| k.strip_prefix("error."))
            .filter(|rest| !codes.contains(rest.split('.').next().unwrap()))
            .collect();
        assert!(stale.is_empty(), "words for no code: {stale:?}");
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

    #[test]
    fn grinder_ip_is_an_ipv4_address_off_this_computer() {
        let ip = |s: &str| grinder_ip(s.parse().unwrap()).map(|ip| ip.to_string());
        assert_eq!(ip("192.168.1.20").as_deref(), Some("192.168.1.20"));
        assert_eq!(
            ip("::ffff:192.168.1.20").as_deref(),
            Some("192.168.1.20"),
            "IPv4 on an IPv6 socket"
        );
        assert_eq!(ip("127.0.0.1"), None, "this computer only");
        assert_eq!(ip("::1"), None);
        assert_eq!(ip("169.254.83.10"), None, "link-local");
        assert_eq!(ip("100.101.102.103"), None, "Tailscale");
        assert_eq!(ip("100.63.0.1").as_deref(), Some("100.63.0.1"));
        assert_eq!(ip("100.128.0.1").as_deref(), Some("100.128.0.1"));
        assert_eq!(ip("0.0.0.0"), None);
        assert_eq!(ip("fe80::1"), None, "IPv6");
    }

    #[test]
    fn docker_range_leaves_home_networks_out() {
        let docker = |s: &str| in_docker_range(s.parse().unwrap());
        assert!(docker("172.17.0.2"), "the default bridge");
        assert!(docker("172.18.0.5"), "a compose network");
        assert!(docker("172.31.255.254"));
        assert!(!docker("172.16.0.10"));
        assert!(!docker("172.32.0.1"));
        assert!(!docker("192.168.1.20"));
        assert!(!docker("10.0.0.5"));
    }
}
