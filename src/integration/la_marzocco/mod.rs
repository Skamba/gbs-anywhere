//! La Marzocco cloud: reports shots from the machine's own coffee log.
//!
//! Connected La Marzocco machines upload every coffee to the La Marzocco
//! cloud with the extraction time and, with a Connected Scale, the weight in
//! the cup: the same numbers the machine shows after a shot. This integration
//! logs in like the La Marzocco Home app and, while the grinder is waiting
//! for a shot after a knob press, polls that list until a new coffee appears,
//! then reports it.
//!
//! This works on machines whose live brewing state never reaches the cloud
//! (the Linea Mini R, for one: Home Assistant's `brewing_active` sensor stays
//! off there) and reports the measured weight; the price is a few seconds of
//! delay after the shot and a login with the La Marzocco account.
//!
//! * [`config`]: the setup form, the `--lm-*` flags, [`config::Config`].
//! * [`cloud`]: the customer-app API client and its request signing.
//! * [`run`]: the task.

pub mod cloud;
pub mod config;
pub mod run;

use crate::integration::{Integration, Kind, Settings};

const TITLE: &str = "La Marzocco cloud";

pub static KIND: Kind = Kind {
    id: "la_marzocco",
    title: TITLE,
    summary: "Time and weight from a connected La Marzocco's coffee log \
              (weight needs the Connected Scale). Uses your La Marzocco Home account.",
    icon: include_str!("icon.svg"),
    fields: config::FIELDS,
    build,
};

fn build(settings: &Settings) -> anyhow::Result<Box<dyn Integration>> {
    Ok(Box::new(run::LaMarzocco {
        cfg: config::Config::from_settings(settings)?,
    }))
}
