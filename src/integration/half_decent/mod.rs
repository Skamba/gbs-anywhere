//! Half Decent Scale: reports shots weighed on Decent's open-source scale,
//! over WiFi.
//!
//! The scale (firmware 3.0 or newer, WiFi turned on) serves its readings on
//! a WebSocket at `ws://<scale>/snapshot`: `{"grams":25.66,"ms":12345}`,
//! where `ms` is the scale's own clock. After each knob press this
//! integration connects, asks for 10 readings a second and watches for the
//! shot: from the first drops in the cup to the flow stopping, with the
//! weight in the cup. Then it lets go, because a connected app keeps the
//! scale from switching itself off. It tares the scale at each knob press,
//! so the display counts the coffee from zero; nothing else on the scale is
//! changed. Up to four apps can read the scale at once, so the Decent app
//! keeps working.
//!
//! * [`config`]: the setup form, the `--hds-*` flags, [`config::Config`].
//! * [`shot`]: finds the shot in the readings.
//! * [`run`]: the task.

pub mod config;
pub mod run;
pub mod shot;

use crate::integration::{Integration, Kind, Settings};

const TITLE: &str = "Half Decent Scale";

pub static KIND: Kind = Kind {
    id: "half_decent",
    title: TITLE,
    summary: "Time from first drops to flow stop and the weight in the cup, \
              from a Half Decent Scale on your WiFi (firmware 3.0 or newer).",
    icon: include_str!("icon.svg"),
    fields: config::FIELDS,
    build,
};

fn build(settings: &Settings) -> anyhow::Result<Box<dyn Integration>> {
    Ok(Box::new(run::HalfDecent {
        cfg: config::Config::from_settings(settings)?,
    }))
}
