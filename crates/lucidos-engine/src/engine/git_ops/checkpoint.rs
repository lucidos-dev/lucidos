//! Command checkpoint / undo (ADR 0002, Phase 4) : git snapshots that make an
//! in-workspace `ReversibleDanger` command recoverable, and the diff that says
//! what the command actually did.
//!
//! **Two snapshots, one per side of the command.** Before the guard runs such a
//! command it writes a **pre** image: a snapshot of the working tree (every
//! git-tracked and untracked-but-not-ignored file, including pending deletions)
//! as a commit on `refs/lucidos/command-checkpoints/<id>`. After the command
//! returns it writes a **post** image the same way, on
//! `refs/lucidos/command-post-images/<id>`. Both are **non-invasive**: they
//! stage into a throwaway index via `GIT_INDEX_FILE`, so the repo's real index
//! and working tree are never touched and a concurrent engine commit cannot be
//! disturbed.
//!
//! **The pair is what makes undo precise.** `diff_checkpoint_effects` diffs pre
//! against post, so the engine knows exactly which files that one command
//! created (`A`), overwrote (`M`) and deleted (`D`). Anything that happened
//! later cannot be in that set, which is what makes removing the created files
//! safe where the blanket `git clean` ADR 0002 rejected was not.
//!
//! **Undo** restores the working tree from the pre image (re-creating deleted
//! files, reverting overwritten ones) and then removes the files the command
//! created, each only if it is still byte-identical to the post image. The refs
//! survive the undo, because they are also what the card's diff viewer reads;
//! they are reclaimed by [`prune_expired_checkpoints`] once they age out.
//!
//! **An empty diff means the command changed nothing git-visible** (the usual
//! cause is destruction inside a gitignored path such as `.lucidos/`, which the
//! snapshot never captured). The guard then deletes both refs and emits no
//! event at all, rather than offering an Undo that can neither restore nor
//! remove anything.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use super::{git_answer, git_cmd, git_cmd_env};

/// How long a checkpoint ref pair is kept before [`prune_expired_checkpoints`]
/// reclaims it. The pair backs the card's diff viewer, so it has to outlive the
/// undo itself; 30 days is well past the point where a user is still reasoning
/// about one command's blast radius, and a reclaimed pair degrades to a card
/// that renders its history and says the snapshot is gone.
pub(crate) const CHECKPOINT_RETENTION_SECS: i64 = 30 * 24 * 60 * 60;

/// The safety ref a checkpoint's **pre** image lives on. Deterministic from the
/// checkpoint id (a UUID), so the undo path re-derives it without storing it.
pub(crate) fn command_checkpoint_ref(checkpoint_id: &str) -> String {
    format!("refs/lucidos/command-checkpoints/{checkpoint_id}")
}

/// The ref a checkpoint's **post** image lives on. A separate namespace rather
/// than a suffix under the pre ref, because git cannot hold both
/// `refs/x/<id>` and `refs/x/<id>/post` at once (a directory/file conflict).
pub(crate) fn command_post_image_ref(checkpoint_id: &str) -> String {
    format!("refs/lucidos/command-post-images/{checkpoint_id}")
}

/// A throwaway index path for one checkpoint op, in the OS temp dir keyed by the
/// (UUID) checkpoint id and the side being written, so concurrent workspaces
/// never collide. git writes tree objects into the repo's own object store
/// regardless of where the index lives, so the location only needs to be
/// writable.
fn temp_index_path(checkpoint_id: &str, slot: &str) -> PathBuf {
    std::env::temp_dir().join(format!("lucidos-checkpoint-{checkpoint_id}-{slot}.index"))
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

/// Snapshot the current working tree of `repo_root` onto `ref_name`.
/// Non-invasive: stages into a throwaway index, never the repo's real one.
/// Shared by both sides of a checkpoint so the pre and post images are built
/// exactly alike and their diff reflects the command rather than a difference
/// in how they were taken.
async fn snapshot_onto_ref(
    repo_root: &Path,
    ref_name: &str,
    tmp_index: &Path,
    message: &str,
) -> Result<(), String> {
    // Drop any stale index left by a crashed prior run before staging.
    let _ = std::fs::remove_file(tmp_index);
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
        // throwaway index. Every file becomes an addition, so the resulting
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
        let commit_args: Vec<&str> = match head.as_deref() {
            Some(h) => vec!["commit-tree", &tree, "-p", h, "-m", message],
            None => vec!["commit-tree", &tree, "-m", message],
        };
        let commit = ok_stdout(
            git_cmd_env(&commit_args, repo_root, envs).await?,
            "git commit-tree (checkpoint)",
        )?;
        let update = git_cmd(&["update-ref", ref_name, &commit], repo_root).await?;
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
    let _ = std::fs::remove_file(tmp_index);
    result
}

/// Write the **pre** image, before the reversible command runs. Returns `Err`
/// (without creating a ref) if any git step fails: the caller then runs the
/// command unguarded rather than emit a `CommandCheckpointed` event with no
/// recoverable snapshot behind it.
pub(crate) async fn create_command_checkpoint(
    repo_root: &Path,
    checkpoint_id: &str,
) -> Result<(), String> {
    // Opportunistic reclamation, on the one path that creates new pairs. Cheap
    // (a single `for-each-ref` when nothing has expired) and best-effort: a
    // failure here must not stop the checkpoint it precedes.
    prune_expired_checkpoints(
        repo_root,
        chrono::Utc::now().timestamp(),
        CHECKPOINT_RETENTION_SECS,
    )
    .await;
    snapshot_onto_ref(
        repo_root,
        &command_checkpoint_ref(checkpoint_id),
        &temp_index_path(checkpoint_id, "pre"),
        &format!("lucidos command checkpoint {checkpoint_id}"),
    )
    .await
}

/// Write the **post** image, once the reversible command has returned. `Err`
/// leaves the pre ref in place for the caller to clean up; with no post image
/// there is no way to tell what the command did, so the caller emits no event.
pub(crate) async fn create_command_post_image(
    repo_root: &Path,
    checkpoint_id: &str,
) -> Result<(), String> {
    snapshot_onto_ref(
        repo_root,
        &command_post_image_ref(checkpoint_id),
        &temp_index_path(checkpoint_id, "post"),
        &format!("lucidos command post-image {checkpoint_id}"),
    )
    .await
}

/// Restore `repo_root`'s working tree from the checkpoint's pre image:
/// re-create files the command deleted and revert ones it overwrote. Removing
/// what the command *created* is a separate step ([`remove_created_files`]),
/// because it needs the post image too. Non-invasive: reads into a throwaway
/// index, never the repo's real one. `Err` if the ref is gone or any git step
/// fails.
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
            "checkpoint {checkpoint_id} not found (pruned, or never written)"
        ));
    }

    let tmp_index = temp_index_path(checkpoint_id, "restore");
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
        // deleted ones). Files NOT in the snapshot are left alone here; the
        // ones this command created are handled by `remove_created_files`.
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

/// What one checkpointed command did to the git-visible working tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CheckpointEffects {
    /// Files the command deleted or overwrote: what restoring the pre image
    /// puts back.
    pub restores: u32,
    /// Repo-relative paths the command created, exactly as git reported them:
    /// what undo removes.
    pub created: Vec<String>,
}

impl CheckpointEffects {
    /// The command changed nothing git-visible, so there is nothing to show and
    /// nothing either half of undo could do.
    pub(crate) fn is_empty(&self) -> bool {
        self.restores == 0 && self.created.is_empty()
    }

    /// How many files undo would remove. Named for the card copy, which pairs
    /// it with `restores`.
    pub(crate) fn removes(&self) -> u32 {
        self.created.len() as u32
    }
}

/// Are both of a checkpoint's images still on disk?
///
/// `false` covers a checkpoint written before the post image existed, a pair
/// reclaimed by [`prune_expired_checkpoints`], and one orphaned by a crash
/// between the two snapshots.
pub(crate) async fn checkpoint_pair_available(repo_root: &Path, checkpoint_id: &str) -> bool {
    for ref_name in [
        command_checkpoint_ref(checkpoint_id),
        command_post_image_ref(checkpoint_id),
    ] {
        let exists = git_answer(&["rev-parse", "--verify", "--quiet", &ref_name], repo_root).await;
        if exists.is_unknown() {
            crate::log!(
                "[CommandGuard] could not probe {}; treating the checkpoint pair as unavailable",
                ref_name
            );
        }
        // Unknown falls to "gone": the callers spend this answer on removing
        // files and on rendering a diff, and neither is worth guessing at.
        if !exists.or_unknown(false) {
            return false;
        }
    }
    true
}

/// Diff a checkpoint's pre image against its post image.
///
/// `Ok(None)` means the pair is not available: a checkpoint written before the
/// post image existed, a pair already reclaimed by [`prune_expired_checkpoints`],
/// one orphaned by a crash between the two snapshots, or a ref probe that could
/// not run at all. All four collapse to the same caller behaviour (restore
/// only, remove nothing), and that is the side `Unknown` falls to deliberately:
/// an unanswered probe must never be what authorizes deleting a file.
pub(crate) async fn diff_checkpoint_effects(
    repo_root: &Path,
    checkpoint_id: &str,
) -> Result<Option<CheckpointEffects>, String> {
    if !checkpoint_pair_available(repo_root, checkpoint_id).await {
        return Ok(None);
    }
    let pre = command_checkpoint_ref(checkpoint_id);
    let post = command_post_image_ref(checkpoint_id);

    let out = git_cmd(
        &["diff-tree", "-r", "-z", "--no-renames", &pre, &post],
        repo_root,
    )
    .await?;
    if !out.status.success() {
        return Err(format!(
            "git diff-tree (checkpoint effects): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(Some(parse_diff_tree_z(&out.stdout)))
}

/// Parse `git diff-tree -r -z --no-renames` output into [`CheckpointEffects`].
///
/// Each record is `:<srcmode> <dstmode> <srcsha> <dstsha> <status>` followed by
/// a NUL, the path, and another NUL. `-z` means the path is raw bytes rather
/// than git's C-quoted form, so a path that is not valid UTF-8 is skipped
/// rather than lossily decoded into a path that would not resolve. Split out as
/// a pure function so the classification is testable without a repo.
fn parse_diff_tree_z(stdout: &[u8]) -> CheckpointEffects {
    let mut effects = CheckpointEffects::default();
    let mut fields = stdout.split(|b| *b == 0);
    while let Some(meta) = fields.next() {
        if meta.is_empty() {
            continue;
        }
        let Some(path) = fields.next() else { break };
        let Ok(meta) = std::str::from_utf8(meta) else {
            continue;
        };
        let parts: Vec<&str> = meta.trim_start_matches(':').split_whitespace().collect();
        let [_src_mode, _dst_mode, _src_sha, _dst_sha, status] = parts[..] else {
            continue;
        };
        match status.as_bytes().first() {
            Some(b'A') => {
                let Ok(path) = std::str::from_utf8(path) else {
                    crate::log!(
                        "[CommandGuard] skipping non-UTF-8 created path in checkpoint diff"
                    );
                    continue;
                };
                effects.created.push(path.to_string());
            }
            // Deleted, overwritten, or type-changed: all three are put back by
            // restoring the pre image.
            Some(b'D') | Some(b'M') | Some(b'T') => effects.restores += 1,
            _ => {}
        }
    }
    effects
}

/// Resolve a repo-relative path from a checkpoint diff to an absolute path
/// inside `repo_root`, or `None` if it escapes.
///
/// The paths come from git's own output over trees we wrote, so this should
/// never reject in practice. It is still checked, because the value reaches a
/// `remove_file` and the alternative to checking is trusting that nothing ever
/// puts a `..` in front of it. Containment is **resolved**, not lexical: the
/// longest existing prefix is canonicalized, so a symlink inside the workspace
/// pointing outside it cannot be used to step out.
fn safe_repo_path(repo_root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() {
        return None;
    }
    let rel_path = Path::new(rel);
    for component in rel_path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            // Absolute, a drive prefix, or a `..` hop: all out.
            _ => return None,
        }
    }
    let joined = repo_root.join(rel_path);
    let root = std::fs::canonicalize(repo_root).ok()?;
    // The file itself may be a symlink we must not follow, so the parent is
    // what gets resolved.
    let parent = std::fs::canonicalize(joined.parent()?).ok()?;
    if !parent.starts_with(&root) {
        return None;
    }
    Some(joined)
}

/// Which of `created` still matches the post image, and may therefore be
/// removed.
///
/// Asked as one `git diff-files` against a throwaway index loaded from the post
/// tree, rather than one `git hash-object` per file. That is fewer subprocesses,
/// but the reason is correctness: it is git's own comparison, so it is right
/// about the cases a blob-sha compare is wrong about. A **symlink** is the
/// headline one, since git stores it as a blob holding the link *target path*
/// while `hash-object` follows the link and hashes the target *file's bytes*, so
/// the two never agree; a dangling symlink does not even open. Mode changes and
/// clean filters land the same way.
///
/// `None` on any git failure, which the caller reads as "remove nothing".
async fn unchanged_since_post_image(
    repo_root: &Path,
    checkpoint_id: &str,
    created: &[String],
) -> Option<BTreeSet<String>> {
    let tmp_index = temp_index_path(checkpoint_id, "verify");
    let _ = std::fs::remove_file(&tmp_index);
    let envs: &[(&str, &OsStr)] = &[("GIT_INDEX_FILE", tmp_index.as_os_str())];
    let post = command_post_image_ref(checkpoint_id);

    let result = async {
        let read = git_cmd_env(&["read-tree", &post], repo_root, envs)
            .await
            .ok()?;
        if !read.status.success() {
            return None;
        }
        // A just-read index carries no stat data, and `diff-files` decides on
        // stat alone, so without this every entry would come back "modified"
        // and undo would remove nothing. `--refresh` is what makes the
        // comparison about content: it re-matches stat info, hashing where the
        // stat differs. It exits non-zero when something genuinely needs
        // updating, which is the answer we are asking for rather than a
        // failure, so only a spawn/timeout error bails out here.
        //
        // **No pathspec**, deliberately, even though only `created` interests
        // us. `git update-index <path>` RE-REGISTERS that path from the working
        // tree, and it keeps doing so when `--refresh` is also passed, so the
        // narrower-looking form silently rewrites each entry to the current
        // content and every file then compares equal. That is the exact inverse
        // of this function's job: it would report a file the user edited after
        // the command as untouched, and undo would delete their edit.
        git_cmd_env(&["update-index", "-q", "--refresh"], repo_root, envs)
            .await
            .ok()?;
        // The index now IS the post image, so `diff-files` reports exactly the
        // paths whose working-tree state has moved away from it since. No
        // pathspec here either: the refresh above already walked the whole
        // index, so scoping this saves nothing, and passing N paths on one argv
        // would put a ceiling on how many created files a command may have.
        let out = git_cmd_env(&["diff-files", "--name-only", "-z"], repo_root, envs)
            .await
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let changed: BTreeSet<&[u8]> = out
            .stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .collect();
        // Returns the UNCHANGED set rather than the changed one, deliberately.
        // The caller removes what is in it, so a set that came back wrongly
        // empty deletes nothing; inverted, the same mistake would delete
        // everything the command created.
        Some(
            created
                .iter()
                .filter(|p| !changed.contains(p.as_bytes()))
                .cloned()
                .collect(),
        )
    }
    .await;
    let _ = std::fs::remove_file(&tmp_index);
    result
}

/// Remove the files the checkpointed command created, then prune the
/// directories that leaves empty. Returns how many files were actually removed.
///
/// A file is removed only when it still matches what the command wrote, per
/// [`unchanged_since_post_image`]. Anything edited since, already gone, or
/// unresolvable is left alone: this path deletes user data, so every uncertainty
/// resolves to keeping the file, and a git failure that leaves the comparison
/// unanswerable removes nothing at all.
pub(crate) async fn remove_created_files(
    repo_root: &Path,
    checkpoint_id: &str,
    created: &[String],
) -> u32 {
    if created.is_empty() {
        return 0;
    }
    let Some(unchanged) = unchanged_since_post_image(repo_root, checkpoint_id, created).await
    else {
        crate::log!(
            "[CommandGuard] could not compare {} created file(s) against the post image; \
             keeping all of them",
            created.len()
        );
        return 0;
    };

    let mut removed = 0u32;
    let mut emptied: BTreeSet<PathBuf> = BTreeSet::new();
    for path in created {
        if !unchanged.contains(path) {
            crate::log!(
                "[CommandGuard] {} changed since the command ran; keeping it",
                path
            );
            continue;
        }
        let Some(abs) = safe_repo_path(repo_root, path) else {
            crate::log!(
                "[CommandGuard] refusing to remove {} (escapes the workspace)",
                path
            );
            continue;
        };
        // `symlink_metadata` rather than `exists()`, which follows the link and
        // so reports a dangling symlink as absent, leaving it behind.
        if std::fs::symlink_metadata(&abs).is_err() {
            continue;
        }
        match std::fs::remove_file(&abs) {
            Ok(()) => {
                removed += 1;
                if let Some(parent) = abs.parent() {
                    emptied.insert(parent.to_path_buf());
                }
            }
            Err(e) => crate::log!("[CommandGuard] failed to remove {}: {}", path, e),
        }
    }
    prune_empty_dirs(repo_root, emptied);
    removed
}

/// Remove directories left empty by the removals above, walking up to (but
/// never including) `repo_root`. `remove_dir` refuses a non-empty directory, so
/// this cannot take out a directory that still holds anything, and a directory
/// that predates the command survives as soon as one of its other entries does.
fn prune_empty_dirs(repo_root: &Path, dirs: BTreeSet<PathBuf>) {
    let Ok(root) = std::fs::canonicalize(repo_root) else {
        return;
    };
    for dir in dirs {
        let mut cursor = dir;
        loop {
            let Ok(resolved) = std::fs::canonicalize(&cursor) else {
                break;
            };
            if resolved == root || !resolved.starts_with(&root) {
                break;
            }
            if std::fs::remove_dir(&cursor).is_err() {
                break;
            }
            let Some(parent) = cursor.parent().map(Path::to_path_buf) else {
                break;
            };
            cursor = parent;
        }
    }
}

/// Best-effort delete of both refs of one checkpoint. Used when the command
/// turned out to change nothing git-visible (no event is emitted, so nothing
/// would ever read them again) and by the retention sweep. Logs on failure: a
/// leftover ref is harmless, it just pins a cheap, deduped commit object.
pub(crate) async fn delete_command_checkpoint_pair(repo_root: &Path, checkpoint_id: &str) {
    for ref_name in [
        command_checkpoint_ref(checkpoint_id),
        command_post_image_ref(checkpoint_id),
    ] {
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
}

/// Reclaim checkpoint pairs older than `max_age_secs`, judged by the pre image's
/// committer date. Best-effort and silent when nothing has expired; scoped to
/// the two checkpoint namespaces, so no other ref can be reached from here.
pub(crate) async fn prune_expired_checkpoints(repo_root: &Path, now_unix: i64, max_age_secs: i64) {
    let Ok(out) = git_cmd(
        &[
            "for-each-ref",
            "--format=%(refname) %(committerdate:unix)",
            "refs/lucidos/command-checkpoints/",
        ],
        repo_root,
    )
    .await
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let listing = String::from_utf8_lossy(&out.stdout);
    let cutoff = now_unix - max_age_secs;
    let mut expired = 0u32;
    for line in listing.lines() {
        let Some((ref_name, date)) = line.rsplit_once(' ') else {
            continue;
        };
        let Ok(committed) = date.trim().parse::<i64>() else {
            continue;
        };
        if committed > cutoff {
            continue;
        }
        let Some(id) = ref_name.strip_prefix("refs/lucidos/command-checkpoints/") else {
            continue;
        };
        delete_command_checkpoint_pair(repo_root, id).await;
        expired += 1;
    }
    if expired > 0 {
        crate::log!(
            "[CommandGuard] reclaimed {} expired checkpoint pair(s)",
            expired
        );
    }
}

#[cfg(test)]
#[path = "checkpoint_tests.rs"]
mod tests;
