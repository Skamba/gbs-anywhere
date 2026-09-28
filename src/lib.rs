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
