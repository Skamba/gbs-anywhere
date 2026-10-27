//! Helpers shared by the tests.

use std::path::{Path, PathBuf};

/// A path of its own under the system's temp directory, not created:
/// `gbs-anywhere-<name>-<uuid>`, so tests running side by side (and runs
/// that left files behind) never share one.
pub fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("gbs-anywhere-{name}-{}", uuid::Uuid::new_v4()))
}

/// A directory of its own under the system's temp directory, created
/// ([`temp_path`]). The test removes it when done.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = temp_path(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The permission bits of `path`, such as `0o600`.
#[cfg(unix)]
pub fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Sets the permission bits of `path`.
#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}
