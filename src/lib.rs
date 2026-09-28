//! Grind-by-Sync for the Mahlkönig E64 WS with any espresso machine.
//!
//! * [`protocol`] — the grinder link as plain types, no I/O.
//! * [`server`] — the HTTP server the grinder polls, plus the control API and
//!   phone app.
//! * [`integration`] — machine integrations that report the shot on their
//!   own (the La Marzocco cloud, ...), and the small API for writing one.

pub mod integration;
pub mod protocol;
pub mod server;

use std::sync::LazyLock;

/// The version people see: the release number from Cargo.toml, plus the
/// commit for builds between releases (`0.1.0+1a2b3c4`). CI passes the commit
/// in as `GBS_COMMIT` at build time; tagged releases build without it.
pub fn version() -> &'static str {
    static VERSION: LazyLock<String> = LazyLock::new(|| {
        let release = env!("CARGO_PKG_VERSION");
        match option_env!("GBS_COMMIT") {
            Some(c) if !c.is_empty() => format!("{release}+{}", &c[..c.len().min(7)]),
            _ => release.to_owned(),
        }
    });
    &VERSION
}
