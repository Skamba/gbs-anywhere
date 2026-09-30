//! Eureka Precisa: reports shots weighed and timed by the scale under the cup.
//!
//! The Precisa (a Krell CFS-9002 sold by Eureka) sends its weight and timer
//! over Bluetooth Low Energy. This integration keeps it connected and, after
//! each knob press, tares it, starts its timer and watches the cup fill. The
//! shot is over when the timer is stopped or the weight stops rising; its
//! time and the weight in the cup go to the grinder at once.
//!
//! Needs a Bluetooth adapter on the host (BlueZ on Linux) within reach of the
//! scale, and the scale not held by another app.
//!
//! * [`config`]: the setup form, the `--precisa*` flags, [`config::Config`].
//! * [`precisa`]: the scale's protocol.
//! * [`shot`]: finding the end of a shot in the readings.
//! * [`run`]: the task.

pub mod config;
pub mod precisa;
pub mod run;
pub mod shot;

use crate::integration::{Integration, Kind, Settings};

const TITLE: &str = "Eureka Precisa";

pub static KIND: Kind = Kind {
    id: "eureka_precisa",
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