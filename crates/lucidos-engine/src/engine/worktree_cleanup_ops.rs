use super::*;

/// INVARIANT: Recognize a deterministic worktree directory name (`thread-<8-hex>`) and
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

pub(crate) fn parse_temp_change_id(dir_name: &str) -> Option<Uuid> {
    for prefix in TEMP_WORKTREE_PREFIXES {
        if let Some(suffix) = dir_name.strip_prefix(prefix) {
            return Uuid::parse_str(suffix).ok();
        }
    }
    None
}

/// INVARIANT: True iff `child` is strictly inside `parent` after canonicalization. Used to
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskReading {
    pub free_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskThresholds {
    pub soft_bytes: u64,
    pub hard_bytes: u64,
}

impl DiskThresholds {
    pub(crate) fn for_volume(total_bytes: u64, soft_cap_bytes: u64, hard_cap_bytes: u64) -> Self {
        Self {
            soft_bytes: soft_cap_bytes.min(total_bytes / 4),
            hard_bytes: hard_cap_bytes.min(total_bytes / 10),
        }
    }

    pub fn for_reading(
        reading: Option<DiskReading>,
        soft_cap_bytes: u64,
        hard_cap_bytes: u64,
    ) -> Self {
        let total_bytes = reading.map_or(u64::MAX, |r| r.total_bytes);
        Self::for_volume(total_bytes, soft_cap_bytes, hard_cap_bytes)
    }
}

/// INVARIANT: Free and total space on the filesystem hosting `path`.
///
/// Returns `None` when the path doesn't exist or the platform call fails.
/// We call this on `<workspace>/.lucidos/worktrees/` (or the workspace root
/// when the worktrees dir is missing) — both live on the same volume.
/// On ZFS the OS total shrinks as the pool fills, so [`zfs::volume_size_bytes`]
/// gives it.
pub(crate) fn disk_reading(path: &Path) -> Option<DiskReading> {
    let free_bytes = fs2::available_space(path).ok()?;
    let os_total_bytes = fs2::total_space(path).ok()?;
    let total_bytes = if zfs::is_zfs(path) {
        zfs::volume_size_bytes(path, os_total_bytes)
    } else {
        os_total_bytes
    };
    Some(DiskReading {
        free_bytes,
        total_bytes,
    })
}

pub fn volume_reading(worktrees_dir: &Path, workspace_root: &Path) -> Option<DiskReading> {
    disk_reading(worktrees_dir).or_else(|| disk_reading(workspace_root))
}

/// INVARIANT: Reads the workspace volume. Every caller probes again at the moment it
/// decides, because free space can change by hundreds of GB while a cycle
/// waits on the database. Tests script the readings.
pub(crate) type FreeDiskProbe = Arc<dyn Fn() -> Option<DiskReading> + Send + Sync>;

pub(crate) fn os_free_disk_probe(workspace_root: PathBuf) -> FreeDiskProbe {
    let dir = worktrees_dir(&workspace_root);
    Arc::new(move || volume_reading(&dir, &workspace_root))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PressureLevel {
    Comfortable,
    Soft,
    Hard,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DiskPressure {
    pub free_bytes: Option<u64>,
    pub under_soft: bool,
    pub under_hard: bool,
}

impl DiskPressure {
    /// INVARIANT: A failed probe reads as no pressure: it never opens the retention gate.
    pub(crate) fn classify(
        reading: Option<DiskReading>,
        soft_cap_bytes: u64,
        hard_cap_bytes: u64,
    ) -> Self {
        let thresholds = DiskThresholds::for_reading(reading, soft_cap_bytes, hard_cap_bytes);
        let free_bytes = reading.map(|r| r.free_bytes);
        Self {
            free_bytes,
            under_soft: free_bytes.is_some_and(|free| free < thresholds.soft_bytes),
            under_hard: free_bytes.is_some_and(|free| free < thresholds.hard_bytes),
        }
    }

    /// INVARIANT: Tier 1 idle window: [`TIER_1_IDLE`], `forced` under soft pressure, and
    /// zero under hard pressure. Liveness, not idle age, protects a tree then.
    pub(crate) fn tier1_idle(&self, forced: Duration) -> Duration {
        if self.under_hard {
            Duration::ZERO
        } else if self.under_soft {
            forced
        } else {
            TIER_1_IDLE
        }
    }

    pub(crate) fn level(&self) -> PressureLevel {
        if self.under_hard {
            PressureLevel::Hard
        } else if self.under_soft {
            PressureLevel::Soft
        } else {
            PressureLevel::Comfortable
        }
    }

    pub(crate) fn zero_info_grace(&self) -> Duration {
        if self.under_hard {
            Duration::ZERO
        } else {
            TIER_0_GRACE
        }
    }

    /// INVARIANT: Archived-removal grace: [`ARCHIVED_REMOVAL_GRACE`], or zero under hard
    /// pressure. Liveness, not idle age, protects a tree then.
    pub(crate) fn archived_grace(&self) -> Duration {
        if self.under_hard {
            Duration::ZERO
        } else {
            ARCHIVED_REMOVAL_GRACE
        }
    }

    pub(crate) fn hard_reclaimed(&self, freed: Option<u64>) -> u64 {
        match freed {
            Some(bytes) if self.under_hard => bytes,
            _ => 0,
        }
    }
}

pub(crate) fn directory_age(path: &Path) -> Option<Duration> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    mtime.elapsed().ok()
}

pub async fn directory_size_bytes(path: &Path) -> u64 {
    measure_on_blocking_pool(path, walk_directory_size_bytes).await
}

pub(super) async fn measure_on_blocking_pool(path: &Path, walk: fn(&Path) -> u64) -> u64 {
    let walk_path = path.to_path_buf();
    match tokio::task::spawn_blocking(move || walk(&walk_path)).await {
        Ok(bytes) => bytes,
        Err(e) => {
            log_legacy!(
                "[WorktreeCleanup] size walk of {} failed: {}",
                path.display(),
                e
            );
            0
        }
    }
}

/// INVARIANT: The blocking walk behind [`directory_size_bytes`]. Best-effort: silently
/// skips entries it can't stat. Call it only from the blocking pool.
pub(super) fn walk_directory_size_bytes(path: &Path) -> u64 {
    directory_size_bytes_skipping(path, &[])
}

fn directory_size_bytes_skipping(path: &Path, skip: &[PathBuf]) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                let child = entry.path();
                if !skip.contains(&child) {
                    stack.push(child);
                }
            } else if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
            // INVARIANT: Symlinks: counted as 0; we do not follow.
        }
    }
    total
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct WorktreeSize {
    pub total_bytes: u64,
    pub artifact_bytes: u64,
}

/// INVARIANT: Measure a worktree in one walk: every file is read once, either inside one
/// of `artifact_dirs` or outside all of them. Pass [`prunable_artifact_dirs`],
/// so the artifact share is what a strip would free.
pub(crate) fn worktree_size_breakdown(worktree: &Path, artifact_dirs: &[&str]) -> WorktreeSize {
    let artifact_dirs: Vec<PathBuf> = artifact_dirs.iter().map(|sub| worktree.join(sub)).collect();
    let artifact_bytes: u64 = artifact_dirs
        .iter()
        .map(|dir| walk_directory_size_bytes(dir))
        .sum();
    let rest = directory_size_bytes_skipping(worktree, &artifact_dirs);
    WorktreeSize {
        total_bytes: rest.saturating_add(artifact_bytes),
        artifact_bytes,
    }
}

/// INVARIANT: A *finished worktree* holds nothing that removing it would lose: no owed
/// fan-in, no pending change, a clean `git status`, and a branch with no
/// commits ahead of main. Tier 0 and the recommended cleanup both ask this.
/// Liveness and pins are the caller's question.
///
/// Every unanswered probe answers "not finished", because "finished"
/// authorizes a removal.
pub(crate) async fn is_finished_worktree(
    pool: &PgPool,
    changes: &crate::core::changes_projection::ChangesProjection,
    thread_id: Uuid,
    worktree: &Path,
) -> bool {
    if has_pending_fan_in(pool, thread_id).await {
        return false;
    }
    match changes.pending_for_thread(thread_id).await {
        Ok(pending) if pending.is_empty() => {}
        Ok(_) => return false,
        Err(e) => {
            log_legacy!(
                "[WorktreeCleanup] pending_for_thread({}) failed: {}; not finished",
                thread_id,
                e
            );
            return false;
        }
    }
    if is_worktree_dirty(worktree).await {
        return false;
    }
    let Some(branch) = crate::engine::git_ops::worktree_current_branch(worktree).await else {
        return false;
    };
    let Some(repo_root) = resolve_repo_root_from_worktree(worktree).await else {
        return false;
    };
    // INVARIANT: An unanswered `git log` reads as "has commits".
    !has_branch_commits(&repo_root, &branch).await
}

/// INVARIANT: The untracked directories in `worktree` that are git repositories of their
/// own, such as a clone the agent made there. `Err` when git cannot answer.
///
/// With every untracked file listed, git still stops at a nested repository
/// and reports it as one `?? dir/` record, however deep it sits.
pub(crate) async fn untracked_repositories(worktree: &Path) -> Result<Vec<String>, String> {
    let out = git_cmd(
        &["status", "--porcelain", "-z", "--untracked-files=all"],
        worktree,
    )
    .await?;
    if !out.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter_map(|record| record.strip_prefix("?? "))
        .filter(|path| path.ends_with('/') && worktree.join(path).join(".git").exists())
        .map(str::to_string)
        .collect())
}

pub(crate) async fn thread_is_saved(pool: &PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT is_saved FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(p,)| p).unwrap_or(false))
}

pub(crate) async fn has_pending_fan_in(pool: &PgPool, thread_id: Uuid) -> bool {
    match sqlx::query_scalar::<_, bool>(
        "SELECT t.active_children_count > 0 OR EXISTS ( \
             SELECT 1 FROM thread_summaries c \
             WHERE c.parent_thread_id = t.thread_id AND c.is_stopped_child \
               AND c.archive_state <> 'archived') \
         FROM thread_summaries t WHERE t.thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(true)) => return true,
        Ok(_) => {}
        Err(e) => {
            log_legacy!(
                "[WorktreeCleanup] children lookup failed for thread {}: {}; keeping worktree",
                thread_id,
                e
            );
            return true;
        }
    }
    // INVARIANT: (b) a completed-but-unprocessed child completion is the thread's last
    // persisted word (no resume emitted a later event). A sibling's
    // `ChildThreadStopped`, `ChildThreadDetached` or `ChildReportFiled` note
    // wakes nothing, so it cannot have processed the card, and it must not
    // hide it. Nor can a side question, which no agent reads.
    match sqlx::query_scalar::<_, String>(
        "SELECT event_type FROM events \
         WHERE aggregate = 'thread' AND aggregate_id = $1::text \
           AND event_type <> ALL($2) \
           AND event_type <> ALL($3) \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(crate::engine::thread_events::ThreadEvent::PARENT_NOTE_EVENT_TYPES)
    .bind(crate::engine::thread_events::ThreadEvent::SIDE_QUESTION_EVENT_TYPES)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(event_type)) => event_type == "ChildThreadCompleted",
        Ok(None) => false,
        Err(e) => {
            log_legacy!(
                "[WorktreeCleanup] latest-event lookup failed for thread {}: {}; keeping worktree",
                thread_id,
                e
            );
            true
        }
    }
}

pub async fn resolve_repo_root_from_worktree(worktree: &Path) -> Option<PathBuf> {
    let common = crate::engine::git_ops::git_common_dir(worktree)
        .await
        .ok()?;
    Some(common.parent()?.to_path_buf())
}

/// INVARIANT: True iff `worktree` is a *stranded* linked worktree: its `.git` file points
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
pub fn worktree_git_admin_missing(worktree: &Path) -> bool {
    let dotgit = worktree.join(".git");
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

/// INVARIANT: Remove a *stranded* worktree (see [`worktree_git_admin_missing`]) by plain
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
pub async fn remove_stranded_worktree(
    worktrees_dir: &Path,
    worktree: &Path,
    pre_size: u64,
) -> Option<u64> {
    if !is_safe_subpath(worktrees_dir, worktree) {
        log_legacy!(
            "[WorktreeCleanup] refusing stranded removal outside worktrees dir: {}",
            worktree.display()
        );
        return None;
    }
    match tokio::fs::remove_dir_all(worktree).await {
        Ok(()) => Some(pre_size),
        Err(e) => {
            log_legacy!(
                "[WorktreeCleanup] stranded remove_dir_all failed for {}: {}",
                worktree.display(),
                e
            );
            None
        }
    }
}

pub async fn is_worktree_dirty(worktree: &Path) -> bool {
    crate::engine::git_ops::worktree_dirtiness(worktree)
        .await
        .or_unknown(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchDisposal {
    Keep,
    WhenMerged,
    /// INVARIANT: Delete the branch whatever it holds. Two callers pass this. A thread
    /// delete does: the owner asked for the thread and its work to be gone,
    /// and the confirmation said unapplied branch work goes with it. Once the
    /// thread's events are removed, nothing can resolve this branch back to a
    /// thread. The landing lane does for a closed batch's integration branch,
    /// which is its own.
    Always,
}

pub struct RemoveWorktreeOutcome {
    pub freed_bytes: u64,
    pub branch_deleted: bool,
    /// INVARIANT: The branch the worktree was on, read before the removal. `None` for a
    /// detached HEAD or a git call that could not answer.
    ///
    /// Carried so a caller can clean up rows keyed on the branch. The delete
    /// path sweeps `hardened_branches` and `planned_branches` with it.
    pub branch: Option<String>,
    pub repo_root: std::path::PathBuf,
}

pub async fn prune_build_artifacts(worktree: &Path) -> Option<u64> {
    let mut freed: u64 = 0;
    let mut pruned: Vec<&'static str> = Vec::new();
    for sub in prunable_artifact_dirs(worktree).await {
        let target = worktree.join(sub);
        let size = directory_size_bytes(&target).await;
        match tokio::fs::remove_dir_all(&target).await {
            Ok(()) => {
                freed = freed.saturating_add(size);
                pruned.push(sub);
            }
            Err(e) => {
                log_legacy!(
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
        log_legacy!(
            "[WorktreeCleanup] pruned build artifacts in {}: {:?} ({} bytes)",
            worktree.display(),
            pruned,
            freed
        );
        Some(freed)
    }
}

pub(crate) async fn prunable_artifact_dirs(worktree: &Path) -> Vec<&'static str> {
    let mut prunable = Vec::new();
    for &sub in TIER_1_PRUNE_DIRS {
        let target = worktree.join(sub);
        if !target.exists() {
            continue;
        }
        if !is_safe_subpath(worktree, &target) {
            log_legacy!(
                "[WorktreeCleanup] refusing prune of suspicious path {}",
                target.display()
            );
            continue;
        }
        if holds_tracked_files(worktree, sub).await {
            log_legacy!(
                "[WorktreeCleanup] not pruning {}: it holds tracked files, or git could not say",
                target.display()
            );
            continue;
        }
        prunable.push(sub);
    }
    prunable
}

async fn holds_tracked_files(worktree: &Path, sub: &str) -> bool {
    crate::engine::git_ops::git_answer_when_ok(&["ls-files", "-z", "--", sub], worktree, |o| {
        !o.stdout.is_empty()
    })
    .await
    .or_unknown(true)
}

pub(crate) const RECLAIMING_PREFIX: &str = "reclaiming-";

/// INVARIANT: Move `worktree` aside to a `reclaiming-` sibling, then drop git's
/// registration of the old path. Returns the new path.
///
/// The rename is atomic on one volume. So a crash leaves the whole tree at its
/// path or a `reclaiming-` directory, never a half-deleted tree. Such a tree
/// reads as dirty, and a later snapshot would commit its deletions.
pub(crate) async fn move_worktree_aside(worktree: &Path, repo_root: &Path) -> Option<PathBuf> {
    let name = worktree.file_name()?.to_string_lossy();
    let aside = worktree.with_file_name(format!("{RECLAIMING_PREFIX}{name}"));
    if aside.exists() && !remove_reclaiming_dir(&aside).await {
        return None;
    }
    if let Err(e) = tokio::fs::rename(worktree, &aside).await {
        log_legacy!(
            "[WorktreeCleanup] could not move {} aside: {}",
            worktree.display(),
            e
        );
        return None;
    }
    crate::engine::git_ops::prune_worktrees(repo_root).await;
    Some(aside)
}

pub(crate) async fn remove_reclaiming_dir(dir: &Path) -> bool {
    match tokio::fs::remove_dir_all(dir).await {
        Ok(()) => true,
        Err(_) if !dir.exists() => true,
        Err(e) => {
            log_legacy!(
                "[WorktreeCleanup] remove_dir_all failed for {}: {}; the next pass retries",
                dir.display(),
                e
            );
            false
        }
    }
}

/// INVARIANT: Remove a worktree directory, and delete its branch per `disposal`.
///
/// It deletes the tree whatever it holds, so a caller gates it on
/// [`is_worktree_dirty`] first, or commits the dirt. Three callers skip the
/// gate. The disk-usage endpoint's Tier 3 and a thread delete do, because the
/// user confirmed a destructive action naming this tree. The landing lane does
/// for the lane worktree of a closed batch, which holds nobody's work: the
/// batch landed, or failed whole. Add a caller and you add the gate with it.
/// `rg is_worktree_dirty` lists the current set.
///
/// The tree is moved aside before anything is deleted ([`move_worktree_aside`]),
/// so a crash midway never leaves a partial tree at `worktree`.
///
/// `pre_size` lets the Tier 2 worker pass the directory size it already
/// surveyed at the top of `run_once`, avoiding a second walk of the same
/// tree (worktrees can be tens of GB). Callers without a precomputed size
/// pass `None` and the helper measures.
///
/// Returns `None`, with the tree untouched, when the repo root cannot be
/// resolved or the tree cannot be moved aside. The caller decides whether that
/// is a hard error.
pub async fn remove_worktree_and_optionally_delete_branch(
    worktree: &Path,
    pre_size: Option<u64>,
    disposal: BranchDisposal,
) -> Option<RemoveWorktreeOutcome> {
    // INVARIANT: Capture the branch + repo root BEFORE we remove the worktree —
    // once the directory is gone we can't read either.
    let branch = crate::engine::git_ops::worktree_current_branch(worktree).await;
    let repo_root = match resolve_repo_root_from_worktree(worktree).await {
        Some(p) => p,
        None => {
            log_legacy!(
                "[WorktreeCleanup] cannot resolve repo root from worktree {}",
                worktree.display()
            );
            return None;
        }
    };

    let total_size = match pre_size {
        Some(size) => size,
        None => directory_size_bytes(worktree).await,
    };

    let aside = move_worktree_aside(worktree, &repo_root).await?;

    // INVARIANT: The default branch can be checked out in an agent worktree and reads
    // as merged into itself. Preserve it even when branch disposal is requested.
    let mut branch_deleted = false;
    if let Some(branch_name) = branch
        .as_deref()
        .filter(|_| disposal != BranchDisposal::Keep)
    {
        let kept_because =
            if branch_name == crate::engine::git_ops::default_local_branch(&repo_root).await {
                Some("it is the repo's default branch")
            } else if disposal == BranchDisposal::WhenMerged
                && has_branch_commits(&repo_root, branch_name).await
            {
                Some("it has unmerged commits")
            } else {
                None
            };
        if let Some(reason) = kept_because {
            log_legacy!(
                "[WorktreeCleanup] preserving branch {}: {}",
                branch_name,
                reason
            );
        } else {
            match git_cmd(&["branch", "-D", branch_name], &repo_root).await {
                Ok(o) if o.status.success() => {
                    branch_deleted = true;
                    log_legacy!(
                        "[WorktreeCleanup] deleted branch {} ({:?})",
                        branch_name,
                        disposal
                    );
                }
                Ok(o) => log_legacy!(
                    "[WorktreeCleanup] git branch -D {} failed: {}",
                    branch_name,
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
                Err(e) => log_legacy!(
                    "[WorktreeCleanup] git branch -D {} errored: {}",
                    branch_name,
                    e
                ),
            }
        }
    }

    let freed_bytes = if remove_reclaiming_dir(&aside).await {
        total_size
    } else {
        0
    };

    Some(RemoveWorktreeOutcome {
        freed_bytes,
        branch_deleted,
        branch,
        repo_root,
    })
}

#[derive(Debug, serde::Serialize)]
pub struct WorktreeInventoryRow {
    pub thread_id: Uuid,
    pub thread_title: Option<String>,
    pub worktree_path: String,
    pub size_bytes: u64,
    pub artifact_bytes: u64,
    pub last_activity: Option<chrono::DateTime<Utc>>,
    pub is_dirty: bool,
    pub is_saved: bool,
    pub is_active: bool,
    pub is_finished: bool,
}

pub async fn inventory_worktrees(
    pool: &sqlx::PgPool,
    workspace_root: &Path,
    active_threads: &dyn ActiveThreads,
) -> Vec<WorktreeInventoryRow> {
    let changes = crate::core::changes_projection::ChangesProjection::new(pool.clone());
    let dir = worktrees_dir(workspace_root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            log_legacy!(
                "[WorktreeCleanup] cannot read worktrees dir {}: {}",
                dir.display(),
                e
            );
            return Vec::new();
        }
    };
    let candidates: Vec<(PathBuf, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let short = parse_thread_short(path.file_name()?.to_str()?)?;
            Some((path, short))
        })
        .collect();
    let mut rows: Vec<WorktreeInventoryRow> = {
        use futures::StreamExt;
        futures::stream::iter(
            candidates
                .into_iter()
                .map(|(path, short)| inventory_row(pool, &changes, active_threads, path, short)),
        )
        .buffer_unordered(INVENTORY_CONCURRENCY)
        .filter_map(std::future::ready)
        .collect()
        .await
    };
    rows.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    rows
}

const INVENTORY_CONCURRENCY: usize = 4;

async fn inventory_row(
    pool: &sqlx::PgPool,
    changes: &crate::core::changes_projection::ChangesProjection,
    active_threads: &dyn ActiveThreads,
    path: PathBuf,
    short: String,
) -> Option<WorktreeInventoryRow> {
    let thread_id = match lookup_thread_by_short(pool, &short).await {
        ShortThreadLookup::Found(id) => id,
        ShortThreadLookup::NotFound | ShortThreadLookup::Unknown => return None,
    };
    let prunable = prunable_artifact_dirs(&path).await;
    let walk_path = path.clone();
    let size = tokio::task::spawn_blocking(move || worktree_size_breakdown(&walk_path, &prunable));
    let (size, is_dirty, (title, is_saved), last_activity, is_active) = tokio::join!(
        size,
        is_worktree_dirty(&path),
        lookup_thread_summary(pool, thread_id),
        lookup_last_activity(pool, thread_id),
        active_threads.is_active(thread_id),
    );
    let size = size.unwrap_or_else(|e| {
        log_legacy!(
            "[WorktreeCleanup] size walk of {} failed: {}",
            path.display(),
            e
        );
        WorktreeSize::default()
    });
    let is_finished =
        !is_dirty && !is_active && is_finished_worktree(pool, changes, thread_id, &path).await;
    Some(WorktreeInventoryRow {
        thread_id,
        thread_title: title,
        worktree_path: path.to_string_lossy().into_owned(),
        size_bytes: size.total_bytes,
        artifact_bytes: size.artifact_bytes,
        last_activity,
        is_dirty,
        is_saved,
        is_active,
        is_finished,
    })
}

/// INVARIANT: The answer to "which thread owns this `thread-<8-hex>` directory?".
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
    Found(Uuid),
    NotFound,
    Unknown,
}

pub(crate) async fn lookup_thread_by_short(pool: &sqlx::PgPool, short: &str) -> ShortThreadLookup {
    let pattern = format!("{}%", short);
    let rows: Vec<(Uuid,)> = match sqlx::query_as(SHORT_THREAD_LOOKUP_SQL)
        .bind(pattern)
        .bind(short)
        .bind(prefix_upper_bound(short))
        .fetch_all(pool)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            log_legacy!(
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
            log_legacy!(
                "[WorktreeCleanup] short id {} matches more than one thread, skipping",
                short
            );
            ShortThreadLookup::Unknown
        }
    }
}

/// INVARIANT: Binds: the LIKE pattern, the short id, and [`prefix_upper_bound`] of it.
///
/// It matches on `aggregate_id`, the canonical per-event thread id, not the
/// legacy `thread_id` column. The LIKE decides the match. The byte-order range
/// beside it is what `idx_events_thread_aggregate_id_pattern` serves, even
/// under a generic plan, where a bound LIKE pattern cannot use an index.
pub(crate) const SHORT_THREAD_LOOKUP_SQL: &str = "SELECT DISTINCT aggregate_id::uuid FROM events \
     WHERE aggregate = 'thread' AND aggregate_id LIKE $1 \
       AND aggregate_id ~>=~ $2 AND aggregate_id ~<~ $3 \
     LIMIT 2";

pub(crate) fn prefix_upper_bound(prefix: &str) -> String {
    let mut bytes = prefix.as_bytes().to_vec();
    if let Some(last) = bytes.last_mut() {
        *last += 1;
    }
    String::from_utf8(bytes).expect("a hex digit plus one is still ASCII")
}

pub(crate) async fn last_activity_age(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<Duration> {
    let row: Option<(Option<i64>,)> = sqlx::query_as(&format!(
        "SELECT EXTRACT(EPOCH FROM now() - MAX(created))::bigint {THREAD_EVENT_CREATED_FENCED}"
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .ok()?;
    let secs = row.and_then(|(opt,)| opt)?;
    Some(Duration::from_secs(secs.max(0) as u64))
}

/// INVARIANT: The `FROM` clause both last-activity reads aggregate `MAX(created)` over.
///
/// `OFFSET 0` fences the subquery, so the planner reads the thread's rows
/// through an index on `aggregate_id`. Unfenced, it answers
/// `MAX(created)` by walking `idx_events_created` backwards and filtering,
/// which reads millions of rows for a quiet thread. Keep `MAX(created)` rather
/// than the newest row by `sequence`: a backfill writes old-dated rows late,
/// and reading one as the newest would make a live thread look idle.
pub(crate) const THREAD_EVENT_CREATED_FENCED: &str = "FROM (SELECT created FROM events \
     WHERE aggregate = 'thread' AND aggregate_id = $1::text OFFSET 0) thread_events";

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
    let row: Option<(Option<chrono::DateTime<Utc>>,)> = sqlx::query_as(&format!(
        "SELECT MAX(created) {THREAD_EVENT_CREATED_FENCED}"
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .ok()?;
    row.and_then(|(opt,)| opt)
}
