//! Worktree git/disk helper functions, split out of worktree_cleanup.rs.
use super::*;

/// Recognize a deterministic worktree directory name (`thread-<8-hex>`) and
/// return the lowercase 8-char short id. Returns `None` for legacy /
/// random-suffix names so the caller skips them.
///
/// The short id is used as a prefix lookup against the `events.aggregate_id`
/// column to recover the full thread `Uuid`. We don't reconstruct a Uuid by
/// zero-padding because the original Uuid is random across all 32 hex chars
/// and the padded form would never match.
pub(crate) fn parse_thread_short(dir_name: &str) -> Option<String> {
    let stripped = dir_name.strip_prefix("thread-")?;
    if stripped.len() != SHORT_THREAD_ID_LEN {
        return None;
    }
    if !stripped.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(stripped.to_ascii_lowercase())
}

/// Extract the change id from a temporary worktree dir name
/// (`<prefix><change_id.as_simple()>`, e.g. `harden-7f9717...`). Returns `None`
/// when no known temp prefix matches or the suffix isn't a valid uuid — so a
/// `cc-<uuid>` recovery worktree (different prefix) never parses here.
pub(crate) fn parse_temp_change_id(dir_name: &str) -> Option<Uuid> {
    for prefix in TEMP_WORKTREE_PREFIXES {
        if let Some(suffix) = dir_name.strip_prefix(prefix) {
            return Uuid::parse_str(suffix).ok();
        }
    }
    None
}

/// True iff `child` is strictly inside `parent` after canonicalization. Used to
/// guard `remove_dir_all` against symlink escapes and accidental top-level
/// deletes. Falls back to a literal prefix check if canonicalization fails
/// (e.g. the path no longer exists), which is fine for the worktree-removal
/// path because the failure case is "child doesn't exist" → no harm done.
pub(crate) fn is_safe_subpath(parent: &Path, child: &Path) -> bool {
    let parent_canon = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    let child_canon = child.canonicalize().unwrap_or_else(|_| child.to_path_buf());
    child_canon.starts_with(&parent_canon) && child_canon != parent_canon
}

/// Free space on the filesystem hosting `path`, in bytes.
///
/// Returns `None` when the path doesn't exist or the platform call fails.
/// We call this on `<workspace>/.lucidos/worktrees/` (or the workspace root
/// when the worktrees dir is missing) — both live on the same volume.
pub(crate) fn available_disk_bytes(path: &Path) -> Option<u64> {
    fs2::available_space(path).ok()
}

/// Time since `path`'s mtime, or `None` if the metadata read fails. Used by
/// the orphan-path sweep to apply a grace window without an event stream.
pub(crate) fn directory_age(path: &Path) -> Option<Duration> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    mtime.elapsed().ok()
}

/// Sum file sizes under `path` recursively. Best-effort — silently skips
/// entries we can't stat. Returns 0 if the path doesn't exist.
pub(crate) fn directory_size_bytes(path: &Path) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
            // Symlinks: counted as 0; we do not follow.
        }
    }
    total
}

/// Resolve the main working tree (the one whose `.git` is a real directory,
/// not a `.git` file pointing at `worktrees/<name>`) by asking the worktree
/// itself: `git rev-parse --path-format=absolute --show-superproject-working-tree`
/// returns nothing for a top-level repo, so we use
/// `git rev-parse --git-common-dir` (the canonical `.git` directory) and
/// strip the trailing `.git` segment to get the working tree.
///
/// Returns `None` when the worktree path isn't a recognized git repo (e.g.
/// the user manually deleted `.git/worktrees/<name>` so the worktree is now
/// stranded). Tier 2 callers fall back to a `remove_dir_all` in that case.
pub(crate) async fn resolve_repo_root_from_worktree(worktree: &Path) -> Option<PathBuf> {
    let out = git_cmd(&["rev-parse", "--git-common-dir"], worktree)
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    // `--git-common-dir` may return a relative path (relative to the worktree)
    // or an absolute path. Resolve to absolute against the worktree.
    let common = PathBuf::from(&raw);
    let abs = if common.is_absolute() {
        common
    } else {
        worktree.join(common)
    };
    // Strip the trailing `.git` segment to get the main working tree root.
    let parent = abs.parent()?.to_path_buf();
    Some(parent)
}

/// True iff `worktree` is a *stranded* linked worktree: its `.git` file points
/// at an admin directory (`<repo>/.git/worktrees/<name>`) that no longer
/// exists. In that state every `git` invocation from the worktree fails with
/// `fatal: not a git repository`, so the normal tier logic (which leans on
/// `git status` / `git rev-parse`) can never act on it — and
/// [`remove_worktree_and_optionally_delete_branch`] early-returns because it
/// can't resolve the repo root.
///
/// Conservative by construction: returns `false` for any shape that isn't a
/// positively-broken gitdir link — a real `.git` directory (top-level repo),
/// a `.git` symlink, a missing/unreadable `.git`, or content that isn't
/// `gitdir: …`. We return `true` only when we can read the link target and
/// confirm it's gone, so a healthy worktree is never misclassified as
/// removable. Relative gitdir targets (possible when `worktree.useRelativePaths`
/// is on) are resolved against the worktree dir before the existence check —
/// mirroring [`resolve_repo_root_from_worktree`] — so a working relative link
/// is not a false positive.
pub(crate) fn worktree_git_admin_missing(worktree: &Path) -> bool {
    let dotgit = worktree.join(".git");
    // Only a regular file carries the `gitdir:` link. A directory is a real
    // repo; a symlink is not a shape git produces for worktrees — both mean
    // "not a stranded linked worktree".
    match std::fs::symlink_metadata(&dotgit) {
        Ok(meta) if meta.file_type().is_file() => {}
        _ => return false,
    }
    let Ok(content) = std::fs::read_to_string(&dotgit) else {
        return false;
    };
    let Some(rest) = content.trim().strip_prefix("gitdir:") else {
        return false;
    };
    let target = PathBuf::from(rest.trim());
    let resolved = if target.is_absolute() {
        target
    } else {
        worktree.join(target)
    };
    !resolved.exists()
}

/// Remove a *stranded* worktree (see [`worktree_git_admin_missing`]) by plain
/// directory deletion. Used when git can't act on the path (the admin entry is
/// gone) so [`remove_worktree_and_optionally_delete_branch`] would early-return
/// without reclaiming anything.
///
/// Guarded by [`is_safe_subpath`] against `worktrees_dir`: never deletes a path
/// outside the worktrees directory or the directory itself, and the
/// canonicalization inside the guard blocks a symlinked entry from escaping.
/// Never touches branch refs — committed work lives in the main repo's branch
/// and is preserved; only never-committed edits in the detached working copy
/// are lost. Returns bytes freed, or `None` if the guard rejected the path or
/// the removal failed.
pub(crate) fn remove_stranded_worktree(
    worktrees_dir: &Path,
    worktree: &Path,
    pre_size: u64,
) -> Option<u64> {
    if !is_safe_subpath(worktrees_dir, worktree) {
        log!(
            "[WorktreeCleanup] refusing stranded removal outside worktrees dir: {}",
            worktree.display()
        );
        return None;
    }
    match std::fs::remove_dir_all(worktree) {
        Ok(()) => Some(pre_size),
        Err(e) => {
            log!(
                "[WorktreeCleanup] stranded remove_dir_all failed for {}: {}",
                worktree.display(),
                e
            );
            None
        }
    }
}

/// True iff the worktree has any uncommitted changes (tracked or untracked),
/// OR git could not say. Every caller uses this to decide whether a tree is
/// safe to delete, so an unanswerable `git status` (path is no longer a work
/// tree, a filter such as git-crypt failed, a spawn error, a timeout) counts as
/// dirty: we don't silently nuke a tree we could not describe. The question
/// itself lives in [`worktree_dirtiness`], shared with the session-end path.
pub(crate) async fn is_worktree_dirty(worktree: &Path) -> bool {
    crate::engine::git_ops::worktree_dirtiness(worktree)
        .await
        .or_unknown(true)
}

/// Outcome of [`remove_worktree_and_optionally_delete_branch`].
pub(crate) struct RemoveWorktreeOutcome {
    /// Bytes reclaimed from the on-disk worktree (best-effort directory size
    /// captured before removal).
    pub freed_bytes: u64,
    /// True iff the worktree's branch was fully merged and deleted.
    pub branch_deleted: bool,
}

/// Strip regenerable build artifacts (`target/`, `node_modules/`,
/// `.lucidos/cache/`) from a worktree. Reused by the background cleanup
/// worker (Tier 1) and by the disk-usage settings page so the user can
/// reclaim space on demand without waiting for the hourly tick.
///
/// Returns the bytes freed, or `None` if there was nothing to prune.
pub(crate) fn prune_build_artifacts(worktree: &Path) -> Option<u64> {
    let mut freed: u64 = 0;
    let mut pruned: Vec<&'static str> = Vec::new();
    for sub in TIER_1_PRUNE_DIRS {
        let target = worktree.join(sub);
        if !target.exists() {
            continue;
        }
        if !is_safe_subpath(worktree, &target) {
            log!(
                "[WorktreeCleanup] refusing prune of suspicious path {}",
                target.display()
            );
            continue;
        }
        let size = directory_size_bytes(&target);
        match std::fs::remove_dir_all(&target) {
            Ok(()) => {
                freed = freed.saturating_add(size);
                pruned.push(sub);
            }
            Err(e) => {
                log!(
                    "[WorktreeCleanup] prune failed for {}: {}",
                    target.display(),
                    e
                );
            }
        }
    }
    if pruned.is_empty() {
        None
    } else {
        log!(
            "[WorktreeCleanup] pruned build artifacts in {}: {:?} ({} bytes)",
            worktree.display(),
            pruned,
            freed
        );
        Some(freed)
    }
}

/// Remove a worktree directory and (when its branch is fully merged into
/// main) delete the branch.
///
/// It opens with `git worktree remove --force`, so a caller gates it on
/// [`is_worktree_dirty`] first. Only the disk-usage endpoint's Tier 3 skips
/// the gate, where the user confirmed a force. Add a caller and you add the
/// gate with it. `rg is_worktree_dirty` lists the current set.
///
/// `pre_size` lets the Tier 2 worker pass the directory size it already
/// surveyed at the top of `run_once`, avoiding a second walk of the same
/// tree (worktrees can be tens of GB). Callers without a precomputed size
/// pass `None` and the helper measures.
///
/// Returns `None` when the repo root cannot be resolved from the worktree
/// (e.g. the worktree was already removed manually). The caller decides
/// whether absence is a hard error.
pub(crate) async fn remove_worktree_and_optionally_delete_branch(
    worktree: &Path,
    pre_size: Option<u64>,
) -> Option<RemoveWorktreeOutcome> {
    // Capture the branch + repo root BEFORE we remove the worktree —
    // once the directory is gone we can't read either.
    let branch = crate::engine::git_ops::worktree_current_branch(worktree).await;
    let repo_root = match resolve_repo_root_from_worktree(worktree).await {
        Some(p) => p,
        None => {
            log!(
                "[WorktreeCleanup] cannot resolve repo root from worktree {}",
                worktree.display()
            );
            return None;
        }
    };

    let total_size = pre_size.unwrap_or_else(|| directory_size_bytes(worktree));

    // Use `git worktree remove --force` so git's bookkeeping (the
    // `worktrees/<name>` admin dir under `.git/`) is cleaned up too. If
    // git doesn't know about the path (e.g. it was moved/copied), fall
    // back to `remove_dir_all` so we still reclaim the bytes.
    let removed_via_git = match git_cmd(
        &["worktree", "remove", "--force", &worktree.to_string_lossy()],
        &repo_root,
    )
    .await
    {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            log!(
                "[WorktreeCleanup] git worktree remove failed for {}: {}",
                worktree.display(),
                String::from_utf8_lossy(&o.stderr).trim()
            );
            false
        }
        Err(e) => {
            log!(
                "[WorktreeCleanup] git worktree remove errored for {}: {}",
                worktree.display(),
                e
            );
            false
        }
    };
    if !removed_via_git && worktree.exists() {
        if let Err(e) = std::fs::remove_dir_all(worktree) {
            log!(
                "[WorktreeCleanup] remove_dir_all failed for {}: {}",
                worktree.display(),
                e
            );
            return None;
        }
    }

    // Branch deletion (Phase 10.3): only when fully merged. `has_branch_commits`
    // returns true on error (conservative) so we keep branches when in doubt.
    let mut branch_deleted = false;
    if let Some(branch_name) = branch.as_deref() {
        if !has_branch_commits(&repo_root, branch_name).await {
            match git_cmd(&["branch", "-D", branch_name], &repo_root).await {
                Ok(o) if o.status.success() => {
                    branch_deleted = true;
                    log!(
                        "[WorktreeCleanup] deleted fully-merged branch {}",
                        branch_name
                    );
                }
                Ok(o) => log!(
                    "[WorktreeCleanup] git branch -D {} failed: {}",
                    branch_name,
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
                Err(e) => log!(
                    "[WorktreeCleanup] git branch -D {} errored: {}",
                    branch_name,
                    e
                ),
            }
        } else {
            log!(
                "[WorktreeCleanup] preserving branch {} — has unmerged commits",
                branch_name
            );
        }
    }

    Some(RemoveWorktreeOutcome {
        freed_bytes: total_size,
        branch_deleted,
    })
}

/// One row in the disk-usage inventory served by `/api/v1/disk-usage/worktrees`.
/// Combines on-disk facts (path, size, dirty) with thread metadata
/// (title, last activity, saved).
#[derive(Debug, serde::Serialize)]
pub struct WorktreeInventoryRow {
    pub thread_id: Uuid,
    pub thread_title: Option<String>,
    pub worktree_path: String,
    pub size_bytes: u64,
    pub last_activity: Option<chrono::DateTime<Utc>>,
    pub is_dirty: bool,
    pub is_saved: bool,
}

/// Snapshot the worktrees directory and pair each `thread-<short>` directory
/// with its thread metadata. Sorted by `size_bytes` descending so the disk-
/// usage page shows the largest worktrees first.
///
/// Skips legacy random-suffix worktrees and orphaned directories whose short
/// id doesn't resolve to a thread — same rules as the cleanup worker.
pub(crate) async fn inventory_worktrees(
    pool: &sqlx::PgPool,
    workspace_root: &Path,
) -> Vec<WorktreeInventoryRow> {
    let dir = worktrees_dir(workspace_root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            log!(
                "[WorktreeCleanup] cannot read worktrees dir {}: {}",
                dir.display(),
                e
            );
            return Vec::new();
        }
    };
    let mut rows: Vec<WorktreeInventoryRow> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(short) = parse_thread_short(name) else {
            continue;
        };
        let thread_id = match lookup_thread_by_short(pool, &short).await {
            ShortThreadLookup::Found(id) => id,
            ShortThreadLookup::NotFound | ShortThreadLookup::Unknown => continue,
        };
        let size_bytes = directory_size_bytes(&path);
        let is_dirty = is_worktree_dirty(&path).await;
        let (title, is_saved) = lookup_thread_summary(pool, thread_id).await;
        let last_activity = lookup_last_activity(pool, thread_id).await;
        rows.push(WorktreeInventoryRow {
            thread_id,
            thread_title: title,
            worktree_path: path.to_string_lossy().into_owned(),
            size_bytes,
            last_activity,
            is_dirty,
            is_saved,
        });
    }
    rows.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    rows
}

/// The answer to "which thread owns this `thread-<8-hex>` directory?".
///
/// `Unknown` is the arm that has to stay distinguishable from `NotFound`, for
/// exactly the reason [`crate::engine::git_ops::GitAnswer`] exists on the git
/// side: the caller reacts to `NotFound` by treating the directory as an
/// ORPHAN, and the orphan arm deletes it without consulting
/// `active_threads::is_active`. So a lookup that merely could not run must
/// never be reported as "no thread owns this". That would be an unanswered
/// probe authorizing a worktree deletion, the class that emptied a live
/// coding-agent worktree on 2026-08-03.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortThreadLookup {
    /// Exactly one thread's id carries this prefix.
    Found(Uuid),
    /// The query ran and no thread's id carries this prefix: a real orphan.
    NotFound,
    /// The question could not be answered: a sqlx error, or an ambiguous
    /// prefix matching more than one thread. Callers must skip the entry.
    Unknown,
}

/// Resolve a `thread-<8-hex>` directory's short id back to a full `Uuid`
/// by prefix-matching against the events table. The 8-char prefix is
/// effectively unique across the per-workspace thread space (collision
/// probability ~ N²/2³², so for ~10k threads/workspace the expected
/// collisions are ≪ 1).
///
/// A collision, or any sqlx error, yields [`ShortThreadLookup::Unknown`] so the
/// caller skips the worktree entirely. Refusing to act is always safer than
/// acting on the wrong thread, or than mistaking a database blip for proof that
/// nobody owns the directory.
pub(crate) async fn lookup_thread_by_short(pool: &sqlx::PgPool, short: &str) -> ShortThreadLookup {
    // The id column is uuid; cast to text for the prefix LIKE. We match
    // against `aggregate_id` (the canonical per-event thread id) instead
    // of the legacy `thread_id` column to stay aligned with the rest of
    // the codebase.
    let pattern = format!("{}%", short);
    let rows: Vec<(Uuid,)> = match sqlx::query_as(
        "SELECT DISTINCT aggregate_id::uuid FROM events \
         WHERE aggregate = 'thread' AND aggregate_id LIKE $1 \
         LIMIT 2",
    )
    .bind(pattern)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            log!(
                "[WorktreeCleanup] cannot resolve thread for short id {}: {}. \
                 Treating as unknown, never as an orphan",
                short,
                e
            );
            return ShortThreadLookup::Unknown;
        }
    };
    match rows.len() {
        0 => ShortThreadLookup::NotFound,
        1 => ShortThreadLookup::Found(rows[0].0),
        _ => {
            log!(
                "[WorktreeCleanup] short id {} matches more than one thread, skipping",
                short
            );
            ShortThreadLookup::Unknown
        }
    }
}

/// Time since the most recent thread event, or `None` when no events exist
/// (a stranded worktree) or the lookup fails. The cleanup worker treats
/// `None` as "don't act" rather than guessing.
///
/// The subtraction happens in SQL (ADR 0053). Postgres stamps
/// `events.created`, so `Utc::now() - created` puts the host clock and the
/// database clock on opposite sides of one minus. In dev those are two
/// machines. A container behind the host inflates every age. A thread active
/// minutes ago then crosses `TIER_0_GRACE` and loses its worktree to the
/// retention gate.
///
/// A negative age (the drift pointing the other way) clamps to zero, so a
/// freshly-emitted event never reads as ancient.
pub(crate) async fn last_activity_age(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<Duration> {
    let row: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT EXTRACT(EPOCH FROM now() - MAX(created))::bigint FROM events \
         WHERE aggregate = 'thread' AND aggregate_id = $1::text",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .ok()?;
    let secs = row.and_then(|(opt,)| opt)?;
    Some(Duration::from_secs(secs.max(0) as u64))
}

pub(crate) async fn lookup_thread_summary(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> (Option<String>, bool) {
    let row: Option<(Option<String>, bool)> =
        sqlx::query_as("SELECT title, is_saved FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    match row {
        Some((title, saved)) => (title, saved),
        None => (None, false),
    }
}

pub(crate) async fn lookup_last_activity(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Option<chrono::DateTime<Utc>> {
    let row: Option<(Option<chrono::DateTime<Utc>>,)> = sqlx::query_as(
        "SELECT MAX(created) FROM events \
         WHERE aggregate = 'thread' AND aggregate_id = $1::text",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .ok()?;
    row.and_then(|(opt,)| opt)
}
