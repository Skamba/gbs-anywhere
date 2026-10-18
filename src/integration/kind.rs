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

    /// Whether `a` and `b` fill in the form the same way: as the
    /// integration reads them, with a field's default for an empty one and
    /// numbers by value. A key that is not on the form (a test server)
    /// counts as a difference.
    pub fn same_settings(&self, a: &Settings, b: &Settings) -> bool {
        fn value(f: &Field, s: &Settings) -> Option<String> {
            let v = match f.input {
                Input::Password => s.secret(f.key),
                _ => s.text(f.key),
            }
            .or(Some(f.default).filter(|d| !d.is_empty()))?;
            // "3", "3.0" and "3,0" are the same number.
            let n = (f.input == Input::Number)
                .then(|| v.replace(',', ".").parse::<f64>().ok())
                .flatten();
            Some(n.map_or_else(|| v.to_owned(), |n| n.to_string()))
        }
        let on_form = |s: &Settings| s.0.keys().all(|k| self.field(k).is_some());
        on_form(a) && on_form(b) && self.fields.iter().all(|f| value(f, a) == value(f, b))
    }

    /// The form's values as the app may see them: everything but passwords,
    /// and which passwords have a value.
    pub fn shown(&self, settings: &Settings) -> (BTreeMap<String, String>, Vec<String>) {
        let mut shown = BTreeMap::new();
        let mut secrets_set = Vec::new();
        for f in self.fields {
            match f.input {
                Input::Password if settings.secret(f.key).is_some() => {
                    secrets_set.push(f.key.to_owned());
                }
                Input::Password => {}
                _ => {
                    if let Some(v) = settings.0.get(f.key) {
                        shown.insert(f.key.to_owned(), v.clone());
                    }
                }
            }
        }
        (shown, secrets_set)
    }

    /// `new` as the app sent it, with each password that is missing or empty
    /// there taking its value from `old`.
    pub fn keep_secrets(&self, mut new: Settings, old: &Settings) -> Settings {
        for f in self.fields.iter().filter(|f| f.input == Input::Password) {
            if new.secret(f.key).is_none()
                && let Some(v) = old.secret(f.key)
            {
                new.0.insert(f.key.to_owned(), v.to_owned());
            }
        }
        new
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
        let hidden = ok.with("base_url", Some("http://x"));
        assert_eq!(err(&hidden, true), "unknown setting `base_url`");
        assert!(KIND.create(&hidden, false).is_ok());
    }

    #[test]
    fn passwords_stay_in() {
        let saved = Settings::new()
            .with("user", Some("me"))
            .with("pass", Some("secret"))
            .with("every", Some("3"));
        let (shown, secrets_set) = KIND.shown(&saved);
        assert_eq!(shown.get("user").map(String::as_str), Some("me"));
        assert_eq!(shown.get("every").map(String::as_str), Some("3"));
        assert!(!shown.contains_key("pass"), "never sent out");
        assert_eq!(secrets_set, ["pass"]);
        assert!(KIND.shown(&Settings::new()).1.is_empty());

        // Left out or empty: the saved one. Typed: the new one.
        let edited = Settings::new().with("user", Some("you"));
        let merged = KIND.keep_secrets(edited.clone().with("pass", Some("")), &saved);
        assert_eq!(merged.secret("pass"), Some("secret"));
        assert_eq!(merged.text("user"), Some("you"));
        assert_eq!(
            KIND.keep_secrets(edited.clone(), &saved).secret("pass"),
            Some("secret")
        );
        let typed = KIND.keep_secrets(edited.with("pass", Some("new")), &saved);
        assert_eq!(typed.secret("pass"), Some("new"));

        // Compared as read: trimmed, and empty is the default.
        let flags = Settings::new()
            .with("user", Some(" me "))
            .with("every", Some("3"));
        let app = Settings::new().with("user", Some("me"));
        assert!(KIND.same_settings(&flags, &app));
        assert!(!KIND.same_settings(&flags, &app.clone().with("every", Some("4"))));
        // Numbers by value; a key that is not on the form differs.
        assert!(KIND.same_settings(&flags, &app.clone().with("every", Some("3,0"))));
        let test_server = flags.clone().with("base_url", Some("http://x"));
        assert!(!KIND.same_settings(&test_server, &app));
    }
}
