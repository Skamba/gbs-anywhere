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
    let _ = rustls::crypto::ring::default_provider().install_default();
    let state = Server::new(MachineConfig {
        // Long enough for a few polls below, short enough to keep tests fast.
        finishing_hold: Duration::from_millis(300),
        ..MachineConfig::default()
    });
    let app = control_router(state.clone()).merge(grinder_router(state));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    spawn_server(listener, app, addr);
    format!("http://{addr}")
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

    // The person types what the machine showed.
    let shot = json!({ "time_s": 30, "weight_g": 36 });
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

    let add = json!({ "kind": "nope", "settings": {} });
    let (code, _) = call(post, format!("{base}/api/integrations"), Some(add)).await;
    assert_eq!(code, 400);

    let url = format!("{base}/api/integrations/nope");
    let (code, _) = call(reqwest::Method::DELETE, url, None).await;
    assert_eq!(code, 404);
}
