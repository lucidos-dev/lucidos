use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use super::{git_answer, git_cmd, git_cmd_env, lucidos_identity_env};

pub(crate) const CHECKPOINT_RETENTION_SECS: i64 = 30 * 24 * 60 * 60;

pub fn command_checkpoint_ref(checkpoint_id: &str) -> String {
    format!("refs/lucidos/command-checkpoints/{checkpoint_id}")
}

/// INVARIANT: The ref a checkpoint's **post** image lives on. A separate namespace rather
/// than a suffix under the pre ref, because git cannot hold both
/// `refs/x/<id>` and `refs/x/<id>/post` at once (a directory/file conflict).
pub fn command_post_image_ref(checkpoint_id: &str) -> String {
    format!("refs/lucidos/command-post-images/{checkpoint_id}")
}

struct TempIndex(tempfile::TempDir);

impl TempIndex {
    fn new() -> Result<Self, String> {
        tempfile::Builder::new()
            .prefix("lucidos-checkpoint-")
            .tempdir()
            .map(TempIndex)
            .map_err(|e| format!("create a checkpoint index dir: {e}"))
    }

    fn path(&self) -> PathBuf {
        self.0.path().join("index")
    }
}

fn ok_stdout(out: std::process::Output, what: &str) -> Result<String, String> {
    if !out.status.success() {
        return Err(format!(
            "{what}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn snapshot_onto_ref(repo_root: &Path, ref_name: &str, message: &str) -> Result<(), String> {
    let index = TempIndex::new()?;
    let tmp_index = index.path();
    let mut env = vec![("GIT_INDEX_FILE", tmp_index.as_os_str())];
    env.extend(lucidos_identity_env());
    let envs: &[(&str, &OsStr)] = &env;

    async {
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
    .await
}

pub(crate) async fn create_command_checkpoint(
    repo_root: &Path,
    checkpoint_id: &str,
) -> Result<(), String> {
    prune_expired_checkpoints(
        repo_root,
        chrono::Utc::now().timestamp(),
        CHECKPOINT_RETENTION_SECS,
    )
    .await;
    snapshot_onto_ref(
        repo_root,
        &command_checkpoint_ref(checkpoint_id),
        &format!("lucidos command checkpoint {checkpoint_id}"),
    )
    .await
}

pub(crate) async fn create_command_post_image(
    repo_root: &Path,
    checkpoint_id: &str,
) -> Result<(), String> {
    snapshot_onto_ref(
        repo_root,
        &command_post_image_ref(checkpoint_id),
        &format!("lucidos command post-image {checkpoint_id}"),
    )
    .await
}

/// INVARIANT: Undo a checkpointed command's effect on the working tree. Returns how many
/// created files it removed.
///
/// It runs in the reverse of the command's own order: remove what the command
/// created, then put back what it deleted or overwrote. The order matters when
/// the command swapped a file for a directory, or the reverse. The created
/// path then stands where the deleted one was. A restore run first would take
/// it for the user's work and skip the deleted path.
///
/// A checkpoint with no post image removes nothing and restores only. A diff
/// that fails touches nothing and returns `Err`.
pub(crate) async fn revert_command_effects(
    repo_root: &Path,
    checkpoint_id: &str,
) -> Result<u32, String> {
    let removed = match diff_checkpoint_effects(repo_root, checkpoint_id).await {
        Ok(Some(effects)) => {
            let removed = remove_created_files(repo_root, checkpoint_id, &effects.created).await;
            crate::log_legacy!(
                "[CommandGuard] undo {checkpoint_id}: removed {removed} of {} created",
                effects.removes()
            );
            removed
        }
        Ok(None) => {
            crate::log_legacy!(
                "[CommandGuard] undo {checkpoint_id}: no post image, restoring only"
            );
            0
        }
        Err(e) => return Err(format!("could not diff checkpoint {checkpoint_id}: {e}")),
    };
    restore_command_checkpoint(repo_root, checkpoint_id).await?;
    Ok(removed)
}

/// INVARIANT: Restore from the checkpoint's pre image what the command deleted,
/// overwrote or retyped. [`revert_command_effects`] runs it after removing
/// what the command created. Non-invasive: reads into a throwaway index,
/// never the repo's real one. `Err` if the ref is gone or any git step fails.
///
/// Only the command's own paths are written, and only those the working tree
/// has not moved on from since (see [`restore_targets`]). The pre image holds
/// the WHOLE tree. Writing all of it would revert every later edit.
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

    let targets = restore_targets(repo_root, checkpoint_id).await?;
    if targets.as_ref().is_some_and(Vec::is_empty) {
        crate::log_legacy!("[CommandGuard] undo {checkpoint_id}: nothing left to restore");
        return Ok(());
    }

    let index = TempIndex::new()?;
    let tmp_index = index.path();
    let envs: &[(&str, &OsStr)] = &[("GIT_INDEX_FILE", tmp_index.as_os_str())];

    async {
        let read = git_cmd_env(&["read-tree", &ref_name], repo_root, envs).await?;
        if !read.status.success() {
            return Err(format!(
                "git read-tree (restore): {}",
                String::from_utf8_lossy(&read.stderr).trim()
            ));
        }
        let batches: Vec<Vec<&str>> = match &targets {
            None => vec![vec!["checkout-index", "-a", "-f"]],
            Some(paths) => paths
                .chunks(RESTORE_BATCH)
                .map(|chunk| {
                    let mut args = vec!["checkout-index", "-f", "--"];
                    args.extend(chunk.iter().map(String::as_str));
                    args
                })
                .collect(),
        };
        for args in &batches {
            let checkout = git_cmd_env(args, repo_root, envs).await?;
            if !checkout.status.success() {
                return Err(format!(
                    "git checkout-index (restore): {}",
                    String::from_utf8_lossy(&checkout.stderr).trim()
                ));
            }
        }
        match &targets {
            None => {
                crate::log_legacy!("[CommandGuard] undo {checkpoint_id}: restored the pre image")
            }
            Some(paths) => crate::log_legacy!(
                "[CommandGuard] undo {checkpoint_id}: restored {} file(s)",
                paths.len()
            ),
        }
        Ok(())
    }
    .await
}

const RESTORE_BATCH: usize = 200;

/// INVARIANT: The paths undo writes back from the pre image, or `None` to write all of it.
///
/// A path qualifies when the command deleted, overwrote or retyped it, and the
/// working tree still holds what the command left:
///
/// - Overwritten or retyped: still matches the post image.
/// - Deleted: still absent. A file recreated since is the user's, and kept.
///
/// `None` means the post image is provably missing, so nothing says what the
/// command touched. That is the restore-only shape of a pre-image-only
/// checkpoint. A probe or comparison that could not run is `Err`. An unknown
/// never authorizes overwriting a file.
async fn restore_targets(
    repo_root: &Path,
    checkpoint_id: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(stdout) = checkpoint_diff_tree(repo_root, checkpoint_id).await? else {
        // INVARIANT: `checkpoint_pair_available` folds an unanswered probe into "gone".
        // Writing the whole pre image needs a definite "no post image".
        let post_ref = command_post_image_ref(checkpoint_id);
        let post = git_answer(&["rev-parse", "--verify", "--quiet", &post_ref], repo_root).await;
        if post.or_unknown(true) {
            return Err(format!(
                "could not read the checkpoint's before-and-after pair ({post_ref}); \
                 restoring nothing"
            ));
        }
        return Ok(None);
    };
    let mut deleted = Vec::new();
    let mut changed = Vec::new();
    for (status, path) in diff_tree_records(&stdout) {
        let bucket = match status {
            b'D' => &mut deleted,
            b'M' | b'T' => &mut changed,
            _ => continue,
        };
        match std::str::from_utf8(path) {
            Ok(path) => bucket.push(path.to_string()),
            Err(_) => {
                crate::log_legacy!("[CommandGuard] skipping non-UTF-8 path in checkpoint restore")
            }
        }
    }

    let unchanged = if changed.is_empty() {
        BTreeSet::new()
    } else {
        unchanged_since_post_image(repo_root, checkpoint_id, &changed)
            .await
            .ok_or_else(|| {
                format!(
                    "could not compare {} overwritten file(s) against the post image; \
                     restoring nothing",
                    changed.len()
                )
            })?
    };

    let mut targets: Vec<String> = deleted
        .into_iter()
        .filter(|path| {
            let absent = matches!(
                std::fs::symlink_metadata(repo_root.join(path)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound
            );
            if !absent {
                crate::log_legacy!(
                    "[CommandGuard] {path} was recreated since the command ran; keeping it"
                );
            }
            absent
        })
        .collect();
    for path in changed {
        if unchanged.contains(&path) {
            targets.push(path);
        } else {
            crate::log_legacy!("[CommandGuard] {path} changed since the command ran; keeping it");
        }
    }
    Ok(Some(targets))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CheckpointEffects {
    pub restores: u32,
    pub created: Vec<String>,
}

impl CheckpointEffects {
    pub(crate) fn is_empty(&self) -> bool {
        self.restores == 0 && self.created.is_empty()
    }

    pub(crate) fn removes(&self) -> u32 {
        self.created.len() as u32
    }
}

pub async fn checkpoint_pair_available(repo_root: &Path, checkpoint_id: &str) -> bool {
    for ref_name in [
        command_checkpoint_ref(checkpoint_id),
        command_post_image_ref(checkpoint_id),
    ] {
        let exists = git_answer(&["rev-parse", "--verify", "--quiet", &ref_name], repo_root).await;
        if exists.is_unknown() {
            crate::log_legacy!(
                "[CommandGuard] could not probe {}; treating the checkpoint pair as unavailable",
                ref_name
            );
        }
        if !exists.or_unknown(false) {
            return false;
        }
    }
    true
}

/// INVARIANT: Diff a checkpoint's pre image against its post image.
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
    Ok(checkpoint_diff_tree(repo_root, checkpoint_id)
        .await?
        .map(|stdout| parse_diff_tree_z(&stdout)))
}

async fn checkpoint_diff_tree(
    repo_root: &Path,
    checkpoint_id: &str,
) -> Result<Option<Vec<u8>>, String> {
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
    Ok(Some(out.stdout))
}

fn diff_tree_records(stdout: &[u8]) -> impl Iterator<Item = (u8, &[u8])> + '_ {
    let mut fields = stdout.split(|b| *b == 0);
    std::iter::from_fn(move || loop {
        let meta = fields.next()?;
        if meta.is_empty() {
            continue;
        }
        let path = fields.next()?;
        let Ok(meta) = std::str::from_utf8(meta) else {
            continue;
        };
        let parts: Vec<&str> = meta.trim_start_matches(':').split_whitespace().collect();
        let [_src_mode, _dst_mode, _src_sha, _dst_sha, status] = parts[..] else {
            continue;
        };
        if let Some(&letter) = status.as_bytes().first() {
            return Some((letter, path));
        }
    })
}

fn parse_diff_tree_z(stdout: &[u8]) -> CheckpointEffects {
    let mut effects = CheckpointEffects::default();
    for (status, path) in diff_tree_records(stdout) {
        match status {
            b'A' => {
                let Ok(path) = std::str::from_utf8(path) else {
                    crate::log_legacy!(
                        "[CommandGuard] skipping non-UTF-8 created path in checkpoint diff"
                    );
                    continue;
                };
                effects.created.push(path.to_string());
            }
            b'D' | b'M' | b'T' => effects.restores += 1,
            _ => {}
        }
    }
    effects
}

/// INVARIANT: Resolve a repo-relative path from a checkpoint diff to an absolute path
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
            _ => return None,
        }
    }
    let joined = repo_root.join(rel_path);
    let root = std::fs::canonicalize(repo_root).ok()?;
    let parent = std::fs::canonicalize(joined.parent()?).ok()?;
    if !parent.starts_with(&root) {
        return None;
    }
    Some(joined)
}

/// INVARIANT: Which of `paths` still match the post image. Undo removes a created file
/// only when it does, and restores an overwritten one only when it does.
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
/// `None` on any git failure, which each caller reads as "touch nothing".
async fn unchanged_since_post_image(
    repo_root: &Path,
    checkpoint_id: &str,
    paths: &[String],
) -> Option<BTreeSet<String>> {
    let index = TempIndex::new().ok()?;
    let tmp_index = index.path();
    let envs: &[(&str, &OsStr)] = &[("GIT_INDEX_FILE", tmp_index.as_os_str())];
    let post = command_post_image_ref(checkpoint_id);

    async {
        let read = git_cmd_env(&["read-tree", &post], repo_root, envs)
            .await
            .ok()?;
        if !read.status.success() {
            return None;
        }
        // INVARIANT: A just-read index carries no stat data, and `diff-files` decides on
        // stat alone, so without this every entry would come back "modified"
        // and undo would remove nothing. `--refresh` is what makes the
        // comparison about content: it re-matches stat info, hashing where the
        // stat differs. It exits non-zero when something genuinely needs
        // updating, which is the answer we are asking for rather than a
        // failure, so only a spawn/timeout error bails out here.
        //
        // **No pathspec**, deliberately, even though only `paths` interest
        // us. `git update-index <path>` RE-REGISTERS that path from the working
        // tree, and it keeps doing so when `--refresh` is also passed, so the
        // narrower-looking form silently rewrites each entry to the current
        // content and every file then compares equal. That is the exact inverse
        // of this function's job: it would report a file the user edited after
        // the command as untouched, and undo would delete their edit.
        git_cmd_env(&["update-index", "-q", "--refresh"], repo_root, envs)
            .await
            .ok()?;
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
        // INVARIANT: Returns the UNCHANGED set rather than the changed one, deliberately.
        // The caller removes what is in it, so a set that came back wrongly
        // empty deletes nothing; inverted, the same mistake would delete
        // everything the command created.
        Some(
            paths
                .iter()
                .filter(|p| !changed.contains(p.as_bytes()))
                .cloned()
                .collect(),
        )
    }
    .await
}

/// INVARIANT: Remove the files the checkpointed command created, then prune the
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
        crate::log_legacy!(
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
            crate::log_legacy!(
                "[CommandGuard] {} changed since the command ran; keeping it",
                path
            );
            continue;
        }
        let Some(abs) = safe_repo_path(repo_root, path) else {
            crate::log_legacy!(
                "[CommandGuard] refusing to remove {} (escapes the workspace)",
                path
            );
            continue;
        };
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
            Err(e) => crate::log_legacy!("[CommandGuard] failed to remove {}: {}", path, e),
        }
    }
    prune_empty_dirs(repo_root, emptied);
    removed
}

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

pub(crate) async fn delete_command_checkpoint_pair(repo_root: &Path, checkpoint_id: &str) {
    for ref_name in [
        command_checkpoint_ref(checkpoint_id),
        command_post_image_ref(checkpoint_id),
    ] {
        match git_cmd(&["update-ref", "-d", &ref_name], repo_root).await {
            Ok(out) if out.status.success() => {}
            Ok(out) => crate::log_legacy!(
                "[CommandGuard] failed to delete checkpoint ref {}: {}",
                ref_name,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => crate::log_legacy!(
                "[CommandGuard] failed to delete checkpoint ref {}: {}",
                ref_name,
                e
            ),
        }
    }
}

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
        crate::log_legacy!(
            "[CommandGuard] reclaimed {} expired checkpoint pair(s)",
            expired
        );
    }
}

#[cfg(test)]
#[path = "checkpoint_tests.rs"]
mod tests;
