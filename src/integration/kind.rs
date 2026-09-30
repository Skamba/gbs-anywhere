//! What the app needs to know about each kind of integration before it runs: its name,
//! icon and the fields of its setup form, and how to build it from what was
//! filled in.

use std::collections::BTreeMap;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use super::Integration;

/// One kind of integration people can add: a vendor cloud, a scale, a hub.
/// Each integration's folder exports one as `KIND`, listed in [`super::KINDS`].
#[derive(Serialize)]
pub struct Kind {
    /// Stable id: the folder name, e.g. `la_marzocco`. Used in the API, the
    /// settings file and for `--icons` files.
    pub id: &'static str,
    /// Human name, e.g. `La Marzocco cloud`.
    pub title: &'static str,
    /// One line for the list behind the app's + button: what it reads and what it needs.
    pub summary: &'static str,
    /// Inline SVG glyph (24×24, `currentColor`), from the folder's `icon.svg`.
    pub icon: &'static str,
    /// The setup form, in order.
    pub fields: &'static [Field],
    /// Turns filled-in settings into the integration. [`Kind::create`] has
    /// already checked required fields and numbers; check the rest here.
    #[serde(skip)]
    pub build: fn(&Settings) -> anyhow::Result<Box<dyn Integration>>,
}

/// One field of an integration's setup form.
#[derive(Debug, Serialize)]
pub struct Field {
    /// Settings key, e.g. `username`.
    pub key: &'static str,
    pub label: &'static str,
    /// Shown under the field; empty for none.
    pub help: &'static str,
    pub input: Input,
    pub required: bool,
    /// Used when left empty, shown as the placeholder; empty for none.
    pub default: &'static str,
}

/// How a field is entered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Input {
    Text,
    Email,
    /// Never sent back by the API, not trimmed.
    Password,
    Number,
}

impl Kind {
    /// Checks `settings` against the form and builds the integration.
    /// `from_app` also refuses keys that are not form fields, so the app
    /// cannot set what only the command line may (such as a test server URL).
    pub fn create(
        &self,
        settings: &Settings,
        from_app: bool,
    ) -> anyhow::Result<Box<dyn Integration>> {
        if from_app
            && let Some(key) = settings
                .0
                .keys()
                .find(|k| !self.fields.iter().any(|f| f.key == k.as_str()))
        {
            bail!("unknown setting `{key}`");
        }
        for f in self.fields {
            let present = match f.input {
                Input::Password => settings.secret(f.key).is_some(),
                _ => settings.text(f.key).is_some(),
            };
            if f.required && !present {
                bail!("{} is required", f.label);
            }
            if f.input == Input::Number {
                settings.number(f.key)?;
            }
        }
        (self.build)(settings)
    }

    pub fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key == key)
    }

    /// The keys of its password fields: never sent back by the API.
    pub fn secret_keys(&self) -> impl Iterator<Item = &'static str> {
        self.fields
            .iter()
            .filter(|f| f.input == Input::Password)
            .map(|f| f.key)
    }
}

/// Filled-in form values by field key. Everything is a string, as typed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Settings(BTreeMap<String, String>);

impl Settings {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `key` unless `value` is `None`.
    pub fn with(mut self, key: &str, value: Option<impl ToString>) -> Self {
        if let Some(v) = value {
            self.0.insert(key.to_owned(), v.to_string());
        }
        self
    }

    /// Trimmed value; `None` when missing or blank.
    pub fn text(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(|v| v.trim()).filter(|v| !v.is_empty())
    }

    /// Value as typed (passwords may start or end with a space); `None` when
    /// missing or empty.
    pub fn secret(&self, key: &str) -> Option<&str> {
        self.0
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// A number, accepting a decimal comma; `None` when blank.
    pub fn number(&self, key: &str) -> anyhow::Result<Option<f64>> {
        let Some(v) = self.text(key) else {
            return Ok(None);
        };
        let n: f64 = v
            .replace(',', ".")
            .parse()
            .ok()
            .filter(|n: &f64| n.is_finite())
            .with_context(|| format!("`{key}` must be a number, got `{v}`"))?;
        Ok(Some(n))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// A copy without `keys`, e.g. without the passwords before settings go
    /// out to the app.
    pub fn without(&self, keys: &[&str]) -> Self {
        Self(
            self.0
                .iter()
                .filter(|(k, _)| !keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )
    }

    /// Takes `key` from `old` where it is missing or empty here: a password
    /// left empty when changing settings keeps the saved one.
    pub fn keep(mut self, key: &str, old: &Settings) -> Self {
        if self.secret(key).is_none()
            && let Some(v) = old.0.get(key)
        {
            self.0.insert(key.to_owned(), v.clone());
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::{BoxFuture, Link};

    struct Nothing;
    impl Integration for Nothing {
        fn run(self: Box<Self>, _link: Link) -> BoxFuture {
            Box::pin(std::future::pending())
        }
    }

    static KIND: Kind = Kind {
        id: "test",
        title: "Test",
        summary: "",
        icon: "",
        fields: &[
            Field {
                key: "user",
                label: "User",
                help: "",
                input: Input::Text,
                required: true,
                default: "",
            },
            Field {
                key: "pass",
                label: "Password",
                help: "",
                input: Input::Password,
                required: true,
                default: "",
            },
            Field {
                key: "every",
                label: "Every",
                help: "",
                input: Input::Number,
                required: false,
                default: "3",
            },
        ],
        build: |_| Ok(Box::new(Nothing)),
    };

    fn err(s: &Settings, from_app: bool) -> String {
        KIND.create(s, from_app).err().unwrap().to_string()
    }

    #[test]
    fn create_checks_the_form() {
        let ok = Settings::new()
            .with("user", Some(" me "))
            .with("pass", Some(" x "));
        assert!(KIND.create(&ok, true).is_ok());
        assert_eq!(ok.text("user"), Some("me"));
        assert_eq!(ok.secret("pass"), Some(" x "));

        assert_eq!(
            err(&Settings::new().with("pass", Some("x")), true),
            "User is required"
        );
        assert_eq!(
            err(&ok.clone().with("pass", Some("")), true),
            "Password is required"
        );
        assert_eq!(
            err(&ok.clone().with("every", Some("soon")), true),
            "`every` must be a number, got `soon`"
        );
        assert_eq!(
            ok.clone()
                .with("every", Some("2,5"))
                .number("every")
                .unwrap(),
            Some(2.5)
        );
        // Keys outside the form: command line only.
        let hidden = ok.clone().with("base_url", Some("http://x"));
        assert_eq!(err(&hidden, true), "unknown setting `base_url`");
        assert!(KIND.create(&hidden, false).is_ok());

        // Passwords stay out of what goes to the app, and an empty one keeps
        // the saved one when settings change.
        assert_eq!(KIND.secret_keys().collect::<Vec<_>>(), ["pass"]);
        let shown = ok.without(&["pass"]);
        assert_eq!((shown.text("user"), shown.secret("pass")), (Some("me"), None));
        let changed = Settings::new()
            .with("user", Some("you"))
            .with("pass", Some(""))
            .keep("pass", &ok);
        assert_eq!(changed.secret("pass"), Some(" x "));
        let typed = Settings::new().with("pass", Some("new")).keep("pass", &ok);
        assert_eq!(typed.secret("pass"), Some("new"));
    }
}
