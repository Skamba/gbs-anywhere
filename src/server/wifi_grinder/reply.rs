//! Requests to the grinder and what its replies say.

use std::net::SocketAddr;

use anyhow::{Context, bail};
use reqwest::StatusCode;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{MAX_REPLY_BYTES, WifiGrinder};
use crate::protocol::wifi;

/// A grinder reply: HTTP status, JSON body (`Null` when empty) and where it
/// came from.
pub(super) struct Reply {
    pub(super) http: StatusCode,
    pub(super) body: Value,
    pub(super) peer: Option<SocketAddr>,
}

impl Reply {
    pub(super) fn status(&self) -> &str {
        self.body
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    /// `Err` with the grinder's status unless it is `Ok`.
    pub(super) fn ok(&self) -> anyhow::Result<()> {
        match self.status() {
            wifi::STATUS_OK => Ok(()),
            "" => bail!("the grinder answered HTTP {}", self.http),
            s => bail!("the grinder answered {s:?}"),
        }
    }

    pub(super) fn parse<T: DeserializeOwned>(self) -> anyhow::Result<T> {
        serde_json::from_value(self.body).context("unexpected reply from the grinder")
    }
}

impl WifiGrinder {
    /// Any answer, even an error, means the grinder can be reached again.
    pub(super) async fn post(
        &mut self,
        path: &str,
        body: &impl Serialize,
    ) -> anyhow::Result<Reply> {
        let base = &self.cfg.base_url;
        let mut res = self
            .http
            .post(format!("{base}{path}"))
            .json(body)
            .send()
            .await
            .with_context(|| format!("cannot reach the grinder at {base}"))?;
        self.reached();
        let http = res.status();
        let peer = res.remote_addr();
        let bytes =
            crate::server::read_capped(&mut res, MAX_REPLY_BYTES, "the grinder's reply").await?;
        let text = wifi::decode(&bytes);
        tracing::debug!("grinder {path} -> {http} {text}");
        let body = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).context("unreadable reply from the grinder")?
        };
        Ok(Reply { http, body, peer })
    }

    /// The grinder answered: if it could not be reached before, say it is back.
    fn reached(&mut self) {
        if std::mem::take(&mut self.reach.unreachable) {
            tracing::info!("grinder: reachable again");
        }
    }
}

/// A client error from the grinder, which a retry would only repeat.
pub(super) fn refused(reply: &Reply) -> bool {
    reply.status().starts_with("ClientError")
}

/// Why a request that is tried again did not go through.
pub(super) fn failure(outcome: anyhow::Result<Reply>) -> String {
    match outcome.and_then(|r| r.ok()) {
        Err(e) => format!("{e:#}"),
        Ok(()) => "unexpected reply".to_owned(),
    }
}

/// Whether a request got no answer from the grinder: it failed in any way (a
/// grinder that is starting up can fail them in several), or the grinder sent
/// an HTTP error without a status.
pub(super) fn no_answer(outcome: &anyhow::Result<Reply>) -> bool {
    match outcome {
        Err(_) => true,
        Ok(reply) => reply.status().is_empty() && !reply.http.is_success(),
    }
}

/// Whether the grinder did not answer at all: connecting failed or timed out,
/// as while it is off, in standby, or gone from the network.
pub(super) fn unreachable(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|c| c.downcast_ref::<reqwest::Error>())
        .any(|r| r.is_connect() || r.is_timeout())
}
