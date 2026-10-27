//! Files readable by their owner only: the settings file (passwords), the
//! E65/E80 pairing file (what lets anyone speak to the grinder as
//! gbs-anywhere), the history (when someone is home) and the grinder
//! transcript.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;

/// Writes `bytes` to `path`, readable by the owner only. The mode is set on
/// the open file too, because a file that is already there (say, left over
/// from a crash) keeps its own. That is best effort: some shares (SMB, NFS
/// that maps the owner) refuse a change of mode, and the file is still
/// written there, as before; [`make_private`] warns about it at startup.
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::test_support::{mode, set_mode, temp_dir};

    #[test]
    fn a_save_leaves_no_file_others_can_read() {
        let dir = temp_dir("private-mode");
        let path = dir.join("settings.json");
        save_private(&path, b"{}").unwrap();
        // One left over from a crash, as others could read it, and the file
        // as an older version left it.
        let tmp = dir.join("settings.json.tmp");
        std::fs::write(&tmp, "{}").unwrap();
        set_mode(&tmp, 0o644);
        set_mode(&path, 0o644);
        save_private(&path, b"{}").unwrap();
        assert_eq!(mode(&path), 0o600);
        // Written in place too.
        set_mode(&path, 0o644);
        write_private(&path, b"{}").unwrap();
        assert_eq!(mode(&path), 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn files_are_only_ever_made_closer() {
        let dir = temp_dir("private-closer");
        let path = dir.join("grinder-pairing.json");
        std::fs::write(&path, "{}").unwrap();
        set_mode(&path, 0o644);
        make_private(&path);
        assert_eq!(mode(&path), 0o600);
        // A file only its owner may read stays as it is, and a missing one
        // is fine.
        set_mode(&path, 0o400);
        make_private(&path);
        assert_eq!(mode(&path), 0o400);
        make_private(&dir.join("missing.json"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
