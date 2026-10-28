//! The pairing as it is kept across restarts, and who may write it.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};

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
}

impl Pairing {
    fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        let fresh = || Self {
            brew_device_id: uuid::Uuid::new_v4().to_string(),
            pairing_id: None,
            refused_pairing_id: None,
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
