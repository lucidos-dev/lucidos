use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
pub const MANAGED_DIR: &str = "/Library/Application Support/ClaudeCode";
#[cfg(not(target_os = "macos"))]
pub const MANAGED_DIR: &str = "/etc/claude-code";

pub const MANAGED_SETTINGS_FILE: &str = "managed-settings.json";

pub fn managed_settings_path(managed_dir: &Path) -> PathBuf {
    managed_dir.join(MANAGED_SETTINGS_FILE)
}
