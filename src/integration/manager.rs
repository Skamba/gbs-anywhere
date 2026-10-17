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
pub enum Source {
    /// Flags or environment variables; only those can turn it off.
    CommandLine,
    /// Added in the app; can be removed there.
    App,
}

/// An integration as the API shows it.
#[derive(Debug, Clone, Serialize)]
pub struct View {
    #[serde(flatten)]
    pub status: StatusSnapshot,
    pub source: Source,
    /// Set on the command line and can be moved into the app (there is a
    /// settings file to keep it in).
    pub adoptable: bool,
}

/// The settings of an integration added in the app, as the app may see them
/// to edit it: no passwords, only which of them are set.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FormView {
    pub kind: String,
    pub settings: BTreeMap<String, String>,
    pub secrets_set: Vec<String>,
}

/// Why a change failed. Nothing changed in any case.
#[derive(Debug, PartialEq)]
pub enum ChangeError {
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
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list(&self) -> Vec<View> {
        let persistent = self.persistent();
        self.lock().iter().map(|e| view(e, persistent)).collect()
    }

    /// Whether integrations added in the app survive a restart.
    pub fn persistent(&self) -> bool {
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
            "{}: enabled, shots are reported automatically after a knob press",
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
    pub fn add(
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
    pub fn remove(&self, id: &str) -> Result<(), ChangeError> {
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
    pub fn form(&self, id: &str) -> Result<FormView, ChangeError> {
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
    pub fn update(
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

    /// Moves one set on the command line into the app: its settings (the
    /// form's fields) are saved as an app entry with the same id, and its
    /// flags are ignored from the next start on. It keeps running as it is.
    pub fn adopt(&self, id: &str) -> Result<View, ChangeError> {
        let file = self.file.get().ok_or(ChangeError::NotPersistent)?;
        let mut entries = self.lock();
        let i = entries
            .iter()
            .position(|e| e.id() == id)
            .ok_or(ChangeError::NotFound)?;
        let entry = &entries[i];
        let (Source::CommandLine, Some(kind), Some(cli)) = (entry.source, entry.kind, &entry.cli)
        else {
            return Err(ChangeError::InApp);
        };
        let settings = kind.form_only(cli);
        kind.create(&settings, true)
            .map_err(|e| ChangeError::Invalid(format!("{e:#}")))?;
        let saved = SavedIntegration {
            id: id.to_owned(),
            kind: kind.id.to_owned(),
            settings,
        };
        entries[i].source = Source::App;
        entries[i].saved = Some(saved);
        let list = saved_list(&entries);
        let saved = file.update(|d| {
            d.integrations = list;
            if !d.adopted.iter().any(|k| k == kind.id) {
                d.adopted.push(kind.id.to_owned());
            }
        });
        if let Err(e) = saved {
            entries[i].source = Source::CommandLine;
            entries[i].saved = None;
            return Err(ChangeError::Save(format!("{e:#}")));
        }
        tracing::info!(
            "{}: moved into the app ({id}); its command-line settings are ignored from the next \
             start on and can be removed",
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
    }
}

/// The app's entries, as the settings file keeps them.
fn saved_list(entries: &[Entry]) -> Vec<SavedIntegration> {
    entries.iter().filter_map(|e| e.saved.clone()).collect()
}

fn spawn(server: &Arc<Server>, integration: Box<dyn Integration>, status: Status) -> AbortHandle {
    // Integrations that speak TLS share rustls; make sure a provider is set
    // before the first one connects (reqwest leaves it to the application).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let link = Link {
        server: server.clone(),
        status: status.clone(),
    };
    tokio::spawn(async move {
        integration.run(link).await;
        status.stopped();
    })
    .abort_handle()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::{BoxFuture, Field, Health, Input};
    use crate::protocol::MachineConfig;
    use std::path::PathBuf;

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

    fn temp_file(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gbs-anywhere-test-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
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
        let path = temp_file("keep");
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
        assert_eq!(gone.status.health, Health::Error);
        assert!(std::fs::read_to_string(&path).unwrap().contains("gone"));
        y.integrations().remove("gone").unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("gone"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn a_failed_save_changes_nothing() {
        let path = temp_file("fail");
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
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn edit_keeps_the_saved_password() {
        let path = temp_file("edit");
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
        let path = temp_file("adopt");
        let x = Server::new(MachineConfig::default());
        let m = x.integrations();
        // The flags' settings include one that is not on the form.
        let cli = login("me@example.com", "secret").with("base_url", Some("http://127.0.0.1:9"));
        m.start_cli(&x, &CLOUD, &cli).unwrap();
        m.load(&x, open(&path), &[&CLOUD]).unwrap();
        assert!(m.list()[0].adoptable);
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
        assert_eq!(m.adopt("cloud").err(), Some(ChangeError::InApp));
        assert_eq!(m.adopt("nope").err(), Some(ChangeError::NotFound));
        let data = open(&path).data();
        assert_eq!(data.adopted, ["cloud"]);
        let saved = &data.integrations[0];
        assert_eq!((saved.id.as_str(), saved.kind.as_str()), ("cloud", "cloud"));
        assert_eq!(saved.settings.secret("pass"), Some("secret"));
        assert_eq!(saved.settings.text("base_url"), None, "form fields only");
        assert_eq!(m.form("cloud").unwrap().secrets_set, ["pass"]);

        // Removed in the app: it stays moved, so the flags do not bring it back.
        m.remove("cloud").unwrap();
        let data = open(&path).data();
        assert!(data.integrations.is_empty());
        assert_eq!(data.adopted, ["cloud"]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
