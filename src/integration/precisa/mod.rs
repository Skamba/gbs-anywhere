//! Eureka Precisa: reports shots weighed and timed by the scale under the cup.
//!
//! The Precisa (a Krell CFS-9002 sold by Eureka) sends its weight and timer
//! over Bluetooth LE. This integration keeps it connected and, after each
//! knob press (on an E65/E80 GbS: each Grind-by-Sync grind), waits a moment
//! for the machine to be started, then tares the scale, starts its timer and
//! watches the cup fill. The shot ends when the cup reaches the grinder's
//! recipe weight, when the weight stops rising, or when the scale's timer is
//! stopped; its time and the weight in the cup go to the grinder at once,
//! and the scale beeps twice (four times when nothing was reported).
//!
//! Needs the `bluetooth` feature, a Bluetooth adapter on the host within
//! reach of the scale, and the scale not held by another app.
//!
//! * [`config`]: the setup form, the `--precisa*` flags, [`config::Config`].
//! * [`protocol`]: the scale's protocol.
//! * [`shot`]: finding the end of a shot in the readings.
//! * [`run`]: the task.

pub mod config;
pub mod protocol;
pub mod run;
pub mod shot;

use crate::integration::{Integration, Kind, Settings};

const TITLE: &str = "Eureka Precisa";

pub static KIND: Kind = Kind {
    id: "precisa",
    title: TITLE,
    summary: "Time and weight from a Eureka Precisa scale under the cup, over Bluetooth. \
              Needs Bluetooth on the computer running gbs-anywhere.",
    icon: include_str!("icon.svg"),
    fields: config::FIELDS,
    build,
};

fn build(settings: &Settings) -> anyhow::Result<Box<dyn Integration>> {
    Ok(Box::new(run::Precisa {
        cfg: config::Config::from_settings(settings)?,
    }))
}
