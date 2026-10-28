//! The customer-app API the La Marzocco Home app uses
//! (`lion.lamarzocco.io/api/customer-app`), as implemented by pylamarzocco:
//! each *installation* has a P-256 key registered once with
//! `POST /auth/init`; every request carries a nonce, a timestamp, a custom
//! proof and an ECDSA signature over them. The installation key here is
//! derived deterministically from the account credentials, so nothing needs
//! to be stored between runs.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::EncodePublicKey;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::TITLE;
use super::config::Config;

/// The customer-app API the La Marzocco Home app uses.
pub const BASE_URL: &str = "https://lion.lamarzocco.io/api/customer-app";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// Tokens last an hour; refresh this long before that.
const TOKEN_LIFETIME: Duration = Duration::from_secs(3600);
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(600);
const BBW_TIMEOUT: Duration = Duration::from_secs(8);
const BBW_MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// Select a matching preset, or set Dose1 to the target while preserving Dose2.
/// These commands never start a brew.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BbwSelection {
    Confirmed,
    AlreadySet,
    /// A POST may have taken effect. Do not repeat it for this brew.
    Unknown,
    /// No command was sent; the reason contains no vendor response data.
    NotApplied(&'static str),
}

#[derive(Debug, PartialEq, Eq)]
struct BbwState {
    mode: String,
    doses: [u32; 2],
}

/// Presets have 0.1 g resolution; tolerate only the cloud's float32 noise.
fn dose_tenths(grams: f64) -> Option<u32> {
    let tenths = grams * 10.0;
    (grams.is_finite() && grams > 0.0 && grams <= 200.0 && (tenths - tenths.round()).abs() < 0.001)
        .then_some(tenths.round() as u32)
        .filter(|value| *value > 0)
}

fn parse_bbw_dashboard(value: &Value, serial: &str) -> Result<BbwState, &'static str> {
    if value["serialNumber"].as_str() != Some(serial) {
        return Err("Machine identity could not be confirmed.");
    }
    if value["connected"].as_bool() != Some(true) {
        return Err("Coffee machine is offline or its connection is unknown.");
    }
    let widgets = value["widgets"]
        .as_array()
        .ok_or("Dashboard is incomplete.")?;
    let output = |code| {
        let mut matching = widgets.iter().filter(|w| w["code"].as_str() == Some(code));
        let first = matching.next().ok_or("Dashboard is incomplete.")?;
        if matching.next().is_some() || !first["output"].is_object() {
            return Err("Dashboard is ambiguous or incomplete.");
        }
        Ok(&first["output"])
    };
    if output("CMMachineStatus")?["status"].as_str() != Some("PoweredOn") {
        return Err("Coffee machine is not ready.");
    }
    let bbw = output("CMBrewByWeightDoses")?;
    if bbw["scaleConnected"].as_bool() != Some(true) {
        return Err("Coffee scale is disconnected or its connection is unknown.");
    }
    let mode = bbw["mode"]
        .as_str()
        .ok_or("Current dose mode is unavailable.")?;
    if !matches!(mode, "Dose1" | "Dose2" | "Continuous" | "PulsesType") {
        return Err("Current dose mode is unsupported.");
    }
    let dose = |name: &str| {
        bbw["doses"][name]["dose"]
            .as_f64()
            .and_then(dose_tenths)
            .ok_or("Stored doses are incomplete or invalid.")
    };
    Ok(BbwState {
        mode: mode.to_owned(),
        doses: [dose("Dose1")?, dose("Dose2")?],
    })
}

async fn bbw_json(mut response: reqwest::Response) -> Result<Value, &'static str> {
    if !response.status().is_success() {
        return Err("Cloud dashboard request failed.");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Cloud dashboard read failed.")?
    {
        if bytes.len() + chunk.len() > BBW_MAX_RESPONSE_BYTES {
            return Err("Cloud dashboard is too large.");
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "Cloud dashboard is not valid JSON.")
}

// ---------------------------------------------------------------------------
// Installation key and request signing
// ---------------------------------------------------------------------------

/// Key material of one "installation" of the app.
#[derive(Clone)]
pub struct InstallationKey {
    pub installation_id: String,
    key: SigningKey,
    /// 32 bytes derived from the id and the public key (see [`derive_secret`]).
    secret: [u8; 32],
}

impl std::fmt::Debug for InstallationKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallationKey")
            .field("installation_id", &self.installation_id)
            .finish_non_exhaustive()
    }
}

impl InstallationKey {
    /// Derives the same installation for the same credentials every time, so
    /// the app registration done with `POST /auth/init` stays valid across
    /// restarts without storing anything.
    pub fn derive(username: &str, password: &str) -> Self {
        let seed = Sha256::new()
            .chain_update(b"gbs-anywhere La Marzocco installation v1\0")
            .chain_update(username.trim().to_lowercase().as_bytes())
            .chain_update(b"\0")
            .chain_update(password.as_bytes())
            .finalize();
        let id_bytes = Sha256::new()
            .chain_update(seed)
            .chain_update(b"installation-id")
            .finalize();
        let installation_id = uuid_from_bytes(&id_bytes[..16]);
        let key = (0u8..)
            .find_map(|i| {
                let scalar = Sha256::new()
                    .chain_update(seed)
                    .chain_update(b"private-key")
                    .chain_update([i])
                    .finalize();
                SigningKey::from_slice(&scalar).ok()
            })
            .expect("a valid P-256 scalar within a few tries");
        let secret = derive_secret(&installation_id, &public_key_der(&key));
        Self {
            installation_id,
            key,
            secret,
        }
    }

    /// Public key, base64 of DER `SubjectPublicKeyInfo` (the `pk` in `/auth/init`).
    pub fn public_key_b64(&self) -> String {
        B64.encode(public_key_der(&self.key))
    }

    /// `installation_id.base64(sha256(public key DER))`.
    pub fn base_string(&self) -> String {
        let hash = Sha256::digest(public_key_der(&self.key));
        format!("{}.{}", self.installation_id, B64.encode(hash))
    }

    /// Headers for `POST /auth/init`.
    pub fn registration_headers(&self) -> Vec<(&'static str, String)> {
        vec![
            ("X-App-Installation-Id", self.installation_id.clone()),
            (
                "X-Request-Proof",
                request_proof(&self.base_string(), &self.secret),
            ),
        ]
    }

    /// Headers for every other request: nonce, timestamp, proof, signature.
    pub fn request_headers(&self) -> Vec<(&'static str, String)> {
        let nonce = uuid::Uuid::new_v4().to_string();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
            .to_string();
        self.request_headers_with(&nonce, &timestamp)
    }

    fn request_headers_with(&self, nonce: &str, timestamp: &str) -> Vec<(&'static str, String)> {
        let proof_input = format!("{}.{nonce}.{timestamp}", self.installation_id);
        let proof = request_proof(&proof_input, &self.secret);
        let signature: Signature = self.key.sign(format!("{proof_input}.{proof}").as_bytes());
        vec![
            ("X-App-Installation-Id", self.installation_id.clone()),
            ("X-Timestamp", timestamp.to_owned()),
            ("X-Nonce", nonce.to_owned()),
            ("X-Request-Signature", B64.encode(signature.to_der())),
        ]
    }
}

fn public_key_der(key: &SigningKey) -> Vec<u8> {
    key.verifying_key()
        .to_public_key_der()
        .expect("P-256 public key encodes")
        .as_bytes()
        .to_vec()
}

/// A lowercase UUID string (version 4 layout) from 16 bytes.
fn uuid_from_bytes(b: &[u8]) -> String {
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&b[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

/// `sha256("{id}.{base64(pub)}.{base64(sha256(id))}")`.
pub fn derive_secret(installation_id: &str, public_key_der: &[u8]) -> [u8; 32] {
    let id_hash = Sha256::digest(installation_id.as_bytes());
    let triple = format!(
        "{installation_id}.{}.{}",
        B64.encode(public_key_der),
        B64.encode(id_hash)
    );
    Sha256::digest(triple.as_bytes()).into()
}

/// La Marzocco's proof: fold the input into the 32-byte secret with
/// XOR-and-rotate, then hash. Base64 of the result.
pub fn request_proof(input: &str, secret: &[u8; 32]) -> String {
    let mut work = *secret;
    for &b in input.as_bytes() {
        let idx = usize::from(b % 32);
        let shift = u32::from(work[(idx + 1) % 32] & 7);
        let x = b ^ work[idx];
        work[idx] = x.rotate_left(shift);
    }
    B64.encode(Sha256::digest(work))
}

// ---------------------------------------------------------------------------
// Cloud client
// ---------------------------------------------------------------------------

struct Token {
    access: String,
    refresh: String,
    expires_at: Instant,
}

/// A minimal client for the customer-app API.
pub struct CloudClient {
    http: reqwest::Client,
    base_url: String,
    username: String,
    password: String,
    key: InstallationKey,
    token: Option<Token>,
}

/// One coffee from the machine's log.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastCoffee {
    /// Unix time in milliseconds.
    pub time: i64,
    pub extraction_seconds: f64,
    /// Weight in the cup in grams (with a Connected Scale), else absent.
    #[serde(default)]
    pub dose_value: Option<f64>,
    #[serde(default)]
    pub dose_mode: Option<String>,
    #[serde(default = "default_true")]
    pub valid: bool,
    #[serde(default)]
    pub invalid_reason: Option<String>,
}

fn default_true() -> bool {
    true
}

/// The coffees in a `LAST_COFFEE` reply, newest first. The cloud now and
/// then sends `null` in place of the list, so a list that is `null` or left
/// out means no coffees. A reply without an `output` object, or anything
/// else in place of the list, is still an error, so a change of format
/// does not go unnoticed.
pub fn parse_last_coffees(v: &Value) -> anyhow::Result<Vec<LastCoffee>> {
    let Some(output) = v.get("output").and_then(Value::as_object) else {
        bail!("LAST_COFFEE: unexpected shape: no output object");
    };
    let mut list = match output.get("lastCoffees") {
        None | Some(Value::Null) => Vec::new(),
        Some(raw) => {
            Vec::<LastCoffee>::deserialize(raw).context("LAST_COFFEE: unexpected shape")?
        }
    };
    list.sort_by_key(|c| std::cmp::Reverse(c.time));
    Ok(list)
}

/// A machine on the account.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thing {
    pub serial_number: String,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub model_name: String,
    #[serde(default)]
    pub name: String,
}

impl CloudClient {
    pub fn new(cfg: &Config) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .context("http client")?;
        Ok(Self {
            http,
            base_url: cfg.base_url.trim_end_matches('/').to_owned(),
            username: cfg.username.trim().to_owned(),
            password: cfg.password.clone(),
            key: InstallationKey::derive(&cfg.username, &cfg.password),
            token: None,
        })
    }

    pub fn installation_id(&self) -> &str {
        &self.key.installation_id
    }

    /// `POST /auth/init`: registers this installation's public key. Safe to
    /// repeat.
    pub async fn register(&self) -> anyhow::Result<()> {
        let mut req = self
            .http
            .post(format!("{}/auth/init", self.base_url))
            .json(&json!({ "pk": self.key.public_key_b64() }));
        for (k, v) in self.key.registration_headers() {
            req = req.header(k, v);
        }
        let resp = req
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("auth/init")?;
        if resp.status().is_success() {
            return Ok(());
        }
        Err(refused("auth/init", resp).await)
    }

    async fn access_token(&mut self) -> anyhow::Result<String> {
        let now = Instant::now();
        match &self.token {
            Some(t) if t.expires_at > now + TOKEN_REFRESH_MARGIN => Ok(t.access.clone()),
            Some(t) if t.expires_at > now => {
                let refresh = t.refresh.clone();
                let body = json!({ "username": self.username, "refreshToken": refresh });
                match self.token_request("auth/refreshtoken", &body).await {
                    Ok(t) => Ok(self.token.insert(t).access.clone()),
                    Err(e) => {
                        tracing::debug!("{TITLE}: refresh failed ({e:#}), signing in");
                        self.sign_in().await
                    }
                }
            }
            _ => self.sign_in().await,
        }
    }

    async fn sign_in(&mut self) -> anyhow::Result<String> {
        let body = json!({ "username": self.username, "password": self.password });
        let t = self.token_request("auth/signin", &body).await?;
        Ok(self.token.insert(t).access.clone())
    }

    async fn token_request(&self, path: &str, body: &Value) -> anyhow::Result<Token> {
        let mut req = self
            .http
            .post(format!("{}/{path}", self.base_url))
            .json(body);
        for (k, v) in self.key.request_headers() {
            req = req.header(k, v);
        }
        let resp = req
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .with_context(|| path.to_owned())?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!("{path}: login refused (check the e-mail address and password)");
        }
        if !status.is_success() {
            return Err(refused(path, resp).await);
        }
        let v: Value = resp
            .json()
            .await
            .map_err(reqwest::Error::without_url)
            .with_context(|| format!("{path}: bad JSON"))?;
        let (Some(access), Some(refresh)) = (v["accessToken"].as_str(), v["refreshToken"].as_str())
        else {
            bail!("{path}: no tokens in reply");
        };
        Ok(Token {
            access: access.to_owned(),
            refresh: refresh.to_owned(),
            expires_at: Instant::now() + TOKEN_LIFETIME,
        })
    }

    /// Authenticated `GET`, signing in again once on 401. Errors name `what`
    /// was asked for rather than the path, which holds the serial number.
    async fn get(&mut self, path: &str, what: &str) -> anyhow::Result<Value> {
        for attempt in 0..2 {
            let token = self.access_token().await?;
            let mut req = self
                .http
                .get(format!("{}/{path}", self.base_url))
                .bearer_auth(&token);
            for (k, v) in self.key.request_headers() {
                req = req.header(k, v);
            }
            let resp = req
                .send()
                .await
                // The error's URL holds the serial number.
                .map_err(reqwest::Error::without_url)
                .with_context(|| what.to_owned())?;
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                self.token = None;
                continue;
            }
            if !status.is_success() {
                return Err(refused(what, resp).await);
            }
            return resp
                .json()
                .await
                .map_err(reqwest::Error::without_url)
                .with_context(|| format!("{what}: bad JSON"));
        }
        bail!("{what}: still unauthorized after signing in again")
    }

    /// Prefer a matching preset; otherwise select Dose1 and update its weight,
    /// preserving Dose2. The caller must not retry an unknown result and must
    /// check its brew identity and TTL in `still_current`. All reads and writes
    /// share eight seconds; each command is sent at most once. Historical
    /// coffees never supply the current mode or doses.
    pub async fn select_bbw_target(
        &mut self,
        serial: &str,
        grams: f64,
        still_current: impl Fn() -> bool,
    ) -> BbwSelection {
        let mut sent = false;
        let attempt = self.select_bbw_once(serial, grams, &still_current, &mut sent);
        match tokio::time::timeout(BBW_TIMEOUT, attempt).await {
            Ok(result) => result,
            Err(_) if sent => BbwSelection::Unknown,
            Err(_) => BbwSelection::NotApplied("Recipe selection deadline expired."),
        }
    }

    async fn bbw_dashboard(
        &self,
        http: &reqwest::Client,
        serial: &str,
        token: &str,
    ) -> Result<BbwState, &'static str> {
        let mut request = http
            .get(format!("{}/things/{serial}/dashboard", self.base_url))
            .bearer_auth(token);
        for (key, value) in self.key.request_headers() {
            request = request.header(key, value);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "Cloud dashboard request failed.")?;
        let value = bbw_json(response).await?;
        parse_bbw_dashboard(&value, serial)
    }

    async fn select_bbw_once(
        &mut self,
        serial: &str,
        grams: f64,
        still_current: &impl Fn() -> bool,
        sent: &mut bool,
    ) -> BbwSelection {
        let Some(target) = dose_tenths(grams) else {
            return BbwSelection::NotApplied("Recipe target must be 0.1 to 200 g in 0.1 g steps.");
        };
        if serial.is_empty()
            || serial.len() > 64
            || !serial
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return BbwSelection::NotApplied("Machine identity is invalid.");
        }
        if !still_current() {
            return BbwSelection::NotApplied("Brew changed or its deadline expired.");
        }
        // A separate control client cannot replay a POST after a redirect or
        // a transport retry. The existing read-only polling client is unchanged.
        let Ok(http) = reqwest::Client::builder()
            .timeout(BBW_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
        else {
            return BbwSelection::NotApplied("Cloud control client could not start.");
        };
        let Ok(token) = self.access_token().await else {
            return BbwSelection::NotApplied("Cloud authentication failed.");
        };
        let before = match self.bbw_dashboard(&http, serial, &token).await {
            Ok(state) => state,
            Err(reason) => return BbwSelection::NotApplied(reason),
        };
        if !still_current() {
            return BbwSelection::NotApplied("Brew changed or its deadline expired.");
        }
        let modes = ["Dose1", "Dose2"];
        if modes
            .iter()
            .enumerate()
            .any(|(i, mode)| before.mode == *mode && before.doses[i] == target)
        {
            return BbwSelection::AlreadySet;
        }
        let (mode, update_dose1) = match (before.doses[0] == target, before.doses[1] == target) {
            (true, false) => (modes[0], false),
            (false, true) => (modes[1], false),
            (true, true) => {
                return BbwSelection::NotApplied("Both presets match; select one manually.");
            }
            (false, false) => (modes[0], true),
        };
        let mut observed = before;
        if observed.mode != mode {
            let mut request = http
                .post(format!(
                    "{}/things/{serial}/command/CoffeeMachineBrewByWeightChangeMode",
                    self.base_url
                ))
                .bearer_auth(&token)
                .json(&json!({"mode": mode}));
            for (key, value) in self.key.request_headers() {
                request = request.header(key, value);
            }
            if !still_current() {
                return BbwSelection::NotApplied("Brew changed or its deadline expired.");
            }
            *sent = true;
            match request.send().await {
                Ok(response) if response.status().is_success() => drop(response),
                _ => return BbwSelection::Unknown,
            }
            // A 2xx is only receipt. Before changing any weight, require a
            // fresh mode confirmation with both original doses still intact.
            observed = match self.bbw_dashboard(&http, serial, &token).await {
                Ok(after)
                    if still_current() && after.mode == mode && after.doses == observed.doses =>
                {
                    after
                }
                _ => return BbwSelection::Unknown,
            };
        }
        if !update_dose1 {
            return BbwSelection::Confirmed;
        }
        let expected_doses = [target, observed.doses[1]];
        let mut request = http
            .post(format!(
                "{}/things/{serial}/command/CoffeeMachineBrewByWeightSettingDoses",
                self.base_url
            ))
            .bearer_auth(&token)
            .json(&json!({"doses":{
                "Dose1":f64::from(target) / 10.0,
                "Dose2":f64::from(observed.doses[1]) / 10.0
            }}));
        for (key, value) in self.key.request_headers() {
            request = request.header(key, value);
        }
        // Last check after the fresh preflight and immediately before I/O.
        if !still_current() {
            return if *sent {
                BbwSelection::Unknown
            } else {
                BbwSelection::NotApplied("Brew changed or its deadline expired.")
            };
        }
        *sent = true;
        let response = match request.send().await {
            Ok(response) if response.status().is_success() => response,
            _ => return BbwSelection::Unknown,
        };
        // A 2xx only means the command was received. Never log its body,
        // replay it, or treat the old dashboard as proof it took effect.
        drop(response);
        let after = self.bbw_dashboard(&http, serial, &token).await;
        match after {
            Ok(after) if still_current() && after.mode == mode && after.doses == expected_doses => {
                BbwSelection::Confirmed
            }
            _ => BbwSelection::Unknown,
        }
    }

    pub async fn things(&mut self) -> anyhow::Result<Vec<Thing>> {
        let v = self.get("things", "machine list").await?;
        serde_json::from_value(v).context("things: unexpected shape")
    }

    /// The machine's recent coffees, newest first.
    pub async fn last_coffees(
        &mut self,
        serial: &str,
        days: u32,
    ) -> anyhow::Result<Vec<LastCoffee>> {
        let v = self
            .get(
                &format!("things/{serial}/stats/LAST_COFFEE/1?days={days}"),
                "coffee log",
            )
            .await?;
        parse_last_coffees(&v)
    }
}

/// A refused request as an error with its status only. Response bodies can
/// repeat passwords or tokens, so they must not enter even the debug log.
async fn refused(what: &str, resp: reqwest::Response) -> anyhow::Error {
    let status = resp.status();
    anyhow::anyhow!("{what} failed: {status}")
}

#[cfg(test)]
#[path = "cloud_bbw_tests.rs"]
mod bbw_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::VerifyingKey;
    use p256::ecdsa::signature::Verifier;

    // Vectors from pylamarzocco's algorithm run in Python.
    const IID: &str = "0b7c1a2e-3d4f-4a5b-8c6d-7e8f9a0b1c2d";

    fn fake_pub() -> Vec<u8> {
        (0u8..91).collect()
    }

    #[test]
    fn secret_and_proof_match_reference() {
        let secret = derive_secret(IID, &fake_pub());
        assert_eq!(
            B64.encode(secret),
            "YUvEPU91VouHGpwnyZfjIGsjSvQ3NEhQR5U5kApHI/s="
        );
        let base = format!("{IID}.{}", B64.encode(Sha256::digest(fake_pub())));
        assert_eq!(
            base,
            "0b7c1a2e-3d4f-4a5b-8c6d-7e8f9a0b1c2d.WNKUWbITCi4VElLUCLlebaxCTFZAYuuRHMdkQMuSbKA="
        );
        assert_eq!(
            request_proof(&base, &secret),
            "Xp7iqWry4Mg6VSdgZwdB4Wh2ehPgiejrPJ9b+4YZU3c="
        );
        assert_eq!(
            request_proof(
                &format!("{IID}.6f1d2c3b-4a59-4e6f-9a8b-7c6d5e4f3a2b.1790000000000"),
                &secret
            ),
            "9jww7upu1rj6nFFmsbQB8GFKpSDfI8R9BIRjrJsSQRM="
        );
        assert_eq!(
            request_proof("abc", &[0u8; 32]),
            "P5ynD3LZU7oRYIN0D2ZtOlbpepJdBXQH9vdqJGOfq/o="
        );
    }

    /// Pins the installation derived from a login, so a crypto crate update
    /// cannot silently give existing users a different installation.
    #[test]
    fn installation_key_and_signature_are_stable() {
        let a = InstallationKey::derive("rick@example.org", "hunter2");
        assert_eq!(a.installation_id, "1f7adb07-de9a-4453-941f-1b054dd04a7a");
        assert_eq!(
            a.public_key_b64(),
            "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEVCFLIfS/hQzZ3TwM1pAwaHVBw5YvJzV4GZbaB5BaViDxBYR+mIzSXprUaOhqPaiBXBfb5CovzvMom1//AGD4PA=="
        );
        assert_eq!(
            B64.encode(a.secret),
            "nKkcrK3K9+vUSI1k4VD/FTWNPZfbRrM+E0DFDogvf4E="
        );
        // ECDSA here is deterministic (RFC 6979), so the signature is too.
        let headers =
            a.request_headers_with("6f1d2c3b-4a59-4e6f-9a8b-7c6d5e4f3a2b", "1790000000000");
        let sig = headers
            .iter()
            .find(|(h, _)| *h == "X-Request-Signature")
            .unwrap();
        assert_eq!(
            sig.1,
            "MEQCIG24suhbxEwYgSZdIohWLaHKCog90AWRlJFEeDoMjTE8AiBxJbWCXR7KR380KtFggmYPlqYsgR1vsnlPkovW3gOroA=="
        );
    }

    #[test]
    fn installation_is_deterministic_and_signs() {
        let a = InstallationKey::derive("rick@example.org", "hunter2");
        let b = InstallationKey::derive(" Rick@Example.org ", "hunter2");
        let c = InstallationKey::derive("rick@example.org", "other");
        assert_eq!(a.installation_id, b.installation_id);
        assert_eq!(a.public_key_b64(), b.public_key_b64());
        assert_ne!(a.installation_id, c.installation_id);
        assert_eq!(a.installation_id.len(), 36);
        assert_eq!(&a.installation_id[14..15], "4");
        assert_eq!(a.base_string(), b.base_string());
        assert_eq!(
            a.secret,
            derive_secret(&a.installation_id, &public_key_der(&a.key))
        );

        let headers =
            a.request_headers_with("6f1d2c3b-4a59-4e6f-9a8b-7c6d5e4f3a2b", "1790000000000");
        let get = |k: &str| headers.iter().find(|(h, _)| *h == k).unwrap().1.clone();
        let proof_input = format!(
            "{}.6f1d2c3b-4a59-4e6f-9a8b-7c6d5e4f3a2b.1790000000000",
            a.installation_id
        );
        let signed = format!("{proof_input}.{}", request_proof(&proof_input, &a.secret));
        let sig = Signature::from_der(&B64.decode(get("X-Request-Signature")).unwrap()).unwrap();
        VerifyingKey::from(&a.key)
            .verify(signed.as_bytes(), &sig)
            .unwrap();
        assert_eq!(get("X-Nonce"), "6f1d2c3b-4a59-4e6f-9a8b-7c6d5e4f3a2b");
        assert_eq!(get("X-Timestamp"), "1790000000000");
    }

    /// A list in `output` that is `null` or missing is no coffees, not an
    /// error; any other shape still is.
    #[test]
    fn a_null_coffee_list_reads_as_no_coffees() {
        for none in [
            json!({"output": {"lastCoffees": null}}),
            json!({"output": {}}),
        ] {
            let list = parse_last_coffees(&none).unwrap();
            assert!(list.is_empty(), "{none}");
        }
        for wrong in [
            json!({"output": {"lastCoffees": {}}}),
            json!({"output": {"lastCoffees": "none"}}),
            json!({"output": {"lastCoffees": [null]}}),
            json!({"output": {"lastCoffees": [{"extractionSeconds": 25.0}]}}),
            json!({"output": null}),
            json!({}),
            json!({"output": []}),
            json!({"output": [null]}),
            json!({"output": 0}),
            json!([{"output": {}}]),
            json!([]),
            json!(null),
        ] {
            let e = parse_last_coffees(&wrong).unwrap_err();
            assert!(
                format!("{e:#}").starts_with("LAST_COFFEE: unexpected shape: "),
                "{wrong}: {e:#}"
            );
        }
    }

    #[test]
    fn coffees_come_newest_first() {
        let list = parse_last_coffees(&json!({"output": {"lastCoffees": [
            {"time": 1, "extractionSeconds": 20.0},
            {"time": 3, "extractionSeconds": 22.0},
            {"time": 2, "extractionSeconds": 21.0},
        ]}}))
        .unwrap();
        let times: Vec<i64> = list.iter().map(|c| c.time).collect();
        assert_eq!(times, [3, 2, 1]);
    }
}
