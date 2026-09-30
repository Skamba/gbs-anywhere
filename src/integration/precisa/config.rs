//! The setup form, the command-line flags, and the config built from them.

use std::time::Duration;

use anyhow::bail;

use super::precisa::DEFAULT_NAME_PREFIX;
use super::shot::EndRule;
use crate::integration::{Field, Input, Settings};

/// The form in the app. `timer` is not on it: command line only.
pub const FIELDS: &[Field] = &[
    Field {
        key: "address",
        label: "Bluetooth address",
        help: "Only needed with more than one scale in range, e.g. AA:BB:CC:DD:EE:FF.",
        input: Input::Text,
        required: false,
        default: "",
    },
    Field {
        key: "name",
        label: "Bluetooth name",
        help: "How the scale starts its name when it advertises.",
        input: Input::Text,
        required: false,
        default: DEFAULT_NAME_PREFIX,
    },
    Field {
        key: "stable_s",
        label: "Seconds without a rise",
        help: "The shot is over once the weight stops rising for this long.",
        input: Input::Number,
        required: false,
        default: "3",
    },
    Field {
        key: "min_g",
        label: "Minimum grams",
        help: "A stable weight only ends the shot with at least this much in the cup.",
        input: Input::Number,
        required: false,
        default: "5",
    },
    Field {
        key: "live_ms",
        label: "Live display refresh (ms)",
        help: "How often the app updates weight and time during a shot: lower is smoother, \
               higher means less traffic. 100 to 2000.",
        input: Input::Number,
        required: false,
        default: "250",
    },
];

const DEFAULT_STABLE_S: f64 = 3.0;
const DEFAULT_MIN_G: f64 = 5.0;
const DEFAULT_LIVE_MS: f64 = 250.0;
const DEFAULT_SCAN: Duration = Duration::from_secs(10);

/// The scale to use and when a shot counts as over.
#[derive(Debug, Clone)]
pub struct Config {
    /// Bluetooth address. `None`: the first scale whose name starts with
    /// `name_prefix`.
    pub address: Option<String>,
    pub name_prefix: String,
    /// Tare, reset and start the scale's timer on the knob press and stop it
    /// at the end. Off: only tare; a timer someone started by hand still ends
    /// the shot when stopped.
    pub drive_timer: bool,
    /// No rise in weight for this long ends a shot ...
    pub stable_for: Duration,
    /// ... with at least this many grams in the cup.
    pub min_weight_g: f64,
    /// How often the app refreshes the live display during a shot, and how
    /// often a running shot is checked here.
    pub live_every: Duration,
    /// How long one search for the scale lasts.
    pub scan_for: Duration,
}

impl Config {
    pub fn from_settings(s: &Settings) -> anyhow::Result<Self> {
        let stable_s = s.number("stable_s")?.unwrap_or(DEFAULT_STABLE_S);
        if !(1.0..=15.0).contains(&stable_s) {
            bail!("seconds without a rise must be between 1 and 15");
        }
        let min_g = s.number("min_g")?.unwrap_or(DEFAULT_MIN_G);
        if min_g <= 0.0 {
            bail!("minimum grams must be above 0");
        }
        let live_ms = s.number("live_ms")?.unwrap_or(DEFAULT_LIVE_MS);
        if !(100.0..=2000.0).contains(&live_ms) {
            bail!("live display refresh must be between 100 and 2000 ms");
        }
        Ok(Self {
            address: s.text("address").map(str::to_owned),
            name_prefix: s.text("name").unwrap_or(DEFAULT_NAME_PREFIX).to_owned(),
            drive_timer: s.text("timer") != Some("off"),
            stable_for: Duration::from_secs_f64(stable_s),
            min_weight_g: min_g,
            live_every: Duration::from_secs_f64(live_ms / 1000.0),
            scan_for: DEFAULT_SCAN,
        })
    }

    pub fn end_rule(&self) -> EndRule {
        EndRule {
            stable_for: self.stable_for,
            min_weight_g: self.min_weight_g,
            ..EndRule::default()
        }
    }
}

/// Command-line flags. `settings` turns them into the form's settings when
/// `--precisa` is given.
#[derive(Debug, Clone, clap::Args)]
#[group(id = "eureka_precisa")]
#[command(next_help_heading = "Eureka Precisa scale (Bluetooth)")]
pub struct Args {
    /// Report shots weighed and timed by a Eureka Precisa scale over
    /// Bluetooth.
    #[arg(long, env = "PRECISA")]
    pub precisa: bool,

    /// Bluetooth address of the scale. Needed with more than one in range.
    #[arg(long, env = "PRECISA_ADDRESS")]
    pub precisa_address: Option<String>,

    /// How the scale's Bluetooth name starts.
    #[arg(long, env = "PRECISA_NAME", default_value = DEFAULT_NAME_PREFIX)]
    pub precisa_name: String,

    /// Leave the scale's timer alone; only tare on the knob press.
    #[arg(long, env = "PRECISA_NO_TIMER")]
    pub precisa_no_timer: bool,

    /// Seconds without a rise in weight that end a shot.
    #[arg(long, env = "PRECISA_STABLE_S", default_value_t = DEFAULT_STABLE_S)]
    pub precisa_stable_s: f64,

    /// Grams in the cup before a stable weight ends a shot.
    #[arg(long, env = "PRECISA_MIN_G", default_value_t = DEFAULT_MIN_G)]
    pub precisa_min_g: f64,

    /// Milliseconds between live display updates in the app during a shot
    /// (100 to 2000).
    #[arg(long, env = "PRECISA_LIVE_MS", default_value_t = DEFAULT_LIVE_MS)]
    pub precisa_live_ms: f64,
}

impl Args {
    /// The settings, or `None` when `--precisa` is not given.
    pub fn settings(&self) -> Option<Settings> {
        if !self.precisa {
            return None;
        }
        Some(
            Settings::new()
                .with("address", self.precisa_address.as_ref())
                .with("name", Some(&self.precisa_name))
                .with("timer", self.precisa_no_timer.then_some("off"))
                .with("stable_s", Some(self.precisa_stable_s))
                .with("min_g", Some(self.precisa_min_g))
                .with("live_ms", Some(self.precisa_live_ms)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::eureka_precisa::KIND;

    #[test]
    fn flags_and_form_give_the_same_config() {
        let args = Args {
            precisa: true,
            precisa_address: Some(" AA:BB:CC:DD:EE:FF ".into()),
            precisa_name: DEFAULT_NAME_PREFIX.into(),
            precisa_no_timer: true,
            precisa_stable_s: 4.0,
            precisa_min_g: 8.0,
            precisa_live_ms: 500.0,
        };
        let cfg = Config::from_settings(&args.settings().unwrap()).unwrap();
        assert_eq!(cfg.address.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
        assert!(!cfg.drive_timer);
        assert_eq!(cfg.stable_for, Duration::from_secs(4));
        assert_eq!(cfg.min_weight_g, 8.0);
        assert_eq!(cfg.live_every, Duration::from_millis(500));
        assert!(KIND.create(&args.settings().unwrap(), false).is_ok());

        // The form: everything optional.
        let form = Settings::new().with("address", Some(" "));
        let cfg = Config::from_settings(&form).unwrap();
        assert_eq!(cfg.address, None);
        assert_eq!(cfg.name_prefix, DEFAULT_NAME_PREFIX);
        assert!(cfg.drive_timer);
        assert_eq!(cfg.stable_for, Duration::from_secs(3));
        assert_eq!(cfg.live_every, Duration::from_millis(250));

        let off = Args {
            precisa: false,
            ..args
        };
        assert!(off.settings().is_none());
        let too_quick = Settings::new().with("stable_s", Some("0.5"));
        assert!(Config::from_settings(&too_quick).is_err());
        let no_grams = Settings::new().with("min_g", Some("0"));
        assert!(Config::from_settings(&no_grams).is_err());
        let too_often = Settings::new().with("live_ms", Some("50"));
        assert!(Config::from_settings(&too_often).is_err());
    }
}
