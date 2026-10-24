//! Where Claude Code reads the managed policy an administrator installs.

use std::path::{Path, PathBuf};

/// The managed-policy directory.
#[cfg(target_os = "macos")]
pub const MANAGED_DIR: &str = "/Library/Application Support/ClaudeCode";
#[cfg(not(target_os = "macos"))]
pub const MANAGED_DIR: &str = "/etc/claude-code";

/// The managed settings file's name inside [`MANAGED_DIR`].
pub const MANAGED_SETTINGS_FILE: &str = "managed-settings.json";

/// The managed settings file inside `managed_dir`.
pub fn managed_settings_path(managed_dir: &Path) -> PathBuf {
    managed_dir.join(MANAGED_SETTINGS_FILE)
}
