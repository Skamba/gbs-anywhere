//! The grinder and a person, end to end over HTTP: the routers the binary
//! serves, on a free local port, with [`GrinderModel`] reading the replies the
//! way the grinder does.

use std::time::Duration;

use serde_json::{Value, json};

use super::*;
use crate::protocol::{
    AbortCause, BrewState, GrinderEvent, GrinderModel, MachineStatus, MakoState, PATH_BREWRATIO,
    PATH_MACHINE, PATH_MAKO, PATH_SCRIPTS_EXECUTE, START_BREW_SCRIPT_ID,
};

/// Serves both routers on one port, like the binary with the app on the
/// grinder port.
async fn start() -> String {
    start_server().await.0
}

/// Like [`start`], also returning the server.
async fn start_server() -> (String, Arc<Server>) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = MachineConfig {
        // Long enough for a few polls below, short enough to keep tests fast.
        finishing_hold: Duration::from_millis(300),
        ..MachineConfig::default()
    };
    // A log book of its own, so tests running side by side don't mix lines.
    let state = Server::with_log(config, Arc::new(LogBook::new(logbook::CAPACITY)));
    let app = control_router(state.clone(), AllowedHosts::default())
        .merge(grinder_router(state.clone(), false));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    spawn_server(listener, app, addr);
    (format!("http://{addr}"), state)
}

/// A directory of its own under the system's temp directory.
fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gbs-anywhere-routes-{name}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A client that polls and posts like the grinder and remembers which shot
/// it accepted.
struct Grinder {
    http: reqwest::Client,
    base: String,
    model: GrinderModel,
    accepted: Option<u32>,
}

impl Grinder {
    fn new(base: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: base.to_owned(),
            model: GrinderModel::default(),
            accepted: None,
        }
    }

    async fn poll(&mut self) -> MakoState {
        let mako: MakoState = self.get(PATH_MAKO).await;
        for e in self.model.observe(&mako) {
            if let GrinderEvent::ShotAccepted { time_ms, .. } = e {
                self.accepted = Some(time_ms);
            }
        }
        mako
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> T {
        let res = self.http.get(format!("{}{path}", self.base)).send();
        res.await.unwrap().json().await.unwrap()
    }

    async fn post(&self, path: &str, body: Value) {
        let res = self.http.post(format!("{}{path}", self.base)).json(&body);
        assert_eq!(res.send().await.unwrap().status(), 200, "{path}");
    }
}

/// Sends a control API request and returns the status code and JSON body.
async fn call(method: reqwest::Method, url: String, body: Option<Value>) -> (u16, Value) {
    let mut req = reqwest::Client::new().request(method, url);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let res = req.send().await.unwrap();
    (res.status().as_u16(), res.json().await.unwrap())
}

/// Sends `request` exactly as written and returns the status of the reply,
/// for requests no HTTP client would send.
async fn raw_status(base: &str, request: &str) -> u16 {
    use tokio::io::AsyncReadExt;
    let mut conn = tokio::net::TcpStream::connect(base.trim_start_matches("http://"))
        .await
        .unwrap();
    conn.write_all(request.as_bytes()).await.unwrap();
    // Only the status line: the server may close the connection on what it
    // has not read.
    let mut reply = Vec::new();
    let mut buf = [0; 1024];
    while !reply.windows(2).any(|w| w == b"\r\n") {
        let read = tokio::time::timeout(Duration::from_secs(5), conn.read(&mut buf));
        match read.await.expect("a reply in time") {
            Ok(0) | Err(_) => break,
            Ok(n) => reply.extend_from_slice(&buf[..n]),
        }
    }
    let reply = String::from_utf8_lossy(&reply);
    reply
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("not an HTTP reply: {reply:?}"))
}

#[tokio::test]
async fn grinder_accepts_a_shot_entered_in_the_app() {
    let base = start().await;
    let mut grinder = Grinder::new(&base);

    let identity: Value = grinder.get(PATH_MACHINE).await;
    assert!(identity.get("MA_SN").is_some());
    assert_eq!(grinder.poll().await.status, MachineStatus::On);

    let state: Value = grinder.get("/api/state").await;
    assert_eq!(state["server"]["version"], crate::version());

    // Grind with a Grind-by-Sync recipe, then the knob press.
    let recipe = json!({ "SYNC_BEVERAGE_WEIGHT": 36.0 });
    grinder.post(PATH_BREWRATIO, recipe).await;
    grinder.model.request_start();
    let start = json!({ "ID": START_BREW_SCRIPT_ID });
    grinder.post(PATH_SCRIPTS_EXECUTE, start).await;
    grinder.poll().await;
    assert_eq!(grinder.model.brew_state(), BrewState::Extraction);

    // The person types the time and leaves the weight blank.
    let shot = json!({ "time_s": 30 });
    let (code, _) = call(
        reqwest::Method::POST,
        format!("{base}/api/shot/result"),
        Some(shot),
    )
    .await;
    assert_eq!(code, 200);

    for _ in 0..40 {
        if grinder.poll().await.status == MachineStatus::On {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(grinder.accepted, Some(30_000));
    assert_eq!(grinder.model.brew_state(), BrewState::Idle);
    let state: Value = grinder.get("/api/state").await;
    assert_eq!(state["shot"]["last"]["weight_g"], 36.0, "the recipe weight");
    assert_eq!(state["shot"]["phase"], "ready");

    // The events tell the shot's course, in the grinder's and the shot's
    // words.
    let events: Value = grinder.get("/api/events?after=0").await;
    let kinds: Vec<_> = events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "grinder_grind_finished",
            "grinder_shot_requested",
            "shot_started",
            "shot_reported",
            "shot_finished"
        ]
    );
    assert_eq!(events[0]["result"]["target_weight_g"], 36.0);
    assert_eq!(
        (&events[1]["script_id"], &events[1]["target_time_ms"]),
        (&json!(START_BREW_SCRIPT_ID), &Value::Null)
    );
    assert_eq!(
        events[3]["result"],
        json!({ "time_ms": 30_000, "weight_g": 36.0 })
    );
}

#[tokio::test]
async fn a_weight_sent_as_volume_ml_still_counts() {
    // Its name in 0.2.1: without it, the shot would get the recipe weight.
    // Sent with `weight_g`, as a client in between might, `weight_g` wins.
    for (shot, weight) in [
        (json!({ "time_s": 30, "volume_ml": 38.5 }), 38.5),
        (
            json!({ "time_s": 30, "weight_g": 36, "volume_ml": 38.5 }),
            36.0,
        ),
    ] {
        let base = start().await;
        let post = reqwest::Method::POST;
        call(post.clone(), format!("{base}/api/shot/start"), None).await;
        let (code, state) = call(post, format!("{base}/api/shot/result"), Some(shot)).await;
        assert_eq!(code, 200);
        assert_eq!(state["shot"]["last"]["weight_g"], weight);
    }
}

/// The app is one HTML document with its styles, script and texts inline,
/// so a phone needs nothing more, also on a network without internet.
#[tokio::test]
async fn the_app_comes_in_one_response() {
    let base = start().await;
    let res = reqwest::get(format!("{base}/")).await.unwrap();
    assert_eq!(res.status(), 200);
    let kind = res.headers()[reqwest::header::CONTENT_TYPE]
        .to_str()
        .unwrap();
    assert_eq!(kind, "text/html; charset=utf-8");
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    assert_eq!(res.headers()["referrer-policy"], "no-referrer");
    assert_eq!(res.text().await.unwrap(), control_api::page());
}

#[tokio::test]
async fn browser_icon_requests_are_not_grinder_traffic() {
    let base = start().await;
    for path in [
        "/favicon.ico",
        "/apple-touch-icon.png",
        "/apple-touch-icon-precomposed.png",
    ] {
        let res = reqwest::get(format!("{base}{path}")).await.unwrap();
        assert_eq!(res.status(), 404, "{path}");
    }
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_eq!(state["grinder"]["requests"], 0);
    assert_eq!(state["grinder"]["peer"], Value::Null);
}

#[tokio::test]
async fn other_devices_looking_for_their_kind_are_not_grinder_traffic() {
    let base = start().await;
    // Smart-home discovery on port 80, e.g. for Shelly devices.
    let res = reqwest::get(format!("{base}/shelly")).await.unwrap();
    assert_eq!(res.status(), 200, "answered like any unknown request");
    assert_eq!(res.text().await.unwrap(), "{}");
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_eq!(state["grinder"]["requests"], 0);
    assert_eq!(state["grinder"]["peer"], Value::Null);

    // The grinder still counts, also with a request this version does not
    // know (a newer firmware may ask for more).
    let mut grinder = Grinder::new(&base);
    grinder.poll().await;
    for path in ["/api/v2/something-new", "/api/v2/"] {
        let res = reqwest::get(format!("{base}{path}")).await.unwrap();
        assert_eq!(res.text().await.unwrap(), "{}");
    }
    // Not the grinder's: only starts like its paths.
    reqwest::get(format!("{base}/api/v2x")).await.unwrap();
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_eq!(state["grinder"]["requests"], 3);
    assert_eq!(state["grinder"]["polls"], 1, "only the poll is a poll");
    assert_ne!(state["grinder"]["peer"], Value::Null);
}

#[tokio::test]
async fn events_say_how_long_ago_they_happened() {
    let base = start().await;
    let post = reqwest::Method::POST;
    let (code, _) = call(post, format!("{base}/api/shot/start"), None).await;
    assert_eq!(code, 200);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (_, events) = call(
        reqwest::Method::GET,
        format!("{base}/api/events?after=0"),
        None,
    )
    .await;
    let started = events
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "shot_started")
        .expect("a shot_started event");
    let age = started["age_ms"].as_u64().expect("age_ms");
    assert!((300..10_000).contains(&age), "age {age} ms");
}

#[tokio::test]
async fn an_abort_says_why_over_the_api() {
    let base = start().await;
    let post = reqwest::Method::POST;
    call(post.clone(), format!("{base}/api/shot/start"), None).await;
    let (code, _) = call(post, format!("{base}/api/shot/abort"), None).await;
    assert_eq!(code, 200);
    let (_, events) = call(
        reqwest::Method::GET,
        format!("{base}/api/events?after=0"),
        None,
    )
    .await;
    let aborted = events
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "shot_aborted")
        .expect("a shot_aborted event");
    assert_eq!(
        (&aborted["cause"], &aborted["reason"]),
        (&json!("shot_aborted_by_user"), &json!("aborted by user"))
    );
}

#[tokio::test]
async fn each_start_has_its_own_boot_id() {
    let (a, b) = (start().await, start().await);
    let boot_id = |state: &Value| state["server"]["boot_id"].as_str().unwrap().to_owned();
    let (_, first) = call(reqwest::Method::GET, format!("{a}/api/state"), None).await;
    let (_, again) = call(reqwest::Method::GET, format!("{a}/api/state"), None).await;
    let (_, other) = call(reqwest::Method::GET, format!("{b}/api/state"), None).await;
    assert_eq!(boot_id(&first).len(), 32);
    assert_eq!(boot_id(&first), boot_id(&again), "stable while running");
    assert_ne!(boot_id(&first), boot_id(&other), "new on every start");
}

#[tokio::test]
async fn control_api_rejects_what_it_cannot_do() {
    let base = start().await;
    let post = reqwest::Method::POST;

    // Each error says what is wrong in words, and as a code to match on.
    let code_of = |(status, body): (u16, Value)| (status, body["code"].as_str().map(str::to_owned));
    let expect = |status: u16, code: &str| (status, Some(code.to_owned()));

    let shot = json!({ "time_s": 30 });
    let reply = call(post.clone(), format!("{base}/api/shot/result"), Some(shot)).await;
    assert_eq!(reply.1["error"], "no shot is running");
    assert_eq!(code_of(reply), expect(409, "shot_not_running"));
    let reply = call(post.clone(), format!("{base}/api/shot/abort"), None).await;
    assert_eq!(code_of(reply), expect(409, "shot_not_running"));

    let (code, body) = call(
        post.clone(),
        format!("{base}/api/shot/result"),
        Some(json!({ "weight_g": 36 })),
    )
    .await;
    assert_eq!(
        (code, body["error"].as_str(), body["code"].as_str()),
        (
            400,
            Some("need time_s or time_ms"),
            Some("shot_result_invalid")
        )
    );

    for time in [json!({ "time_s": 1e20 }), json!({ "time_ms": 600_001 })] {
        let (code, body) = call(post.clone(), format!("{base}/api/shot/result"), Some(time)).await;
        assert_eq!(
            (code, body["error"].as_str()),
            (400, Some("time must be between 0 and 600 seconds"))
        );
    }

    // Without a weight or a recipe weight to fall back on.
    call(post.clone(), format!("{base}/api/shot/start"), None).await;
    let reply = call(post.clone(), format!("{base}/api/shot/start"), None).await;
    assert_eq!(code_of(reply), expect(409, "shot_already_running"));
    let shot = json!({ "time_s": 30 });
    let reply = call(post.clone(), format!("{base}/api/shot/result"), Some(shot)).await;
    assert_eq!(code_of(reply), expect(400, "shot_weight_missing"));

    let add = json!({ "kind": "nope", "settings": {} });
    let reply = call(post.clone(), format!("{base}/api/integrations"), Some(add)).await;
    assert_eq!(code_of(reply), expect(400, "integration_kind_unknown"));

    let url = format!("{base}/api/integrations/nope");
    let reply = call(reqwest::Method::DELETE, url, None).await;
    assert_eq!(code_of(reply), expect(404, "integration_not_found"));

    let pairing = |id: u32| Some(json!({ "pairing_id": id }));
    let url = format!("{base}/api/grinder/pairing");
    let reply = call(post.clone(), url.clone(), pairing(0)).await;
    assert_eq!(code_of(reply), expect(400, "grinder_pairing_id_invalid"));
    let reply = call(post.clone(), url, pairing(1234)).await;
    assert_eq!(
        code_of(reply),
        expect(409, "grinder_not_set_up"),
        "no --grinder"
    );

    let reply = call(
        reqwest::Method::PUT,
        format!("{base}/api/settings"),
        Some(json!({ "nope": 1 })),
    )
    .await;
    assert_eq!(code_of(reply), expect(400, "settings_invalid"));

    let patch = Some(json!({ "MA_STATUS": "not a number" }));
    let reply = call(post.clone(), format!("{base}/api/machine"), patch).await;
    assert_eq!(code_of(reply), expect(400, "machine_patch_invalid"));

    // A body or query it can't read: malformed, of the wrong shape, or not
    // said to be JSON.
    let http = reqwest::Client::new();
    let result = format!("{base}/api/shot/result");
    for req in [
        http.post(&result)
            .header("content-type", "application/json")
            .body("{"),
        http.post(&result).json(&json!({ "time_s": "thirty" })),
        http.post(&result).body(r#"{"time_s":30}"#),
        http.get(format!("{base}/api/events?after=x")),
        http.get(format!("{base}/api/log?after=-1")),
    ] {
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        let body: Value = res.json().await.unwrap();
        assert!((400..500).contains(&status), "{status}");
        assert_eq!(body["code"], "request_invalid", "{body}");
        assert!(body["error"].is_string());
    }
}

#[tokio::test]
async fn settings_change_in_the_app_and_are_saved() {
    let (base, x) = start_server().await;
    let dir = temp_dir("settings");
    let path = dir.join("settings.json");
    let startup = Startup {
        pairing_file: Some(dir.join("grinder-pairing.json")),
        ..Startup::default()
    };
    x.start_settings(startup, Some(SettingsFile::open(&path).unwrap()))
        .unwrap();
    let put = |body: Value| {
        call(
            reqwest::Method::PUT,
            format!("{base}/api/settings"),
            Some(body),
        )
    };

    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    let settings = &state["settings"];
    assert_eq!(settings["shot_window_s"], 180);
    assert_eq!(settings["grinder_host"], Value::Null);
    assert_eq!(
        settings["source"],
        json!({ "shot_window_s": "default", "grinder_host": "default", "update_check": "default" })
    );
    assert_eq!(settings["persistent"], true);
    assert_eq!(settings["update_check"], true, "on unless turned off");
    assert_eq!(
        settings["shot_window_range"],
        json!({ "min_s": 30, "max_s": 3600 }),
        "what the app can set, besides 0"
    );
    assert!(x.update_check_on());
    assert_eq!(state["update"]["latest"], Value::Null);

    let (code, settings) = put(json!({ "shot_window_s": 120 })).await;
    assert_eq!(code, 200);
    assert_eq!(settings["shot_window_s"], 120);
    assert_eq!(settings["source"]["shot_window_s"], "app");
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_eq!(
        state["machine"]["config"]["shot_window_s"], 120,
        "the machine uses it"
    );
    // No limit is 0 in both places.
    put(json!({ "shot_window_s": 0 })).await;
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_eq!(
        (
            &state["settings"]["shot_window_s"],
            &state["machine"]["config"]["shot_window_s"]
        ),
        (&json!(0), &json!(0))
    );
    put(json!({ "shot_window_s": 120 })).await;

    for bad in [
        json!({ "shot_window_s": 20 }),
        json!({ "shot_window_s": 90.5 }),
        json!({ "grinder_host": 1 }),
        json!({ "grinder_host": "http://127.0.0.1:9" }),
        json!({ "grinder_host": "127.0.0.1\n:9" }),
        json!({ "grinder_host": "a".repeat(300) }),
        json!({ "update_check": "off" }),
        json!({ "nope": 1 }),
    ] {
        let (code, body) = put(bad.clone()).await;
        assert_eq!(
            (code, &body["code"]),
            (400, &json!("settings_invalid")),
            "{bad}"
        );
        assert!(body["error"].is_string());
    }

    // The update check, turned off in the app.
    let (code, settings) = put(json!({ "update_check": false })).await;
    assert_eq!(code, 200);
    assert_eq!(settings["update_check"], false);
    assert!(!x.update_check_on());

    // An E65/E80 address starts the link (here to a closed local port), and
    // the pairing is kept where it was set up to be.
    let (code, settings) = put(json!({ "grinder_host": " 127.0.0.1:9 " })).await;
    assert_eq!(code, 200);
    assert_eq!(settings["grinder_host"], "127.0.0.1:9", "tidied up");
    wait_for("the link", || x.grinder_link().wifi.is_some()).await;
    let wifi = x.grinder_link().wifi.unwrap();
    assert_eq!(wifi.grinder, "https://127.0.0.1:9", "always HTTPS");
    assert_eq!(wifi.pairing_id, None);
    assert!(dir.join("grinder-pairing.json").exists());
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        saved["settings"],
        json!({ "shot_window_s": 120, "grinder_host": "127.0.0.1:9", "update_check": false })
    );

    // No address (`""`): the link stops, and a pairing ID has nowhere to go.
    // Without a start address there is nothing to stand in for: the app's
    // address is forgotten, as with `null`.
    let (code, settings) = put(json!({ "grinder_host": "" })).await;
    assert_eq!(code, 200);
    assert_eq!(settings["grinder_host"], Value::Null);
    assert_eq!(settings["source"]["grinder_host"], "default");
    wait_for("the link to stop", || x.grinder_link().wifi.is_none()).await;
    let url = format!("{base}/api/grinder/pairing");
    let (code, body) = call(reqwest::Method::POST, url, Some(json!({ "pairing_id": 7 }))).await;
    assert_eq!((code, &body["code"]), (409, &json!("grinder_not_set_up")));

    // `null` goes back to the start settings, here the defaults: the app's
    // values are forgotten.
    let all = json!({ "shot_window_s": null, "grinder_host": null, "update_check": null });
    let (code, settings) = put(all).await;
    assert_eq!(code, 200);
    assert_eq!(
        settings["source"],
        json!({ "shot_window_s": "default", "grinder_host": "default", "update_check": "default" })
    );
    assert_eq!(settings["shot_window_s"], 180);
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(saved.get("settings").is_none(), "{saved}");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn null_goes_back_to_the_grinder_address_of_the_start_settings() {
    let (base, x) = start_server().await;
    let dir = temp_dir("host-reset");
    let startup = Startup {
        grinder_host: Some("127.0.0.1:9".into()),
        pairing_file: Some(dir.join("grinder-pairing.json")),
        ..Startup::default()
    };
    let file = SettingsFile::open(&dir.join("settings.json")).unwrap();
    x.start_settings(startup, Some(file)).unwrap();
    let put = |host: Value| {
        call(
            reqwest::Method::PUT,
            format!("{base}/api/settings"),
            Some(json!({ "grinder_host": host })),
        )
    };
    let linked_to = |grinder: &'static str| {
        let x = x.clone();
        move || x.grinder_link().wifi.is_some_and(|w| w.grinder == grinder)
    };
    wait_for("the flag's link", linked_to("https://127.0.0.1:9")).await;

    // Changed in the app, then back to the flag.
    let (_, settings) = put(json!("127.0.0.1:10")).await;
    assert_eq!(settings["source"]["grinder_host"], "app");
    assert_eq!(settings["start"]["grinder_host"], "127.0.0.1:9");
    wait_for("the app's link", linked_to("https://127.0.0.1:10")).await;
    let (code, settings) = put(Value::Null).await;
    assert_eq!(code, 200);
    assert_eq!(settings["grinder_host"], "127.0.0.1:9");
    assert_eq!(settings["source"]["grinder_host"], "command_line");
    wait_for("the flag's link again", linked_to("https://127.0.0.1:9")).await;

    // Removed in the app: no link, whatever the flag says.
    let (_, settings) = put(json!("")).await;
    assert_eq!(settings["grinder_host"], Value::Null);
    assert_eq!(settings["source"]["grinder_host"], "app");
    wait_for("the link to stop", || x.grinder_link().wifi.is_none()).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_new_grinder_address_ends_the_shot_it_waited_for() {
    let (base, x) = start_server().await;
    x.start_settings(
        Startup {
            grinder_host: Some("http://127.0.0.1:9".into()),
            ..Startup::default()
        },
        None,
    )
    .unwrap();
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_eq!(state["settings"]["source"]["grinder_host"], "command_line");
    assert_eq!(state["settings"]["persistent"], false);
    let put = |host: &str| {
        call(
            reqwest::Method::PUT,
            format!("{base}/api/settings"),
            Some(json!({ "grinder_host": host })),
        )
    };
    let ended = || {
        x.with(|m, _| {
            m.events_after(0).any(|e| {
                e.event
                    == crate::protocol::MachineEvent::shot_aborted(
                        AbortCause::GrinderAddressChanged,
                    )
            })
        })
    };

    // An E64 WS knob press on the same server is not the link's business.
    x.with(|m, now| m.on_start_request(now, Some(9)));
    assert_eq!(put("127.0.0.1:10").await.0, 200);
    assert!(!ended());
    // The new link shows at once (connecting, or already failing to reach
    // the closed port), never no link at all.
    let wifi = x.grinder_link().wifi.expect("shown at once");
    assert_eq!(wifi.grinder, "https://127.0.0.1:10");
    x.with(|m, now| m.abort(now)).unwrap();

    // A brew for the old link's job cannot reach the grinder any more.
    let job = crate::protocol::GrindResult::parse(br#"{"SYNC_BEVERAGE_WEIGHT":40}"#).unwrap();
    wait_for("ready again", || {
        x.with(|m, now| m.on_brew_job(now, job.clone(), Some(28_000)))
    })
    .await;
    assert_eq!(put("127.0.0.1:11").await.0, 200);
    assert!(ended());
    wait_for("the new link", || {
        x.grinder_link()
            .wifi
            .is_some_and(|w| w.grinder == "https://127.0.0.1:11")
    })
    .await;
}

#[tokio::test]
async fn the_log_is_kept_for_the_app() {
    let (base, x) = start_server().await;
    // The server has a log book of its own (see `start_server`), and nothing
    // logs into it but this test.
    let (_, before) = call(reqwest::Method::GET, format!("{base}/api/log"), None).await;
    let after = before["last_seq"].as_u64().unwrap();
    assert_eq!(after, 0);
    assert_eq!(before["capacity"], 1000);

    let mut stream = reqwest::get(format!("{base}/api/log/stream?after={after}"))
        .await
        .unwrap();
    assert_eq!(
        stream.headers()["content-type"],
        "text/event-stream",
        "server-sent events"
    );
    let first = x.log().push(tracing::Level::WARN, "routes test: first");
    x.log().push(tracing::Level::INFO, "routes test: second");
    x.log().push(tracing::Level::ERROR, "routes test: third");

    let (code, body) = call(
        reqwest::Method::GET,
        format!("{base}/api/log?after={after}"),
        None,
    )
    .await;
    assert_eq!(code, 200);
    let ours: Vec<&Value> = body["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["text"].as_str().unwrap().starts_with("routes test:"))
        .collect();
    assert_eq!(ours.len(), 3);
    assert_eq!(ours[0]["seq"], first.seq);
    assert_eq!(ours[0]["text"], "routes test: first");
    assert!(ours[0]["at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(body["last_seq"], first.seq + 2);
    // Lower case, like the API's other codes.
    let levels: Vec<_> = ours.iter().map(|l| l["level"].as_str().unwrap()).collect();
    assert_eq!(levels, ["warn", "info", "error"]);

    // The stream sends the same lines as `log` events.
    let mut text = String::new();
    while !text.contains("routes test: second") {
        let chunk = tokio::time::timeout(Duration::from_secs(5), stream.chunk())
            .await
            .expect("a line in time")
            .unwrap()
            .expect("the stream stays open");
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(text.contains("event: log"), "{text}");
    assert!(text.contains(&format!("id: {}", first.seq)), "{text}");
    assert!(text.contains(r#""text":"routes test: first""#), "{text}");
    assert!(text.contains(r#""level":"warn""#), "{text}");

    // A browser reconnecting says what it saw last, and gets what it missed;
    // that wins over the query of the URL it reconnects to.
    let mut again = reqwest::Client::new()
        .get(format!("{base}/api/log/stream?after=0"))
        .header("last-event-id", first.seq.to_string())
        .send()
        .await
        .unwrap();
    let chunk = tokio::time::timeout(Duration::from_secs(5), again.chunk())
        .await
        .expect("the missed line at once")
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&chunk);
    assert!(text.contains(&format!("id: {}", first.seq + 1)), "{text}");
    assert!(!text.contains("routes test: first"), "{text}");
}

#[tokio::test]
async fn shots_the_grinder_asked_for_go_into_the_history() {
    let (base, x) = start_server().await;
    let dir = temp_dir("history");
    x.record_history(Some(&dir.join("history.json"))).unwrap();
    let mut grinder = Grinder::new(&base);
    let post = reqwest::Method::POST;

    // A flush started by hand is not a shot.
    let (code, _) = call(post.clone(), format!("{base}/api/shot/start"), None).await;
    assert_eq!(code, 200);
    let shot = json!({ "time_s": 5, "weight_g": 20 });
    call(post.clone(), format!("{base}/api/shot/result"), Some(shot)).await;
    while grinder.poll().await.status != MachineStatus::On {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // A grind, the knob press, and the shot typed in.
    grinder
        .post(PATH_BREWRATIO, json!({ "SYNC_BEVERAGE_WEIGHT": 36.0 }))
        .await;
    grinder
        .post(PATH_SCRIPTS_EXECUTE, json!({ "ID": START_BREW_SCRIPT_ID }))
        .await;
    let shot = json!({ "time_s": 28.5, "weight_g": 37.2 });
    let (code, _) = call(post.clone(), format!("{base}/api/shot/result"), Some(shot)).await;
    assert_eq!(code, 200);
    wait_for("the shot", || x.history().list().len() == 1).await;
    while grinder.poll().await.status != MachineStatus::On {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // A knob press and a shot too short for the grinder.
    grinder
        .post(PATH_SCRIPTS_EXECUTE, json!({ "ID": START_BREW_SCRIPT_ID }))
        .await;
    let shot = json!({ "time_s": 8, "weight_g": 18 });
    let (code, _) = call(post.clone(), format!("{base}/api/shot/result"), Some(shot)).await;
    assert_eq!(code, 200);
    wait_for("the short shot", || x.history().list().len() == 2).await;
    while grinder.poll().await.status != MachineStatus::On {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Another knob press, aborted.
    grinder
        .post(PATH_SCRIPTS_EXECUTE, json!({ "ID": START_BREW_SCRIPT_ID }))
        .await;
    call(post, format!("{base}/api/shot/abort"), None).await;
    wait_for("the abort", || x.history().list().len() == 3).await;

    let (code, body) = call(reqwest::Method::GET, format!("{base}/api/history"), None).await;
    assert_eq!(code, 200);
    assert_eq!(
        (&body["capacity"], &body["persistent"]),
        (&json!(200), &json!(true))
    );
    let shots = body["shots"].as_array().unwrap();
    assert_eq!(shots[0]["outcome"], "aborted", "newest first");
    assert_eq!(shots[0]["reason"], "aborted by user");
    assert_eq!(shots[0]["cause"], "shot_aborted_by_user");
    assert_eq!(shots[0]["time_ms"], Value::Null);
    assert_eq!(shots[0]["in_grinder_range"], Value::Null);
    let short = &shots[1];
    assert_eq!(short["outcome"], "succeeded");
    assert_eq!(short["time_ms"], 8_000);
    assert_eq!(short["in_grinder_range"], false, "the grinder ignores it");
    let shot = &shots[2];
    assert_eq!(shot["outcome"], "succeeded");
    assert_eq!(
        (&shot["time_ms"], &shot["weight_g"]),
        (&json!(28_500), &json!(37.2))
    );
    assert_eq!(shot["in_grinder_range"], true);
    assert_eq!(shot["target_weight_g"], 36.0);
    assert_eq!(shot["target_time_ms"], Value::Null, "the E64 WS sends none");
    assert_eq!(
        (&shot["reason"], &shot["cause"]),
        (&Value::Null, &Value::Null)
    );
    assert!(shot["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(shot["started_at"].as_str().unwrap().ends_with('Z'));
    assert!(shot["ended_at"].as_str().unwrap() >= shot["started_at"].as_str().unwrap());
    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.join("history.json")).unwrap()).unwrap();
    assert_eq!(saved["shots"].as_array().unwrap().len(), 3);
    let _ = std::fs::remove_dir_all(dir);
}

/// `grinder.shot_range` in `GET /api/state`, which is always there (`null`
/// when not known).
async fn grinder_range(base: &str) -> Value {
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    state["grinder"]
        .get("shot_range")
        .cloned()
        .expect("shot_range")
}

#[tokio::test]
async fn the_state_comes_in_groups() {
    let base = start().await;
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    let groups: Vec<_> = state.as_object().unwrap().keys().cloned().collect();
    let mut expected = [
        "shot",
        "grinder",
        "machine",
        "integrations",
        "settings",
        "update",
        "server",
    ];
    expected.sort_unstable();
    assert_eq!(groups, expected);

    assert_eq!(state["shot"]["phase"], "ready");
    assert_eq!(state["shot"]["last"], Value::Null);
    assert_eq!(
        state["grinder"]["shot_range"],
        Value::Null,
        "no request yet"
    );
    assert_eq!(state["grinder"]["last_grind"], Value::Null);
    assert_eq!(state["grinder"]["grind_blocked"], Value::Null);
    let config = &state["machine"]["config"];
    assert_eq!(
        (&config["shot_window_s"], &config["finishing_hold_s"]),
        (&json!(180), &json!(0.3))
    );
    assert_eq!(state["machine"]["mako"]["MA_STATUS"], 1);
    assert_eq!(state["machine"]["overrides"], json!({}));
    assert_eq!(state["server"]["last_event_seq"], 0);
    assert!(state["server"]["boot_id"].is_string());
    assert!(state["update"].get("enabled").is_none(), "in the settings");

    // While a shot runs, the grinder is told so.
    let post = reqwest::Method::POST;
    let (_, state) = call(post, format!("{base}/api/shot/start"), None).await;
    assert_eq!(
        (
            &state["shot"]["phase"],
            &state["shot"]["requested_by_grinder"]
        ),
        (&json!("running"), &json!(false))
    );
    assert_eq!(state["grinder"]["grind_blocked"]["cause"], "shot_running");
}

#[tokio::test]
async fn the_state_says_which_shot_times_the_grinder_takes() {
    let (base, x) = start_server().await;
    assert_eq!(grinder_range(&base).await, Value::Null, "no request yet");

    let grinder = Grinder::new(&base);
    grinder
        .post(PATH_SCRIPTS_EXECUTE, json!({ "ID": START_BREW_SCRIPT_ID }))
        .await;
    assert_eq!(
        grinder_range(&base).await,
        json!({ "min_ms": 10_000, "max_ms": 80_000 }),
        "the E64 WS's"
    );
    x.with(|m, now| m.abort(now)).unwrap();

    // An E65/E80 brew job's: within 15 s of its target time.
    let job = crate::protocol::GrindResult::parse(br#"{"SYNC_BEVERAGE_WEIGHT":40}"#).unwrap();
    wait_for("ready again", || {
        x.with(|m, now| m.on_brew_job(now, job.clone(), Some(28_000)))
    })
    .await;
    assert_eq!(
        grinder_range(&base).await,
        json!({ "min_ms": 13_000, "max_ms": 43_000 })
    );
}

#[tokio::test]
async fn an_e65_shot_counts_within_15_s_of_its_target_time() {
    let (base, x) = start_server().await;
    x.record_history(None).unwrap();
    let job = crate::protocol::GrindResult::parse(br#"{"SYNC_BEVERAGE_WEIGHT":40}"#).unwrap();

    // A target time, a shot that long, and whether the grinder adjusts from
    // it.
    for (n, (target_ms, time_ms, adjusts)) in [
        (Some(28_000), 13_000, json!(false)),
        (Some(24_000), 11_000, json!(true)),
        (None, 50_000, Value::Null),
    ]
    .into_iter()
    .enumerate()
    {
        wait_for("ready for the next job", || {
            x.with(|m, now| m.on_brew_job(now, job.clone(), target_ms))
        })
        .await;
        let shot = json!({ "time_ms": time_ms, "weight_g": 40 });
        let (code, _) = call(
            reqwest::Method::POST,
            format!("{base}/api/shot/result"),
            Some(shot),
        )
        .await;
        assert_eq!(code, 200);
        wait_for("the shot", || x.history().list().len() == n + 1).await;

        let (_, events) = call(reqwest::Method::GET, format!("{base}/api/events"), None).await;
        let reported = events
            .as_array()
            .unwrap()
            .iter()
            .rfind(|e| e["kind"] == "shot_reported")
            .unwrap();
        // There as `null` when the window is not known, not left out.
        let adjusts = Some(&adjusts);
        assert_eq!(reported.get("in_grinder_range"), adjusts, "{time_ms} ms");
        let (_, history) = call(reqwest::Method::GET, format!("{base}/api/history"), None).await;
        let newest = &history["shots"][0];
        assert_eq!(newest["outcome"], "succeeded");
        assert_eq!(newest.get("in_grinder_range"), adjusts, "{time_ms} ms");
    }
}

#[tokio::test]
async fn integrations_are_edited_and_moved_into_the_app() {
    let (base, x) = start_server().await;
    let dir = temp_dir("integrations");
    let file = SettingsFile::open(&dir.join("settings.json")).unwrap();
    let scale = |host: &str| crate::integration::Settings::new().with("host", Some(host));
    // A Half Decent Scale from the command line, at a local address nobody
    // answers on.
    let kind = &crate::integration::half_decent::KIND;
    x.integrations()
        .start_cli(&x, kind, &scale("127.0.0.1:9"))
        .unwrap();
    let url = |path: &str| format!("{base}/api/integrations/half_decent{path}");

    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    let entry = &state["integrations"][0];
    assert_eq!(
        (&entry["source"], &entry["adoptable"]),
        (&json!("command_line"), &json!(false))
    );
    // The status line in English, and as a key and values the app can word
    // in its own language (connecting, or not reached at startup).
    assert!(entry["detail"].as_str().is_some_and(|d| !d.is_empty()));
    let key = entry["detail_key"].as_str().unwrap();
    assert!(key.starts_with("half_decent."), "{key}");
    assert!(entry["detail_vars"].is_object());
    let (code, body) = call(reqwest::Method::POST, url("/adopt"), None).await;
    assert_eq!(
        (code, &body["code"]),
        (409, &json!("settings_file_missing"))
    );

    x.integrations()
        .load(&x, file, crate::integration::KINDS)
        .unwrap();
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_eq!(state["integrations"][0]["adoptable"], true);
    let on_command_line = (409, json!("integration_on_command_line"));
    let (code, body) = call(reqwest::Method::GET, url("/settings"), None).await;
    assert_eq!((code, body["code"].clone()), on_command_line);
    let edit = json!({ "settings": { "host": "127.0.0.1:10" } });
    let (code, body) = call(reqwest::Method::PUT, url(""), Some(edit.clone())).await;
    assert_eq!((code, body["code"].clone()), on_command_line);

    let (code, view) = call(reqwest::Method::POST, url("/adopt"), None).await;
    assert_eq!(code, 200);
    assert_eq!(
        (&view["id"], &view["source"], &view["adoptable"]),
        (&json!("half_decent"), &json!("app"), &json!(false))
    );
    let (code, body) = call(reqwest::Method::POST, url("/adopt"), None).await;
    assert_eq!((code, &body["code"]), (409, &json!("integration_in_app")));

    let (code, form) = call(reqwest::Method::GET, url("/settings"), None).await;
    assert_eq!(code, 200);
    assert_eq!(
        form,
        json!({ "kind": "half_decent", "settings": { "host": "127.0.0.1:9" }, "secrets_set": [] })
    );
    let (code, body) = call(reqwest::Method::PUT, url(""), Some(edit)).await;
    assert_eq!((code, &body["id"]), (200, &json!("half_decent")));
    let (_, form) = call(reqwest::Method::GET, url("/settings"), None).await;
    assert_eq!(form["settings"]["host"], "127.0.0.1:10");
    let bad = json!({ "settings": { "host": "not an address" } });
    let (code, body) = call(reqwest::Method::PUT, url(""), Some(bad)).await;
    assert_eq!(
        (code, &body["code"]),
        (400, &json!("integration_settings_invalid"))
    );
    let unknown = format!("{base}/api/integrations/nope");
    let not_found = (404, json!("integration_not_found"));
    let (code, body) = call(reqwest::Method::PUT, unknown.clone(), Some(json!({}))).await;
    assert_eq!((code, body["code"].clone()), not_found);
    let (code, body) = call(reqwest::Method::GET, format!("{unknown}/settings"), None).await;
    assert_eq!((code, body["code"].clone()), not_found);
    let (code, body) = call(reqwest::Method::POST, format!("{unknown}/adopt"), None).await;
    assert_eq!((code, body["code"].clone()), not_found);

    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
    assert_eq!(saved["integrations"][0]["from_command_line"], true);
    assert_eq!(saved["integrations"][0]["settings"]["host"], "127.0.0.1:10");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn web_pages_cannot_act_as_the_grinder() {
    let base = start().await;
    let http = reqwest::Client::new();
    // A page on another site, posting a start request as a CORS simple
    // request (no preflight), and reading a poll reply.
    let res = http
        .post(format!("{base}{PATH_SCRIPTS_EXECUTE}"))
        .header("origin", "https://evil.example")
        .header("content-type", "text/plain")
        .body(r#"{"ID":9}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    assert!(res.headers().get("access-control-allow-origin").is_none());
    // A read by fetch() from another site carries `Origin`; over HTTPS
    // every request also carries `Sec-Fetch-Site`.
    for (name, value) in [
        ("origin", "https://evil.example"),
        ("sec-fetch-site", "cross-site"),
    ] {
        let res = http
            .get(format!("{base}{PATH_MAKO}"))
            .header(name, value)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "{name}");
    }
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_ne!(state["shot"]["phase"], "running");

    assert_eq!(
        state["grinder"]["requests"], 0,
        "not counted as the grinder"
    );

    // The grinder sends neither header, and nor does a page's plain read
    // over HTTP (an image, say): it is answered, but with no CORS header,
    // so another site can't see the reply.
    let res = http.get(format!("{base}{PATH_MAKO}")).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.headers().get("access-control-allow-origin").is_none());

    // A body far bigger than any of the grinder's is refused.
    let big = format!(
        r#"{{"SYNC_BEVERAGE_WEIGHT":36,"pad":"{}"}}"#,
        "x".repeat(8 * 1024)
    );
    let res = http
        .post(format!("{base}{PATH_BREWRATIO}"))
        .body(big)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 413);
}

#[tokio::test]
async fn the_app_answers_home_names_only_and_refuses_other_sites() {
    let base = start().await;
    let http = reqwest::Client::new();
    let get_state = |host: &'static str| {
        http.get(format!("{base}/api/state"))
            .header("host", host)
            .send()
    };
    // DNS rebinding: a page on its own, public name.
    let res = get_state("attacker.example").await.unwrap();
    assert_eq!(res.status(), 421);
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body["code"], "request_host_refused",
        "an error as any other"
    );
    assert_eq!(get_state("gbs.fritz.box").await.unwrap().status(), 200);
    assert_eq!(get_state("kitchen-pi").await.unwrap().status(), 200);
    assert_eq!(get_state("").await.unwrap().status(), 421, "an empty Host");

    // Every route of the app, the page and the event stream included.
    for path in [
        "/",
        "/icons/la_marzocco.svg",
        "/favicon.ico",
        "/api/state",
        "/api/history",
        "/api/events",
        "/api/events/stream",
        "/api/log",
        "/api/log/stream",
        "/api/integrations/kinds",
        "/api/integrations/x/settings",
    ] {
        let res = http
            .get(format!("{base}{path}"))
            .header("host", "attacker.example")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 421, "{path}");
    }
    for (method, path) in [
        (reqwest::Method::PUT, "/api/settings"),
        (reqwest::Method::POST, "/api/shot/result"),
        (reqwest::Method::POST, "/api/shot/abort"),
        (reqwest::Method::POST, "/api/integrations"),
        (reqwest::Method::PUT, "/api/integrations/x"),
        (reqwest::Method::DELETE, "/api/integrations/x"),
        (reqwest::Method::POST, "/api/integrations/x/adopt"),
        (reqwest::Method::POST, "/api/machine"),
        (reqwest::Method::POST, "/api/grinder/pairing"),
        (reqwest::Method::PUT, "/api/overrides"),
    ] {
        let res = http
            .request(method.clone(), format!("{base}{path}"))
            .header("origin", "https://evil.example")
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "{method} {path}");
    }

    // CSRF: a body-less POST from another site.
    let res = http
        .post(format!("{base}/api/shot/start"))
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["code"], "request_from_other_site");
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_ne!(state["shot"]["phase"], "running");

    // The app's own page, same origin, still works.
    let res = http
        .post(format!("{base}/api/shot/start"))
        .header("origin", base.as_str())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn without_a_host_header_the_host_in_the_uri_is_checked() {
    // HTTP/2 names the host only in the URI; an HTTP/1.1 request in absolute
    // form without a Host header arrives the same way.
    let base = start().await;
    let start_shot = |origin: &str| {
        format!(
            "POST http://192.168.1.20/api/shot/start HTTP/1.1\r\norigin: {origin}\r\n\
             content-length: 0\r\nconnection: close\r\n\r\n"
        )
    };
    assert_eq!(
        raw_status(&base, &start_shot("https://evil.example")).await,
        403
    );
    assert_eq!(
        raw_status(&base, &start_shot("http://192.168.1.20:8080")).await,
        403,
        "another site on the same computer"
    );
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_ne!(state["shot"]["phase"], "running");

    assert_eq!(
        raw_status(&base, &start_shot("http://192.168.1.20")).await,
        200
    );
    let rebound = "GET http://attacker.example/api/state HTTP/1.1\r\nconnection: close\r\n\r\n";
    assert_eq!(raw_status(&base, rebound).await, 421);
}

#[tokio::test]
async fn a_broken_body_is_not_taken_for_a_large_one() {
    let base = start().await;
    let post = |body: &str| {
        format!(
            "POST {PATH_BREWRATIO} HTTP/1.1\r\nhost: 127.0.0.1\r\n\
             transfer-encoding: chunked\r\nconnection: close\r\n\r\n{body}"
        )
    };
    assert_eq!(
        raw_status(&base, &post("not a chunk size\r\n\r\n")).await,
        400
    );
    // Too large is too large, also in chunks.
    let big = "x".repeat(5000);
    let chunked = format!("{:x}\r\n{big}\r\n0\r\n\r\n", big.len());
    assert_eq!(raw_status(&base, &post(&chunked)).await, 413);
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_eq!(
        state["grinder"]["requests"], 0,
        "not counted as the grinder"
    );
}

#[tokio::test]
async fn browser_headers_can_be_let_through_to_the_grinder_routes() {
    // `--accept-browser-headers`, in case a grinder ever sends them.
    let state = Server::new(MachineConfig::default());
    let app = grinder_router(state, true);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    spawn_server(listener, app, addr);
    let res = reqwest::Client::new()
        .get(format!("http://{addr}{PATH_MAKO}"))
        .header("origin", "null")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}
