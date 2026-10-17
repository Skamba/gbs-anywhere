//! The integrations that are running, whether started from the command line
//! or added in the app, and how the app's ones are kept in the settings file
//! across restarts.

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
}

/// Why adding or removing failed.
#[derive(Debug, PartialEq)]
pub enum ChangeError {
    /// The settings do not work for this integration.
    Invalid(String),
    NotFound,
    /// Set on the command line, so it can't be removed from the app.
    CommandLine,
    /// The settings file could not be written; nothing changed.
    Save(String),
}

impl std::fmt::Display for ChangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(e) => f.write_str(e),
            Self::NotFound => f.write_str("no such integration"),
            Self::CommandLine => {
                f.write_str("set on the command line: remove its flags to turn it off")
            }
            Self::Save(e) => write!(f, "could not save the settings: {e}"),
        }
    }
}

impl std::error::Error for ChangeError {}

struct Entry {
    status: Status,
    source: Source,
    /// What goes into the settings file (app entries only).
    saved: Option<SavedIntegration>,
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
        self.lock()
            .iter()
            .map(|e| View {
                status: e.status.snapshot(),
                source: e.source,
            })
            .collect()
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
            saved: None,
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
            saved: Some(SavedIntegration {
                id: id.clone(),
                kind: kind.id.to_owned(),
                settings,
            }),
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
                saved: Some(saved),
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

    fn host(h: &str) -> Settings {
        Settings::new().with("host", Some(h))
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
        m.load(&x, open(&path), &[&SCALE]).unwrap();
        // Replace the file with a directory: renaming onto it fails.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(matches!(
            m.add(&x, &SCALE, host("a")),
            Err(ChangeError::Save(_))
        ));
        assert!(ids(&x).is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
