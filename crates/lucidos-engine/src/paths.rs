//! Runtime filesystem path helpers.
//!
//! Resolves the repo root from `current_exe()` instead of compile-time
//! `CARGO_MANIFEST_DIR`, so a project rename after build doesn't strand
//! script-spawns and asset reads at a path that no longer exists.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const REPO_MARKER: &str = "scripts/web-dev.sh";

/// Does `path` lie inside a coding-agent worktree — one of the
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
    let comps: Vec<_> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    comps
        .windows(2)
        .any(|w| w[0] == ".lucidos" && w[1] == "worktrees")
}

/// Resolve the Lucidos repo root from the running binary's path.
/// Cached for the process lifetime; called from polled handlers (`health`).
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

/// The Lucidos repo root above `exe`, or `None` when there isn't one.
///
/// Walks ancestors rather than counting `..` hops, so it is independent of HOW
/// DEEP the binary sits under the checkout. That is load-bearing, not
/// incidental: the dev launcher publishes the engine to
/// `target/<profile>/launch/<variant>/lucidos-engine` (ADR 0022), two levels
/// deeper than the historical `target/<profile>/`, and every dev-mode resource
/// lookup (`scripts/`, `system-knowhow/`, the SDK bundle) resolves through here.
/// Split out from [`compute_repo_root`] so the depth-independence is testable
/// without touching `current_exe()`.
///
/// The gateway has a hand-synced copy (`crates/lucidos-gateway/src/build_id.rs`
/// `repo_root_from`) — ADR 0014 §1 keeps it free of any dependency on the
/// engine. Keep the two in step.
pub(crate) fn repo_root_above(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|a| a.join(REPO_MARKER).exists())
        .map(Path::to_path_buf)
}

/// Was this engine launched from a Lucidos **source checkout**?
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

/// Best-effort repo root: falls back to compile-time `CARGO_MANIFEST_DIR`
/// when the binary lives outside a Lucidos tree (engine startup, recovery).
pub fn repo_root_or_compile_time_fallback() -> PathBuf {
    repo_root().unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
    })
}

/// Path to a script under `<repo_root>/scripts/<name>`. Errors with the
/// resolved path when the script is missing, so spawn failures surface a
/// useful message rather than a bare `No such file or directory`.
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
    fn repo_root_finds_scripts_marker_from_test_binary() {
        let root = repo_root().expect("repo_root must resolve from a cargo-run test binary");
        assert!(
            root.join(REPO_MARKER).exists(),
            "resolved repo root {} does not contain {REPO_MARKER}",
            root.display()
        );
    }

    /// The checkout is found by WALKING ancestors, at any depth — the dev
    /// launcher publishes the engine two levels deeper than the historical
    /// `target/<profile>/` (ADR 0022), and every dev-mode resource lookup
    /// (`scripts/`, `system-knowhow/`, the SDK bundle) hangs off this. A
    /// fixed-hop-count resolver would silently stop finding the checkout — which
    /// is exactly how the SDK bundle fell back to its stub.
    #[test]
    fn repo_root_above_is_independent_of_binary_depth() {
        let dir = std::env::temp_dir().join(format!(
            "lucidos-reporoot-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join(REPO_MARKER), "#!/bin/bash\n").unwrap();

        for rel in [
            "target/debug/lucidos-engine",
            "target/debug/launch/plain/lucidos-engine",
            "target/release/launch/e2e-test-hooks/lucidos-engine",
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

        std::fs::remove_dir_all(&dir).ok();
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

    /// Bind-mounting PGDATA on macOS pushes every WAL fsync through Docker
    /// Desktop's virtiofs, which throttles host writes and crashes the VM
    /// under sustained e2e load (34 GB dirtied in 4 hours, see thread
    /// 50724c03 — `com.apple.Virtualization.VirtualMachine` diagnostic at
    /// 22:44, DB pool timeouts at 05:50, force restart at 07:18). Switching
    /// to a Docker named volume keeps fsyncs inside the VM's ext4.
    ///
    /// PG 18's image relocated PGDATA to /var/lib/postgresql/18/docker and
    /// declares its VOLUME at the parent /var/lib/postgresql (PG 17 used
    /// /var/lib/postgresql/data for both), so the named volume must be mounted
    /// at the parent — mounting the old /data path would strand PGDATA on an
    /// anonymous volume that is lost on each container recreate.
    #[test]
    fn dev_postgres_uses_named_volume_not_bind_mount() {
        let repo = repo_root().expect("repo root");
        let yml = std::fs::read_to_string(repo.join("docker-compose.dev.yml"))
            .expect("docker-compose.dev.yml must exist");

        assert!(
            yml.contains("- lucidos-pg-data:/var/lib/postgresql\n"),
            "Postgres PGDATA must be a named volume mounted at the parent\n\
             /var/lib/postgresql (PG 18 keeps the cluster under it at 18/docker).\n\
             Expected line: '- lucidos-pg-data:/var/lib/postgresql'\n\
             Got docker-compose.dev.yml:\n{yml}"
        );
        assert!(
            !yml.contains("- lucidos-pg-data:/var/lib/postgresql/data"),
            "PG 17 mount path must be gone — PG 18 relocated PGDATA, so mounting\n\
             the named volume at /var/lib/postgresql/data loses data on recreate.\n\
             Got docker-compose.dev.yml:\n{yml}"
        );
        assert!(
            !yml.contains("/data/postgres:/var/lib/postgresql/data"),
            "Legacy host bind-mount entry must be removed.\n\
             Got docker-compose.dev.yml:\n{yml}"
        );
    }

    /// Regression cover for the 2026-07-26 incident: the engine served a `dist/`
    /// inside an orphaned coding-agent worktree, so the build-watch republished a
    /// different directory and every frontend Apply silently did nothing.
    #[test]
    fn flags_coding_agent_worktree_paths() {
        for p in [
            "/w/dev/.lucidos/worktrees/thread-abc",
            "/w/dev/.lucidos/worktrees/thread-abc/crates/lucidos-app/dist",
        ] {
            assert!(path_is_in_cc_worktree(Path::new(p)), "{p}");
        }
    }

    /// Must not fire on a real checkout, on a directory merely NAMED `worktrees`,
    /// or on the engine's own `.lucidos/served-frontend` snapshot dir.
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

    /// Pure path test — an orphaned worktree is often already deleted, which is
    /// exactly when the stranded-Apply explanation is still needed.
    #[test]
    fn does_not_require_the_path_to_exist() {
        assert!(path_is_in_cc_worktree(Path::new(
            "/gone/.lucidos/worktrees/thread-x/crates"
        )));
    }
}
