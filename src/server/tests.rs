//! The grinder and a person, end to end over HTTP: the routers the binary
//! serves, on a free local port, with [`GrinderModel`] reading the replies the
//! way the grinder does.

use std::time::Duration;

use serde_json::{Value, json};

use super::*;
use crate::protocol::{
    BrewState, GrinderEvent, GrinderModel, MachineStatus, MakoState, PATH_BREWRATIO, PATH_MACHINE,
    PATH_MAKO, PATH_SCRIPTS_EXECUTE, START_BREW_SCRIPT_ID,
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
    assert_eq!(state["version"], crate::version());

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
    assert_eq!(state["last_shot"]["volume_ml"], 36.0, "the recipe weight");
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
        .find(|e| e["kind"] == "brew_started")
        .expect("a brew_started event");
    let age = started["age_ms"].as_u64().expect("age_ms");
    assert!((300..10_000).contains(&age), "age {age} ms");
}

#[tokio::test]
async fn each_start_has_its_own_boot_id() {
    let (a, b) = (start().await, start().await);
    let boot_id = |state: &Value| state["boot_id"].as_str().unwrap().to_owned();
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

    let shot = json!({ "time_s": 30 });
    let (code, _) = call(post.clone(), format!("{base}/api/shot/result"), Some(shot)).await;
    assert_eq!(code, 409, "no brew running");

    let (code, body) = call(
        post.clone(),
        format!("{base}/api/shot/result"),
        Some(json!({ "weight_g": 36 })),
    )
    .await;
    assert_eq!(
        (code, body["error"].as_str()),
        (400, Some("need time_s or time_ms"))
    );

    for time in [json!({ "time_s": 1e20 }), json!({ "time_ms": 600_001 })] {
        let (code, body) = call(post.clone(), format!("{base}/api/shot/result"), Some(time)).await;
        assert_eq!(
            (code, body["error"].as_str()),
            (400, Some("time must be between 0 and 600 seconds"))
        );
    }

    let add = json!({ "kind": "nope", "settings": {} });
    let (code, _) = call(post.clone(), format!("{base}/api/integrations"), Some(add)).await;
    assert_eq!(code, 400);

    let url = format!("{base}/api/integrations/nope");
    let (code, _) = call(reqwest::Method::DELETE, url, None).await;
    assert_eq!(code, 404);

    let pairing = |id: u32| Some(json!({ "pairing_id": id }));
    let url = format!("{base}/api/grinder/pairing");
    let (code, _) = call(post.clone(), url.clone(), pairing(0)).await;
    assert_eq!(code, 400, "out of range");
    let (code, _) = call(post, url, pairing(1234)).await;
    assert_eq!(code, 409, "no --grinder");
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
    assert_eq!(state["update"]["enabled"], true);
    assert_eq!(state["update"]["latest"], Value::Null);

    let (code, settings) = put(json!({ "shot_window_s": 120 })).await;
    assert_eq!(code, 200);
    assert_eq!(settings["shot_window_s"], 120);
    assert_eq!(settings["source"]["shot_window_s"], "app");
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_eq!(
        state["config"]["brew_timeout"]["secs"], 120,
        "the machine uses it"
    );

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
        assert_eq!(code, 400, "{bad}");
        assert!(body["error"].is_string());
    }

    // The update check, turned off in the app.
    let (code, settings) = put(json!({ "update_check": false })).await;
    assert_eq!(code, 200);
    assert_eq!(settings["update_check"], false);
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_eq!(state["update"]["enabled"], false);

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

    // No address: the link stops, and a pairing ID has nowhere to go.
    let (code, settings) = put(json!({ "grinder_host": null })).await;
    assert_eq!(code, 200);
    assert_eq!(settings["grinder_host"], Value::Null);
    assert_eq!(settings["source"]["grinder_host"], "app");
    wait_for("the link to stop", || x.grinder_link().wifi.is_none()).await;
    let url = format!("{base}/api/grinder/pairing");
    let (code, _) = call(reqwest::Method::POST, url, Some(json!({ "pairing_id": 7 }))).await;
    assert_eq!(code, 409);
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
                    == crate::protocol::MachineEvent::ShotAborted {
                        reason: settings::GRINDER_CHANGED,
                    }
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
    assert_eq!(ours.len(), 2);
    assert_eq!(ours[0]["seq"], first.seq);
    assert_eq!(ours[0]["level"], "WARN");
    assert_eq!(ours[0]["text"], "routes test: first");
    assert!(ours[0]["ts"].as_str().unwrap().ends_with('Z'));
    assert_eq!(body["last_seq"], first.seq + 1);

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

    // Another knob press, aborted.
    grinder
        .post(PATH_SCRIPTS_EXECUTE, json!({ "ID": START_BREW_SCRIPT_ID }))
        .await;
    call(post, format!("{base}/api/shot/abort"), None).await;
    wait_for("the abort", || x.history().list().len() == 2).await;

    let (code, body) = call(reqwest::Method::GET, format!("{base}/api/history"), None).await;
    assert_eq!(code, 200);
    assert_eq!(
        (&body["capacity"], &body["persistent"]),
        (&json!(200), &json!(true))
    );
    let shots = body["shots"].as_array().unwrap();
    assert_eq!(shots[0]["kind"], "aborted", "newest first");
    assert_eq!(shots[0]["reason"], "aborted by user");
    assert_eq!(shots[0]["time_s"], Value::Null);
    let shot = &shots[1];
    assert_eq!(shot["kind"], "success");
    assert_eq!(
        (&shot["time_s"], &shot["weight_g"]),
        (&json!(28.5), &json!(37.2))
    );
    assert_eq!(shot["target_weight_g"], 36.0);
    assert_eq!(shot["target_time_s"], Value::Null, "the E64 WS sends none");
    assert_eq!(shot["reason"], Value::Null);
    assert!(shot["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(shot["at"].as_str().unwrap().ends_with('Z'));
    assert!(shot["ended_at"].as_str().unwrap() >= shot["at"].as_str().unwrap());
    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.join("history.json")).unwrap()).unwrap();
    assert_eq!(saved["shots"].as_array().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(dir);
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
    let (code, _) = call(reqwest::Method::POST, url("/adopt"), None).await;
    assert_eq!(code, 409, "no settings file");

    x.integrations()
        .load(&x, file, crate::integration::KINDS)
        .unwrap();
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_eq!(state["integrations"][0]["adoptable"], true);
    let (code, _) = call(reqwest::Method::GET, url("/settings"), None).await;
    assert_eq!(code, 409, "command line");
    let edit = json!({ "settings": { "host": "127.0.0.1:10" } });
    let (code, _) = call(reqwest::Method::PUT, url(""), Some(edit.clone())).await;
    assert_eq!(code, 409, "command line");

    let (code, view) = call(reqwest::Method::POST, url("/adopt"), None).await;
    assert_eq!(code, 200);
    assert_eq!(
        (&view["id"], &view["source"], &view["adoptable"]),
        (&json!("half_decent"), &json!("app"), &json!(false))
    );
    let (code, _) = call(reqwest::Method::POST, url("/adopt"), None).await;
    assert_eq!(code, 409, "already in the app");

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
    let (code, _) = call(reqwest::Method::PUT, url(""), Some(bad)).await;
    assert_eq!(code, 400);
    let unknown = format!("{base}/api/integrations/nope");
    let (code, _) = call(reqwest::Method::PUT, unknown.clone(), Some(json!({}))).await;
    assert_eq!(code, 404);
    let (code, _) = call(reqwest::Method::GET, format!("{unknown}/settings"), None).await;
    assert_eq!(code, 404);
    let (code, _) = call(reqwest::Method::POST, format!("{unknown}/adopt"), None).await;
    assert_eq!(code, 404);

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
    let res = http
        .get(format!("{base}{PATH_MAKO}"))
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_ne!(state["phase"]["phase"], "brewing");
    assert_eq!(
        state["grinder"]["requests"], 0,
        "not counted as the grinder"
    );

    // The grinder sends neither header; it gets no CORS header either.
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
    assert_eq!(get_state("attacker.example").await.unwrap().status(), 421);
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
    let (_, state) = call(reqwest::Method::GET, format!("{base}/api/state"), None).await;
    assert_ne!(state["phase"]["phase"], "brewing");

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
    assert_ne!(state["phase"]["phase"], "brewing");
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
