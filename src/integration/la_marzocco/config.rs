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
        label: "Seconds between checks",
        help: "How often to look for the new coffee while the grinder waits.",
        input: Input::Number,
        required: false,
        default: "3",
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
    /// API base URL; only tests point this elsewhere.
    pub base_url: String,
}

impl Config {
    pub fn from_settings(s: &Settings) -> anyhow::Result<Self> {
        let poll_s = s.number("poll_s")?.unwrap_or(DEFAULT_POLL_S);
        if poll_s < 1.0 {
            bail!("check at most once a second (poll_s at least 1)");
        }
        Ok(Self {
            username: s.text("username").context("E-mail is required")?.to_owned(),
            password: s
                .secret("password")
                .context("Password is required")?
                .to_owned(),
            serial: s.text("serial").map(str::to_owned),
            poll: Duration::from_secs_f64(poll_s),
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
                .with("base_url", Some(&self.lm_base_url)),
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
            lm_base_url: "http://127.0.0.1:9".into(),
        };
        let cfg = Config::from_settings(&args.settings().unwrap()).unwrap();
        assert_eq!(cfg.username, "me@example.org");
        assert_eq!(cfg.poll, Duration::from_secs(2));
        assert_eq!(cfg.base_url, "http://127.0.0.1:9");
        assert!(KIND.create(&args.settings().unwrap(), false).is_ok());

        let form = Settings::new()
            .with("username", Some("me@example.org"))
            .with("password", Some("hunter2"))
            .with("serial", Some(" "));
        let cfg = Config::from_settings(&form).unwrap();
        assert_eq!(cfg.serial, None);
        assert_eq!(cfg.poll, Duration::from_secs(3));
        assert_eq!(cfg.base_url, BASE_URL);

        let no_flags = Args {
            lm_username: None,
            ..args
        };
        assert!(no_flags.settings().is_none());
        let too_fast = form.with("poll_s", Some("0.5"));
        assert!(Config::from_settings(&too_fast).is_err());
    }
}
