//! Common user-install bin dirs for spawned tools.
//!
//! A packaged engine (launchd LaunchAgent / systemd --user unit / the macOS
//! `.app`) inherits the service manager's minimal PATH —
//! `/usr/bin:/bin:/usr/sbin:/sbin` on macOS — which omits Homebrew, npm and
//! `~/.local/bin`. Anything resolved by bare name (the `claude`/`codex` CLI
//! fallbacks, chat bash/python tools shelling out to user-installed binaries,
//! stdio MCP servers like `npx`/`uvx`) then ENOENTs even though the tool is
//! installed. Dev never sees this because everything inherits the user's
//! shell PATH.
//!
//! `augment_process_path` fixes this once, at engine startup: it prepends the
//! well-known install dirs (deduplicated, order-preserving) to the engine's
//! own process PATH, so every child — coding agents, chat tools, MCP servers,
//! the engine's own `git` shell-outs — inherits the same resolution a dev
//! shell provides. On a dev engine the dirs are already on PATH, so the
//! augmentation is a no-op by construction.

use std::path::{Path, PathBuf};

/// Build a PATH with the common interpreter / package-manager bin dirs
/// (Homebrew, `/usr/local/bin`, `~/.local/bin`, npm-global) prepended to
/// `existing`, de-duplicated and order-preserving: dirs already present keep
/// their position, and empty components (an empty PATH element means "current
/// directory" — a security smell) are dropped. Pure (`home` injected) so it's
/// unit-testable without touching process env.
pub(crate) fn augmented_user_path(
    existing: Option<std::ffi::OsString>,
    home: Option<&Path>,
) -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = home {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".npm-global/bin"));
    }
    let existing = existing.unwrap_or_default();
    let existing_dirs: Vec<PathBuf> = std::env::split_paths(&existing)
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    let present: std::collections::HashSet<&PathBuf> = existing_dirs.iter().collect();
    let prefix: Vec<PathBuf> = dirs.into_iter().filter(|d| !present.contains(d)).collect();
    std::env::join_paths(prefix.into_iter().chain(existing_dirs.iter().cloned()))
        .unwrap_or(existing)
}

/// Apply `augmented_user_path` to the ENGINE'S OWN process PATH. Call once at
/// startup, before anything spawns or probes tools (the git/python preflights
/// included, so they see the same PATH later spawns will).
pub fn augment_process_path() {
    let augmented = augmented_user_path(
        std::env::var_os("PATH"),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
    );
    // SAFETY: called once at startup, before the engine begins serving
    // requests or running recovery sweeps — no other thread reads process env
    // at this point. Same contract as
    // `environment_variables::apply_to_process_env`.
    unsafe {
        std::env::set_var("PATH", &augmented);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn augmented_user_path_prepends_common_dirs_in_order() {
        let home = PathBuf::from("/home/u");
        let path = augmented_user_path(Some("/usr/bin:/bin".into()), Some(&home));
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/home/u/.local/bin"),
                PathBuf::from("/home/u/.npm-global/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
            ],
            "common install dirs come first, then the inherited PATH"
        );
    }

    #[test]
    fn augmented_user_path_dedupes_already_present_dirs() {
        // /usr/local/bin is already on PATH → it must not be duplicated, and
        // its existing position must win (dev PATH order is preserved).
        let path = augmented_user_path(Some("/usr/local/bin:/usr/bin".into()), None);
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/usr/bin"),
            ],
            "an already-present dir keeps its position and is not duplicated"
        );
    }

    #[test]
    fn augmented_user_path_handles_empty_inherited_path() {
        let path = augmented_user_path(None, None);
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ]
        );
    }
}
