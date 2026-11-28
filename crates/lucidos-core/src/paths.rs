use std::path::{Path, PathBuf};
use std::sync::OnceLock;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const REPO_MARKER: &str = "scripts/web-dev.sh";

/// INVARIANT: Where a workspace keeps its coding-agent worktrees, relative to its root.
/// The gateway's `stack::path_is_in_cc_worktree` and `scripts/lib/workspace.sh`
/// spell the same two components and cannot import this.
pub const WORKTREES_SUBPATH: &str = ".lucidos/worktrees";

/// INVARIANT: Does `path` lie inside a coding-agent worktree — one of the
/// `<workspace>/.lucidos/worktrees/<thread>/` copies the engine creates per
/// coding-agent thread?
///
/// A worktree is a throwaway checkout pinned to one commit, so anything
/// long-lived resolving into one is frozen at that commit. Used to explain a
/// stranded frontend Apply: when the served `dist/` sits in a worktree, the
/// build-watch is republishing a DIFFERENT directory and the served client can
/// never advance (the 2026-07-26 incident — see
/// `docs/plans/2026-07-26-worktree-pinned-stack-guard.md`).
///
/// Mirrors the gateway's `stack::path_is_in_cc_worktree` and the bash
/// `path_is_in_cc_worktree` in `scripts/lib/workspace.sh` — keep the three in
/// step. A pure path test on purpose: it must stay correct for an orphaned
/// worktree whose directory is already gone. Matches the ADJACENT `.lucidos` +
/// `worktrees` component pair, so `~/worktrees/lucidos` and
/// `.lucidos/served-frontend` are not caught.
pub fn path_is_in_cc_worktree(path: &Path) -> bool {
    let marker: Vec<_> = Path::new(WORKTREES_SUBPATH).components().collect();
    let comps: Vec<_> = path.components().collect();
    comps.windows(marker.len()).any(|w| w == marker.as_slice())
}

pub fn user_cache_root_from(xdg_cache_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    let base = match xdg_cache_home.filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(home.filter(|h| !h.is_empty())?).join(".cache"),
    };
    Some(base.join("lucidos"))
}

pub fn user_cache_root() -> Option<PathBuf> {
    let root = user_cache_root_from(
        std::env::var("XDG_CACHE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )?;
    match lucidos_home::ensure_storage_root(&root) {
        Ok(()) => Some(root),
        Err(e) => {
            crate::log_legacy!("[Paths] No shared user cache: {e}");
            None
        }
    }
}

pub fn gateway_data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<_> = lucidos_installs::scan_this_machine(None)
        .installs
        .into_iter()
        .filter_map(|install| install.data_dir)
        .collect();
    if let Some(dir) = lucidos_home::gateway_data_dir().filter(|dir| !dirs.contains(dir)) {
        dirs.push(dir);
    }
    dirs
}

pub fn registered_workspace_dirs(data_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for data in data_dirs {
        let Ok(bytes) = std::fs::read(data.join("config/workspaces.json")) else {
            continue;
        };
        let Ok(registry) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        for entry in registry["workspaces"].as_array().into_iter().flatten() {
            if let Some(dir) = entry["dir"].as_str() {
                roots.push(data.join(dir));
            }
        }
    }
    roots
}

pub fn repo_root() -> Result<PathBuf, BoxError> {
    static CACHED: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    CACHED
        .get_or_init(compute_repo_root)
        .clone()
        .map_err(|e| e.into())
}

fn compute_repo_root() -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("Cannot resolve current executable path: {e}"))?;

    repo_root_above(&exe).ok_or_else(|| {
        format!(
            "No Lucidos repo root above current executable ({}); \
             expected an ancestor containing {REPO_MARKER}",
            exe.display()
        )
    })
}

pub(crate) fn repo_root_above(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .take_while(|a| a.extension().is_none_or(|ext| ext != "app"))
        .find(|a| a.join(REPO_MARKER).exists())
        .map(Path::to_path_buf)
}

/// INVARIANT: Was this engine launched from a Lucidos **source checkout**?
///
/// The one predicate for "can anything here edit the Lucidos platform's own
/// code". A dev engine is built from source and resolves [`repo_root`]; a
/// packaged `.app` / headless install ships only the binary, so `repo_root`
/// errs and there is no platform source to edit.
///
/// Three surfaces key off this exact signal and must never disagree:
///
///  - startup skips registering the reserved `Lucidos` repository
///    (`engine_impl::construction`) — without the gate it would register the
///    *workspace* dir, because [`crate::engine::git_ops::main_worktree`] falls
///    back to the cwd;
///  - `/api/v1/health`'s `packaged` flag (`api::history::is_packaged`
///    delegates here), which is how the compose destination picker decides to
///    hide the "Lucidos source" target;
///  - the chat system prompt and `run_coding_agent`, which must tell the model
///    the truth and refuse a Lucidos-source spawn when there is no source.
///
/// NOT the same as [`crate::runtime::is_packaged`] (`LUCIDOS_PACKAGED=1`),
/// which describes the *staging layout* of bundled resources, not the presence
/// of a checkout.
pub fn has_lucidos_source() -> bool {
    repo_root().is_ok()
}

/// INVARIANT: Root for startup and recovery: the runtime checkout when present, otherwise
/// this workspace. A build machine's compile-time source path is never authority.
pub fn repo_or_workspace_root() -> PathBuf {
    repo_root().unwrap_or_else(|_| {
        workspace_root_fallback(std::env::var_os("LUCIDOS_WORKSPACE").map(PathBuf::from))
    })
}

fn workspace_root_fallback(workspace: Option<PathBuf>) -> PathBuf {
    workspace.unwrap_or_else(|| PathBuf::from("."))
}

pub fn script(name: &str) -> Result<PathBuf, BoxError> {
    let path = repo_root()?.join("scripts").join(name);
    if !path.exists() {
        return Err(format!("Script not found: {}", path.display()).into());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_bundle_inside_a_checkout_does_not_expose_platform_source() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("scripts")).unwrap();
        std::fs::write(root.path().join(REPO_MARKER), "").unwrap();
        let engine = root
            .path()
            .join("target/release/bundle/macos/Demo.app/Contents/Resources/lucidos-engine");
        assert_eq!(repo_root_above(&engine), None);
    }

    #[test]
    fn packaged_recovery_falls_back_to_its_workspace_without_build_machine_paths() {
        let workspace = PathBuf::from("/owned/workspace");
        assert_eq!(workspace_root_fallback(Some(workspace.clone())), workspace);
        assert_eq!(workspace_root_fallback(None), PathBuf::from("."));
    }

    #[test]
    fn repo_root_above_is_independent_of_binary_depth() {
        let scratch = tempfile::tempdir().unwrap();
        let dir = scratch.path().to_path_buf();
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join(REPO_MARKER), "#!/bin/bash\n").unwrap();

        for rel in [
            "target/debug/lucidos-engine",
            ".launch/debug/plain/lucidos-engine",
            ".launch/release/e2e-test-hooks/lucidos-engine",
            "target/debug/deps/lucidos_engine-abc123",
        ] {
            assert_eq!(
                repo_root_above(&dir.join(rel)).as_deref(),
                Some(dir.as_path()),
                "repo root must resolve from {rel}"
            );
        }
        assert_eq!(
            repo_root_above(Path::new("/tmp/elsewhere/lucidos-engine")),
            None
        );
    }

    #[test]
    fn script_returns_existing_path() {
        let path = script("web-dev.sh").expect("web-dev.sh must exist in dev tree");
        assert!(path.ends_with("scripts/web-dev.sh"));
        assert!(path.exists());
    }

    #[test]
    fn script_errors_when_missing() {
        let err = script("definitely-not-a-real-script.sh").expect_err("missing script must err");
        let msg = err.to_string();
        assert!(
            msg.contains("definitely-not-a-real-script.sh"),
            "error must name the missing path; got: {msg}"
        );
    }

    #[test]
    fn flags_coding_agent_worktree_paths() {
        for p in [
            "/w/dev/.lucidos/worktrees/thread-abc",
            "/w/dev/.lucidos/worktrees/thread-abc/crates/lucidos-app/dist",
            "/w/dev/.lucidos/worktrees/thread-abc/.launch/debug/plain/lucidos-engine",
        ] {
            assert!(path_is_in_cc_worktree(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn leaves_other_paths_alone() {
        for p in [
            "/Users/me/projects/lucidos/crates/lucidos-app/dist",
            "/Users/me/worktrees/lucidos/crates/lucidos-app/dist",
            "/w/dev/.lucidos/served-frontend/0",
            "/w/dev/.lucidos/cache/worktrees/thread-abc",
        ] {
            assert!(!path_is_in_cc_worktree(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn does_not_require_the_path_to_exist() {
        assert!(path_is_in_cc_worktree(Path::new(
            "/gone/.lucidos/worktrees/thread-x/crates"
        )));
    }

    mod guard_tests {
        use super::*;

        #[test]
        fn repo_root_finds_scripts_marker_from_test_binary() {
            let root = repo_root().expect("repo_root must resolve from a cargo-run test binary");
            assert!(
                root.join(REPO_MARKER).exists(),
                "resolved repo root {} does not contain {REPO_MARKER}",
                root.display()
            );
        }
    }
}
