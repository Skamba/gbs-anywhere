//! The app page (`GET /`): `web/index.html` with the styles, the script,
//! the texts (`web/strings/<lang>.json`) and the version filled in, so it
//! comes in one response with nothing more to load. The tests here check the
//! page and its texts against what the server sends.

use std::sync::LazyLock;

use serde_json::json;

// The app page as a template: `index.html` names where the styles and the
// script go, and `app.js` where the texts and the version go. `page()` fills
// them in, so the page comes in one response with everything inline.
const PAGE_HTML: &str = include_str!("../../web/index.html");
const PAGE_CSS: &str = include_str!("../../web/app.css");
const PAGE_JS: &str = include_str!("../../web/app.js");
const CSS_SLOT: &str = "/* web/app.css */";
const JS_SLOT: &str = "/* web/app.js */";
const STRINGS_SLOT: &str = "/* web/strings/<lang>.json */";
/// `PAGE_VERSION` in app.js: the version the page comes with, so a page from
/// the browser's cache can tell that another version answers.
const VERSION_SLOT: &str = "/* version */";

/// The app's languages and their texts; the page takes its list from these.
/// What else a new language needs is under "Web app" in AGENTS.md.
const LANGS: &[(&str, &str)] = &[
    ("en", include_str!("../../web/strings/en.json")),
    ("de", include_str!("../../web/strings/de.json")),
];

/// The page with its slots filled, built once.
pub(super) fn page() -> &'static str {
    static PAGE: LazyLock<String> = LazyLock::new(|| {
        // `STRINGS` in app.js: {"en": {…}, "de": {…}}.
        let strings = LANGS
            .iter()
            .map(|(lang, json)| format!("{}: {}", json!(lang), script_safe(json.trim())))
            .collect::<Vec<_>>()
            .join(",\n");
        let version = script_safe(&json!(crate::version()).to_string());
        let script = fill(
            PAGE_JS.trim_end(),
            &[
                (STRINGS_SLOT, strings.as_str()),
                (VERSION_SLOT, version.as_str()),
            ],
        );
        let styles = PAGE_CSS.trim_end();
        fill(PAGE_HTML, &[(CSS_SLOT, styles), (JS_SLOT, script.as_str())])
    });
    &PAGE
}

/// `template` with each slot replaced by its text, in order; what is filled
/// in is not searched again.
fn fill(template: &str, slots: &[(&str, &str)]) -> String {
    let mut out = String::new();
    let mut rest = template;
    for (slot, text) in slots {
        let (before, after) = rest
            .split_once(slot)
            .unwrap_or_else(|| panic!("the app page has no {slot}"));
        out.push_str(before);
        out.push_str(text);
        rest = after;
    }
    out.push_str(rest);
    out
}

/// JSON to put inside a `<script>` element: `</` as `<\/` and `<!--` as
/// `<\u0021--`, so no text can end the element early. Still the same JSON.
fn script_safe(json: &str) -> String {
    json.replace("</", "<\\/").replace("<!--", "<\\u0021--")
}

#[cfg(test)]
mod tests {
    //! The app page and its texts (`web/strings/<lang>.json`), checked
    //! without a JavaScript engine: every English text is in every language,
    //! with the same placeholders, every key the page uses exists, and the
    //! page comes with its styles, script, texts and version filled in.

    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
    use serde_json::Value;

    use super::{
        CSS_SLOT, JS_SLOT, LANGS, PAGE_CSS, PAGE_HTML, PAGE_JS, STRINGS_SLOT, VERSION_SLOT, page,
        script_safe,
    };

    /// One language's texts by key: a text, or a plural
    /// (`{"one": …, "other": …}`).
    fn catalog(lang: &str) -> BTreeMap<String, Value> {
        let (_, json) = LANGS
            .iter()
            .find(|(l, _)| *l == lang)
            .unwrap_or_else(|| panic!("no {lang} catalog"));
        let Entries(entries) =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("web/strings/{lang}.json: {e}"));
        let count = entries.len();
        let map: BTreeMap<_, _> = entries.into_iter().collect();
        assert_eq!(map.len(), count, "a key twice in {lang}");
        map
    }

    /// The languages besides English.
    fn translations() -> impl Iterator<Item = &'static str> {
        LANGS
            .iter()
            .map(|(lang, _)| *lang)
            .filter(|lang| *lang != "en")
    }

    /// A JSON object's entries in file order, a key that comes twice kept
    /// twice (a map would keep only the last).
    struct Entries(Vec<(String, Value)>);

    impl<'de> Deserialize<'de> for Entries {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct Each;
            impl<'de> Visitor<'de> for Each {
                type Value = Entries;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("an object of texts")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Entries, A::Error> {
                    let mut entries = Vec::new();
                    while let Some(entry) = map.next_entry()? {
                        entries.push(entry);
                    }
                    Ok(Entries(entries))
                }
            }
            d.deserialize_map(Each)
        }
    }

    /// The `{name}` placeholders in a value (all forms of a plural).
    fn placeholders(value: &Value) -> BTreeSet<&str> {
        let texts: Vec<&str> = match value {
            Value::Object(forms) => forms.values().filter_map(Value::as_str).collect(),
            other => other.as_str().into_iter().collect(),
        };
        texts
            .into_iter()
            .flat_map(|text| text.split('{').skip(1))
            .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
            .filter(|name| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
            .collect()
    }

    /// Texts an integration sends in English: not in `en`.
    fn from_server(key: &str) -> bool {
        key.starts_with("kind.") || key.starts_with("integ.detail.")
    }

    #[test]
    fn every_english_text_is_in_every_language() {
        let en = catalog("en");
        assert!(en.len() > 100, "the catalog was found");
        assert!(translations().any(|lang| lang == "de"), "German is there");
        for lang in translations() {
            let other = catalog(lang);
            let missing: Vec<_> = en.keys().filter(|k| !other.contains_key(*k)).collect();
            assert!(missing.is_empty(), "missing in {lang}: {missing:?}");
            let extra: Vec<_> = other
                .keys()
                .filter(|k| !en.contains_key(*k) && !from_server(k))
                .collect();
            assert!(extra.is_empty(), "only in {lang}: {extra:?}");
        }
    }

    #[test]
    fn every_entry_is_a_dotted_key_and_a_text() {
        // t() picks a plural's form by the language's plural rules, else
        // `other`; serverText() takes only a text.
        const FORMS: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];
        for (lang, _) in LANGS {
            for (key, value) in catalog(lang) {
                // What the key scan below reads; and a key such as
                // `__proto__` would mean something else in the page's script.
                let dotted = key.starts_with(|c: char| c.is_ascii_alphabetic())
                    && key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
                assert!(dotted, "{lang} {key:?}: not a dotted key");
                match &value {
                    Value::String(_) => {}
                    Value::Object(forms) if !from_server(&key) => {
                        assert!(forms.contains_key("other"), "{lang} {key}: no `other`");
                        for (form, text) in forms {
                            assert!(FORMS.contains(&form.as_str()), "{lang} {key}: {form}?");
                            assert!(text.is_string(), "{lang} {key}.{form}: not a text");
                        }
                    }
                    _ => panic!("{lang} {key}: not a text (or, as the app's own, a plural)"),
                }
            }
        }
    }

    #[test]
    fn both_grinder_wordings_are_there() {
        // tv() picks `<key>.e64` or `<key>.wifi`; one without the other
        // would show the bare key to that grinder's owners.
        let en = catalog("en");
        for key in en.keys() {
            for (this, other) in [(".e64", ".wifi"), (".wifi", ".e64")] {
                if let Some(base) = key.strip_suffix(this) {
                    let twin = format!("{base}{other}");
                    assert!(en.contains_key(twin.as_str()), "{key} without {twin}");
                }
            }
        }
    }

    #[test]
    fn translations_keep_the_placeholders() {
        let en = catalog("en");
        for lang in translations() {
            let other = catalog(lang);
            for (key, value) in &en {
                let Some(text) = other.get(key) else { continue };
                assert_eq!(
                    placeholders(value),
                    placeholders(text),
                    "placeholders of {key} in {lang}"
                );
            }
        }
    }

    #[test]
    fn keys_the_page_uses_exist() {
        let en = catalog("en");
        // A key may get a variant appended (`.e64`, `.wifi`), and one ending
        // in `.` is completed at run time.
        let known = |key: &str| {
            en.contains_key(key) || {
                let prefix = if key.ends_with('.') {
                    key.to_owned()
                } else {
                    format!("{key}.")
                };
                en.keys().any(|k| k.starts_with(&prefix))
            }
        };
        let markup = &PAGE_HTML[PAGE_HTML.find("<body").expect("body")..];
        // Every quoted string in the script that reads as a key of one of
        // the catalog's groups: in t() and tv(), and in the tables and
        // ternaries the script picks its keys from. A key is never made by
        // adding to a prefix, except for the prefixes in DYNAMIC, whose
        // keys other tests check.
        const DYNAMIC: [&str; 4] = [
            "kind.",         // kindKey(): integration texts, integration_texts_are_german
            "integ.detail.", // integDetail(): the same
            "history.kind.", // kindLabel(): the_page_knows_every_shot_kind
            "error.",        // errorText(): the_page_words_every_error_code
        ];
        let de = catalog("de");
        let groups: BTreeSet<&str> = en
            .keys()
            .chain(de.keys())
            .filter_map(|k| k.split('.').next())
            .collect();
        let mut used = BTreeSet::new();
        let mut prefixes = BTreeSet::new();
        for literal in js_strings(PAGE_JS) {
            let dotted = literal.contains('.')
                && literal.starts_with(|c: char| c.is_ascii_lowercase())
                && literal
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
            let group = literal.split('.').next().unwrap_or("");
            if !dotted || !groups.contains(group) {
                continue;
            }
            if literal.ends_with('.') {
                prefixes.insert(literal);
            } else {
                used.insert(literal.to_owned());
            }
        }
        let dynamic: BTreeSet<&str> = DYNAMIC.into_iter().collect();
        assert_eq!(prefixes, dynamic, "keys made from a prefix");
        // data-i18n="key" and data-i18n-attr="attr:key;…" in the markup.
        for (i, _) in markup.match_indices("data-i18n=\"") {
            let rest = &markup[i + "data-i18n=\"".len()..];
            used.insert(rest[..rest.find('"').unwrap()].to_owned());
        }
        for (i, _) in markup.match_indices("data-i18n-attr=\"") {
            let rest = &markup[i + "data-i18n-attr=\"".len()..];
            for pair in rest[..rest.find('"').unwrap()].split(';') {
                if let Some((_, key)) = pair.split_once(':') {
                    used.insert(key.trim().to_owned());
                }
            }
        }
        assert!(used.len() > 300, "the page's keys were found");
        // Keys picked through tables, not t("…"), are in the scan too.
        for key in [
            "badge.pressKnob",
            "setup.pill.restart",
            "grinder.state.problem",
        ] {
            assert!(used.contains(key), "{key} was found");
        }
        let unknown: Vec<_> = used.iter().filter(|k| !known(k)).collect();
        assert!(unknown.is_empty(), "keys not in the catalog: {unknown:?}");
    }

    /// The contents of the double- and single-quoted strings in `script`,
    /// with comments and template strings left out.
    fn js_strings(script: &str) -> Vec<&str> {
        let bytes = script.as_bytes();
        let mut found = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    i = script[i..].find('\n').map_or(bytes.len(), |n| i + n);
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    i = script[i + 2..]
                        .find("*/")
                        .map_or(bytes.len(), |n| i + n + 4);
                }
                quote @ (b'"' | b'\'' | b'`') => {
                    let start = i + 1;
                    i = start;
                    while i < bytes.len() && bytes[i] != quote {
                        i += if bytes[i] == b'\\' { 2 } else { 1 };
                    }
                    assert!(i < bytes.len(), "a string that does not end at {start}");
                    if quote != b'`' {
                        found.push(&script[start..i]);
                    }
                    i += 1;
                }
                _ => i += 1,
            }
        }
        found
    }

    /// The keys of the script's table `const <name> = { … };`, one
    /// `    <key>: …` line each.
    fn js_table(name: &str) -> Vec<&'static str> {
        let head = format!("\n  const {name} = {{\n");
        let start = PAGE_JS
            .find(&head)
            .unwrap_or_else(|| panic!("the {name} table"));
        let table = &PAGE_JS[start + head.len()..];
        let table = &table[..table
            .find("\n  };")
            .unwrap_or_else(|| panic!("end of {name}"))];
        table
            .lines()
            .filter_map(|line| line.strip_prefix("    ")?.split_once(": "))
            .map(|(key, _)| key)
            .collect()
    }

    #[test]
    fn the_page_knows_every_integration_health() {
        use crate::integration::Health;
        let all = [
            Health::Starting,
            Health::Connected,
            Health::Watching,
            Health::Failing,
            Health::Stopped,
        ];
        for health in all {
            // No wildcard: a new health has to be named here, next to `all`.
            match health {
                Health::Starting
                | Health::Connected
                | Health::Watching
                | Health::Failing
                | Health::Stopped => {}
            }
        }
        let names: BTreeSet<String> = all
            .iter()
            .map(|h| {
                serde_json::to_value(h)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let table: BTreeSet<String> = js_table("HEALTH").into_iter().map(str::to_owned).collect();
        assert_eq!(table, names, "HEALTH in app.js and the server's Health");
    }

    #[test]
    fn the_page_knows_every_shot_phase() {
        use crate::protocol::{Phase, ShotResult};
        use std::time::Instant;
        let all = [
            Phase::Ready,
            Phase::Running {
                since: Instant::now(),
                requested_by_grinder: true,
            },
            Phase::Finishing {
                until: Instant::now(),
                result: ShotResult {
                    time_ms: 25_000,
                    weight_g: 36.0,
                },
                aborted: false,
            },
        ];
        for phase in all {
            // No wildcard: a new phase has to be named here, next to `all`.
            match phase {
                Phase::Ready | Phase::Running { .. } | Phase::Finishing { .. } => {}
            }
        }
        let names: BTreeSet<String> = all
            .iter()
            .map(|p| {
                serde_json::to_value(p).unwrap()["phase"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let table: BTreeSet<String> = js_table("GROUP").into_iter().map(str::to_owned).collect();
        assert_eq!(table, names, "GROUP in app.js and the server's Phase");
    }

    #[test]
    fn the_page_listens_for_every_event() {
        use crate::protocol::{AbortCause, GrindResult, MachineEvent, ShotResult};
        let result = ShotResult {
            time_ms: 25_000,
            weight_g: 36.0,
        };
        let all = [
            MachineEvent::GrinderGrindFinished {
                result: GrindResult {
                    target_weight_g: Some(36.0),
                    filter: None,
                    raw: serde_json::Value::Null,
                },
            },
            MachineEvent::GrinderShotRequested {
                script_id: Some(9),
                target_time_ms: None,
            },
            MachineEvent::ShotStarted {
                requested_by_grinder: true,
                human_action: "",
            },
            MachineEvent::ShotReported {
                result,
                in_grinder_range: None,
                running_ms: 25_000,
                integration_id: None,
            },
            MachineEvent::shot_aborted(
                AbortCause::ShotTimedOut,
                std::time::Duration::from_secs(180),
            ),
            MachineEvent::ShotFinished,
        ];
        for event in &all {
            // No wildcard: a new event has to be named here, next to `all`.
            match event {
                MachineEvent::GrinderGrindFinished { .. }
                | MachineEvent::GrinderShotRequested { .. }
                | MachineEvent::ShotStarted { .. }
                | MachineEvent::ShotReported { .. }
                | MachineEvent::ShotAborted { .. }
                | MachineEvent::ShotFinished => {}
            }
        }
        let kinds: BTreeSet<String> = all
            .iter()
            .map(|e| {
                serde_json::to_value(e).unwrap()["kind"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        // The stream names every event by its kind (stream_events), so the
        // page listens for exactly these.
        let start = PAGE_JS.find("const EVENT_KINDS = [").expect("EVENT_KINDS");
        let list = &PAGE_JS[start..];
        let list = &list[..list.find("];").expect("end of EVENT_KINDS")];
        let listened: BTreeSet<String> = js_strings(list).into_iter().map(str::to_owned).collect();
        assert_eq!(listened, kinds, "EVENT_KINDS in app.js and MachineEvent");
    }

    #[test]
    fn the_theme_colour_is_the_background() {
        // --bg of a palette in app.css: `--bg: #…;` in the block after `head`.
        let bg = |head: &str| {
            let block = &PAGE_CSS[PAGE_CSS.find(head).unwrap_or_else(|| panic!("{head}"))..];
            let block = &block[..block.find('}').unwrap()];
            let at = block.find("--bg:").expect("--bg") + "--bg:".len();
            block[at..].split(';').next().unwrap().trim().to_owned()
        };
        let light = bg(":root {");
        let dark = bg(":root[data-theme=\"dark\"] {");
        // The meta tag the page starts with, and the head script's choice.
        assert!(
            PAGE_HTML.contains(&format!("<meta name=\"theme-color\" content=\"{light}\">")),
            "the theme-color meta tag is {light}"
        );
        assert!(
            PAGE_HTML.contains(&format!("dark ? \"{dark}\" : \"{light}\"")),
            "the head script sets {dark} and {light}"
        );
    }

    #[test]
    fn the_page_knows_every_abort_cause() {
        use crate::protocol::AbortCause;
        // The script's `CAUSES` table, one `    <code>: { … },` line per cause.
        let start = PAGE_JS
            .find("\n  const CAUSES = {\n")
            .expect("the CAUSES table");
        let table = &PAGE_JS[start..];
        let table = &table[..table.find("\n  };").expect("end of CAUSES")];
        let rows: BTreeMap<_, _> = table
            .lines()
            .filter_map(|line| line.strip_prefix("    ")?.split_once(": {"))
            .collect();
        let en = catalog("en");
        let mut codes = BTreeSet::new();
        for cause in AbortCause::ALL {
            let code = serde_json::to_value(cause).unwrap();
            let code = code.as_str().unwrap().to_owned();
            let row = rows
                .get(code.as_str())
                .unwrap_or_else(|| panic!("{code} is not in CAUSES"));
            let on_purpose = format!("onPurpose: {}", cause.on_purpose());
            assert!(row.contains(&on_purpose), "{code}: not {on_purpose}");
            // A timeout's texts say how long it waited, and the history's
            // own cause never reaches the Start tab.
            if cause != AbortCause::ShotTimedOut {
                assert!(row.contains("title: \""), "{code}: no history title");
                if cause != AbortCause::ShotEndMissed {
                    assert!(row.contains("text: \""), "{code}: no Start tab text");
                }
            }
            for key in row.split('"').skip(1).step_by(2) {
                assert!(en.contains_key(key), "{code}: {key} is not in the catalog");
            }
            codes.insert(code);
        }
        let stale: Vec<_> = rows.keys().filter(|k| !codes.contains(**k)).collect();
        assert!(stale.is_empty(), "not a cause: {stale:?}");
    }

    /// The script's table `const <name> = { … };` of `    <code>: "<key>",`
    /// lines, as code and catalog key, checked against `codes`: one row per
    /// code, each key in the catalog.
    fn check_key_table(name: &str, codes: BTreeSet<String>) {
        let head = format!("\n  const {name} = {{\n");
        let start = PAGE_JS.find(&head).unwrap_or_else(|| panic!("{name}"));
        let table = &PAGE_JS[start + head.len()..];
        let table = &table[..table.find("\n  };").expect("end of the table")];
        let en = catalog("en");
        let mut rows = BTreeSet::new();
        for line in table.lines() {
            let (code, key) = line
                .trim()
                .split_once(": ")
                .unwrap_or_else(|| panic!("{name}: {line}"));
            let key = key.trim_end_matches(',').trim_matches('"');
            assert!(
                en.contains_key(key),
                "{name}.{code}: {key} is not in the catalog"
            );
            rows.insert(code.to_owned());
        }
        assert_eq!(rows, codes, "{name} in app.js and the server's codes");
    }

    /// The JSON string `value` serializes to.
    fn code_of(value: impl serde::Serialize) -> String {
        serde_json::to_value(value)
            .unwrap()
            .as_str()
            .expect("a code")
            .to_owned()
    }

    #[test]
    fn the_page_words_every_link_problem() {
        use crate::protocol::AbortCause;
        use crate::server::wifi_grinder::{ProblemCause, ResultProblem};
        for cause in ProblemCause::ALL {
            // No wildcard: a new problem has to be named here, next to `ALL`.
            match cause {
                ProblemCause::Shot(
                    AbortCause::GrinderShotDropped
                    | AbortCause::GrinderShotRefused
                    | AbortCause::GrinderUnpaired,
                )
                | ProblemCause::Result(
                    ResultProblem::GrinderResultExpired
                    | ResultProblem::GrinderResultRefused
                    | ResultProblem::GrinderResultDelayed,
                ) => {}
                ProblemCause::Shot(other) => panic!("{other:?} is not a link problem"),
            }
        }
        let codes = ProblemCause::ALL.into_iter().map(code_of).collect();
        check_key_table("PROBLEMS", codes);
    }

    #[test]
    fn the_page_words_every_grind_block() {
        use crate::protocol::wire::GrindBlocked;
        for b in GrindBlocked::ALL {
            // No wildcard: a new cause has to be named here, next to `ALL`.
            match b {
                GrindBlocked::ShotRunning
                | GrindBlocked::MachineInStandby
                | GrindBlocked::MachineInEco
                | GrindBlocked::MachineTesting
                | GrindBlocked::MachineFaulty
                | GrindBlocked::MachineWarmingUp
                | GrindBlocked::MachineTankEmpty => {}
            }
        }
        let codes = GrindBlocked::ALL.into_iter().map(code_of).collect();
        check_key_table("BLOCKED", codes);
    }

    /// Every `code` an error reply can carry: the codes of the error types,
    /// and the literal ones in `error(<status>, "<code>", …)` anywhere in
    /// `src/` (read from the source, so a new one can't be left out).
    fn error_codes() -> BTreeSet<String> {
        use crate::integration::ChangeError;
        use crate::protocol::MachineError;
        use crate::server::SettingsError;
        use crate::server::wifi_grinder::PairingIdRefusal;
        let machine = [
            MachineError::NotRunning,
            MachineError::AlreadyRunning,
            MachineError::NoWeight,
        ];
        let settings = [
            SettingsError::Save(String::new()),
            SettingsError::Grinder(String::new()),
        ];
        let change = [
            ChangeError::Invalid(String::new()),
            ChangeError::NotFound,
            ChangeError::CommandLine,
            ChangeError::InApp,
            ChangeError::NotPersistent,
            ChangeError::Save(String::new()),
        ];
        let pairing = [
            PairingIdRefusal::NoLink,
            PairingIdRefusal::NotNeeded,
            PairingIdRefusal::NotAsking,
            PairingIdRefusal::NotAccepting,
            PairingIdRefusal::Busy,
        ];
        // No wildcards: a new error has to be named here, next to its list.
        for e in &machine {
            match e {
                MachineError::NotRunning
                | MachineError::AlreadyRunning
                | MachineError::NoWeight => {}
            }
        }
        for e in &settings {
            match e {
                SettingsError::Save(_) | SettingsError::Grinder(_) => {}
            }
        }
        for e in &change {
            match e {
                ChangeError::Invalid(_)
                | ChangeError::NotFound
                | ChangeError::CommandLine
                | ChangeError::InApp
                | ChangeError::NotPersistent
                | ChangeError::Save(_) => {}
            }
        }
        for e in pairing {
            match e {
                PairingIdRefusal::NoLink
                | PairingIdRefusal::NotNeeded
                | PairingIdRefusal::NotAsking
                | PairingIdRefusal::NotAccepting
                | PairingIdRefusal::Busy => {}
            }
        }
        let mut codes: BTreeSet<String> = machine
            .iter()
            .map(MachineError::code)
            .chain(settings.iter().map(SettingsError::code))
            .chain(change.iter().map(ChangeError::code))
            .chain(pairing.iter().map(PairingIdRefusal::code))
            .map(str::to_owned)
            .collect();
        let mut dirs = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut literal = BTreeSet::new();
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap();
                for (i, _) in source.match_indices("error(") {
                    let before = source[..i].chars().next_back();
                    let line = &source[source[..i].rfind('\n').map_or(0, |n| n + 1)..i];
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
                        || line.trim_start().starts_with("//")
                    {
                        continue;
                    }
                    // The status, then the code as a literal.
                    let args = &source[i + "error(".len()..];
                    let Some((_, rest)) = args.split_once(',') else {
                        continue;
                    };
                    let Some(rest) = rest.trim_start().strip_prefix('"') else {
                        continue;
                    };
                    let code = &rest[..rest.find('"').unwrap()];
                    let snake = code.contains('_')
                        && code.chars().all(|c| c.is_ascii_lowercase() || c == '_');
                    assert!(snake, "{}: error code {code:?}", path.display());
                    literal.insert(code.to_owned());
                }
            }
        }
        for code in [
            "request_invalid",
            "request_host_refused",
            "settings_invalid",
        ] {
            assert!(literal.contains(code), "{code} was found in the source");
        }
        codes.extend(literal);
        codes
    }

    #[test]
    fn the_page_words_every_error_code() {
        // The page's own: no answer came, or one without the API's body.
        let page = ["request_failed", "request_status"];
        for code in page {
            assert!(PAGE_JS.contains(&format!("\"{code}\"")), "{code} in app.js");
        }
        let mut codes = error_codes();
        codes.extend(page.map(str::to_owned));
        let en = catalog("en");
        let missing: Vec<_> = codes
            .iter()
            .filter(|c| !en.contains_key(format!("error.{c}").as_str()))
            .collect();
        assert!(missing.is_empty(), "codes without words: {missing:?}");
        // And none for a code that is gone; `error.<code>.<field>` words
        // one for a field the page sent.
        let stale: Vec<_> = en
            .keys()
            .filter_map(|k| k.strip_prefix("error."))
            .filter(|rest| !codes.contains(rest.split('.').next().unwrap()))
            .collect();
        assert!(stale.is_empty(), "words for no code: {stale:?}");
    }

    #[test]
    fn the_page_knows_every_shot_kind() {
        use crate::server::history::ShotOutcome;
        let all = [
            ShotOutcome::Succeeded,
            ShotOutcome::Aborted,
            ShotOutcome::Failed,
        ];
        for kind in all {
            // No wildcard, so a new kind has to be named here, next to
            // `all`, as a reminder to add it there too.
            match kind {
                ShotOutcome::Succeeded | ShotOutcome::Aborted | ShotOutcome::Failed => {}
            }
        }
        let en = catalog("en");
        for kind in all {
            let code = serde_json::to_value(kind).unwrap();
            let code = code.as_str().unwrap();
            assert!(
                PAGE_CSS.contains(&format!("\n.k-{code} {{")),
                "no .k-{code} colours"
            );
            let key = format!("history.kind.{code}");
            assert!(en.contains_key(key.as_str()), "{key} is not in the catalog");
        }
    }

    #[test]
    fn integration_texts_are_german() {
        let de = catalog("de");
        let (mut missing, mut wanted) = (Vec::new(), BTreeSet::new());
        let mut need = |key: String| {
            if !de.contains_key(key.as_str()) {
                missing.push(key.clone());
            }
            wanted.insert(key);
        };
        for kind in crate::integration::KINDS {
            need(format!("kind.{}.summary", kind.id));
            for field in kind.fields {
                need(format!("kind.{}.field.{}.label", kind.id, field.key));
                if !field.help.is_empty() {
                    need(format!("kind.{}.field.{}.help", kind.id, field.key));
                }
            }
        }
        // Status lines: every `Detail::keyed("<id>.<state>", …)` of a known kind.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/integration");
        let mut keys = BTreeSet::new();
        for kind in crate::integration::KINDS {
            for entry in std::fs::read_dir(dir.join(kind.id)).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let code = std::fs::read_to_string(&path).unwrap();
                for (i, _) in code.match_indices("Detail::keyed(") {
                    let rest = code[i..].split_once('"').map(|(_, r)| r).unwrap_or("");
                    if let Some((key, _)) = rest.split_once('"') {
                        assert!(
                            key.starts_with(&format!("{}.", kind.id)),
                            "{}: status key {key} does not start with {}.",
                            path.display(),
                            kind.id
                        );
                        keys.insert(key.to_owned());
                    }
                }
            }
        }
        assert!(!keys.is_empty(), "status keys were found");
        for key in keys {
            need(format!("integ.detail.{key}"));
        }
        assert!(missing.is_empty(), "no German for: {missing:?}");
        // And none left behind by a field or status line that is gone (a
        // field's placeholder may have its own wording).
        let field_of = |k: &str| k.strip_suffix(".placeholder").map(|f| format!("{f}.label"));
        let unused: Vec<_> = de
            .keys()
            .filter(|k| from_server(k) && !wanted.contains(k.as_str()))
            .filter(|k| field_of(k).is_none_or(|label| !wanted.contains(&label)))
            .collect();
        assert!(unused.is_empty(), "German for nothing: {unused:?}");
    }

    #[test]
    fn the_page_comes_with_its_styles_script_and_texts() {
        let slots = [
            ("index.html", PAGE_HTML, CSS_SLOT),
            ("index.html", PAGE_HTML, JS_SLOT),
            ("app.js", PAGE_JS, STRINGS_SLOT),
            ("app.js", PAGE_JS, VERSION_SLOT),
        ];
        for (file, text, slot) in slots {
            assert_eq!(text.matches(slot).count(), 1, "{slot} in {file}");
        }
        assert!(PAGE_HTML.find(CSS_SLOT) < PAGE_HTML.find(JS_SLOT));
        assert!(PAGE_JS.find(STRINGS_SLOT) < PAGE_JS.find(VERSION_SLOT));
        let langs: BTreeSet<_> = LANGS.iter().map(|(lang, _)| lang).collect();
        assert_eq!(langs.len(), LANGS.len(), "a language twice in LANGS");
        // Nothing in them ends their element early.
        assert!(!PAGE_CSS.to_ascii_lowercase().contains("</style"));
        let js = PAGE_JS.to_ascii_lowercase();
        assert!(!js.contains("</script") && !js.contains("<!--"));

        let page = page();
        for (_, _, slot) in slots {
            assert!(!page.contains(slot), "{slot} left in the page");
        }
        let css = format!("<style>\n{}\n</style>", PAGE_CSS.trim_end());
        assert!(page.contains(&css), "the styles");
        let (head, rest) = PAGE_JS.split_once(STRINGS_SLOT).unwrap();
        let (middle, tail) = rest.split_once(VERSION_SLOT).unwrap();
        assert!(page.contains(&format!("<script>\n{head}")), "the script");
        // The version the page comes with, as a JSON string.
        let version = serde_json::to_string(crate::version()).unwrap();
        let script_end = format!("{middle}{version}{}\n</script>", tail.trim_end());
        assert!(page.contains(&script_end), "the rest of the script");
        assert!(page.contains(&format!("const PAGE_VERSION = {version};")));
        for (lang, json) in LANGS {
            let texts = format!("\"{lang}\": {}", script_safe(json.trim()));
            assert!(page.contains(&texts), "the {lang} texts");
        }
        // One document, with nothing more to load.
        assert!(page.starts_with("<!doctype html>") && page.ends_with("</html>\n"));
        assert_eq!(page.matches("<style>").count(), 1);
        assert!(!page.contains("<script src") && !page.contains("rel=\"stylesheet\""));
        // And with the texts in, still only the theme script's and the app
        // script's end, and the styles' end.
        let lower = page.to_ascii_lowercase();
        assert_eq!(
            lower.matches("</script").count(),
            2,
            "</script> in the page"
        );
        assert_eq!(lower.matches("</style").count(), 1, "</style> in the page");
        let at = page.find(&format!("<script>\n{head}")).unwrap();
        assert!(!page[at..].contains("<!--"), "<!-- in the app's script");
    }

    #[test]
    fn every_language_has_its_button() {
        // Status → Settings → Language: Auto, and a button per language.
        let attr = "data-lang-choice=\"";
        let buttons: BTreeSet<&str> = PAGE_HTML
            .match_indices(attr)
            .filter_map(|(i, _)| PAGE_HTML[i + attr.len()..].split_once('"'))
            .map(|(code, _)| code)
            .filter(|code| *code != "auto")
            .collect();
        let langs: BTreeSet<&str> = LANGS.iter().map(|(lang, _)| *lang).collect();
        assert_eq!(buttons, langs, "the language buttons and LANGS");
    }

    #[test]
    fn texts_cannot_end_the_script_early() {
        let json = r#"{"a": "</script><!-- b"}"#;
        let safe = script_safe(json);
        assert_eq!(safe, r#"{"a": "<\/script><\u0021-- b"}"#);
        let (a, b): (Value, Value) = (
            serde_json::from_str(json).unwrap(),
            serde_json::from_str(&safe).unwrap(),
        );
        assert_eq!(a, b, "still the same JSON");
    }
}
