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
//! | GET | `/api/state` | everything, by group (see [below](#get-apistate)) |
//! | PUT | `/api/settings` | `{"shot_window_s":120,"grinder_host":"192.168.1.30","update_check":false}` (any of them) — changes and saves them (see [below](#put-apisettings)) |
//! | GET | `/api/integrations/kinds` | the integrations the app can add: title, icon, setup form; whether additions are saved |
//! | POST | `/api/integrations` | `{"kind":"la_marzocco","settings":{...}}` — adds and starts one |
//! | GET | `/api/integrations/<id>/settings` | the settings of one added in the app, without passwords (`secrets_set` names the ones that are set) |
//! | PUT | `/api/integrations/<id>` | `{"settings":{...}}` — changes one added in the app and restarts it; a password left out or empty stays |
//! | POST | `/api/integrations/<id>/adopt` | moves one set on the command line into the app (needs a settings file) |
//! | DELETE | `/api/integrations/<id>` | stops and removes one added in the app; replies `{"id":…}`, as adding and changing do |
//! | GET | `/api/history` | the last shots the grinder asked for (see [below](#get-apihistory)) |
//! | GET | `/api/log?after=N` | the server's last log lines with `seq > N` (see [below](#get-apilog)) |
//! | GET | `/api/log/stream` | new log lines, live, as server-sent events (`log`), each as in `/api/log` with its `age_ms` as it is sent; `Last-Event-ID` (or else `?after=N`) sends the missed ones first |
//! | GET | `/api/events?after=N` | events with `seq > N`, each with its `kind` (see [below](#get-apievents)) |
//! | GET | `/api/events/stream` | the same, live, as server-sent events named by `kind`, each with its `seq` as the event `id`, none skipped; `Last-Event-ID` (or else `?after=N`) sends the missed ones still kept first, all of them for an `N` from before a restart |
//! | POST | `/api/shot/result` | `{"time_s":30,"weight_g":36}` (or `time_ms`) — ends the shot with these numbers; without a weight, the recipe's |
//! | POST | `/api/shot/abort` | ends the shot as a user abort (grinder skips it) |
//! | POST | `/api/shot/start` | starts a shot without a grinder request (an E64 WS sees a flush; an E65/E80 nothing) |
//! | POST | `/api/machine` | patch the `ON`-state mako fields, e.g. `{"TANK_LEVEL":0}` |
//! | POST | `/api/grinder/pairing` | `{"pairing_id":1234}` — the pairing ID an E65/E80 grinder shows, only while the link asks for it (see [below](#post-apigrinderpairing)) |
//! | POST | `/api/grinder/accepted` | the person confirms they accepted gbs-anywhere on the grinder; empty body (see [below](#post-apigrinderaccepted)) |
//! | PUT/DELETE | `/api/overrides` | raw keys forced into every mako reply |
//!
//! The routes with more to say follow, one line per group or field, so a
//! change to one touches only its line. The numbers in them are checked
//! against the constants they come from (`the_api_reference_has_the_current_numbers`).
//!
//! ## GET /api/state
//!
//! Everything, by group:
//!
//! * `shot`: its `phase` (`ready`, `running`, `finishing`); while running, `requested_by_grinder` and `phase_age_ms` (how long it has run); while finishing, `result` and `aborted`; and the `last` result.
//! * `grinder`: the link: `peer`, `polls`, `requests`, `last_poll_age_ms`, and `wifi` for an E65/E80 (see `grinder.wifi` below).
//! * `grinder.last_grind`: `target_weight_g`, `filter`, `raw`.
//! * `grinder.grind_blocked`: `cause`, e.g. `machine_tank_empty`, and `grinder_message`, what the grinder shows, in English.
//! * `grinder.shot_range`: the shot times the grinder adjusts from for its current or last request, over `min_ms` and up to `max_ms`, as `in_grinder_range` checks them: 10 000 and 80 000 after an E64 WS knob press, 15 000 below (but at least 10 000) and 15 000 above the target time after an E65/E80 brew job; `null` before the grinder's first request and after a brew job without a target time.
//! * `machine`: the current `mako` reply, the `overrides`, and its `config` (`shot_window_s`, 0 for no limit, `finishing_hold_s`, …).
//! * `integrations`: each with
//!   * its `id` (e.g. `la_marzocco`, `la_marzocco-2`, as `integration_id` names it on `shot_reported`), `kind`, `title` and `subject` (what it watches once known, e.g. a machine's serial number and model; else empty);
//!   * its `health` (`starting`, `connected`, `watching`, `failing`, `stopped`) and `health_age_ms`;
//!   * its status line `detail` in English, also as `detail_key` and `detail_vars` for wording it in another language (an `age_ms` among them is as of this reply), and `last_error` (while `failing`, in English);
//!   * `reports` (the shots it reported since the start) and `last_report` (the last one's `time_ms` and `weight_g`, or `null`);
//!   * its `source` (`app`, or `command_line` for one from the start settings), `adoptable` (from the command line, and there is a settings file to move it into, see `/adopt`) and `from_command_line` (moved into the app from there, so deleting it brings the flags' one back at the next start).
//! * `settings`: each setting, with
//!   * its `source`, and the `start` value it goes back to;
//!   * `shot_window_range`: the shot windows from `min_s` to `max_s` seconds, 30 and 3600, besides 0 for no limit;
//!   * `persistent`: whether there is a settings file to keep them in.
//! * `update`: the update check: the `latest` release found, whether it is newer (`available`), whether looking `failing`, and the releases page `url`.
//! * `server`: `version`, `boot_id` (changes on every start, so an app can tell its event and log numbers start over) and `last_event_seq`; on this route also
//!   * `ip`: the IPv4 address the request came in on, which an E64 WS can use as its Xenia hostname (`null` over loopback, a link-local or shared (Tailscale) address, or IPv6);
//!   * `ip_in_docker_range`: whether it is in 172.17.0.0 to 172.31.255.255, where Docker most likely gave it to its container, so the grinder can't reach it.
//!
//! `grinder.wifi`, the link to an E65/E80:
//!
//! * `grinder`: its address.
//! * `state`: `connecting`, `pairing`, `paired` or `failing`.
//! * `state_age_ms`: how long it has been in its `state` (from the start of the link: a restart or a new address starts it over).
//! * `phase`, only while `pairing`:
//!   * `accepting` while gbs-anywhere shows the code and waits for the person to accept it on the grinder and confirm that in the app, for as long as it takes (meanwhile it checks the pairing with the grinder every 40 s);
//!   * `holding` while it waits 30 s after the confirmation and sends the grinder nothing;
//!   * `checking` while it checks the pairing with the grinder after that (if the grinder has not accepted it yet, the phase is `accepting` again);
//!   * `quiet` while gbs-anywhere sends the grinder nothing for a while after a pairing check went unanswered (the grinder may be restarting);
//!   * `asking` while the grinder has it paired but the pairing ID is missing here.
//! * `phase_age_ms`: how long the link has been in that phase (for `quiet`, in the current round).
//! * `quiet_ms`: how long a `quiet` round lasts.
//! * `pairing_code` and `pairing_id` carry the grinder's own names: `pairing_code` is the "Pairing Code" it shows for one attempt (`pairingCode` in its protocol), `pairing_id` the "Pairing ID", the pairing's lasting number (`pairingId`).
//! * `unknown_pairing_id`: the last pairing ID the grinder did not take, while asking.
//! * `pairing_lost` and `unreachable`.
//! * `not_set_up`: not paired, and the grinder answered the pairing application with a client error, as one not set up for pairing would.
//! * `detail`, and `failure`: while `failing`, why, in English, without the steps before it that `detail` lists, e.g. `operation timed out`.
//! * `problem`: what went wrong with the last shot, in English, until the next one goes through.
//! * `problem_cause`: the same as a code: the `AbortCause` of a brew that ended without an answer (`grinder_shot_dropped`, `grinder_shot_refused`, `grinder_unpaired`), or `grinder_result_expired` (the result did not reach the grinder in time), `grinder_result_refused` or `grinder_result_delayed` (not yet, still trying).
//!
//! ## PUT /api/settings
//!
//! * Any of `shot_window_s`, `grinder_host` and `update_check`: changes and saves them; they win over flags from then on.
//! * `null` goes back to the start setting (the flag or environment variable, else the default).
//! * `"grinder_host":""` removes the address (without a start address, the same as `null`).
//! * Replies with the settings as in `/api/state`.
//!
//! ## GET /api/history
//!
//! * `shots`: the last 200 shots the grinder asked for, newest first, each with
//!   * `outcome`: how it ended, `succeeded`, `aborted` or `failed`;
//!   * `started_at` and `ended_at` (UTC, whole seconds; `ended_at` as the clock read when the shot ended, `started_at` counted back from it by the shot's length on a clock that does not jump);
//!   * `running_ms`: how long it ran, from the grinder's request to its end, on that clock; for shots saved by earlier builds, `ended_at` less `started_at`;
//!   * for each that succeeded, `time_ms` and `weight_g`, with `in_grinder_range` and `integration_id` as on `shot_reported` (`in_grinder_range` is `null` when the shot's window was not known, see `grinder.shot_range`; both are `null` on shots saved by earlier builds);
//!   * `target_time_ms` and `target_weight_g` when the grinder said;
//!   * for each other, `reason` and `cause` as on `shot_aborted` (or `shot_end_missed`: still open when the next one started).
//! * `capacity` (200).
//! * `persistent`: whether they survive a restart (false while saving fails).
//!
//! ## GET /api/log
//!
//! * `lines`: the server's last 1000 log lines with `seq > N`, each with `at` (the server's clock, which may not be set), `age_ms` (how long ago), `level` (`info`, `warn` or `error`) and `text`.
//! * `last_seq`: the newest line's number.
//! * `capacity` (1000).
//!
//! ## GET /api/events
//!
//! The events with `seq > N`, each with its `kind`, and `age_ms`: how long ago it happened.
//!
//! * `grinder_grind_finished`: `result`, the grind's `target_weight_g`, `filter`, `raw`.
//! * `grinder_shot_requested`: an E64 WS's `script_id`, an E65/E80's `target_time_ms`.
//! * `shot_started`.
//! * `shot_reported`: `result` (`time_ms`, `weight_g`); `in_grinder_range`: whether the grinder adjusts from its time, by the window of the grinder's request (see `grinder.shot_range`), or `null` when that is not known; `integration_id`: the `id` of the integration that reported it, `null` when it was typed in the app or sent through this API.
//! * `shot_aborted`: why, as `cause`, a fixed code to match on (`shot_aborted_by_user`, `shot_timed_out`, `grinder_shot_dropped`, … see `AbortCause`), and in English words as `reason`.
//! * `shot_finished`.
//! * `shot_reported` and `shot_aborted` say how long the brew ran until then as `running_ms`.
//!
//! ## POST /api/grinder/pairing
//!
//! * Taken only while the link asks for the pairing ID (`phase` `asking`).
//! * Refused with `grinder_already_paired` while paired, `grinder_not_asking` otherwise.
//!
//! ## POST /api/grinder/accepted
//!
//! * gbs-anywhere checks the pairing 30 s later, and sends the grinder nothing meanwhile (`phase` `holding`).
//! * Taken only while the link waits for it (`phase` `accepting`).
//! * Refused with `grinder_not_accepting` otherwise, `grinder_not_set_up` without an E65/E80 and `grinder_pairing_busy` while the last one is handled.

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
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
use crate::server::{Conn, Server, SettingsChange, history, page};

/// The app's routes, answering only to home-network host names (`hosts`
/// adds more) and refusing writes from other sites.
pub fn control_router(state: Arc<Server>, hosts: AllowedHosts) -> axum::Router {
    // The page is built now, so a broken template fails when the server
    // starts, not on someone's first visit.
    page::page();
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
        axum::response::Html(page::page()),
    )
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
    use super::{grinder_ip, in_docker_range};

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

    /// `text` as one line: its words with single spaces between them, as a
    /// phrase reads whichever way it was wrapped.
    fn one_line<'a>(text: impl IntoIterator<Item = &'a str>) -> String {
        let words: Vec<&str> = text.into_iter().flat_map(str::split_whitespace).collect();
        words.join(" ")
    }

    /// Milliseconds as the API reference writes them: `10 000`.
    fn grouped(ms: u32) -> String {
        let digits = ms.to_string();
        let mut out = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push(' ');
            }
            out.push(c);
        }
        out
    }

    #[test]
    fn the_api_reference_has_the_current_numbers() {
        use crate::protocol::{
            E64_POLL_INTERVAL, GrinderRange, MAX_EXTRACTION_TIME_MS, MAX_FINISHING_HOLD,
            MAX_OFF_TARGET_MS, MIN_EXTRACTION_TIME_MS, MIN_FINISHING_HOLD,
        };
        use crate::server::settings::{
            DEFAULT_SHOT_WINDOW_S, MAX_SHOT_WINDOW_S, MIN_SHOT_WINDOW_S,
        };
        use crate::server::wifi_grinder::{ACCEPT_SETTLE, UNCONFIRMED_POLL};
        use crate::server::{history, logbook};

        assert_eq!(grouped(10_000), "10 000");
        assert_eq!(grouped(800), "800");
        let (min, max, off) = (
            MIN_EXTRACTION_TIME_MS,
            MAX_EXTRACTION_TIME_MS,
            MAX_OFF_TARGET_MS,
        );
        let (settle, slow) = (ACCEPT_SETTLE.as_secs(), UNCONFIRMED_POLL.as_secs());
        let (shots, lines) = (history::CAPACITY, logbook::CAPACITY);

        // The route table and the sections after it, at the top of this file.
        let docs = one_line(
            include_str!("control_api.rs")
                .lines()
                .filter_map(|line| line.strip_prefix("//!")),
        );
        let in_docs = [
            format!("checks the pairing with the grinder every {slow} s"),
            format!("waits {settle} s after the confirmation"),
            format!("checks the pairing {settle} s later"),
            format!(
                "from `min_s` to `max_s` seconds, {MIN_SHOT_WINDOW_S} and {MAX_SHOT_WINDOW_S}, \
                 besides 0"
            ),
            format!(
                "{} and {} after an E64 WS knob press",
                grouped(min),
                grouped(max)
            ),
            format!(
                "{} below (but at least {}) and {} above the target time",
                grouped(off),
                grouped(min),
                grouped(off)
            ),
            format!("the last {shots} shots the grinder asked for"),
            format!("`capacity` ({shots})"),
            format!("the server's last {lines} log lines"),
            format!("`capacity` ({lines})"),
        ];
        for phrase in in_docs {
            assert!(docs.contains(&phrase), "not in the API reference: {phrase}");
        }

        // The same numbers where README.md tells people.
        let readme = one_line(include_str!("../../README.md").lines());
        let s = |ms: u32| ms / 1000;
        let range = |target: u32| {
            let r = GrinderRange::around(target * 1000);
            format!(
                "for {target} s: over {} s, up to {} s",
                s(r.min_ms),
                s(r.max_ms)
            )
        };
        let in_readme = [
            format!("checks the pairing {settle} s after you tap"),
            format!("it checks every {slow} s"),
            format!("Shots of {} s or less and over {} s", s(min), s(max)),
            format!(
                "only from a shot over {} s and within {} s of the",
                s(min),
                s(off)
            ),
            range(28),
            range(20),
            format!("| `--shot-window-s <s>` | {DEFAULT_SHOT_WINDOW_S} |"),
            format!("({MIN_SHOT_WINDOW_S} to {MAX_SHOT_WINDOW_S}, as in the app"),
            format!(
                "ready again, {} to {}: the E64 WS polls every {} s",
                MIN_FINISHING_HOLD.as_secs(),
                MAX_FINISHING_HOLD.as_secs(),
                E64_POLL_INTERVAL.as_secs()
            ),
            format!("the last {shots} shots the grinder asked for"),
        ];
        for phrase in in_readme {
            assert!(readme.contains(&phrase), "not in README.md: {phrase}");
        }
    }
}
