//! The Grind-by-Sync ("mako") link between an E64 WS grinder and the espresso
//! machine it syncs with, as plain types.
//!
//! The grinder is the HTTP **client** and the actor: it polls the machine, runs
//! its dial-in algorithm and moves its discs. The machine only reports numbers,
//! so anything that serves these types on port 80 can be the machine.
//!
//! * [`wire`] — the JSON bodies, with the exact keys the grinder reads/writes.
//! * [`grinder`] — a model of how the grinder interprets a sequence of mako
//!   replies (its brew state machine). Handy in tests, or to show what the
//!   grinder believes.
//! * [`machine`] — a time-driven machine that produces the mako replies for a
//!   shot a person runs and measures. No I/O; the caller supplies `Instant`s.

pub mod grinder;
pub mod machine;
pub mod wire;

pub use grinder::{BrewState, GrinderEvent, GrinderModel};
pub use machine::{Machine, MachineConfig, MachineError, MachineEvent, Phase, ShotResult};
pub use wire::{
    GrindBlocked, GrindResult, GrinderRequest, MachineIdentity, MachineStatus, MakoState, StartBrew,
};

/// Endpoints on the machine that the grinder calls.
pub const PATH_MACHINE: &str = "/api/v2/machine";
pub const PATH_MAKO: &str = "/api/v2/mako";
pub const PATH_BREWRATIO: &str = "/api/v2/brewratio";
pub const PATH_SCRIPTS_EXECUTE: &str = "/api/v2/scripts/execute";

/// `ID` the grinder posts to [`PATH_SCRIPTS_EXECUTE`] to start a brew on the
/// knob press.
pub const START_BREW_SCRIPT_ID: i64 = 9;

/// The grinder re-sends the start-brew request until the machine enters
/// `ACTIVE SERVING`, at most this many times.
pub const START_BREW_MAX_SENDS: u32 = 3;

/// The grinder rejects a shot with
/// `REAL_EXTRACTION_TIME_MS > 80000`.
pub const MAX_EXTRACTION_TIME_MS: u32 = 80_000;

/// `MA_EXTRACTION_STATUS` value that means "user abort": the brew goes to
/// `Aborted` instead of `Finishing` and the result is skipped.
pub const EXTRACTION_STATUS_USER_ABORT: i64 = 2;

/// The grinder counts the machine as heated when
/// `BG_SENS_TEMP > BG_SET_TEMP - 2.0`.
pub const HEATED_TOLERANCE_C: f64 = 2.0;

/// The grinder counts the tank as full when `TANK_LEVEL == 1`.
pub const TANK_LEVEL_FULL: i64 = 1;
