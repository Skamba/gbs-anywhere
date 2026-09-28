//! La Marzocco cloud: reports shots from the machine's own coffee log.
//!
//! Connected La Marzocco machines upload every coffee to the La Marzocco
//! cloud with the extraction time and, with a Connected Scale, the weight in
//! the cup: the same numbers the machine shows after a shot. This integration
//! logs in like the La Marzocco Home app and, while the grinder is waiting
//! for a shot after a knob press, polls that list until a new coffee appears,
//! then reports it.
//!
//! This works on machines whose live brewing state never reaches the cloud
//! (the Linea Mini R, for one: Home Assistant's `brewing_active` sensor stays
//! off there) and reports the measured weight; the price is a few seconds of
//! delay after the shot and a login with the La Marzocco account.
//!
//! The protocol is the one the La Marzocco Home app speaks
//! (`lion.lamarzocco.io/api/customer-app`), as reverse engineered by
//! pylamarzocco: each *installation* has a P-256 key registered once with
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

use super::{Backoff, BoxFuture, Integration, Link};

/// The customer-app API the La Marzocco Home app uses.
pub const BASE_URL: &str = "https://lion.lamarzocco.io/api/customer-app";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MIN_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// Tokens last an hour; refresh this long before that.
const TOKEN_LIFETIME: Duration = Duration::from_secs(3600);
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(600);
const TITLE: &str = "La Marzocco cloud";

/// Command-line flags. Flatten into the CLI; `build` turns them into the
/// integration when `--lm-username` is given.
#[derive(Debug, Clone, clap::Args)]
#[command(next_help_heading = "La Marzocco cloud")]
pub struct LaMarzoccoArgs {
    /// La Marzocco Home account e-mail. Enables reporting shots from the
    /// machine's own coffee log in the La Marzocco cloud.
    #[arg(long, env = "LM_USERNAME")]
    pub lm_username: Option<String>,

    /// La Marzocco Home account password.
    #[arg(long, env = "LM_PASSWORD", hide_env_values = true)]
    pub lm_password: Option<String>,

    /// Machine serial number, e.g. MI004024. Needed with more than one
    /// machine on the account.
    #[arg(long, env = "LM_SERIAL")]
    pub lm_serial: Option<String>,

    /// Seconds between checks of the coffee log while the grinder waits.
    #[arg(long, env = "LM_POLL_S", default_value_t = 3.0)]
    pub lm_poll_s: f64,

    /// API base URL (for tests against a fake server).
    #[arg(long, env = "LM_BASE_URL", default_value = BASE_URL, hide = true)]
    pub lm_base_url: String,
}

impl LaMarzoccoArgs {
    /// The integration, or `None` when `--lm-username` is not given.
    pub fn build(&self) -> anyhow::Result<Option<Box<dyn Integration>>> {
        let Some(username) = self.lm_username.as_deref().map(str::trim) else {
            return Ok(None);
        };
        let password = self
            .lm_password
            .as_deref()
            .filter(|p| !p.is_empty())
            .context("--lm-username needs --lm-password (or LM_PASSWORD)")?;
        if !(self.lm_poll_s.is_finite() && self.lm_poll_s >= 1.0) {
            bail!("--lm-poll-s must be at least 1 second");
        }
        Ok(Some(Box::new(LaMarzocco {
            cfg: LmConfig {
                username: username.to_owned(),
                password: password.to_owned(),
                serial: self.lm_serial.clone().filter(|s| !s.trim().is_empty()),
                poll: Duration::from_secs_f64(self.lm_poll_s),
                base_url: self.lm_base_url.clone(),
            },
        })))
    }
}

/// Account and machine to watch.
#[derive(Debug, Clone)]
pub struct LmConfig {
    /// La Marzocco Home account.
    pub username: String,
    pub password: String,
    /// Machine serial (e.g. `MI004024`). `None`: the account's only machine.
    pub serial: Option<String>,
    /// How often to ask for new coffees while the grinder is waiting.
    pub poll: Duration,
    /// API base URL; only tests point this elsewhere.
    pub base_url: String,
}

/// The integration. See the module docs.
pub struct LaMarzocco {
    pub cfg: LmConfig,
}

impl Integration for LaMarzocco {
    fn id(&self) -> &'static str {
        "la_marzocco"
    }

    fn title(&self) -> &'static str {
        TITLE
    }

    fn run(self: Box<Self>, link: Link) -> BoxFuture {
        Box::pin(run(link, self.cfg))
    }
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
    pub fn new(cfg: &LmConfig) -> anyhow::Result<Self> {
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
            .map_err(|e| anyhow::anyhow!("auth/init: {e}"))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        bail!("auth/init failed: {status} {}", body.trim());
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
            .map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!("{path}: login refused (check --lm-username / --lm-password)");
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("{path} failed: {status} {}", body.trim());
        }
        let v: Value = resp
            .json()
            .await
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

    /// Authenticated `GET`, signing in again once on 401.
    async fn get(&mut self, path: &str) -> anyhow::Result<Value> {
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
                .map_err(|e| anyhow::anyhow!("GET {path}: {e}"))?;
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                self.token = None;
                continue;
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                bail!("GET {path} failed: {status} {}", body.trim());
            }
            return resp
                .json()
                .await
                .with_context(|| format!("GET {path}: bad JSON"));
        }
        bail!("GET {path}: still unauthorized after signing in again")
    }

    pub async fn things(&mut self) -> anyhow::Result<Vec<Thing>> {
        let v = self.get("things").await?;
        serde_json::from_value(v).context("things: unexpected shape")
    }

    /// The machine's recent coffees, newest first.
    pub async fn last_coffees(
        &mut self,
        serial: &str,
        days: u32,
    ) -> anyhow::Result<Vec<LastCoffee>> {
        let v = self
            .get(&format!("things/{serial}/stats/LAST_COFFEE/1?days={days}"))
            .await?;
        let mut list: Vec<LastCoffee> = serde_json::from_value(v["output"]["lastCoffees"].clone())
            .context("LAST_COFFEE: unexpected shape")?;
        list.sort_by_key(|c| std::cmp::Reverse(c.time));
        Ok(list)
    }
}

// ---------------------------------------------------------------------------
// The integration
// ---------------------------------------------------------------------------

/// The coffee to report: the newest one logged after `after_ms`, if valid.
/// Invalid ones (the cloud's own judgement) are skipped so the grinder keeps
/// waiting for a real shot.
pub fn new_coffee(coffees: &[LastCoffee], after_ms: i64) -> Option<&LastCoffee> {
    coffees
        .iter()
        .filter(|c| c.time > after_ms)
        .max_by_key(|c| c.time)
        .filter(|c| c.valid)
}

async fn run(link: Link, cfg: LmConfig) {
    let mut backoff = Backoff::new(MIN_BACKOFF, MAX_BACKOFF);
    loop {
        link.status.starting("logging in");
        match connect(&cfg).await {
            Ok((client, machine)) => {
                backoff.reset();
                link.status.subject(&machine.subject);
                link.status.connected(idle_line(machine.last.as_ref()));
                if let Err(e) = watch(&link, &cfg, client, machine).await {
                    tracing::warn!("{TITLE}: {e:#}");
                    link.status.error(format!("{e:#}"));
                }
            }
            Err(e) => {
                tracing::warn!("{TITLE}: {e:#}");
                link.status.error(format!("{e:#}"));
            }
        }
        tracing::info!("{TITLE}: retrying in {} s", backoff.peek().as_secs());
        backoff.wait().await;
    }
}

struct Machine {
    serial: String,
    /// `MI004024 · Linea Mini R`.
    subject: String,
    last: Option<Idle>,
}

/// The newest coffee we know of, for the idle status line.
#[derive(Debug, Clone, Copy)]
struct Idle {
    time_ms: i64,
    extraction_seconds: f64,
    grams: Option<f64>,
}

impl From<&LastCoffee> for Idle {
    fn from(c: &LastCoffee) -> Self {
        Self {
            time_ms: c.time,
            extraction_seconds: c.extraction_seconds,
            grams: c.dose_value,
        }
    }
}

/// `last coffee 3 min ago: 25.1 s, 38.0 g`, with the age as of now.
fn idle_line(last: Option<&Idle>) -> String {
    match last {
        Some(c) => format!(
            "last coffee {} ago: {:.1} s{}",
            age(c.time_ms),
            c.extraction_seconds,
            c.grams.map_or(String::new(), |g| format!(", {g:.1} g"))
        ),
        None => "no coffee in the last 7 days".to_owned(),
    }
}

/// How often the idle line's age is refreshed.
const IDLE_REFRESH: Duration = Duration::from_secs(60);

/// Registers, logs in, finds the machine and reads its last coffee.
async fn connect(cfg: &LmConfig) -> anyhow::Result<(CloudClient, Machine)> {
    let mut client = CloudClient::new(cfg)?;
    tracing::debug!("{TITLE}: installation {}", client.installation_id());
    client.register().await?;
    let things = client.things().await?;
    let machines: Vec<&Thing> = things
        .iter()
        .filter(|t| t.r#type.is_empty() || t.r#type == "CoffeeMachine")
        .collect();
    let machine = match &cfg.serial {
        Some(sn) => {
            let sn = sn.trim().to_uppercase();
            machines
                .iter()
                .find(|t| t.serial_number.eq_ignore_ascii_case(&sn))
                .copied()
                .with_context(|| {
                    format!(
                        "machine {sn} not on this account (found: {})",
                        list_serials(&machines)
                    )
                })?
        }
        None => match machines.as_slice() {
            [one] => one,
            [] => bail!("no coffee machine on this account"),
            many => bail!(
                "more than one machine on this account, set --lm-serial: {}",
                list_serials(many)
            ),
        },
    };
    let serial = machine.serial_number.clone();
    let coffees = client.last_coffees(&serial, 7).await?;
    let last = coffees.first().map(Idle::from);
    let subject = format!("{serial} · {}", machine.model_name);
    tracing::info!(
        "{TITLE}: connected to {subject}, {}",
        idle_line(last.as_ref())
    );
    Ok((
        client,
        Machine {
            serial,
            subject,
            last,
        },
    ))
}

fn list_serials(machines: &[&Thing]) -> String {
    machines
        .iter()
        .map(|t| format!("{} ({})", t.serial_number, t.model_name))
        .collect::<Vec<_>>()
        .join(", ")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn age(time_ms: i64) -> String {
    age_of(now_ms() - time_ms)
}

/// `45 s`, `12 min`, `5 h`, `2 days`.
fn age_of(delta_ms: i64) -> String {
    let s = delta_ms.max(0) / 1000;
    if s < 120 {
        format!("{s} s")
    } else if s < 7200 {
        format!("{} min", s / 60)
    } else if s < 48 * 3600 {
        format!("{} h", s / 3600)
    } else {
        format!("{} days", s / 86_400)
    }
}

/// Waits for knob presses and polls the coffee log during each brew.
async fn watch(
    link: &Link,
    cfg: &LmConfig,
    mut client: CloudClient,
    machine: Machine,
) -> anyhow::Result<()> {
    let mut brews = link.grinder_brews();
    // Newest coffee we know of; only newer ones count as the shot.
    let mut newest_ms = now_ms();
    let mut idle = machine.last;
    let mut refresh = tokio::time::interval(IDLE_REFRESH);
    refresh.tick().await; // the first tick fires at once
    loop {
        tokio::select! {
            pressed = brews.next() => {
                if !pressed {
                    return Ok(());
                }
            }
            _ = refresh.tick() => {
                link.status.connected(idle_line(idle.as_ref()));
                continue;
            }
        }
        // Baseline: whatever the log holds at the knob press is not the shot.
        match client.last_coffees(&machine.serial, 1).await {
            Ok(list) => {
                if let Some(c) = list.first() {
                    newest_ms = newest_ms.max(c.time);
                }
            }
            Err(e) => tracing::warn!("{TITLE}: {e:#}"),
        }
        let poll_s = cfg.poll.as_secs();
        tracing::info!("{TITLE}: grinder is waiting, watching the coffee log every {poll_s} s");
        link.status.watching(format!(
            "grinder is waiting · checking the coffee log every {poll_s} s"
        ));
        let mut errors = 0u32;
        loop {
            tokio::time::sleep(cfg.poll).await;
            if !link.grinder_waiting() {
                tracing::info!("{TITLE}: brew ended without a coffee from the log");
                break;
            }
            let list = match client.last_coffees(&machine.serial, 1).await {
                Ok(l) => {
                    errors = 0;
                    l
                }
                Err(e) => {
                    errors += 1;
                    tracing::warn!("{TITLE}: {e:#}");
                    link.status.error(format!("{e:#}"));
                    if errors >= 10 {
                        bail!("giving up on this session after {errors} errors");
                    }
                    continue;
                }
            };
            let Some(c) = new_coffee(&list, newest_ms) else {
                if let Some(c) = list.iter().find(|c| c.time > newest_ms) {
                    tracing::info!(
                        "{TITLE}: ignoring coffee marked invalid ({}): {:.1} s",
                        c.invalid_reason.as_deref().unwrap_or("no reason"),
                        c.extraction_seconds
                    );
                    newest_ms = c.time;
                }
                continue;
            };
            newest_ms = c.time;
            let time =
                Duration::from_millis((c.extraction_seconds.max(0.0) * 1000.0).round() as u64);
            // The cloud sends float32 noise like 37.4000015; the scale reads 0.1 g.
            let weight = c.dose_value.map(|g| {
                (
                    (g * 10.0).round() / 10.0,
                    "weighed by the machine".to_owned(),
                )
            });
            link.report(time, weight);
            idle = Some(Idle::from(c));
            break;
        }
        link.status.connected(idle_line(idle.as_ref()));
    }
}

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

    #[test]
    fn ages_read_naturally() {
        assert_eq!(age_of(45_000), "45 s");
        assert_eq!(age_of(76 * 60_000), "76 min");
        assert_eq!(age_of(5 * 3_600_000), "5 h");
        assert_eq!(age_of(47 * 3_600_000), "47 h");
        assert_eq!(age_of(3 * 86_400_000 + 5), "3 days");
        assert_eq!(age_of(-5), "0 s");
        let line = idle_line(Some(&Idle {
            time_ms: now_ms() - 90_000,
            extraction_seconds: 25.1,
            grams: Some(38.0),
        }));
        assert_eq!(line, "last coffee 90 s ago: 25.1 s, 38.0 g");
        assert_eq!(idle_line(None), "no coffee in the last 7 days");
    }

    #[test]
    fn parses_and_picks_the_new_coffee() {
        let raw = json!({"output": {"lastCoffees": [
            {"time": 1790536239820i64, "extractionSeconds": 22.091999, "doseMode": "MassType",
             "doseIndex": "DoseA", "doseValue": 37.9, "doseValueNumerator": null,
             "targetTemperature": 93, "valid": true, "invalidReason": null},
            {"time": 1790535892774i64, "extractionSeconds": 24.777, "doseMode": "MassType",
             "doseIndex": "DoseA", "doseValue": 38, "valid": true},
            {"time": 1790535000000i64, "extractionSeconds": 3.0, "valid": false,
             "invalidReason": "TooShort"}
        ]}});
        let mut list: Vec<LastCoffee> =
            serde_json::from_value(raw["output"]["lastCoffees"].clone()).unwrap();
        list.sort_by_key(|c| std::cmp::Reverse(c.time));
        assert_eq!(list[0].dose_value, Some(37.9));
        assert!((list[0].extraction_seconds - 22.092).abs() < 1e-3);
        assert!(!list[2].valid);

        // Nothing newer than the newest: keep waiting.
        assert_eq!(new_coffee(&list, 1790536239820), None);
        // The shot after the knob press.
        assert_eq!(
            new_coffee(&list, 1790535892774).unwrap().time,
            1790536239820
        );
        // Two new ones: the newest wins.
        assert_eq!(new_coffee(&list, 0).unwrap().time, 1790536239820);
        // Only an invalid one is new: skipped.
        let only_invalid = vec![list[2].clone()];
        assert_eq!(new_coffee(&only_invalid, 0), None);
    }
}
