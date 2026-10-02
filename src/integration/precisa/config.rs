//! The setup form, the command-line flags, and the config built from them.

use std::time::Duration;

use anyhow::bail;

use super::protocol::DEFAULT_NAME_PREFIX;
use super::shot::EndRule;
use crate::integration::{Field, Input, Settings};

/// The form in the app. `timer`, `beep` and `target` are not on it: command
/// line only.
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
        key: "min_time_s",
        label: "Minimum seconds",
        help: "A shot is not over before this: a pause in the flow earlier does not end \
               it, and stopping the scale's timer earlier aborts it. 0 to 200.",
        input: Input::Number,
        required: false,
        default: "20",
    },
    Field {
        key: "start_delay_ms",
        label: "Milliseconds to start the machine",
        help: "After the knob press (or a test start) the scale waits this long before it \
               tares and times the shot, so there is time to start the machine. 0 to 10000.",
        input: Input::Number,
        required: false,
        default: "2000",
    },
    Field {
        key: "reconnect_ms",
        label: "Reconnect pause (ms)",
        help: "Pause between searches while the scale is off or out of reach: lower \
               reconnects sooner after switching it on. 500 to 3000.",
        input: Input::Number,
        required: false,
        default: "500",
    },
    Field {
        key: "live_ms",
        label: "Live display refresh (ms)",
        help: "How often the app updates weight and time during a shot: lower is smoother, \
               higher means less traffic. 50 to 2000.",
        input: Input::Number,
        required: false,
        default: "100",
    },
];

const DEFAULT_STABLE_S: f64 = 3.0;
const DEFAULT_MIN_G: f64 = 5.0;
const DEFAULT_MIN_TIME_S: f64 = 20.0;
const DEFAULT_LIVE_MS: f64 = 100.0;
const DEFAULT_START_DELAY_MS: f64 = 2000.0;
const DEFAULT_RECONNECT_MS: f64 = 500.0;
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
    /// Beep twice when a shot is reported, four times when one ends without
    /// a result.
    pub beep: bool,
    /// End the shot when the cup reaches the grinder's recipe weight, timed
    /// to that moment, like a Xenia. Off: the whole shot until the flow
    /// stops.
    pub stop_at_target: bool,
    /// No rise in weight for this long ends a shot ...
    pub stable_for: Duration,
    /// ... with at least this many grams in the cup.
    pub min_weight_g: f64,
    /// A shot is not over before this; a timer stopped earlier aborts it.
    pub min_time: Duration,
    /// How often the app refreshes the live display during a shot, and how
    /// often a running shot is checked here.
    pub live_every: Duration,
    /// Wait after the brew start before taring and timing, to start the
    /// machine by hand. The shot is timed from its end.
    pub start_delay: Duration,
    /// Pause between searches for the scale while it is off or away.
    pub reconnect_every: Duration,
    /// How long one search for the scale lasts.
    pub scan_for: Duration,
}

impl Config {
    pub fn from_settings(s: &Settings) -> anyhow::Result<Self> {
        let stable_s = s.number("stable_s")?.unwrap_or(DEFAULT_STABLE_S);
        if !(1.0..=15.0).contains(&stable_s) {
            bail!("seconds without a rise must be between 1 and 15");
        }
        let min_time_s = s.number("min_time_s")?.unwrap_or(DEFAULT_MIN_TIME_S);
        if !(0.0..=200.0).contains(&min_time_s) {
            bail!("minimum seconds must be between 0 and 200");
        }
        let min_g = s.number("min_g")?.unwrap_or(DEFAULT_MIN_G);
        if min_g <= 0.0 {
            bail!("minimum grams must be above 0");
        }
        let start_delay_ms = s
            .number("start_delay_ms")?
            .unwrap_or(DEFAULT_START_DELAY_MS);
        if !(0.0..=10_000.0).contains(&start_delay_ms) {
            bail!("milliseconds to start the machine must be between 0 and 10000");
        }
        let reconnect_ms = s.number("reconnect_ms")?.unwrap_or(DEFAULT_RECONNECT_MS);
        if !(500.0..=3000.0).contains(&reconnect_ms) {
            bail!("reconnect pause must be between 500 and 3000 ms");
        }
        let live_ms = s.number("live_ms")?.unwrap_or(DEFAULT_LIVE_MS);
        if !(50.0..=2000.0).contains(&live_ms) {
            bail!("live display refresh must be between 50 and 2000 ms");
        }
        Ok(Self {
            address: s.text("address").map(str::to_owned),
            name_prefix: s.text("name").unwrap_or(DEFAULT_NAME_PREFIX).to_owned(),
            drive_timer: s.text("timer") != Some("off"),
            beep: s.text("beep") != Some("off"),
            stop_at_target: s.text("target") != Some("off"),
            stable_for: Duration::from_secs_f64(stable_s),
            min_weight_g: min_g,
            min_time: Duration::from_secs_f64(min_time_s),
            live_every: Duration::from_secs_f64(live_ms / 1000.0),
            start_delay: Duration::from_secs_f64(start_delay_ms / 1000.0),
            reconnect_every: Duration::from_secs_f64(reconnect_ms / 1000.0),
            scan_for: DEFAULT_SCAN,
        })
    }

    pub fn end_rule(&self) -> EndRule {
        EndRule {
            stable_for: self.stable_for,
            min_weight_g: self.min_weight_g,
            min_time: self.min_time,
            target_g: None,
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

    /// No beeps (twice when a shot is reported, four times when one is
    /// aborted).
    #[arg(long, env = "PRECISA_NO_BEEP")]
    pub precisa_no_beep: bool,

    /// Report the whole shot until the flow stops, instead of the time until
    /// the cup reaches the grinder's recipe weight.
    #[arg(long, env = "PRECISA_FULL_SHOT")]
    pub precisa_full_shot: bool,

    /// Seconds without a rise in weight that end a shot.
    #[arg(long, env = "PRECISA_STABLE_S", default_value_t = DEFAULT_STABLE_S)]
    pub precisa_stable_s: f64,

    /// Grams in the cup before a stable weight ends a shot.
    #[arg(long, env = "PRECISA_MIN_G", default_value_t = DEFAULT_MIN_G)]
    pub precisa_min_g: f64,

    /// Seconds before which a shot is not over; stopping the scale's timer
    /// earlier aborts it (0 to 200).
    #[arg(long, env = "PRECISA_MIN_TIME_S", default_value_t = DEFAULT_MIN_TIME_S)]
    pub precisa_min_time_s: f64,

    /// Milliseconds between live display updates in the app during a shot
    /// (50 to 2000).
    #[arg(long, env = "PRECISA_LIVE_MS", default_value_t = DEFAULT_LIVE_MS)]
    pub precisa_live_ms: f64,

    /// Milliseconds after the knob press (or a test start) before the scale
    /// tares and times the shot, to start the machine by hand (0 to 10000).
    #[arg(long, env = "PRECISA_START_DELAY_MS", default_value_t = DEFAULT_START_DELAY_MS)]
    pub precisa_start_delay_ms: f64,

    /// Milliseconds between searches while the scale is off or out of reach
    /// (500 to 3000).
    #[arg(long, env = "PRECISA_RECONNECT_MS", default_value_t = DEFAULT_RECONNECT_MS)]
    pub precisa_reconnect_ms: f64,
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
                .with("beep", self.precisa_no_beep.then_some("off"))
                .with("target", self.precisa_full_shot.then_some("off"))
                .with("stable_s", Some(self.precisa_stable_s))
                .with("min_g", Some(self.precisa_min_g))
                .with("min_time_s", Some(self.precisa_min_time_s))
                .with("live_ms", Some(self.precisa_live_ms))
                .with("start_delay_ms", Some(self.precisa_start_delay_ms))
                .with("reconnect_ms", Some(self.precisa_reconnect_ms)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::precisa::KIND;

    #[test]
    fn flags_and_form_give_the_same_config() {
        let args = Args {
            precisa: true,
            precisa_address: Some(" AA:BB:CC:DD:EE:FF ".into()),
            precisa_name: DEFAULT_NAME_PREFIX.into(),
            precisa_no_timer: true,
            precisa_no_beep: true,
            precisa_full_shot: true,
            precisa_stable_s: 4.0,
            precisa_min_g: 8.0,
            precisa_min_time_s: 0.0,
            precisa_live_ms: 500.0,
            precisa_start_delay_ms: 1500.0,
            precisa_reconnect_ms: 1000.0,
        };
        let cfg = Config::from_settings(&args.settings().unwrap()).unwrap();
        assert_eq!(cfg.address.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
        assert!(!cfg.drive_timer);
        assert!(!cfg.beep);
        assert!(!cfg.stop_at_target);
        assert_eq!(cfg.stable_for, Duration::from_secs(4));
        assert_eq!(cfg.min_weight_g, 8.0);
        assert_eq!(cfg.min_time, Duration::ZERO);
        assert_eq!(cfg.live_every, Duration::from_millis(500));
        assert_eq!(cfg.start_delay, Duration::from_millis(1500));
        assert_eq!(cfg.reconnect_every, Duration::from_secs(1));
        assert!(KIND.create(&args.settings().unwrap(), false).is_ok());

        // The form: everything optional.
        let form = Settings::new().with("address", Some(" "));
        let cfg = Config::from_settings(&form).unwrap();
        assert_eq!(cfg.address, None);
        assert_eq!(cfg.name_prefix, DEFAULT_NAME_PREFIX);
        assert!(cfg.drive_timer);
        assert!(cfg.beep);
        assert!(cfg.stop_at_target);
        assert_eq!(cfg.stable_for, Duration::from_secs(3));
        assert_eq!(cfg.live_every, Duration::from_millis(100));
        assert_eq!(cfg.start_delay, Duration::from_secs(2));
        assert_eq!(cfg.min_time, Duration::from_secs(20));
        assert_eq!(cfg.reconnect_every, Duration::from_millis(500));

        let off = Args {
            precisa: false,
            ..args
        };
        assert!(off.settings().is_none());
        let too_quick = Settings::new().with("stable_s", Some("0.5"));
        assert!(Config::from_settings(&too_quick).is_err());
        let no_grams = Settings::new().with("min_g", Some("0"));
        assert!(Config::from_settings(&no_grams).is_err());
        let too_often = Settings::new().with("live_ms", Some("40"));
        assert!(Config::from_settings(&too_often).is_err());
        for bad in ["400", "3500"] {
            let reconnect = Settings::new().with("reconnect_ms", Some(bad));
            assert!(Config::from_settings(&reconnect).is_err());
        }
        let too_long = Settings::new().with("min_time_s", Some("250"));
        assert!(Config::from_settings(&too_long).is_err());
        let too_late = Settings::new().with("start_delay_ms", Some("10001"));
        assert!(Config::from_settings(&too_late).is_err());
    }
}
