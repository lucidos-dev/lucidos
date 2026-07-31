//! Command checkpoint / undo (ADR 0002, Phase 4) — git snapshots that make an
//! in-workspace `ReversibleDanger` command recoverable.
//!
//! Before the command guard runs such a command it takes a **checkpoint**: a
//! snapshot of the workspace's current working tree (every git-tracked and
//! untracked-but-not-ignored file, including pending deletions) stored as a
//! commit on a safety ref `refs/lucidos/command-checkpoints/<id>`. The snapshot
//! is **non-invasive** — it stages into a throwaway index via `GIT_INDEX_FILE`,
//! so the repo's real index and working tree are never touched and a concurrent
//! engine commit can't be disturbed.
//!
//! **Undo** restores the working tree from that ref (re-creating deleted files,
//! reverting overwritten ones) and deletes the ref. Restore is likewise done
//! through a throwaway index. Documented limit (ADR 0002): undo does NOT remove
//! files the command *created* after the checkpoint — `ReversibleDanger` is
//! about destruction, and a blanket `git clean` would be more dangerous than
//! the residual.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::{git_cmd, git_cmd_env};

/// The safety ref a checkpoint snapshot lives on. Deterministic from the
/// checkpoint id (a UUID), so the undo path re-derives it without storing it.
pub(crate) fn command_checkpoint_ref(checkpoint_id: &str) -> String {
    format!("refs/lucidos/command-checkpoints/{checkpoint_id}")
}

/// A throwaway index path for one checkpoint op, in the OS temp dir keyed by the
/// (UUID) checkpoint id so concurrent workspaces never collide. git writes tree
/// objects into the repo's own object store regardless of where the index lives,
/// so the location only needs to be writable.
fn temp_index_path(checkpoint_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("lucidos-checkpoint-{checkpoint_id}.index"))
}

/// Trim a git command's stdout to a SHA, or surface stderr as an error.
fn ok_stdout(out: std::process::Output, what: &str) -> Result<String, String> {
    if !out.status.success() {
        return Err(format!(
            "{what}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Snapshot the current working tree of `repo_root` onto the checkpoint ref.
/// Non-invasive: stages into a throwaway index, never the repo's real one.
/// Returns `Err` (without creating a ref) if any git step fails — the caller
/// then runs the command unguarded rather than emit a `CommandCheckpointed`
/// event with no recoverable snapshot behind it.
pub(crate) async fn create_command_checkpoint(
    repo_root: &Path,
    checkpoint_id: &str,
) -> Result<(), String> {
    let tmp_index = temp_index_path(checkpoint_id);
    // Drop any stale index left by a crashed prior run before staging.
    let _ = std::fs::remove_file(&tmp_index);
    // A committer identity is forced via env so `commit-tree` works even when
    // the workspace repo has no user.name/email in its config (the engine
    // commits artifacts through git2 with a per-commit signature, not config).
    let envs: &[(&str, &OsStr)] = &[
        ("GIT_INDEX_FILE", tmp_index.as_os_str()),
        ("GIT_AUTHOR_NAME", OsStr::new("Lucidos")),
        ("GIT_AUTHOR_EMAIL", OsStr::new("lucidos@localhost")),
        ("GIT_COMMITTER_NAME", OsStr::new("Lucidos")),
        ("GIT_COMMITTER_EMAIL", OsStr::new("lucidos@localhost")),
    ];

    let result = async {
        // Stage the whole working tree (respecting .gitignore) into the empty
        // throwaway index — every file becomes an addition, so the resulting
        // tree is a faithful snapshot of the current working tree, including
        // files a prior bash command created but never committed.
        let add = git_cmd_env(&["add", "-A"], repo_root, envs).await?;
        if !add.status.success() {
            return Err(format!(
                "git add -A (checkpoint): {}",
                String::from_utf8_lossy(&add.stderr).trim()
            ));
        }
        let tree = ok_stdout(
            git_cmd_env(&["write-tree"], repo_root, envs).await?,
            "git write-tree (checkpoint)",
        )?;
        // Parent on HEAD when the repo has one (gives the checkpoint lineage for
        // diffing); a fresh repo with no commits yields a parentless commit.
        let head = git_cmd(&["rev-parse", "--verify", "HEAD"], repo_root)
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let msg = format!("lucidos command checkpoint {checkpoint_id}");
        let commit_args: Vec<&str> = match head.as_deref() {
            Some(h) => vec!["commit-tree", &tree, "-p", h, "-m", &msg],
            None => vec!["commit-tree", &tree, "-m", &msg],
        };
        let commit = ok_stdout(
            git_cmd_env(&commit_args, repo_root, envs).await?,
            "git commit-tree (checkpoint)",
        )?;
        let ref_name = command_checkpoint_ref(checkpoint_id);
        let update = git_cmd(&["update-ref", &ref_name, &commit], repo_root).await?;
        if !update.status.success() {
            return Err(format!(
                "git update-ref (checkpoint): {}",
                String::from_utf8_lossy(&update.stderr).trim()
            ));
        }
        Ok(())
    }
    .await;
    // Always remove the throwaway index, success or failure.
    let _ = std::fs::remove_file(&tmp_index);
    result
}

/// Restore `repo_root`'s working tree from the checkpoint ref: re-create files
/// the command deleted and revert ones it overwrote. Does NOT remove files
/// created after the checkpoint (documented limit). Non-invasive: reads into a
/// throwaway index, never the repo's real one. `Err` if the ref is gone (an
/// already-undone checkpoint) or any git step fails.
pub(crate) async fn restore_command_checkpoint(
    repo_root: &Path,
    checkpoint_id: &str,
) -> Result<(), String> {
    let ref_name = command_checkpoint_ref(checkpoint_id);
    let verify = git_cmd(
        &["rev-parse", "--verify", &format!("{ref_name}^{{commit}}")],
        repo_root,
    )
    .await?;
    if !verify.status.success() {
        return Err(format!(
            "checkpoint {checkpoint_id} not found (already reverted, or pruned)"
        ));
    }

    let tmp_index = temp_index_path(checkpoint_id);
    let _ = std::fs::remove_file(&tmp_index);
    let envs: &[(&str, &OsStr)] = &[("GIT_INDEX_FILE", tmp_index.as_os_str())];

    let result = async {
        // Load the checkpoint tree into the throwaway index …
        let read = git_cmd_env(&["read-tree", &ref_name], repo_root, envs).await?;
        if !read.status.success() {
            return Err(format!(
                "git read-tree (restore): {}",
                String::from_utf8_lossy(&read.stderr).trim()
            ));
        }
        // … and force-write every entry from it to the working tree. `-f`
        // overwrites existing files; `-a` writes all entries (re-creating
        // deleted ones). Files NOT in the snapshot (created after it) are left
        // alone — the documented residual.
        let checkout = git_cmd_env(&["checkout-index", "-a", "-f"], repo_root, envs).await?;
        if !checkout.status.success() {
            return Err(format!(
                "git checkout-index (restore): {}",
                String::from_utf8_lossy(&checkout.stderr).trim()
            ));
        }
        Ok(())
    }
    .await;
    let _ = std::fs::remove_file(&tmp_index);
    result
}

/// Best-effort delete of a checkpoint ref (after a successful undo). Logs on
/// failure — a leftover ref is harmless (it just pins a cheap, deduped commit
/// object) and the next undo of the same id would find nothing to do.
pub(crate) async fn delete_command_checkpoint_ref(repo_root: &Path, checkpoint_id: &str) {
    let ref_name = command_checkpoint_ref(checkpoint_id);
    match git_cmd(&["update-ref", "-d", &ref_name], repo_root).await {
        Ok(out) if out.status.success() => {}
        Ok(out) => crate::log!(
            "[CommandGuard] failed to delete checkpoint ref {}: {}",
            ref_name,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => crate::log!(
            "[CommandGuard] failed to delete checkpoint ref {}: {}",
            ref_name,
            e
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    async fn git(args: &[&str], dir: &Path) {
        let out = git_cmd(args, dir).await.unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    async fn init_repo(root: &Path) {
        git(&["init", "-b", "main"], root).await;
        git(&["config", "user.email", "t@example.com"], root).await;
        git(&["config", "user.name", "Test"], root).await;
    }

    #[tokio::test]
    async fn checkpoint_restores_deleted_overwritten_and_untracked() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root).await;
        fs::create_dir_all(root.join("data/artifacts")).unwrap();
        fs::write(root.join("data/artifacts/keep.txt"), "original").unwrap();
        fs::write(root.join("data/artifacts/del.txt"), "deleteme").unwrap();
        git(&["add", "-A"], root).await;
        git(&["commit", "-m", "init"], root).await;
        // An uncommitted (untracked) file present at checkpoint time must also be
        // captured — a reversible command could delete it.
        fs::write(root.join("data/artifacts/untracked.txt"), "untracked").unwrap();

        let id = "11111111-1111-1111-1111-111111111111";
        create_command_checkpoint(root, id).await.unwrap();

        // The "reversible" command: delete one, overwrite another, create a new.
        fs::remove_file(root.join("data/artifacts/del.txt")).unwrap();
        fs::write(root.join("data/artifacts/keep.txt"), "CLOBBERED").unwrap();
        fs::remove_file(root.join("data/artifacts/untracked.txt")).unwrap();
        fs::write(root.join("data/artifacts/new.txt"), "created-after").unwrap();

        restore_command_checkpoint(root, id).await.unwrap();

        // Deleted file restored, overwrite reverted, untracked file restored.
        assert_eq!(
            fs::read_to_string(root.join("data/artifacts/del.txt")).unwrap(),
            "deleteme"
        );
        assert_eq!(
            fs::read_to_string(root.join("data/artifacts/keep.txt")).unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(root.join("data/artifacts/untracked.txt")).unwrap(),
            "untracked"
        );
        // Documented limit: a file created after the checkpoint is NOT removed.
        assert!(root.join("data/artifacts/new.txt").exists());

        // The snapshot left the repo's real HEAD untouched (commit still "init").
        let head_subject = git_cmd(&["log", "-1", "--format=%s"], root).await.unwrap();
        assert_eq!(
            String::from_utf8_lossy(&head_subject.stdout).trim(),
            "init",
            "checkpoint must not have moved HEAD"
        );

        // Undo deletes the ref; a second restore then errors.
        delete_command_checkpoint_ref(root, id).await;
        assert!(restore_command_checkpoint(root, id).await.is_err());
    }

    #[tokio::test]
    async fn restore_missing_checkpoint_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root).await;
        fs::write(root.join("f.txt"), "x").unwrap();
        git(&["add", "-A"], root).await;
        git(&["commit", "-m", "init"], root).await;
        assert!(restore_command_checkpoint(root, "nope").await.is_err());
    }

    #[tokio::test]
    async fn checkpoint_in_repo_without_head_is_parentless() {
        // A freshly-initialized repo with no commit still snapshots (parentless
        // commit) — the guard shouldn't fail on a brand-new workspace.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        init_repo(root).await;
        fs::write(root.join("a.txt"), "v1").unwrap();
        let id = "22222222-2222-2222-2222-222222222222";
        create_command_checkpoint(root, id).await.unwrap();
        fs::write(root.join("a.txt"), "v2").unwrap();
        restore_command_checkpoint(root, id).await.unwrap();
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "v1");
        delete_command_checkpoint_ref(root, id).await;
    }
}
