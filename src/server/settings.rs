//! Settings people change in the app (`PUT /api/settings`): how long the
//! machine waits for a shot, the address of an E65/E80 grinder, and whether
//! to look for newer releases (see `update_check.rs`). A value
//! changed in the app wins over its flag or environment variable from then
//! on, because it is saved in the settings file (`--config`). Without a
//! settings file it lasts until the next restart. `null` forgets the app's
//! value, so the start setting counts again: the flag or environment
//! variable, else the default.
//!
//! The settings file also keeps the integrations added in the app (see
//! [`crate::integration::Integrations`]); [`SettingsFile`] is the one place
//! that writes it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};
use tokio::task::AbortHandle;

use crate::integration::Settings;
use crate::protocol::AbortCause;
use crate::server::wifi_grinder::{PairingLock, grinder_address};
use crate::server::{Server, WifiConfig, WifiGrinder};

/// The shot window unless set (`--shot-window-s`).
pub const DEFAULT_SHOT_WINDOW_S: u32 = 180;

/// The shortest shot window the app can set; 0 (no limit) is allowed too.
pub const MIN_SHOT_WINDOW_S: u32 = 30;

/// The longest shot window, an hour, as for `--shot-window-s`.
pub const MAX_SHOT_WINDOW_S: u32 = 3600;

// ---------------------------------------------------------------------------
// The settings file
// ---------------------------------------------------------------------------

/// The settings file (`--config`): read once at startup, then written on
/// every change. Each change is written next to it and renamed into place,
/// so a crash never leaves half a file, and it is readable by the owner only
/// because it holds passwords.
pub struct SettingsFile {
    path: PathBuf,
    data: Mutex<FileData>,
}

/// What the settings file holds. Older files have only `integrations`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileData {
    /// Integrations added in the app (with their passwords).
    #[serde(default)]
    pub integrations: Vec<SavedIntegration>,
    /// Settings changed in the app.
    #[serde(default, skip_serializing_if = "AppSettings::is_empty")]
    pub settings: AppSettings,
    /// Keys this version does not know (written by a newer one), kept.
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// One integration added in the app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedIntegration {
    pub id: String,
    pub kind: String,
    pub settings: Settings,
    /// Moved here from the command line: while it is here, the flags and
    /// environment variables of its kind are ignored.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub from_command_line: bool,
}

impl FileData {
    /// The integration moved into the app from the flags of `kind`, if it
    /// is still there.
    pub fn moved(&self, kind: &str) -> Option<&SavedIntegration> {
        self.integrations
            .iter()
            .find(|s| s.from_command_line && s.kind == kind)
    }
}

impl SettingsFile {
    /// Reads `path` (a missing file counts as empty) and writes it back right
    /// away, so a directory that is not writable fails at startup rather than
    /// on the first change.
    pub fn open(path: &Path) -> anyhow::Result<Arc<Self>> {
        make_private(path);
        let data: FileData = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("{}: not a settings file", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileData::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let file = Self {
            path: path.to_owned(),
            data: Mutex::new(FileData::default()),
        };
        file.update(|d| *d = data)?;
        Ok(Arc::new(file))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the file holds now.
    pub fn data(&self) -> FileData {
        self.lock().clone()
    }

    /// Changes the file: `change` edits a copy, which is written and only
    /// then kept. When writing fails, nothing changed.
    pub fn update(&self, change: impl FnOnce(&mut FileData)) -> anyhow::Result<()> {
        let mut data = self.lock();
        let mut next = data.clone();
        change(&mut next);
        let json = serde_json::to_vec_pretty(&next)?;
        save_private(&self.path, &json)?;
        *data = next;
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, FileData> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Readable by the owner only: the settings file holds passwords, the pairing
/// file what lets anyone speak to the grinder as gbs-anywhere, the history
/// when someone is home. The mode is set on the open file too, because a
/// file that is already there (say, left over from a crash) keeps its own.
/// That is best effort: some shares (SMB, NFS that maps the owner) refuse a
/// change of mode, and the file is still written there, as before;
/// [`make_private`] warns about it at startup.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = f.set_permissions(std::fs::Permissions::from_mode(0o600)) {
            tracing::debug!(
                "{}: cannot make it readable by its owner only: {e}",
                path.display()
            );
        }
    }
    f.write_all(bytes)?;
    f.sync_all()
}

/// Replaces the file at `path` with `bytes`, readable by the owner only
/// ([`write_private`]). It is written next to it as `<path>.tmp` and renamed
/// into place, so a crash never leaves half a file; on unix the directory is
/// synced too, so the rename outlasts a power cut. The `.tmp` name is fixed,
/// so each file needs a single writer.
pub(crate) fn save_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    write_private(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        // Best effort: some file systems (network shares) cannot sync a
        // directory, and the file itself is written by now.
        let dir = match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        if let Err(e) = std::fs::File::open(dir).and_then(|d| d.sync_all()) {
            tracing::debug!("{}: cannot sync its directory: {e}", path.display());
        }
    }
    Ok(())
}

/// Makes a file [`write_private`] keeps readable by its owner only if others
/// may read it (an older version may have left it so), and says so in the
/// log. Run at startup; a missing file is fine, and a failure is only a
/// warning.
pub(crate) fn make_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = match std::fs::metadata(path) {
            Ok(meta) => meta.permissions().mode(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::warn!("{}: cannot check who may read it: {e}", path.display());
                return;
            }
        };
        if mode & 0o077 == 0 {
            return;
        }
        // Checked again afterwards: some file systems (SMB shares, a Windows
        // folder under WSL or Docker Desktop) take the change without
        // keeping it.
        let kept = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .and_then(|()| std::fs::metadata(path))
            .map(|meta| meta.permissions().mode() & 0o077 == 0);
        match kept {
            Ok(true) => tracing::info!(
                "{}: others could read it; made it readable by its owner only",
                path.display()
            ),
            Ok(false) => tracing::warn!(
                "{}: others may read it, and its file system does not keep it readable by its \
                 owner only",
                path.display()
            ),
            Err(e) => tracing::warn!(
                "{}: others may read it, and making it readable by its owner only failed: {e}",
                path.display()
            ),
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

// ---------------------------------------------------------------------------
// The settings
// ---------------------------------------------------------------------------

/// Settings changed in the app, as saved. A missing value was not changed
/// there.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    /// Seconds the machine waits for a shot; 0 waits forever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shot_window_s: Option<u32>,
    /// The E65/E80 grinder's address; `Some(None)` when it was removed in
    /// the app over a start address (`""` in the API, `null` in the file).
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub grinder_host: Option<Option<String>>,
    /// Whether to look for a newer release once a day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_check: Option<bool>,
    /// Settings this version does not know (written by a newer one), kept.
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

impl AppSettings {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A key that is there, even as `null`, is `Some`.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

/// What the flags and environment variables say.
#[derive(Debug, Clone, Default)]
pub struct Startup {
    /// `--shot-window-s`, when given.
    pub shot_window_s: Option<f64>,
    /// `--grinder` (`GRINDER_HOST`), when given.
    pub grinder_host: Option<String>,
    /// Where E65/E80 links keep their pairing: `--grinder-pairing-file`, or
    /// next to the settings file. It stays the same when the address
    /// changes, so the pairing does too.
    pub pairing_file: Option<PathBuf>,
    /// `--grinder-pairing-id`.
    pub pairing_id: Option<u32>,
    /// `--update-check` (`UPDATE_CHECK`), when given.
    pub update_check: Option<bool>,
}

/// Where a setting's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingSource {
    Default,
    CommandLine,
    App,
}

/// A change from `PUT /api/settings`: only the keys that were sent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsChange {
    /// `Some(None)` goes back to the start setting (`null`).
    pub shot_window_s: Option<Option<u32>>,
    pub grinder_host: Option<HostChange>,
    /// `Some(None)` goes back to the start setting (`null`).
    pub update_check: Option<Option<bool>>,
}

/// A new grinder address from the app.
#[derive(Debug, Clone, PartialEq)]
pub enum HostChange {
    /// Back to `--grinder` / `GRINDER_HOST`, if set (`null`): the app's
    /// value is forgotten.
    Reset,
    /// No E65/E80 grinder, whatever the flags say (`""`). Without a start
    /// address it is the same as [`HostChange::Reset`].
    Remove,
    Set(String),
}

/// Whether the app can set this shot window.
fn window_ok(secs: u32) -> bool {
    secs == 0 || (MIN_SHOT_WINDOW_S..=MAX_SHOT_WINDOW_S).contains(&secs)
}

impl SettingsChange {
    /// Checks a request body; the error is a sentence for the person.
    pub fn parse(body: &Value) -> Result<Self, String> {
        let Value::Object(obj) = body else {
            return Err("expected a JSON object with the settings to change".into());
        };
        let mut change = Self::default();
        for (key, value) in obj {
            match key.as_str() {
                "shot_window_s" => {
                    // `null`: back to `--shot-window-s`, else the default.
                    let secs = match value {
                        Value::Null => None,
                        v => Some(
                            v.as_f64()
                                .filter(|s| {
                                    s.fract() == 0.0
                                        && (0.0..=f64::from(MAX_SHOT_WINDOW_S)).contains(s)
                                })
                                .map(|s| s as u32)
                                .filter(|s| window_ok(*s))
                                .ok_or_else(|| {
                                    format!(
                                        "the shot window is 0 (no limit), {MIN_SHOT_WINDOW_S} to \
                                         {MAX_SHOT_WINDOW_S} whole seconds, or null for the start \
                                         setting"
                                    )
                                })?,
                        ),
                    };
                    change.shot_window_s = Some(secs);
                }
                "grinder_host" => {
                    change.grinder_host = Some(match value {
                        Value::Null => HostChange::Reset,
                        Value::String(s) if s.trim().is_empty() => HostChange::Remove,
                        Value::String(s) => {
                            HostChange::Set(grinder_address(s).map_err(|e| format!("{e:#}"))?)
                        }
                        _ => {
                            return Err("the grinder address is text, e.g. 192.168.1.30, \"\" \
                                        for none, or null for the start setting"
                                .into());
                        }
                    });
                }
                "update_check" => {
                    // `null`: back to `--update-check`, else on.
                    let on = match value {
                        Value::Null => None,
                        v => Some(v.as_bool().ok_or(
                            "the update check is true (on), false (off), or null for the start \
                             setting",
                        )?),
                    };
                    change.update_check = Some(on);
                }
                other => return Err(format!("unknown setting `{other}`")),
            }
        }
        Ok(change)
    }
}

/// Why a change was not made. Nothing changed in either case.
#[derive(Debug, PartialEq)]
pub enum SettingsError {
    /// The settings file could not be written.
    Save(String),
    /// The link to the new grinder address could not be set up.
    Grinder(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Save(e) => write!(f, "could not save the settings: {e}"),
            Self::Grinder(e) => write!(f, "could not set up the grinder link: {e}"),
        }
    }
}

impl std::error::Error for SettingsError {}

impl SettingsError {
    /// As a fixed code, for `code` in the API's errors.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Save(_) => "settings_not_saved",
            Self::Grinder(_) => "grinder_link_failed",
        }
    }
}

/// The settings as they are now, and the E65/E80 link they started.
#[derive(Default)]
pub(super) struct Live {
    startup: Startup,
    app: AppSettings,
    file: Option<Arc<SettingsFile>>,
    link: Option<RunningLink>,
}

struct RunningLink {
    task: AbortHandle,
}

/// The settings as read from the file, keeping only values the app could
/// have set: one edited by hand out of range is ignored with a warning, so
/// the flags count again. A value of the wrong type (`"300"`) still stops
/// startup, like any other broken settings file.
fn checked(mut app: AppSettings) -> AppSettings {
    if let Some(s) = app.shot_window_s
        && !window_ok(s)
    {
        tracing::warn!(
            "settings: ignoring the shot window in the settings file ({s} s): it is 0 (no \
             limit) or {MIN_SHOT_WINDOW_S} to {MAX_SHOT_WINDOW_S} seconds"
        );
        app.shot_window_s = None;
    }
    if let Some(Some(host)) = &app.grinder_host
        && let Err(e) = grinder_address(host)
    {
        tracing::warn!("settings: ignoring the grinder address in the settings file: {e:#}");
        app.grinder_host = None;
    }
    app
}

/// The shot window in seconds and where it comes from.
fn shot_window(startup: &Startup, app: &AppSettings) -> (u32, SettingSource) {
    match (app.shot_window_s, startup.shot_window_s) {
        (Some(s), _) => (s, SettingSource::App),
        (None, Some(s)) => (whole_seconds(s), SettingSource::CommandLine),
        (None, None) => (DEFAULT_SHOT_WINDOW_S, SettingSource::Default),
    }
}

/// `--shot-window-s` in whole seconds, as the app shows it; a limit stays
/// a limit (0.4 s is 1 s, not "forever").
fn whole_seconds(secs: f64) -> u32 {
    if secs > 0.0 {
        // The flag is at most an hour.
        (secs.round() as u32).max(1)
    } else {
        0
    }
}

/// The E65/E80 grinder's address and where it comes from.
fn grinder_host(startup: &Startup, app: &AppSettings) -> (Option<String>, SettingSource) {
    match (&app.grinder_host, &startup.grinder_host) {
        (Some(h), _) => (h.clone(), SettingSource::App),
        (None, Some(h)) => (Some(h.clone()), SettingSource::CommandLine),
        (None, None) => (None, SettingSource::Default),
    }
}

/// Whether to look for newer releases (on unless set) and where that comes
/// from.
fn update_check(startup: &Startup, app: &AppSettings) -> (bool, SettingSource) {
    match (app.update_check, startup.update_check) {
        (Some(on), _) => (on, SettingSource::App),
        (None, Some(on)) => (on, SettingSource::CommandLine),
        (None, None) => (true, SettingSource::Default),
    }
}

/// The machine's limit for a shot window of `secs` (0: none).
fn window_limit(secs: u32) -> Option<Duration> {
    (secs > 0).then(|| Duration::from_secs(secs.into()))
}

/// Why a value changed, for the log.
fn changed_by(from: SettingSource) -> &'static str {
    match from {
        SettingSource::App => "changed in the app",
        _ => "the start setting again",
    }
}

impl Live {
    fn json(&self) -> Value {
        let (window, window_from) = shot_window(&self.startup, &self.app);
        let (host, host_from) = grinder_host(&self.startup, &self.app);
        let (update, update_from) = update_check(&self.startup, &self.app);
        // What `null` goes back to: the flags, else the defaults.
        let start = AppSettings::default();
        json!({
            "shot_window_s": window,
            "grinder_host": host,
            "update_check": update,
            "source": {
                "shot_window_s": window_from,
                "grinder_host": host_from,
                "update_check": update_from,
            },
            "start": {
                "shot_window_s": shot_window(&self.startup, &start).0,
                "grinder_host": grinder_host(&self.startup, &start).0,
                "update_check": update_check(&self.startup, &start).0,
            },
            // What the app can set, besides 0 (no limit).
            "shot_window_range": {
                "min_s": MIN_SHOT_WINDOW_S,
                "max_s": MAX_SHOT_WINDOW_S,
            },
            "persistent": self.file.is_some(),
        })
    }

    /// The link for `host`, ready to run; loads (or makes) the pairing.
    fn prepare_link(&self, host: &str, lock: PairingLock) -> anyhow::Result<WifiGrinder> {
        let mut cfg = WifiConfig::from_host(host)?;
        cfg.pairing_file = self.startup.pairing_file.clone();
        cfg.pairing_id = self.startup.pairing_id;
        WifiGrinder::new(cfg, lock)
    }
}

impl Server {
    /// Takes the flags and the settings file at startup: sets the shot
    /// window and starts the E65/E80 link when there is an address. Values
    /// changed in the app win.
    pub fn start_settings(
        self: &Arc<Self>,
        startup: Startup,
        file: Option<Arc<SettingsFile>>,
    ) -> anyhow::Result<()> {
        // A link opens the pairing file only when it starts, if one does.
        if let Some(path) = &startup.pairing_file {
            make_private(path);
        }
        let mut live = self.live();
        live.app = checked(file.as_ref().map(|f| f.data().settings).unwrap_or_default());
        live.startup = startup;
        live.file = file;
        let (window, from) = shot_window(&live.startup, &live.app);
        if from == SettingSource::App && live.startup.shot_window_s.is_some() {
            tracing::info!(
                "settings: the shot window set in the app ({window} s) is used instead of \
                 --shot-window-s"
            );
        }
        self.with(|m, _| m.set_shot_window(window_limit(window)));
        let (update, from) = update_check(&live.startup, &live.app);
        if from == SettingSource::App && live.startup.update_check.is_some() {
            tracing::info!(
                "settings: the update check set in the app ({}) is used instead of \
                 --update-check",
                if update { "on" } else { "off" }
            );
        }
        self.set_update_check(update);
        let (host, from) = grinder_host(&live.startup, &live.app);
        if from == SettingSource::App && live.startup.grinder_host.is_some() {
            if host.is_some() {
                tracing::info!(
                    "settings: the grinder address set in the app is used instead of --grinder"
                );
            } else {
                tracing::info!(
                    "settings: the grinder address was removed in the app; --grinder is ignored"
                );
            }
        }
        if let Some(host) = host {
            let lock = self.pairing_lock.clone();
            let link = live.prepare_link(&host, lock).with_context(|| match from {
                SettingSource::App => "the grinder address set in the app".to_owned(),
                _ => "--grinder".to_owned(),
            })?;
            self.run_link(&mut live, link);
        }
        Ok(())
    }

    /// `settings` in `GET /api/state`, and the reply to `PUT /api/settings`.
    pub fn settings_json(&self) -> Value {
        self.live().json()
    }

    /// Applies a change from the app and saves it. A value that goes back to
    /// the start setting is dropped from the file. A new grinder address
    /// restarts the E65/E80 link (the pairing stays where it was kept); no
    /// address stops it.
    pub fn change_settings(
        self: &Arc<Self>,
        change: SettingsChange,
    ) -> Result<Value, SettingsError> {
        // Held to the end, so changes apply one after the other. Writing the
        // small settings file under it is deliberate, as in
        // `Integrations::add`.
        let mut live = self.live();
        let mut app = live.app.clone();
        if let Some(secs) = change.shot_window_s {
            app.shot_window_s = secs;
        }
        if let Some(host) = change.grinder_host {
            app.grinder_host = match host {
                HostChange::Reset => None,
                // Removing stands in for the start address; without one the
                // app's address is only forgotten, so a GRINDER_HOST set
                // later counts. (The start settings don't change while it
                // runs.)
                HostChange::Remove if live.startup.grinder_host.is_none() => None,
                HostChange::Remove => Some(None),
                HostChange::Set(host) => Some(Some(host)),
            };
        }
        if let Some(on) = change.update_check {
            app.update_check = on;
        }
        let (old_host, old_host_from) = grinder_host(&live.startup, &live.app);
        let (new_host, host_from) = grinder_host(&live.startup, &app);
        let relink = old_host != new_host;
        let link = match &new_host {
            // The old link still runs here, and keeps the pairing file until
            // its task has ended (see `PairingLock`).
            Some(host) if relink => Some(
                live.prepare_link(host, self.pairing_lock.clone())
                    .map_err(|e| SettingsError::Grinder(format!("{e:#}")))?,
            ),
            _ => None,
        };
        if let Some(file) = &live.file {
            file.update(|d| d.settings = app.clone())
                .map_err(|e| SettingsError::Save(format!("{e:#}")))?;
        }
        let old_window = shot_window(&live.startup, &live.app);
        let old_update = update_check(&live.startup, &live.app);
        live.app = app;
        // Logged when the value or where it comes from changes.
        let update = update_check(&live.startup, &live.app);
        if update != old_update {
            tracing::info!(
                "settings: update check now {} ({})",
                if update.0 { "on" } else { "off" },
                changed_by(update.1)
            );
        }
        if update.0 != old_update.0 {
            self.set_update_check(update.0);
        }
        let window = shot_window(&live.startup, &live.app);
        if window != old_window {
            tracing::info!(
                "settings: shot window now {} s ({})",
                window.0,
                changed_by(window.1)
            );
        }
        if window.0 != old_window.0 {
            self.with(|m, _| m.set_shot_window(window_limit(window.0)));
        }
        // (With no address left, the link's line below says it.)
        if old_host_from == SettingSource::App
            && host_from != SettingSource::App
            && new_host.is_some()
        {
            tracing::info!("settings: grinder address is the start setting again");
        }
        if relink {
            self.stop_link(&mut live);
            match link {
                Some(link) => self.run_link(&mut live, link),
                None if host_from == SettingSource::App => tracing::info!(
                    "grinder: address removed in the app (instead of --grinder), link stopped"
                ),
                None => tracing::info!("grinder: no address any more, link stopped"),
            }
        }
        Ok(live.json())
    }

    fn run_link(self: &Arc<Self>, live: &mut Live, link: WifiGrinder) {
        if live.startup.pairing_file.is_none() {
            tracing::warn!(
                "no --config or --grinder-pairing-file: the pairing lasts until restart"
            );
        }
        tracing::info!("grinder: pairing over Local WiFi with {}", link.base_url());
        // Taken here, under the settings lock, rather than in the task: an
        // older link that is still finishing a step can then never take over
        // again, so only this one reports and gets the pairing IDs typed in
        // the app.
        let (owner, ids, accept) = self.attach_wifi();
        // Shown until the link reports for itself, so the app never sees an
        // address without a link.
        self.set_wifi(owner, link.connecting());
        let task = tokio::spawn(link.run(self.clone(), owner, ids, accept)).abort_handle();
        live.link = Some(RunningLink { task });
    }

    /// Stops the running link. A shot it was waiting for (a brew job) can no
    /// longer reach the grinder, so that brew ends; a knob press of an E64 WS
    /// on the same server goes on.
    fn stop_link(&self, live: &mut Live) {
        let Some(old) = live.link.take() else {
            return;
        };
        // The task ends at its next await; if it is in the middle of a step
        // right now, what it still reports is ignored once detached, and the
        // next link waits for it to end before it touches the pairing file.
        old.task.abort();
        self.detach_wifi();
        self.with(|m, now| {
            if m.brewing_for_brew_job(now) {
                let _ = m.give_up(now, AbortCause::GrinderAddressChanged);
            }
        });
    }

    fn live(&self) -> MutexGuard<'_, Live> {
        self.settings.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::MachineConfig;

    #[test]
    fn values_edited_into_what_the_app_refuses_are_ignored() {
        let bad = AppSettings {
            shot_window_s: Some(5),
            grinder_host: Some(Some("http://192.168.1.30/x".into())),
            ..AppSettings::default()
        };
        assert_eq!(checked(bad), AppSettings::default());
        let good = AppSettings {
            shot_window_s: Some(120),
            grinder_host: Some(Some("192.168.1.30".into())),
            update_check: Some(false),
            ..AppSettings::default()
        };
        assert_eq!(checked(good.clone()), good);
        let removed = AppSettings {
            shot_window_s: Some(0),
            grinder_host: Some(None),
            ..AppSettings::default()
        };
        assert_eq!(checked(removed.clone()), removed);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gbs-anywhere-settings-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn change(body: Value) -> Result<SettingsChange, String> {
        SettingsChange::parse(&body)
    }

    #[test]
    fn changes_are_checked() {
        assert_eq!(
            change(json!({ "shot_window_s": 120 })),
            Ok(SettingsChange {
                shot_window_s: Some(Some(120)),
                ..SettingsChange::default()
            })
        );
        for (sent, secs) in [(json!(0), 0), (json!(30), 30), (json!(3600.0), 3600)] {
            assert_eq!(
                change(json!({ "shot_window_s": sent }))
                    .unwrap()
                    .shot_window_s,
                Some(Some(secs)),
                "{sent}"
            );
        }
        for bad in [
            json!(29),
            json!(3601),
            json!(120.5),
            json!(-1),
            json!(-30),
            json!(1e20),
            json!("120"),
            json!(true),
        ] {
            assert!(change(json!({ "shot_window_s": bad })).is_err(), "{bad}");
        }
        // `null` goes back to the start setting; `""` removes the address.
        let c = change(json!({ "shot_window_s": null, "update_check": null })).unwrap();
        assert_eq!((c.shot_window_s, c.update_check), (Some(None), Some(None)));
        let c = change(json!({ "grinder_host": null })).unwrap();
        assert_eq!(c.grinder_host, Some(HostChange::Reset));
        for none in [json!(""), json!("  ")] {
            let c = change(json!({ "grinder_host": none })).unwrap();
            assert_eq!(c.grinder_host, Some(HostChange::Remove), "{none}");
        }
        let c = change(json!({ "grinder_host": " 192.168.1.30 " })).unwrap();
        assert_eq!(c.grinder_host, Some(HostChange::Set("192.168.1.30".into())));
        assert!(change(json!({ "grinder_host": 5 })).is_err());
        assert!(change(json!({ "grinder_host": "two words" })).is_err());
        let c = change(json!({ "update_check": false })).unwrap();
        assert_eq!(c.update_check, Some(Some(false)));
        assert!(change(json!({ "update_check": 0 })).is_err());
        assert_eq!(
            change(json!({ "brew": 1 })),
            Err("unknown setting `brew`".into())
        );
        assert!(change(json!([1])).is_err());
        assert_eq!(change(json!({})), Ok(SettingsChange::default()));
    }

    #[test]
    fn a_flag_in_fractions_shows_in_whole_seconds() {
        assert_eq!(whole_seconds(90.4), 90);
        assert_eq!(whole_seconds(90.5), 91);
        assert_eq!(whole_seconds(0.4), 1, "a limit stays a limit");
        assert_eq!(whole_seconds(0.0), 0);
        let startup = Startup {
            shot_window_s: Some(150.6),
            ..Startup::default()
        };
        let (secs, from) = shot_window(&startup, &AppSettings::default());
        assert_eq!((secs, from), (151, SettingSource::CommandLine));
        assert_eq!(window_limit(secs), Some(Duration::from_secs(151)));
    }

    #[test]
    fn the_file_keeps_what_it_does_not_know() {
        let dir = temp_dir("keep");
        let path = dir.join("settings.json");
        // A file from an older version, plus a key from a newer one.
        let old = json!({
            "integrations": [{ "id": "a", "kind": "scale", "settings": { "host": "x" } }],
            "future": { "x": 1 },
        });
        std::fs::write(&path, old.to_string()).unwrap();
        let file = SettingsFile::open(&path).unwrap();
        assert_eq!(file.data().integrations[0].id, "a");
        assert!(file.data().settings.is_empty());
        file.update(|d| {
            d.settings.shot_window_s = Some(120);
            d.settings.grinder_host = Some(None);
        })
        .unwrap();
        let text: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(text["future"], json!({ "x": 1 }));
        assert_eq!(
            text["settings"],
            json!({ "shot_window_s": 120, "grinder_host": null })
        );
        assert_eq!(text["integrations"][0]["settings"]["host"], "x");
        assert!(
            text["integrations"][0].get("from_command_line").is_none(),
            "left out unless moved"
        );

        let again = SettingsFile::open(&path).unwrap();
        assert_eq!(
            again.data().settings.grinder_host,
            Some(None),
            "null stays null"
        );

        // A setting from a newer version survives a rewrite, also when the
        // ones this version knows are gone.
        let newer = json!({
            "integrations": [],
            "settings": { "shot_window_s": 90, "future_setting": { "x": 1 } },
        });
        std::fs::write(&path, newer.to_string()).unwrap();
        let file = SettingsFile::open(&path).unwrap();
        file.update(|d| d.settings.shot_window_s = Some(120))
            .unwrap();
        let text: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            text["settings"],
            json!({ "shot_window_s": 120, "future_setting": { "x": 1 } })
        );
        file.update(|d| d.settings.shot_window_s = None).unwrap();
        let text: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(text["settings"], json!({ "future_setting": { "x": 1 } }));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "owner only");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_write_changes_nothing() {
        let dir = temp_dir("fail");
        let path = dir.join("settings.json");
        let file = SettingsFile::open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(
            file.update(|d| d.settings.shot_window_s = Some(120))
                .is_err()
        );
        assert!(file.data().settings.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_save_leaves_no_file_others_can_read() {
        let dir = temp_dir("mode");
        let path = dir.join("settings.json");
        let file = SettingsFile::open(&path).unwrap();
        // One left over from a crash, as others could read it, and the file
        // as an older version left it.
        let tmp = dir.join("settings.json.tmp");
        std::fs::write(&tmp, "{}").unwrap();
        set_mode(&tmp, 0o644);
        set_mode(&path, 0o644);
        file.update(|d| d.settings.shot_window_s = Some(120))
            .unwrap();
        assert_eq!(mode(&path), 0o600);
        // Written in place too.
        set_mode(&path, 0o644);
        write_private(&path, b"{}").unwrap();
        assert_eq!(mode(&path), 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn files_others_can_read_are_made_owner_only_at_startup() {
        let dir = temp_dir("startup");
        let settings = dir.join("settings.json");
        let history = dir.join("history.json");
        let pairing = dir.join("grinder-pairing.json");
        let saved_pairing = r#"{"brew_device_id":"x","pairing_id":null}"#;
        std::fs::write(&settings, "{}").unwrap();
        std::fs::write(&history, r#"{"shots":[]}"#).unwrap();
        std::fs::write(&pairing, saved_pairing).unwrap();
        for f in [&settings, &history, &pairing] {
            set_mode(f, 0o644);
        }
        // As the binary starts, with no grinder address: no link opens the
        // pairing file.
        let x = Server::new(MachineConfig::default());
        let file = SettingsFile::open(&settings).unwrap();
        x.record_history(Some(&history)).unwrap();
        let startup = Startup {
            pairing_file: Some(pairing.clone()),
            ..Startup::default()
        };
        x.start_settings(startup, Some(file)).unwrap();
        for f in [&settings, &history, &pairing] {
            assert_eq!(mode(f), 0o600, "{}", f.display());
        }
        assert_eq!(std::fs::read_to_string(&pairing).unwrap(), saved_pairing);

        // Only ever closer: a file only its owner may read stays as it is,
        // and a missing one is fine.
        set_mode(&pairing, 0o400);
        make_private(&pairing);
        assert_eq!(mode(&pairing), 0o400);
        make_private(&dir.join("missing.json"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn app_values_win_over_the_command_line() {
        let dir = temp_dir("win");
        let path = dir.join("settings.json");
        let x = Server::new(MachineConfig::default());
        let startup = Startup {
            shot_window_s: Some(150.0),
            ..Startup::default()
        };
        x.start_settings(startup.clone(), Some(SettingsFile::open(&path).unwrap()))
            .unwrap();
        let s = x.settings_json();
        assert_eq!(s["shot_window_s"], 150);
        assert_eq!(s["source"]["shot_window_s"], "command_line");
        assert_eq!(s["grinder_host"], Value::Null);
        assert_eq!(s["source"]["grinder_host"], "default");
        assert_eq!(s["persistent"], true);

        let s = x
            .change_settings(SettingsChange {
                shot_window_s: Some(Some(90)),
                ..SettingsChange::default()
            })
            .unwrap();
        assert_eq!(s["shot_window_s"], 90);
        assert_eq!(s["source"]["shot_window_s"], "app");
        assert_eq!(s["start"]["shot_window_s"], 150, "the flag");
        let limit = x.with(|m, _| m.config().shot_window);
        assert_eq!(limit, Some(Duration::from_secs(90)));

        // Next start, same flags: the app's value is used.
        let y = Server::new(MachineConfig::default());
        y.start_settings(startup, Some(SettingsFile::open(&path).unwrap()))
            .unwrap();
        assert_eq!(y.settings_json()["shot_window_s"], 90);
        assert_eq!(
            y.with(|m, _| m.config().shot_window),
            Some(Duration::from_secs(90))
        );

        // No limit.
        y.change_settings(SettingsChange {
            shot_window_s: Some(Some(0)),
            ..SettingsChange::default()
        })
        .unwrap();
        assert_eq!(y.with(|m, _| m.config().shot_window), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn null_goes_back_to_the_start_setting() {
        let dir = temp_dir("reset");
        let path = dir.join("settings.json");
        let flags = Startup {
            shot_window_s: Some(150.0),
            update_check: Some(false),
            ..Startup::default()
        };
        let x = Server::new(MachineConfig::default());
        x.start_settings(flags.clone(), Some(SettingsFile::open(&path).unwrap()))
            .unwrap();
        let both = |window: Value, update: Value| {
            change(json!({ "shot_window_s": window, "update_check": update })).unwrap()
        };
        let s = x.change_settings(both(json!(90), json!(true))).unwrap();
        assert_eq!(s["source"]["shot_window_s"], "app");
        assert_eq!(s["source"]["update_check"], "app");
        assert_eq!(
            s["start"],
            json!({ "shot_window_s": 150, "grinder_host": null, "update_check": false })
        );

        let s = x.change_settings(both(Value::Null, Value::Null)).unwrap();
        assert_eq!(s["shot_window_s"], 150);
        assert_eq!(s["source"]["shot_window_s"], "command_line");
        assert_eq!(s["update_check"], false);
        assert_eq!(s["source"]["update_check"], "command_line");
        assert_eq!(
            x.with(|m, _| m.config().shot_window),
            Some(Duration::from_secs(150))
        );
        assert!(!x.update_check_on());
        let text: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(text.get("settings").is_none(), "forgotten: {text}");

        // The flags count after a restart too, also once they change.
        let y = Server::new(MachineConfig::default());
        let flags = Startup {
            shot_window_s: Some(200.0),
            ..flags
        };
        y.start_settings(flags, Some(SettingsFile::open(&path).unwrap()))
            .unwrap();
        assert_eq!(y.settings_json()["shot_window_s"], 200);

        // Without flags, the defaults.
        let z = Server::new(MachineConfig::default());
        z.start_settings(Startup::default(), None).unwrap();
        z.change_settings(both(json!(0), json!(false))).unwrap();
        let s = z.change_settings(both(Value::Null, Value::Null)).unwrap();
        assert_eq!(s["shot_window_s"], DEFAULT_SHOT_WINDOW_S);
        assert_eq!(s["source"]["shot_window_s"], "default");
        assert_eq!(s["update_check"], true);
        assert_eq!(s["source"]["update_check"], "default");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn an_empty_address_without_a_start_address_is_forgotten() {
        let dir = temp_dir("empty");
        let path = dir.join("settings.json");
        let x = Server::new(MachineConfig::default());
        x.start_settings(Startup::default(), Some(SettingsFile::open(&path).unwrap()))
            .unwrap();
        let s = x
            .change_settings(change(json!({ "grinder_host": "" })).unwrap())
            .unwrap();
        assert_eq!(s["grinder_host"], Value::Null);
        assert_eq!(
            s["source"]["grinder_host"], "default",
            "nothing to stand in for"
        );
        let text: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(text.get("settings").is_none(), "no removal saved: {text}");

        // So a GRINDER_HOST set later counts.
        let y = Server::new(MachineConfig::default());
        let flag = Startup {
            grinder_host: Some("127.0.0.1:9".into()),
            ..Startup::default()
        };
        y.start_settings(flag, Some(SettingsFile::open(&path).unwrap()))
            .unwrap();
        let s = y.settings_json();
        assert_eq!(s["grinder_host"], "127.0.0.1:9");
        assert_eq!(s["source"]["grinder_host"], "command_line");

        // With one, `""` stands in for it.
        let s = y
            .change_settings(change(json!({ "grinder_host": "" })).unwrap())
            .unwrap();
        assert_eq!(s["grinder_host"], Value::Null);
        assert_eq!(s["source"]["grinder_host"], "app");
        let text: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(text["settings"], json!({ "grinder_host": null }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn an_address_removed_in_an_older_file_stays_removed() {
        // Before `null` went back to the start setting, it removed the
        // address, saved as `null`; such a file still means "removed".
        let dir = temp_dir("removed");
        let path = dir.join("settings.json");
        let old = json!({ "integrations": [], "settings": { "grinder_host": null } });
        std::fs::write(&path, old.to_string()).unwrap();
        let x = Server::new(MachineConfig::default());
        let flags = Startup {
            grinder_host: Some("192.168.1.30".into()),
            ..Startup::default()
        };
        x.start_settings(flags, Some(SettingsFile::open(&path).unwrap()))
            .unwrap();
        let s = x.settings_json();
        assert_eq!(s["grinder_host"], Value::Null);
        assert_eq!(s["source"]["grinder_host"], "app");
        assert_eq!(s["start"]["grinder_host"], "192.168.1.30");
        assert!(x.grinder_link().wifi.is_none(), "no link started");
        let text: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(text["settings"], json!({ "grinder_host": null }), "kept");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn the_update_check_is_on_unless_turned_off() {
        let x = Server::new(MachineConfig::default());
        x.start_settings(Startup::default(), None).unwrap();
        let s = x.settings_json();
        assert_eq!(s["update_check"], true);
        assert_eq!(s["source"]["update_check"], "default");
        assert!(x.update_check_on());

        // Off by the flag, then on again in the app.
        let x = Server::new(MachineConfig::default());
        let off = Startup {
            update_check: Some(false),
            ..Startup::default()
        };
        x.start_settings(off, None).unwrap();
        assert!(!x.update_check_on());
        assert_eq!(x.settings_json()["source"]["update_check"], "command_line");
        let s = x
            .change_settings(change(json!({ "update_check": true })).unwrap())
            .unwrap();
        assert_eq!(s["update_check"], true);
        assert_eq!(s["source"]["update_check"], "app");
        assert!(x.update_check_on());
        assert!(change(json!({ "update_check": "off" })).is_err());
    }

    #[tokio::test]
    async fn without_a_file_changes_last_until_restart() {
        let x = Server::new(MachineConfig::default());
        x.start_settings(Startup::default(), None).unwrap();
        let s = x
            .change_settings(SettingsChange {
                shot_window_s: Some(Some(120)),
                ..SettingsChange::default()
            })
            .unwrap();
        assert_eq!(s["persistent"], false);
        assert_eq!(s["shot_window_s"], 120);
    }

    #[tokio::test]
    async fn a_failed_save_keeps_the_settings() {
        let dir = temp_dir("save");
        let path = dir.join("settings.json");
        let x = Server::new(MachineConfig::default());
        x.start_settings(Startup::default(), Some(SettingsFile::open(&path).unwrap()))
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let e = x.change_settings(SettingsChange {
            shot_window_s: Some(Some(90)),
            grinder_host: Some(HostChange::Set("127.0.0.1:9".into())),
            update_check: Some(Some(false)),
        });
        assert!(matches!(e, Err(SettingsError::Save(_))), "{e:?}");
        assert_eq!(x.settings_json()["shot_window_s"], DEFAULT_SHOT_WINDOW_S);
        assert_eq!(x.settings_json()["update_check"], true);
        assert!(x.grinder_link().wifi.is_none(), "no link started");
        let _ = std::fs::remove_dir_all(dir);
    }
}
