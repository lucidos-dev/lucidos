//! Direction-aware comparison of two gateway build ids.
//!
//! "A new gateway version is on disk" must mean **newer**, not merely
//! *different*. A checkout's `target/<profile>/` is written by every cargo
//! variant in the tree, and a build whose `build.rs` ran at an earlier commit
//! can land after a newer one — so a bare `disk != running` offers a *downgrade*
//! as an update, and re-exec'ing onto it (`reload_gateway`) walks the machine's
//! only gateway backwards. Same bug class the engine hit as the 2026-07-26
//! endless-toast loop; see
//! `docs/plans/2026-07-27-launch-binary-published-per-variant.md`.
//!
//! **Duplicated from `crates/lucidos-engine/src/engine/engine_version.rs` — keep
//! the two in step.** ADR 0014 §1 keeps the gateway free of any dependency on
//! the engine crate, and the codebase's established answer for a small shared
//! predicate across that boundary is a hand-synced copy with a cross-reference
//! (`path_is_in_cc_worktree` already lives in three: engine `paths.rs`, gateway
//! `stack.rs`, bash `workspace.sh`).

use std::path::{Path, PathBuf};

/// Marker that identifies the repo root, mirroring the engine's `paths.rs`.
const REPO_MARKER: &str = "scripts/web-dev.sh";

/// The commit prefix of a build id, or `None` when there isn't one.
///
/// `build.rs` stamps `<short-sha>` for a clean tree and `<short-sha>-<diffhash>`
/// when gateway source is dirty, falling back to `src-<hash>` with no git. Only
/// the commit is comparable across two binaries, so split at the first `-` and
/// reject the no-git / unstamped forms.
pub(crate) fn build_id_commit(id: &str) -> Option<&str> {
    if id.is_empty() || id.starts_with("src") {
        return None;
    }
    let commit = id.split('-').next().unwrap_or("");
    (!commit.is_empty()).then_some(commit)
}

/// Is the on-disk binary a genuine upgrade, given its id, the running id, and
/// the ancestry answer (`Some(true)` = the disk commit is a strict ancestor of
/// the running commit, i.e. provably older; `None` = git couldn't tell)?
///
/// Indeterminate ancestry keeps the historical difference test: missing a real
/// update (a stale gateway with no reload affordance) is worse than the
/// occasional unresolvable id being offered.
pub(crate) fn disk_upgrade_verdict(
    disk_id: &str,
    running_id: &str,
    disk_is_strict_ancestor: Option<bool>,
) -> bool {
    !disk_id.is_empty() && disk_id != running_id && disk_is_strict_ancestor != Some(true)
}

/// The Lucidos repo root above `exe`, or `None` when the binary lives outside a
/// Lucidos tree (packaged / installed runtime). Mirrors the engine's
/// `paths::repo_root`, resolved from the binary rather than compile-time paths.
pub(crate) fn repo_root_above(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|a| a.join(REPO_MARKER).exists())
        .map(Path::to_path_buf)
}

/// Is `ancestor` a STRICT ancestor of `descendant` (`git merge-base
/// --is-ancestor`, plus the two being different commits)? `None` when git can't
/// answer — no repo, an unknown commit, or a command failure — so callers can
/// tell "provably older" from "don't know".
///
/// STRICT is enforced here, not by git: `git merge-base --is-ancestor X X` exits
/// 0 (a commit is its own ancestor), so the same commit must be screened out
/// first. The screen is prefix-aware because the two sides can arrive
/// abbreviated to different lengths, and a plain `==` would then let the same
/// commit through as "older".
pub(crate) async fn commit_is_strict_ancestor(
    root: &Path,
    ancestor: &str,
    descendant: &str,
) -> Option<bool> {
    if ancestor.starts_with(descendant) || descendant.starts_with(ancestor) {
        return Some(false); // same commit (possibly abbreviated) — not OLDER
    }
    let out = tokio::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(root)
        .output()
        .await
        .ok()?;
    match out.status.code() {
        Some(0) => Some(true),  // is an ancestor
        Some(1) => Some(false), // is not an ancestor
        // Any other code is an error (bad object, not a repo) — not a verdict.
        _ => None,
    }
}

/// Is the on-disk gateway binary a step FORWARD from the running one?
///
/// Forks `git merge-base` only when the two ids differ AND both carry a
/// comparable commit, so the caller's mtime cache keeps this off the picker's
/// 2s poll. Logs the downgrade case so a wedged checkout is diagnosable from
/// the gateway log rather than only from a `/~/api/v1/control/gateway/status`
/// fetch.
pub(crate) async fn disk_id_is_upgrade(exe: &Path, disk_id: &str, running_id: &str) -> bool {
    if disk_id.is_empty() || disk_id == running_id {
        return false;
    }
    let is_older = match (build_id_commit(disk_id), build_id_commit(running_id)) {
        (Some(disk_commit), Some(running_commit)) if disk_commit != running_commit => {
            match repo_root_above(exe) {
                Some(root) => commit_is_strict_ancestor(&root, disk_commit, running_commit).await,
                None => None,
            }
        }
        // Same commit (differing only in the uncommitted-diff suffix) → a
        // rebuilt dirty tree, which IS a real update.
        (Some(_), Some(_)) => Some(false),
        // A `src-…` / empty id on either side → no commit to compare.
        _ => None,
    };
    if is_older == Some(true) {
        crate::log!(
            "[Gateway] on-disk gateway binary ({}) is OLDER than the running one ({}) — \
             not offering a downgrade as a new version. Rebuild it forward with \
             `web-dev.sh -w <ws> -b`.",
            disk_id,
            running_id
        );
    }
    disk_upgrade_verdict(disk_id, running_id, is_older)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_id_commit_takes_the_sha_and_rejects_the_no_git_forms() {
        assert_eq!(build_id_commit("aa7075ee2"), Some("aa7075ee2"));
        // Dirty tree: `<sha>-<diffhash>` — only the sha is comparable.
        assert_eq!(
            build_id_commit("aa7075ee2-0badc0ffee123456"),
            Some("aa7075ee2")
        );
        // No git (shipped build) / unstamped → nothing to compare.
        assert_eq!(build_id_commit("src-0123456789abcdef"), None);
        assert_eq!(build_id_commit(""), None);
        assert_eq!(build_id_commit("-abc"), None);
    }

    /// A DIFFERENT on-disk gateway is an update only when it isn't provably
    /// OLDER. Everything indeterminate keeps the historical difference test, so
    /// this can only remove a false positive — never hide a real update.
    #[test]
    fn disk_upgrade_verdict_offers_only_a_step_forward() {
        // The downgrade: disk `71c8d39b1` is an ancestor of running `aa7075ee2`.
        assert!(!disk_upgrade_verdict("71c8d39b1", "aa7075ee2", Some(true)));
        // The normal case: a newer binary was built (not an ancestor).
        assert!(disk_upgrade_verdict("bb1122334", "aa7075ee2", Some(false)));
        // Same id → nothing to reload onto, whatever git says.
        assert!(!disk_upgrade_verdict("aa7075ee2", "aa7075ee2", Some(false)));
        // Unreadable on-disk id (binary mid-rewrite) → no update.
        assert!(!disk_upgrade_verdict("", "aa7075ee2", None));
        // Indeterminate ancestry (unrelated commits, no repo, unknown object) →
        // fall back to "different is an update": never MISS a real one.
        assert!(disk_upgrade_verdict("cc9988776", "aa7075ee2", None));
        // Same commit, different uncommitted diff → a real rebuild.
        assert!(disk_upgrade_verdict(
            "aa7075ee2-0badc0ffee123456",
            "aa7075ee2",
            Some(false)
        ));
    }

    #[test]
    fn repo_root_above_finds_the_scripts_marker() {
        let dir = std::env::temp_dir().join(format!(
            "lucidos-gw-reporoot-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join(REPO_MARKER), "#!/bin/bash\n").unwrap();
        let exe = dir.join("target/debug/launch/plain/lucidos-gateway");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();

        assert_eq!(repo_root_above(&exe).as_deref(), Some(dir.as_path()));
        assert_eq!(
            repo_root_above(Path::new("/tmp/nowhere/lucidos-gateway")),
            None
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The real git probe, against a throwaway repo: ancestor → `Some(true)`,
    /// the reverse → `Some(false)`, an unknown object → `None` (so an
    /// unresolvable id can never be mistaken for "provably older").
    #[tokio::test]
    async fn commit_is_strict_ancestor_reads_history_direction() {
        let dir = std::env::temp_dir().join(format!(
            "lucidos-gw-ancestry-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git runs in the test environment")
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "first"]);
        let first = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_string();
        std::fs::write(dir.join("a.txt"), "two").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "second"]);
        let second = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_string();

        assert_eq!(
            commit_is_strict_ancestor(&dir, &first, &second).await,
            Some(true),
            "the earlier commit IS a strict ancestor of the later one"
        );
        assert_eq!(
            commit_is_strict_ancestor(&dir, &second, &first).await,
            Some(false),
            "the later commit is NOT an ancestor of the earlier one"
        );
        // The abbreviation trap: build ids carry the SHORT sha, and
        // `git merge-base --is-ancestor X X` exits 0 — without the prefix-aware
        // screen the same commit would read as "provably older".
        assert_eq!(
            commit_is_strict_ancestor(&dir, &second[..9], &second).await,
            Some(false),
            "the same commit abbreviated is still not a strict ancestor of itself"
        );
        assert_eq!(
            commit_is_strict_ancestor(&dir, "0000000000000000000000000000000000000000", &second)
                .await,
            None,
            "an unknown object is indeterminate, never 'provably older'"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
