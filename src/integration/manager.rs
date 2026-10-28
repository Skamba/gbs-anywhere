//! The integrations that are running, whether started from the command line
//! or added in the app, and how the app's ones are kept in the settings file
//! across restarts.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use anyhow::Context;
use serde::Serialize;
use tokio::task::AbortHandle;

use super::{Integration, Kind, Link, Settings, Status, StatusSnapshot};
use crate::server::Server;
use crate::server::settings::{SavedIntegration, SettingsFile};

/// Where an integration came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Source {
    /// Flags or environment variables; only those can turn it off.
    CommandLine,
    /// Added in the app; can be removed there.
    App,
}

/// An integration as the API shows it.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct View {
    #[serde(flatten)]
    pub(crate) status: StatusSnapshot,
    pub(crate) source: Source,
    /// Set on the command line and can be moved into the app (there is a
    /// settings file to keep it in).
    pub(crate) adoptable: bool,
    /// Moved into the app from the command line: deleting it lets its flags
    /// start it again at the next start.
    pub(crate) from_command_line: bool,
}

/// The settings of an integration added in the app, as the app may see them
/// to edit it: no passwords, only which of them are set.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct FormView {
    pub(crate) kind: String,
    pub(crate) settings: BTreeMap<String, String>,
    pub(crate) secrets_set: Vec<String>,
}

/// Why a change failed. Nothing changed in any case.
#[derive(Debug, PartialEq)]
pub(crate) enum ChangeError {
    /// The settings do not work for this integration.
    Invalid(String),
    NotFound,
    /// Set on the command line, so the app can't change or remove it.
    CommandLine,
    /// Already added in the app (or moved there).
    InApp,
    /// No settings file to keep it in.
    NotPersistent,
    /// The settings file could not be written.
    Save(String),
}

impl std::fmt::Display for ChangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(e) => f.write_str(e),
            Self::NotFound => f.write_str("no such integration"),
            Self::CommandLine => f.write_str(
                "set on the command line: change or remove its flags, or move it into the app",
            ),
            Self::InApp => f.write_str("already managed in the app"),
            Self::NotPersistent => f.write_str(
                "there is no settings file to keep it in: start gbs-anywhere with --config",
            ),
            Self::Save(e) => write!(f, "could not save the settings: {e}"),
        }
    }
}

impl std::error::Error for ChangeError {}

impl ChangeError {
    /// As a fixed code, for `code` in the API's errors.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "integration_settings_invalid",
            Self::NotFound => "integration_not_found",
            Self::CommandLine => "integration_on_command_line",
            Self::InApp => "integration_in_app",
            Self::NotPersistent => "settings_file_missing",
            Self::Save(_) => "settings_not_saved",
        }
    }
}

struct Entry {
    status: Status,
    source: Source,
    /// `None` for an entry in the settings file that this version has no
    /// kind for.
    kind: Option<&'static Kind>,
    /// What goes into the settings file (app entries only).
    saved: Option<SavedIntegration>,
    /// The settings from the flags (command-line entries only), for moving
    /// it into the app. Never shown.
    cli: Option<Settings>,
    task: Option<AbortHandle>,
}

impl Entry {
    fn id(&self) -> &str {
        self.status.id()
    }
}

/// All integrations, in the order they were started.
#[derive(Default)]
pub struct Integrations {
    entries: Mutex<Vec<Entry>>,
    file: OnceLock<Arc<SettingsFile>>,
}

impl Integrations {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn list(&self) -> Vec<View> {
        let persistent = self.persistent();
        self.lock().iter().map(|e| view(e, persistent)).collect()
    }

    /// Whether integrations added in the app survive a restart.
    pub(crate) fn persistent(&self) -> bool {
        self.file.get().is_some()
    }

    /// Starts an integration configured on the command line. Its id is the
    /// kind's id.
    pub fn start_cli(
        &self,
        server: &Arc<Server>,
        kind: &'static Kind,
        settings: &Settings,
    ) -> anyhow::Result<()> {
        let integration = kind
            .create(settings, false)
            .with_context(|| format!("{} (command line)", kind.title))?;
        let mut entries = self.lock();
        let id = free_id(&entries, kind.id);
        let status = Status::new(id, kind.id, kind.title);
        let task = spawn(server, integration, status.clone());
        tracing::info!(
            "{}: enabled, shots are reported automatically after a knob press (on an \
             E65/E80 GbS: after a Grind-by-Sync grind)",
            kind.title
        );
        entries.push(Entry {
            status,
            source: Source::CommandLine,
            kind: Some(kind),
            saved: None,
            cli: Some(settings.clone()),
            task: Some(task),
        });
        Ok(())
    }

    /// Adds one from the app: checks the settings, saves them and starts it.
    /// Returns its id.
    pub(crate) fn add(
        &self,
        server: &Arc<Server>,
        kind: &'static Kind,
        settings: Settings,
    ) -> Result<String, ChangeError> {
        let integration = kind
            .create(&settings, true)
            .map_err(|e| ChangeError::Invalid(format!("{e:#}")))?;
        let mut entries = self.lock();
        let id = free_id(&entries, kind.id);
        let status = Status::new(id.clone(), kind.id, kind.title);
        entries.push(Entry {
            status: status.clone(),
            source: Source::App,
            kind: Some(kind),
            saved: Some(SavedIntegration {
                id: id.clone(),
                kind: kind.id.to_owned(),
                settings,
                from_command_line: false,
            }),
            cli: None,
            task: None,
        });
        if let Err(e) = self.save(&entries) {
            entries.pop();
            return Err(ChangeError::Save(format!("{e:#}")));
        }
        let last = entries.last_mut().expect("just pushed");
        last.task = Some(spawn(server, integration, status));
        tracing::info!("{}: added in the app as {id}", kind.title);
        Ok(id)
    }

    /// Stops and forgets one that was added in the app.
    pub(crate) fn remove(&self, id: &str) -> Result<(), ChangeError> {
        let mut entries = self.lock();
        let i = entries
            .iter()
            .position(|e| e.id() == id)
            .ok_or(ChangeError::NotFound)?;
        if entries[i].source == Source::CommandLine {
            return Err(ChangeError::CommandLine);
        }
        let entry = entries.remove(i);
        if let Err(e) = self.save(&entries) {
            entries.insert(i, entry);
            return Err(ChangeError::Save(format!("{e:#}")));
        }
        if let Some(task) = &entry.task {
            task.abort();
        }
        tracing::info!("{}: removed ({id})", entry.status.title());
        Ok(())
    }

    /// The settings of one added in the app, for editing: passwords left out.
    pub(crate) fn form(&self, id: &str) -> Result<FormView, ChangeError> {
        let entries = self.lock();
        let entry = entries
            .iter()
            .find(|e| e.id() == id)
            .ok_or(ChangeError::NotFound)?;
        let saved = match (&entry.saved, entry.source) {
            (Some(saved), Source::App) => saved,
            _ => return Err(ChangeError::CommandLine),
        };
        // Without its kind, which fields are passwords is not known: none
        // are shown.
        let (settings, secrets_set) = match entry.kind {
            Some(kind) => kind.shown(&saved.settings),
            None => Default::default(),
        };
        Ok(FormView {
            kind: saved.kind.clone(),
            settings,
            secrets_set,
        })
    }

    /// Changes the settings of one added in the app and restarts it. A
    /// password left out or empty keeps its saved value.
    pub(crate) fn update(
        &self,
        server: &Arc<Server>,
        id: &str,
        settings: Settings,
    ) -> Result<(), ChangeError> {
        let mut entries = self.lock();
        let i = entries
            .iter()
            .position(|e| e.id() == id)
            .ok_or(ChangeError::NotFound)?;
        let entry = &entries[i];
        let Some(saved) = entry.saved.clone().filter(|_| entry.source == Source::App) else {
            return Err(ChangeError::CommandLine);
        };
        let kind = entry.kind.ok_or_else(|| {
            ChangeError::Invalid(format!(
                "this version of gbs-anywhere has no `{}` integration",
                saved.kind
            ))
        })?;
        let settings = kind.keep_secrets(settings, &saved.settings);
        let integration = kind
            .create(&settings, true)
            .map_err(|e| ChangeError::Invalid(format!("{e:#}")))?;
        entries[i].saved = Some(SavedIntegration {
            settings,
            ..saved.clone()
        });
        if let Err(e) = self.save(&entries) {
            entries[i].saved = Some(saved);
            return Err(ChangeError::Save(format!("{e:#}")));
        }
        let entry = &mut entries[i];
        if let Some(task) = entry.task.take() {
            task.abort();
        }
        entry.status = Status::new(id, kind.id, kind.title);
        entry.task = Some(spawn(server, integration, entry.status.clone()));
        tracing::info!("{}: settings changed in the app ({id})", kind.title);
        Ok(())
    }

    /// Moves one set on the command line into the app: its settings are
    /// saved as an app entry with the same id, marked as moved. While that
    /// entry is in the settings file, the flags are ignored. It keeps
    /// running as it is.
    pub(crate) fn adopt(&self, id: &str) -> Result<View, ChangeError> {
        if !self.persistent() {
            return Err(ChangeError::NotPersistent);
        }
        let mut entries = self.lock();
        let i = entries
            .iter()
            .position(|e| e.id() == id)
            .ok_or(ChangeError::NotFound)?;
        let entry = &entries[i];
        let (Source::CommandLine, Some(kind), Some(settings)) =
            (entry.source, entry.kind, entry.cli.clone())
        else {
            return Err(ChangeError::InApp);
        };
        // A setting that is not on the form (a test server) would be lost,
        // and the next start would use the vendor's real service.
        kind.create(&settings, true)
            .map_err(|e| ChangeError::Invalid(format!("can't be moved into the app: {e:#}")))?;
        entries[i].source = Source::App;
        entries[i].saved = Some(SavedIntegration {
            id: id.to_owned(),
            kind: kind.id.to_owned(),
            settings,
            from_command_line: true,
        });
        if let Err(e) = self.save(&entries) {
            entries[i].source = Source::CommandLine;
            entries[i].saved = None;
            return Err(ChangeError::Save(format!("{e:#}")));
        }
        tracing::info!(
            "{}: moved into the app ({id}); its command-line settings are ignored from the next \
             start on while it is there",
            kind.title
        );
        Ok(view(&entries[i], true))
    }

    /// Uses `file` as the settings file: starts what it holds and saves
    /// every later change there. Entries that cannot start (a kind this
    /// version does not have, settings it rejects) are shown with the error
    /// and kept in the file.
    pub fn load(
        &self,
        server: &Arc<Server>,
        file: Arc<SettingsFile>,
        catalog: &[&'static Kind],
    ) -> anyhow::Result<usize> {
        let saved_entries = file.data().integrations;
        let path = file.path().to_owned();
        if self.file.set(file).is_err() {
            anyhow::bail!("settings file already loaded");
        }
        let mut entries = self.lock();
        let count = saved_entries.len();
        for mut saved in saved_entries {
            let kind = catalog.iter().copied().find(|k| k.id == saved.kind);
            saved.id = free_id(&entries, &saved.id);
            let title = kind.map_or(saved.kind.as_str(), |k| k.title).to_owned();
            let status = Status::new(saved.id.clone(), &saved.kind, &title);
            let task = match kind.map(|k| k.create(&saved.settings, true)) {
                Some(Ok(integration)) => Some(spawn(server, integration, status.clone())),
                Some(Err(e)) => {
                    tracing::warn!("{title}: cannot start ({e:#})");
                    status.error(format!("cannot start: {e:#}"));
                    None
                }
                None => {
                    tracing::warn!("{}: unknown integration in {}", saved.kind, path.display());
                    status.error(format!(
                        "this version of gbs-anywhere has no `{}` integration",
                        saved.kind
                    ));
                    None
                }
            };
            entries.push(Entry {
                status,
                source: Source::App,
                kind,
                saved: Some(saved),
                cli: None,
                task,
            });
        }
        // Ids that had to change are kept.
        self.save(&entries)?;
        Ok(count)
    }

    fn save(&self, entries: &[Entry]) -> anyhow::Result<()> {
        let Some(file) = self.file.get() else {
            return Ok(());
        };
        let list = saved_list(entries);
        file.update(|d| d.integrations = list)
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// `base`, or `base-2`, `base-3`, ... whichever is free.
fn free_id(entries: &[Entry], base: &str) -> String {
    let taken = |id: &str| entries.iter().any(|e| e.id() == id);
    if !base.is_empty() && !taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken(id))
        .expect("a free id")
}

fn view(e: &Entry, persistent: bool) -> View {
    View {
        status: e.status.snapshot(),
        source: e.source,
        adoptable: persistent && e.source == Source::CommandLine && e.cli.is_some(),
        from_command_line: e.saved.as_ref().is_some_and(|s| s.from_command_line),
    }
}

/// The app's entries, as the settings file keeps them.
fn saved_list(entries: &[Entry]) -> Vec<SavedIntegration> {
    entries.iter().filter_map(|e| e.saved.clone()).collect()
}

/// Runs the integration, and watches how its task ends: returning marks it
/// stopped, a panic as failing (and goes to the log, which the app shows).
/// Stopping it with the returned handle, as a removal or a settings change
/// does, is not a failure.
fn spawn(server: &Arc<Server>, integration: Box<dyn Integration>, status: Status) -> AbortHandle {
    let link = Link {
        server: server.clone(),
        status: status.clone(),
    };
    let task = tokio::spawn(async move { integration.run(link).await });
    let handle = task.abort_handle();
    tokio::spawn(async move {
        match task.await {
            Ok(()) => status.stopped(),
            Err(e) if e.is_cancelled() => {}
            Err(e) => {
                let why = e
                    .try_into_panic()
                    .map(|p| panic_text(p.as_ref()))
                    .unwrap_or_default();
                tracing::error!("{}: stopped by an internal error: {why}", status.title());
                status.error(format!("stopped by an internal error: {why}"));
            }
        }
    });
    handle
}

/// Longest panic message kept for the card and the log.
const MAX_PANIC_CHARS: usize = 200;

/// A panic's message, cut to [`MAX_PANIC_CHARS`].
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    let text = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("no message");
    match text.char_indices().nth(MAX_PANIC_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::{BoxFuture, Field, Health, Input};
    use crate::protocol::MachineConfig;
    use crate::test_support::temp_dir;

    struct Idle;
    impl Integration for Idle {
        fn run(self: Box<Self>, link: Link) -> BoxFuture {
            Box::pin(async move {
                link.status.connected("idle");
                std::future::pending::<()>().await;
            })
        }
    }

    static SCALE: Kind = Kind {
        id: "scale",
        title: "Test scale",
        summary: "",
        icon: "",
        fields: &[Field {
            key: "host",
            label: "Host",
            help: "",
            input: Input::Text,
            required: true,
            default: "",
        }],
        build: |_| Ok(Box::new(Idle)),
    };

    /// A cloud with a login, like La Marzocco's.
    static CLOUD: Kind = Kind {
        id: "cloud",
        title: "Test cloud",
        summary: "",
        icon: "",
        fields: &[
            Field {
                key: "user",
                label: "User",
                help: "",
                input: Input::Email,
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
        ],
        build: |_| Ok(Box::new(Idle)),
    };

    struct Panics;
    impl Integration for Panics {
        fn run(self: Box<Self>, link: Link) -> BoxFuture {
            Box::pin(async move {
                link.status.connected("about to fail");
                tokio::task::yield_now().await;
                panic!("{}", "a bug ".repeat(100));
            })
        }
    }

    struct Ends;
    impl Integration for Ends {
        fn run(self: Box<Self>, _: Link) -> BoxFuture {
            Box::pin(async {})
        }
    }

    async fn wait_for_health(status: &Status, health: Health) {
        for _ in 0..100 {
            if status.snapshot().health == health {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("still {:?}", status.snapshot().health);
    }

    #[tokio::test]
    async fn a_panic_shows_as_a_failure_and_a_stop_does_not() {
        let x = Server::new(MachineConfig::default());
        let status = Status::new("bug", "bug", "Test bug");
        spawn(&x, Box::new(Panics), status.clone());
        wait_for_health(&status, Health::Failing).await;
        let error = status.snapshot().last_error.unwrap();
        assert!(
            error.starts_with("stopped by an internal error: a bug a bug"),
            "{error}"
        );
        assert!(error.chars().count() < MAX_PANIC_CHARS + 50, "kept short");

        // Stopped on purpose (removed, or its settings changed): as it was.
        let status = Status::new("idle", "idle", "Test idle");
        let handle = spawn(&x, Box::new(Idle), status.clone());
        wait_for_health(&status, Health::Connected).await;
        handle.abort();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let snap = status.snapshot();
        assert_eq!((snap.health, snap.last_error), (Health::Connected, None));

        // Returning is stopping.
        let status = Status::new("ends", "ends", "Test ends");
        spawn(&x, Box::new(Ends), status.clone());
        wait_for_health(&status, Health::Stopped).await;

        // The app's list shows it too.
        static BUG: Kind = Kind {
            id: "bug",
            title: "Test bug",
            summary: "",
            icon: "",
            fields: &[],
            build: |_| Ok(Box::new(Panics)),
        };
        let m = x.integrations();
        m.start_cli(&x, &BUG, &Settings::new()).unwrap();
        for _ in 0..100 {
            if m.list()[0].status.health == Health::Failing {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(m.list()[0].status.health, Health::Failing);
    }

    fn host(h: &str) -> Settings {
        Settings::new().with("host", Some(h))
    }

    fn login(user: &str, pass: &str) -> Settings {
        Settings::new()
            .with("user", Some(user))
            .with("pass", Some(pass))
    }

    fn ids(x: &Server) -> Vec<String> {
        x.integrations()
            .list()
            .into_iter()
            .map(|v| v.status.id)
            .collect()
    }

    fn open(path: &std::path::Path) -> Arc<SettingsFile> {
        SettingsFile::open(path).unwrap()
    }

    #[tokio::test]
    async fn add_and_remove_from_the_app() {
        let x = Server::new(MachineConfig::default());
        let m = x.integrations();
        assert!(!m.persistent());
        m.start_cli(&x, &SCALE, &host("a")).unwrap();
        assert_eq!(m.add(&x, &SCALE, host("b")).unwrap(), "scale-2");
        assert_eq!(m.add(&x, &SCALE, host("c")).unwrap(), "scale-3");
        assert_eq!(ids(&x), ["scale", "scale-2", "scale-3"]);
        tokio::task::yield_now().await;
        assert_eq!(m.list()[1].status.health, Health::Connected);
        assert_eq!(m.list()[1].source, Source::App);
        assert!(!m.list()[0].adoptable, "no settings file to move it into");

        assert_eq!(
            m.add(&x, &SCALE, Settings::new()),
            Err(ChangeError::Invalid("Host is required".into()))
        );
        assert_eq!(m.remove("scale"), Err(ChangeError::CommandLine));
        assert_eq!(m.remove("nope"), Err(ChangeError::NotFound));
        m.remove("scale-2").unwrap();
        assert_eq!(ids(&x), ["scale", "scale-3"]);
        // A freed id is reused.
        assert_eq!(m.add(&x, &SCALE, host("d")).unwrap(), "scale-2");
        assert_eq!(m.adopt("scale").err(), Some(ChangeError::NotPersistent));
    }

    #[tokio::test]
    async fn the_settings_file_keeps_app_entries() {
        let path = temp_dir("keep").join("settings.json");
        let x = Server::new(MachineConfig::default());
        let m = x.integrations();
        m.start_cli(&x, &SCALE, &host("cli")).unwrap();
        assert_eq!(m.load(&x, open(&path), &[&SCALE]).unwrap(), 0);
        assert!(m.persistent());
        assert!(path.exists(), "created at load");
        m.add(&x, &SCALE, host("one")).unwrap();
        m.add(&x, &SCALE, host("two")).unwrap();
        m.remove("scale-2").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"two\"") && !text.contains("\"one\"") && !text.contains("cli"));

        // Next start: the command-line one takes `scale` again, the saved
        // `scale-3` keeps its id, and an unknown kind is shown, not lost.
        let mut file: serde_json::Value = serde_json::from_str(&text).unwrap();
        file["integrations"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"id": "gone", "kind": "gone", "settings": {}}));
        std::fs::write(&path, file.to_string()).unwrap();
        let y = Server::new(MachineConfig::default());
        y.integrations()
            .start_cli(&y, &SCALE, &host("cli"))
            .unwrap();
        assert_eq!(
            y.integrations().load(&y, open(&path), &[&SCALE]).unwrap(),
            2
        );
        assert_eq!(ids(&y), ["scale", "scale-3", "gone"]);
        let gone = &y.integrations().list()[2];
        assert_eq!(gone.status.health, Health::Failing);
        assert!(std::fs::read_to_string(&path).unwrap().contains("gone"));
        y.integrations().remove("gone").unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("gone"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn a_failed_save_changes_nothing() {
        let path = temp_dir("fail").join("settings.json");
        let x = Server::new(MachineConfig::default());
        let m = x.integrations();
        m.start_cli(&x, &SCALE, &host("cli")).unwrap();
        m.load(&x, open(&path), &[&SCALE]).unwrap();
        // Replace the file with a directory: renaming onto it fails.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(matches!(
            m.add(&x, &SCALE, host("a")),
            Err(ChangeError::Save(_))
        ));
        assert_eq!(ids(&x), ["scale"]);
        assert!(matches!(m.adopt("scale"), Err(ChangeError::Save(_))));
        assert_eq!(m.list()[0].source, Source::CommandLine);
        // Editing too: the saved settings stay as they were.
        std::fs::remove_dir(&path).unwrap();
        let app = m.add(&x, &SCALE, host("app")).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(matches!(
            m.update(&x, &app, host("b")),
            Err(ChangeError::Save(_))
        ));
        let form = m.form(&app).unwrap();
        assert_eq!(form.settings.get("host").map(String::as_str), Some("app"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn edit_keeps_the_saved_password() {
        let path = temp_dir("edit").join("settings.json");
        let x = Server::new(MachineConfig::default());
        let m = x.integrations();
        m.load(&x, open(&path), &[&CLOUD]).unwrap();
        let id = m
            .add(&x, &CLOUD, login("me@example.com", "secret"))
            .unwrap();
        let form = m.form(&id).unwrap();
        assert_eq!(form.kind, "cloud");
        assert_eq!(
            form.settings.get("user").map(String::as_str),
            Some("me@example.com")
        );
        assert!(
            !form.settings.contains_key("pass"),
            "passwords never go out"
        );
        assert_eq!(form.secrets_set, ["pass"]);

        // A new user name, the password left empty: the saved one stays.
        let edit = Settings::new()
            .with("user", Some("you@example.com"))
            .with("pass", Some(""));
        m.update(&x, &id, edit).unwrap();
        let saved = &open(&path).data().integrations[0];
        assert_eq!(saved.settings.text("user"), Some("you@example.com"));
        assert_eq!(saved.settings.secret("pass"), Some("secret"));
        tokio::task::yield_now().await;
        assert_eq!(m.list()[0].status.health, Health::Connected, "restarted");

        assert_eq!(
            m.update(&x, &id, Settings::new().with("pass", Some("x"))),
            Err(ChangeError::Invalid("User is required".into()))
        );
        assert_eq!(
            m.update(&x, "nope", Settings::new()),
            Err(ChangeError::NotFound)
        );
        assert_eq!(m.form("nope"), Err(ChangeError::NotFound));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn a_command_line_one_moves_into_the_app() {
        let path = temp_dir("adopt").join("settings.json");
        let x = Server::new(MachineConfig::default());
        let m = x.integrations();
        let cli = login("me@example.com", "secret");
        m.start_cli(&x, &CLOUD, &cli).unwrap();
        // One with a setting that is not on the form (a test server).
        let test = cli.clone().with("base_url", Some("http://127.0.0.1:9"));
        m.start_cli(&x, &CLOUD, &test).unwrap();
        m.load(&x, open(&path), &[&CLOUD]).unwrap();
        assert!(m.list()[0].adoptable);
        assert!(matches!(m.adopt("cloud-2"), Err(ChangeError::Invalid(_))));
        assert_eq!(m.list()[1].source, Source::CommandLine);
        assert_eq!(m.form("cloud"), Err(ChangeError::CommandLine));
        assert_eq!(
            m.update(&x, "cloud", cli.clone()),
            Err(ChangeError::CommandLine)
        );

        let view = m.adopt("cloud").unwrap();
        assert_eq!(
            (view.status.id.as_str(), view.source),
            ("cloud", Source::App)
        );
        assert!(!view.adoptable);
        assert!(view.from_command_line);
        assert_eq!(m.adopt("cloud").err(), Some(ChangeError::InApp));
        assert_eq!(m.adopt("nope").err(), Some(ChangeError::NotFound));
        let data = open(&path).data();
        let saved = data.moved("cloud").expect("marked as moved");
        assert_eq!(
            (saved.id.as_str(), saved.settings.secret("pass")),
            ("cloud", Some("secret"))
        );
        assert_eq!(m.form("cloud").unwrap().secrets_set, ["pass"]);

        // Still moved after an edit.
        let edit = Settings::new().with("user", Some("you@example.com"));
        m.update(&x, "cloud", edit).unwrap();
        assert!(open(&path).data().moved("cloud").is_some());

        // Deleted in the app: the mark goes with it, so the flags apply
        // again at the next start.
        m.remove("cloud").unwrap();
        let data = open(&path).data();
        assert!(data.integrations.is_empty());
        assert!(data.moved("cloud").is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
