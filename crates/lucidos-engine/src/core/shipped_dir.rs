//! A directory Lucidos ships beside the engine binary: `system-knowhow/` and
//! `system-widgets/`. A packaged build stages each one and points an env var at
//! it. A dev checkout reads it from the repo root.

use std::path::{Path, PathBuf};

/// One shipped directory: its name in the repo and the bundle, and the env var
/// a packaged launcher sets to its staged path.
pub struct ShippedDir {
    pub name: &'static str,
    pub env_var: &'static str,
    pub log_tag: &'static str,
    /// What goes missing when the directory is unavailable, for the warning.
    pub degrades: &'static str,
}

/// Resolve a shipped directory at boot, returning the resolved dir (if any)
/// plus at most one loud warning to log.
///
/// Resolution order (INV-3 of the "package system-knowhow" plan):
///   1. The env var set (non-empty) is **authoritative**: it MUST exist.
///      Set-but-missing is a mis-staged bundle, so warn loudly and treat it as
///      unavailable. NEVER fall back to `repo_root`, which is bogus when packaged.
///   2. Env var unset or empty: the `<repo_root>/<name>` dev fallback.
///   3. Neither resolves: unavailable. On a packaged build there is no checkout,
///      so that is a real defect and warns loudly naming the env var (INV-4).
///      Dev or e2e without the dir is expected and stays quiet.
///
/// Pure over its inputs, so it is unit-testable offline. The caller reads the
/// env var and logs the returned warning.
pub fn resolve_shipped_dir(
    dir: &ShippedDir,
    env_value: Option<&str>,
    repo_root: &Path,
    is_packaged: bool,
) -> (Option<PathBuf>, Option<String>) {
    let ShippedDir {
        name,
        env_var,
        log_tag,
        degrades,
    } = dir;
    // Trim only to DETECT a blank value (= unset). The path keeps its original
    // bytes, since a legitimate dir path may carry edge whitespace.
    if let Some(value) = env_value.filter(|v| !v.trim().is_empty()) {
        let candidate = PathBuf::from(value);
        if candidate.is_dir() {
            return (Some(candidate), None);
        }
        return (
            None,
            Some(format!(
                "{log_tag} {env_var} is set to '{value}' but that directory does not exist: \
                 {degrades}. This is a packaging bug: the bundle must stage {name}/ at that path."
            )),
        );
    }

    let candidate = repo_root.join(name);
    if candidate.is_dir() {
        return (Some(candidate), None);
    }

    if is_packaged {
        return (
            None,
            Some(format!(
                "{log_tag} {name} directory is UNAVAILABLE: {env_var} is unset and no \
                 <repo>/{name} exists, so {degrades}. This is a packaging bug: the bundle \
                 must set {env_var}."
            )),
        );
    }
    (None, None)
}
