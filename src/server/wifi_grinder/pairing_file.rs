//! The pairing as it is kept across restarts, and who may write it.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Deserializer, Serialize};

use super::WifiConfig;

/// Held by the link that may write the pairing file, for as long as its
/// task lives. A replaced link is aborted, which only ends it at its next
/// await, and it may be saving the pairing right then: the next link waits
/// for it before it loads the pairing, so it starts from what the old one
/// saved and the file always has one writer.
pub(crate) type PairingLock = Arc<tokio::sync::Mutex<()>>;

/// What survives a restart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Pairing {
    /// Who this gbs-anywhere is to the grinder; made once.
    pub(super) brew_device_id: String,
    /// Set once the grinder accepted the pairing.
    pub(super) pairing_id: Option<u32>,
    /// The pairing ID the grinder refused for a while, kept to be tried again
    /// before the app asks for one: the grinder may have refused it only
    /// while it was starting up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) refused_pairing_id: Option<u32>,
    /// The pairing request the grinder has open, until it is accepted or
    /// gone: after a restart, gbs-anywhere goes on with it rather than
    /// asking again, which would list gbs-anywhere on the grinder once more.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "open_request_or_none"
    )]
    pub(super) open_request: Option<OpenRequest>,
}

/// An `open_request` this build can't read (written by another version, or
/// edited by hand) is dropped, not the whole file with the pairing in it.
fn open_request_or_none<'de, D: Deserializer<'de>>(d: D) -> Result<Option<OpenRequest>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).ok())
}

/// A pairing request the grinder answered and has not settled yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct OpenRequest {
    pub(super) application_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) pairing_code: Option<u32>,
    /// The address it went to (`WifiConfig::base_url`): a request made to
    /// another grinder is not checked with this one.
    pub(super) grinder: String,
}

impl Pairing {
    fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        let fresh = || Self {
            brew_device_id: uuid::Uuid::new_v4().to_string(),
            pairing_id: None,
            refused_pairing_id: None,
            open_request: None,
        };
        let Some(path) = path else {
            return Ok(fresh());
        };
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| {
                format!(
                    "{}: not a grinder pairing file (delete it to pair again)",
                    path.display()
                )
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(fresh()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// The pairing in `cfg`'s file, and the command line's pairing ID when
    /// it is another one, to fall back on.
    pub(super) fn open(cfg: &WifiConfig) -> anyhow::Result<(Self, Option<u32>)> {
        let mut pairing = Self::load(cfg.pairing_file.as_deref())?;
        let mut spare_id = cfg.pairing_id.filter(|id| pairing.pairing_id != Some(*id));
        if pairing.pairing_id.is_none() {
            pairing.pairing_id = spare_id.take();
        }
        // Paired, also through the command line's ID: no request to go on with.
        if pairing.pairing_id.is_some() {
            pairing.open_request = None;
        }
        Ok((pairing, spare_id))
    }

    pub(super) fn save(&self, path: Option<&Path>) -> anyhow::Result<()> {
        let Some(path) = path else {
            return Ok(());
        };
        let json = serde_json::to_vec_pretty(self)?;
        crate::server::private_file::save_private(path, &json)
    }
}
