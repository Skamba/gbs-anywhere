//! The setup form, the command-line flags, and the config built from them.

use std::time::Duration;

use anyhow::{Context, bail};

use super::cloud::BASE_URL;
use crate::integration::{Field, Input, Settings};

/// The form in the app. `base_url` is not on it: command line only.
pub const FIELDS: &[Field] = &[
    Field {
        key: "username",
        label: "E-mail",
        help: "Your La Marzocco Home account.",
        input: Input::Email,
        required: true,
        default: "",
    },
    Field {
        key: "password",
        label: "Password",
        help: "",
        input: Input::Password,
        required: true,
        default: "",
    },
    Field {
        key: "serial",
        label: "Machine serial number",
        help: "Only needed with more than one machine on the account, e.g. MI000000.",
        input: Input::Text,
        required: false,
        default: "",
    },
    Field {
        key: "poll_s",
        label: "Check interval (seconds)",
        help: "After the grind, how often gbs-anywhere asks the La Marzocco cloud whether \
               the shot has been logged. Lower reports sooner but sends more requests. \
               Default: 3 (1 to 60).",
        input: Input::Number,
        required: false,
        default: "3",
    },
    Field {
        key: "recipe_follow",
        label: "Follow the grinder recipe (experimental)",
        help: "Off by default. Selects a matching Brew-by-Weight preset, or saves the recipe target \
               to Dose 1 and selects it when neither preset matches. Dose 2 is kept. \
               Requires a paired E65/E80 GbS, its Owner password and a connected BBW scale. \
               Wait for target confirmation on Start, check the machine, then start the shot yourself.",
        input: Input::Checkbox,
        required: false,
        default: "false",
    },
    Field {
        key: "grinder_owner_password",
        label: "Grinder Owner password",
        help: "Required only for recipe following: the Owner password from the grinder, \
               not your La Marzocco password. Used to read the grinder's recipe export.",
        input: Input::Password,
        required: false,
        default: "",
    },
];

const DEFAULT_POLL_S: f64 = 3.0;

/// Account and machine to watch.
#[derive(Debug, Clone)]
pub struct Config {
    /// La Marzocco Home account.
    pub username: String,
    pub password: String,
    /// Machine serial (e.g. `MI000000`). `None`: the account's only machine.
    pub serial: Option<String>,
    /// How often to ask for new coffees while the grinder is waiting.
    pub poll: Duration,
    /// Explicit opt-in to selecting a matching BBW preset or saving the target to Dose 1.
    pub recipe_follow: bool,
    /// Grinder Owner password for the recipe export, never returned by the API.
    pub grinder_owner_password: Option<String>,
    /// API base URL; only tests point this elsewhere.
    pub base_url: String,
}

impl Config {
    pub fn from_settings(s: &Settings) -> anyhow::Result<Self> {
        let poll_s = s.number("poll_s")?.unwrap_or(DEFAULT_POLL_S);
        if !(1.0..=60.0).contains(&poll_s) {
            bail!("check between once a second and once a minute (poll_s from 1 to 60)");
        }
        let recipe_follow = s.boolean("recipe_follow")?.unwrap_or(false);
        let grinder_owner_password = s.secret("grinder_owner_password").map(str::to_owned);
        if recipe_follow && grinder_owner_password.is_none() {
            bail!("Grinder Owner password is required when recipe following is enabled");
        }
        Ok(Self {
            username: s.text("username").context("E-mail is required")?.to_owned(),
            password: s
                .secret("password")
                .context("Password is required")?
                .to_owned(),
            serial: s.text("serial").map(str::to_owned),
            poll: Duration::from_secs_f64(poll_s),
            recipe_follow,
            grinder_owner_password,
            base_url: s.text("base_url").unwrap_or(BASE_URL).to_owned(),
        })
    }
}

/// Command-line flags. `settings` turns them into the form's settings when
/// `--lm-username` is given.
#[derive(Debug, Clone, clap::Args)]
#[group(id = "la_marzocco")]
#[command(next_help_heading = "La Marzocco cloud")]
pub struct Args {
    /// La Marzocco Home account e-mail. Enables reporting shots from the
    /// machine's own coffee log in the La Marzocco cloud.
    #[arg(long, env = "LM_USERNAME")]
    pub lm_username: Option<String>,

    /// La Marzocco Home account password.
    #[arg(long, env = "LM_PASSWORD", hide_env_values = true)]
    pub lm_password: Option<String>,

    /// Machine serial number, e.g. MI000000. Needed with more than one
    /// machine on the account.
    #[arg(long, env = "LM_SERIAL")]
    pub lm_serial: Option<String>,

    /// Seconds between checks of the coffee log while the grinder waits.
    #[arg(long, env = "LM_POLL_S", default_value_t = DEFAULT_POLL_S)]
    pub lm_poll_s: f64,

    /// Follow the recipe's BBW target (experimental). Selects a matching preset,
    /// or saves the target to Dose 1 and selects it; keeps Dose 2.
    /// Requires a paired E65/E80 GbS and its Owner password. Never starts brewing.
    #[arg(long, env = "LM_RECIPE_FOLLOW", default_value_t = false)]
    pub lm_recipe_follow: bool,

    /// Grinder Owner password to read recipes when recipe following is enabled.
    #[arg(long, env = "LM_GRINDER_OWNER_PASSWORD", hide_env_values = true)]
    pub lm_grinder_owner_password: Option<String>,

    /// API base URL (for tests against a fake server).
    #[arg(long, env = "LM_BASE_URL", default_value = BASE_URL, hide = true)]
    pub lm_base_url: String,
}

impl Args {
    /// The settings, or `None` when `--lm-username` is not given.
    pub fn settings(&self) -> Option<Settings> {
        self.lm_username.as_ref()?;
        Some(
            Settings::new()
                .with("username", self.lm_username.as_ref())
                .with("password", self.lm_password.as_ref())
                .with("serial", self.lm_serial.as_ref())
                .with("poll_s", Some(self.lm_poll_s))
                .with("recipe_follow", Some(self.lm_recipe_follow))
                .with(
                    "grinder_owner_password",
                    self.lm_grinder_owner_password.as_ref(),
                )
                // Only a test server: it keeps the integration out of the app.
                .with(
                    "base_url",
                    (self.lm_base_url != BASE_URL).then_some(&self.lm_base_url),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::la_marzocco::KIND;

    #[test]
    fn flags_and_form_give_the_same_config() {
        let args = Args {
            lm_username: Some(" me@example.org ".into()),
            lm_password: Some("hunter2".into()),
            lm_serial: None,
            lm_poll_s: 2.0,
            lm_recipe_follow: true,
            lm_grinder_owner_password: Some("owner-secret".into()),
            lm_base_url: "http://127.0.0.1:9".into(),
        };
        let cfg = Config::from_settings(&args.settings().unwrap()).unwrap();
        assert_eq!(cfg.username, "me@example.org");
        assert_eq!(cfg.poll, Duration::from_secs(2));
        assert_eq!(cfg.base_url, "http://127.0.0.1:9");
        assert!(cfg.recipe_follow);
        assert_eq!(cfg.grinder_owner_password.as_deref(), Some("owner-secret"));
        assert!(KIND.create(&args.settings().unwrap(), false).is_ok());
        // Only without a test server can it move into the app.
        assert!(KIND.create(&args.settings().unwrap(), true).is_err());
        let real = Args {
            lm_base_url: BASE_URL.into(),
            ..args.clone()
        };
        assert!(KIND.create(&real.settings().unwrap(), true).is_ok());

        let form = Settings::new()
            .with("username", Some("me@example.org"))
            .with("password", Some("hunter2"))
            .with("serial", Some(" "));
        let cfg = Config::from_settings(&form).unwrap();
        assert_eq!(cfg.serial, None);
        assert_eq!(cfg.poll, Duration::from_secs(3));
        assert_eq!(cfg.base_url, BASE_URL);
        assert!(!cfg.recipe_follow);
        assert!(cfg.grinder_owner_password.is_none());

        let no_flags = Args {
            lm_username: None,
            ..args
        };
        assert!(no_flags.settings().is_none());
        for bad in ["0.5", "61", "1e20", "NaN"] {
            let form = form.clone().with("poll_s", Some(bad));
            assert!(Config::from_settings(&form).is_err(), "{bad}");
        }
    }

    #[test]
    fn recipe_follow_is_opt_in_and_keeps_the_owner_password_private() {
        let old: Settings =
            serde_json::from_str(r#"{"username":"me@example.org","password":"cloud-secret"}"#)
                .unwrap();
        assert!(!Config::from_settings(&old).unwrap().recipe_follow);
        let enabled = old.clone().with("recipe_follow", Some(true));
        assert!(KIND.create(&enabled, true).is_err());
        let enabled = enabled.with("grinder_owner_password", Some(" owner-secret "));
        let cfg = Config::from_settings(&enabled).unwrap();
        assert!(cfg.recipe_follow);
        assert_eq!(
            cfg.grinder_owner_password.as_deref(),
            Some(" owner-secret ")
        );
        assert!(KIND.create(&enabled, true).is_ok());

        let (shown, secrets_set) = KIND.shown(&enabled);
        assert!(!shown.contains_key("grinder_owner_password"));
        assert!(!shown.contains_key("password"));
        assert!(
            secrets_set
                .iter()
                .any(|key| key == "grinder_owner_password")
        );
        let edited = KIND.keep_secrets(old.clone().with("recipe_follow", Some(false)), &enabled);
        let cfg = Config::from_settings(&edited).unwrap();
        assert!(!cfg.recipe_follow);
        assert_eq!(
            cfg.grinder_owner_password.as_deref(),
            Some(" owner-secret ")
        );
        assert!(Config::from_settings(&old.with("recipe_follow", Some("yes"))).is_err());
    }
}
