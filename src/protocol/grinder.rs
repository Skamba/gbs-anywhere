//! What the grinder makes of a sequence of mako replies: a model of its brew
//! state machine.
//!
//! The rule that matters: a shot only counts if the **grinder** started it.
//! After a Grind-by-Sync grind the grinder shows "Press grinder rotary knob to
//! start brewing."; the knob press sets `Starting` and posts
//! `/api/v2/scripts/execute {"ID":9}`. Only when the machine then enters
//! `ACTIVE SERVING` is the brew an `Extraction`; a brew the machine starts on
//! its own is treated as a `Flushing`. The result is taken on the change to
//! `ACTIVE FINISHING`.

use serde::Serialize;

use super::{EXTRACTION_STATUS_USER_ABORT, MAX_EXTRACTION_TIME_MS, MakoState};

/// The grinder's internal brew state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrewState {
    Idle = 0,
    /// The machine brews without the grinder having asked (flush/manual shot).
    Flushing = 1,
    /// A flush went to `ACTIVE FINISHING`; sticky until the next brew.
    FlushDone = 2,
    /// The grinder asked the machine to start (`scripts/execute`).
    Starting = 3,
    /// The machine confirmed by entering `ACTIVE SERVING`; the grinder
    /// restarts its timer and zeroes time/volume.
    Extraction = 4,
    /// Brew ended normally; the grinder emits `brewFinished`.
    Finishing = 5,
    /// Brew ended with `MA_EXTRACTION_STATUS == 2`; `brewAborted`.
    Aborted = 6,
}

/// Something the grinder would do in reaction to a mako reply.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrinderEvent {
    /// The brew state changed.
    BrewState { from: BrewState, to: BrewState },
    /// A shot result was accepted and handed to the dial-in algorithm.
    ShotAccepted { time_ms: u32, volume_ml: f64 },
    /// A finished brew was skipped; `reason` is the grinder's log text.
    ShotSkipped { reason: &'static str },
}

/// Replays mako replies through the grinder's brew logic.
#[derive(Debug, Clone)]
pub struct GrinderModel {
    status: i64,
    extraction_status: i64,
    brew_state: BrewState,
}

impl Default for GrinderModel {
    fn default() -> Self {
        Self {
            status: 0,
            extraction_status: 0,
            brew_state: BrewState::Idle,
        }
    }
}

impl GrinderModel {
    pub fn brew_state(&self) -> BrewState {
        self.brew_state
    }

    /// The knob press after a grind.
    pub fn request_start(&mut self) -> Vec<GrinderEvent> {
        let mut out = Vec::new();
        self.set(BrewState::Starting, &mut out);
        out
    }

    /// Feeds one mako reply.
    pub fn observe(&mut self, s: &MakoState) -> Vec<GrinderEvent> {
        self.observe_raw(
            s.status.code(),
            s.extraction_status,
            s.real_extraction_time_ms,
            s.flow_meter_volume_ml,
        )
    }

    /// Feeds one mako reply given as raw values (status codes the typed enum
    /// cannot hold are allowed; the grinder ignores them).
    pub fn observe_raw(
        &mut self,
        status: i64,
        extraction_status: i64,
        time_ms: u32,
        volume_ml: f64,
    ) -> Vec<GrinderEvent> {
        let mut out = Vec::new();
        let (old_status, old_extr) = (self.status, self.extraction_status);
        self.status = status;
        self.extraction_status = extraction_status;
        if old_status == status && old_extr == extraction_status {
            return out;
        }
        use BrewState::*;
        let next = match status {
            0 | 2 | 5 | 6 => Some(Idle),
            1 => (self.brew_state != FlushDone).then_some(Idle),
            3 if old_status != 3 => Some(if self.brew_state == Starting {
                Extraction
            } else {
                Flushing
            }),
            4 => Some(if matches!(self.brew_state, Flushing | FlushDone) {
                FlushDone
            } else if extraction_status == EXTRACTION_STATUS_USER_ABORT {
                Aborted
            } else {
                Finishing
            }),
            _ => None,
        };
        if let Some(next) = next
            && self.set(next, &mut out)
            && next == Finishing
        {
            out.push(self.brew_finished(time_ms, volume_ml));
        }
        out
    }

    fn set(&mut self, to: BrewState, out: &mut Vec<GrinderEvent>) -> bool {
        if self.brew_state == to {
            return false;
        }
        out.push(GrinderEvent::BrewState {
            from: self.brew_state,
            to,
        });
        self.brew_state = to;
        true
    }

    /// The grinder's verdict on a finished brew.
    fn brew_finished(&self, time_ms: u32, volume_ml: f64) -> GrinderEvent {
        if self.extraction_status == EXTRACTION_STATUS_USER_ABORT {
            GrinderEvent::ShotSkipped {
                reason: "skip result, user abort",
            }
        } else if time_ms > MAX_EXTRACTION_TIME_MS {
            GrinderEvent::ShotSkipped {
                reason: "skip result, duration out of range",
            }
        } else {
            GrinderEvent::ShotAccepted { time_ms, volume_ml }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::MachineStatus;

    fn reply(status: MachineStatus, extr: i64, time_ms: u32) -> MakoState {
        MakoState {
            status,
            extraction_status: extr,
            real_extraction_time_ms: time_ms,
            flow_meter_volume_ml: 40.0,
            ..MakoState::default()
        }
    }

    fn accepted(events: &[GrinderEvent]) -> Option<u32> {
        events.iter().find_map(|e| match e {
            GrinderEvent::ShotAccepted { time_ms, .. } => Some(*time_ms),
            _ => None,
        })
    }

    #[test]
    fn grinder_started_shot_is_accepted() {
        use MachineStatus::*;
        let mut g = GrinderModel::default();
        g.observe(&reply(On, 0, 0));
        g.request_start();
        g.observe(&reply(ActiveServing, 1, 1000));
        assert_eq!(g.brew_state(), BrewState::Extraction);
        g.observe(&reply(ActiveServing, 1, 15000));
        let ev = g.observe(&reply(ActiveFinishing, 1, 29000));
        assert_eq!(accepted(&ev), Some(29000));
        g.observe(&reply(On, 0, 29000));
        assert_eq!(g.brew_state(), BrewState::Idle);
    }

    #[test]
    fn machine_started_shot_is_a_flush() {
        use MachineStatus::*;
        let mut g = GrinderModel::default();
        g.observe(&reply(On, 0, 0));
        g.observe(&reply(ActiveServing, 1, 1000));
        assert_eq!(g.brew_state(), BrewState::Flushing);
        let ev = g.observe(&reply(ActiveFinishing, 1, 29000));
        assert_eq!(accepted(&ev), None);
        assert_eq!(g.brew_state(), BrewState::FlushDone);
        g.observe(&reply(On, 0, 29000));
        assert_eq!(g.brew_state(), BrewState::FlushDone, "sticky");
    }

    #[test]
    fn abort_and_out_of_range_are_skipped() {
        use MachineStatus::*;
        let mut g = GrinderModel::default();
        g.request_start();
        g.observe(&reply(ActiveServing, 1, 0));
        g.observe(&reply(ActiveFinishing, 2, 20000));
        assert_eq!(g.brew_state(), BrewState::Aborted);

        let mut g = GrinderModel::default();
        g.request_start();
        g.observe(&reply(ActiveServing, 1, 0));
        let ev = g.observe(&reply(ActiveFinishing, 1, MAX_EXTRACTION_TIME_MS + 1));
        assert!(
            ev.iter()
                .any(|e| matches!(e, GrinderEvent::ShotSkipped { .. }))
        );
    }

    #[test]
    fn repeated_identical_replies_do_nothing() {
        use MachineStatus::*;
        let mut g = GrinderModel::default();
        g.request_start();
        g.observe(&reply(ActiveServing, 1, 0));
        assert!(g.observe(&reply(ActiveServing, 1, 5000)).is_empty());
    }
}
