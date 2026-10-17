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
    let state = Server::new(config);
    let app = control_router(state.clone()).merge(grinder_router(state.clone()));
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
        json!({ "shot_window_s": "default", "grinder_host": "default" })
    );
    assert_eq!(settings["persistent"], true);

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
        json!({ "nope": 1 }),
    ] {
        let (code, body) = put(bad.clone()).await;
        assert_eq!(code, 400, "{bad}");
        assert!(body["error"].is_string());
    }

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
        json!({ "shot_window_s": 120, "grinder_host": "127.0.0.1:9" })
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
