//! The Grind-by-Sync link between a grinder and the espresso machine it syncs
//! with, as plain types.
//!
//! For the E64 WS ("mako") the grinder is the HTTP **client** and the actor:
//! it polls the machine, runs its dial-in algorithm and moves its discs. The
//! machine only reports numbers, so anything that serves these types on port
//! 80 can be the machine. The E65/E80 GbS works the other way round, see
//! [`wifi`].
//!
//! * [`wire`] — the JSON bodies, with the exact keys the grinder reads/writes.
//! * [`grinder`] — a model of how the E64 WS interprets a sequence of mako
//!   replies (its brew state machine). Handy in tests, or to show what the
//!   grinder believes.
//! * [`machine`] — a time-driven machine that produces the mako replies for a
//!   shot a person runs and measures. No I/O; the caller supplies `Instant`s.
//! * [`wifi`] — the other direction, for the E65/E80 GbS family: the machine
//!   pairs with the grinder and fetches its brew jobs over its Local WiFi.

pub mod grinder;
pub mod machine;
pub mod wifi;
pub mod wire;

use std::time::Duration;

pub use grinder::{BrewState, GrinderEvent, GrinderModel};
pub use machine::{
    AbortCause, GrinderRange, Machine, MachineConfig, MachineError, MachineEvent, Phase, ShotResult,
};
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

/// How often the E64 WS polls `GET /api/v2/mako`.
pub const E64_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The shortest finishing hold (`--finishing-hold-s`): the E64 WS takes a
/// shot's result only from a poll that sees `ACTIVE FINISHING`, so the hold
/// spans at least one poll ([`E64_POLL_INTERVAL`]), with room for a late one.
pub const MIN_FINISHING_HOLD: Duration = Duration::from_secs(3);

/// The longest finishing hold: while a shot is shown as finishing, the
/// machine starts no other, so an E65/E80's next brew job waits for it.
pub const MAX_FINISHING_HOLD: Duration = Duration::from_secs(60);

/// The grinder does not adjust from a shot of 10 s or less
/// (`REAL_EXTRACTION_TIME_MS <= 10000`) and leaves it to the person to grind
/// finer. The E64 WS still takes the result, so [`GrinderModel`] counts it
/// as accepted; the E65/E80 applies the 10 s minimum too, as the floor under
/// its window around the target time ([`GrinderRange::around`]).
pub const MIN_EXTRACTION_TIME_MS: u32 = 10_000;

/// The E64 WS skips a shot with `REAL_EXTRACTION_TIME_MS > 80000` (see
/// [`GrinderModel`]).
pub const MAX_EXTRACTION_TIME_MS: u32 = 80_000;

/// An E65/E80 GbS adjusts only from a shot within 15 s of its brew job's
/// target time ([`GrinderRange::around`]).
pub const MAX_OFF_TARGET_MS: u32 = 15_000;

/// `MA_EXTRACTION_STATUS` value that means "user abort": the brew goes to
/// `Aborted` instead of `Finishing` and the result is skipped.
pub const EXTRACTION_STATUS_USER_ABORT: i64 = 2;

/// `MA_EXTRACTION_STATUS` while ready, while brewing and once a shot is
/// done. Only [`EXTRACTION_STATUS_USER_ABORT`] means something to the
/// grinder; these values are a best guess.
pub const EXTRACTION_STATUS_IDLE: i64 = 0;
pub const EXTRACTION_STATUS_BREWING: i64 = 1;
pub const EXTRACTION_STATUS_FINISHED: i64 = 1;

/// The grinder counts the machine as heated when
/// `BG_SENS_TEMP > BG_SET_TEMP - 2.0`.
pub const HEATED_TOLERANCE_C: f64 = 2.0;

/// The grinder counts the tank as full when `TANK_LEVEL == 1`.
pub const TANK_LEVEL_FULL: i64 = 1;
