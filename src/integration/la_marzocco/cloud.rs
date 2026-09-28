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
            bail!("{path}: login refused (check the e-mail address and password)");
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
}
