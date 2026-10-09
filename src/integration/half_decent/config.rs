//! The setup form, the command-line flags, and the config built from them.

use anyhow::bail;

use crate::integration::{Field, Input, Settings};

/// The scale's own name on the network.
pub const DEFAULT_HOST: &str = "hds.local";

pub const FIELDS: &[Field] = &[Field {
    key: "host",
    label: "Scale address",
    help: "The scale's IP address, e.g. 192.168.1.30 (hds.local works outside Docker). \
           Turn on WiFi in the scale's Setup > Connections first.",
    input: Input::Text,
    required: false,
    default: DEFAULT_HOST,
}];

/// Which scale to read.
#[derive(Debug, Clone)]
pub struct Config {
    /// `host` or `host:port`.
    pub host: String,
}

impl Config {
    pub fn from_settings(s: &Settings) -> anyhow::Result<Self> {
        Ok(Self {
            host: host(s.text("host").unwrap_or(DEFAULT_HOST))?,
        })
    }
}

/// `host` or `host:port`. A pasted `http://…/` or `ws://…/snapshot` is
/// trimmed back to that.
fn host(input: &str) -> anyhow::Result<String> {
    let h = input.trim();
    let h = ["ws://", "http://"]
        .iter()
        .find_map(|p| h.strip_prefix(p))
        .unwrap_or(h);
    let h = h
        .strip_suffix("/snapshot")
        .unwrap_or(h)
        .trim_end_matches('/');
    let bad_port = h
        .split_once(':')
        .is_some_and(|(_, port)| port.parse::<u16>().is_err());
    if h.is_empty() || bad_port || h.contains(|c: char| c.is_whitespace() || "/?#@".contains(c)) {
        bail!(
            "enter the scale's address as an IP address or name, e.g. 192.168.1.30, not `{input}`"
        );
    }
    Ok(h.to_owned())
}

/// Command-line flags. `settings` turns them into the form's settings when
/// `--hds-host` is given.
#[derive(Debug, Clone, clap::Args)]
#[group(id = "half_decent")]
#[command(next_help_heading = "Half Decent Scale")]
pub struct Args {
    /// The scale's address: its IP address, or hds.local outside Docker.
    /// Enables reporting shots weighed on a Half Decent Scale over WiFi.
    #[arg(long, env = "HDS_HOST")]
    pub hds_host: Option<String>,
}

impl Args {
    /// The settings, or `None` when `--hds-host` is not given.
    pub fn settings(&self) -> Option<Settings> {
        Some(Settings::new().with("host", Some(self.hds_host.as_ref()?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_of(input: &str) -> anyhow::Result<String> {
        Config::from_settings(&Settings::new().with("host", Some(input))).map(|c| c.host)
    }

    #[test]
    fn host_is_normalised() {
        assert_eq!(host_of("192.168.1.30").unwrap(), "192.168.1.30");
        assert_eq!(host_of(" hds.local ").unwrap(), "hds.local");
        assert_eq!(host_of("http://192.168.1.30/").unwrap(), "192.168.1.30");
        assert_eq!(host_of("ws://hds.local/snapshot").unwrap(), "hds.local");
        assert_eq!(host_of("127.0.0.1:8123").unwrap(), "127.0.0.1:8123");
        for bad in [
            "hds local",
            "192.168.1.30/x",
            "user@hds.local",
            "https://",
            "/",
        ] {
            let e = host_of(bad).unwrap_err().to_string();
            assert!(e.contains("address"), "{bad}: {e}");
        }
        // Left empty: the default.
        let cfg = Config::from_settings(&Settings::new()).unwrap();
        assert_eq!(cfg.host, DEFAULT_HOST);
    }

    #[test]
    fn flags_and_form_give_the_same_config() {
        let args = Args {
            hds_host: Some("192.168.1.30".into()),
        };
        let from_flags = Config::from_settings(&args.settings().unwrap()).unwrap();
        let form = Settings::new().with("host", Some("192.168.1.30"));
        assert_eq!(from_flags.host, Config::from_settings(&form).unwrap().host);
        assert!(Args { hds_host: None }.settings().is_none());
        // Every settings key is on the form, so the app may set all of them.
        let s = args.settings().unwrap();
        assert!(
            crate::integration::half_decent::config::FIELDS
                .iter()
                .any(|f| f.key == "host")
        );
        assert!(s.text("host").is_some());
    }
}
