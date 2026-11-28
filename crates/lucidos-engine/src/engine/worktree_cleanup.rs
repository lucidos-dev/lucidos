use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use sqlx::PgPool;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::engine::bash_background::BackgroundBashRegistry;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::git_ops::{
    commit_worktree_or_err, git_cmd, has_branch_commits, worktree_current_branch,
    worktree_dirtiness, worktree_liveness, worktrees_dir, GitAnswer, SHORT_THREAD_ID_LEN,
};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::types::AgentSession;
use crate::scheduler::notifications::SettingsPage;

/// INVARIANT: Is something working in this thread's worktree right now? Event age cannot
/// say, because four kinds of work emit nothing while they use the tree:
/// - a session parked on `AskUserQuestion`;
/// - a spawn setting up the worktree before its session registers;
/// - a background task building after its session ended the turn;
/// - a live thread terminal, whose shell sits in the tree (ADR 0356).
///
/// Trait-erased so tests can fake it.
#[async_trait::async_trait]
pub trait ActiveThreads: Send + Sync {
    async fn is_active(&self, thread_id: Uuid) -> bool;
}

pub struct EngineActiveThreads {
    sessions: Arc<tokio::sync::Mutex<HashMap<Uuid, AgentSession>>>,
    background_tasks: BackgroundBashRegistry,
    terminals: crate::engine::terminal::TerminalRegistry,
}

impl EngineActiveThreads {
    pub(crate) fn new(
        sessions: Arc<tokio::sync::Mutex<HashMap<Uuid, AgentSession>>>,
        background_tasks: BackgroundBashRegistry,
        terminals: crate::engine::terminal::TerminalRegistry,
    ) -> Self {
        Self {
            sessions,
            background_tasks,
            terminals,
        }
    }
}

#[async_trait::async_trait]
impl ActiveThreads for EngineActiveThreads {
    async fn is_active(&self, thread_id: Uuid) -> bool {
        let run_holds_thread = self
            .sessions
            .lock()
            .await
            .get(&thread_id)
            .is_some_and(crate::engine::session_claim::holds_thread);
        run_holds_thread
            || self
                .background_tasks
                .has_running_for_thread(thread_id)
                .await
            || self.terminals.has_live(thread_id).await
    }
}

pub use crate::engine::session_queries::deterministic_worktree_path as deterministic_worktree_for;
use crate::engine::session_queries::lookup_session_branch_for_thread;

pub const ARCHIVED_REMOVAL_GRACE: Duration = Duration::from_secs(60 * 60);

pub const SNAPSHOT_COMMIT_MESSAGE: &str = "Coding agent changes (snapshot before worktree removal)";

pub const TIER_1_IDLE: Duration = Duration::from_secs(24 * 60 * 60);

pub const TIER_2_IDLE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

pub const FREE_DISK_SOFT_BYTES: u64 = 20 * 1024 * 1024 * 1024;

pub const FREE_DISK_HARD_BYTES: u64 = 5 * 1024 * 1024 * 1024;

pub const FORCE_TIER_1_IDLE: Duration = Duration::from_secs(60 * 60);

pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(15 * 60);

pub const TIER_0_GRACE: Duration = Duration::from_secs(60 * 60);

pub const RELEASE_ARTIFACTS_IDLE: Duration = TIER_0_GRACE;

/// INVARIANT: Grace window before a *stranded* worktree (its git admin dir under
/// `.git/worktrees/<name>` is gone, so every git call fails) is removed.
/// Equal to [`TIER_0_GRACE`] (1 h) but — unlike Tier 0 — it does NOT drop to
/// 0 under disk pressure. Tier 0 can prove "zero information on disk" via git
/// (clean status, branch at main) before accelerating; a stranded worktree
/// can't be inspected by git at all, so the fixed floor is what rules out
/// nuking an in-flight `git worktree add` whose admin dir is mid-creation.
pub const STRANDED_GRACE: Duration = TIER_0_GRACE;

/// INVARIANT: Grace window (by directory mtime) before an orphaned *temporary* worktree
/// (`harden-`/`apply-`/`merge-` left by a crashed apply/harden/merge flow) is
/// removed. Fixed 2 h, never accelerated under disk pressure: the primary
/// liveness gate is the change row's status (a still-`pending` change may be
/// retried and need its worktree); mtime is only a coarse backstop for the
/// "DB says resolved but cleanup never ran" window. The cost of deleting an
/// in-flight merge/harden worktree (the user re-does conflict resolution) far
/// outweighs the disk reclaimed by accelerating.
pub const TEMP_WORKTREE_GRACE: Duration = Duration::from_secs(2 * 60 * 60);

/// INVARIANT: Directory-name prefixes for the temporary worktrees the apply/harden/merge
/// flows create under the worktrees dir. The background sweep collects these
/// when their change is resolved (see [`WorktreeCleanup::try_temp_worktree`]).
/// `cc-` (random-suffix recovery worktrees) is deliberately NOT here — those
/// are treated as legacy and skipped, same as any non-`thread-<8hex>` name.
const TEMP_WORKTREE_PREFIXES: &[&str] = &["harden-", "apply-", "merge-"];

const BYTES_PER_GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// INVARIANT: Body for the hard-threshold auto-cleanup report.
///
/// The title attributes the action to Lucidos. The body names the volume as
/// the trigger, so the reader does not read it as Lucidos eating disk. It does
/// not claim the volume is still critical: `free_bytes` is read after the
/// reclamation, which may have lifted it above the hard threshold.
fn auto_cleanup_body(free_bytes: u64, freed_bytes: u64) -> String {
    let free_gb = free_bytes as f64 / BYTES_PER_GB;
    let freed_gb = freed_bytes as f64 / BYTES_PER_GB;
    let page = SettingsPage::DISK_USAGE.link();
    format!(
        "Your machine ran critically low on disk, so Lucidos reclaimed {freed_gb:.1} GB \
         from idle coding-agent worktrees. {free_gb:.1} GB is free now. Close saved threads \
         or remove unused worktrees from {page} to reclaim more."
    )
}

const TIER_1_PRUNE_DIRS: &[&str] = &["target", "node_modules", ".lucidos/cache"];

struct ReclaimGate {
    thread_id: Uuid,
    age: Duration,
    archived: Option<bool>,
}

impl ReclaimGate {
    fn new(thread_id: Uuid, age: Duration) -> Self {
        Self {
            thread_id,
            age,
            archived: None,
        }
    }
}

pub struct WorktreeCleanup {
    pool: PgPool,
    bus: Arc<EventBus>,
    workspace_root: PathBuf,
    interval: Duration,
    soft_cap_bytes: u64,
    hard_cap_bytes: u64,
    force_tier1_idle: Duration,
    stranded_grace: Duration,
    temp_worktree_grace: Duration,
    size_walk: fn(&Path) -> u64,
    free_disk: FreeDiskProbe,
    changes: crate::core::changes_projection::ChangesProjection,
    active_threads: Arc<dyn ActiveThreads>,
    cleanup_wake: Arc<Notify>,
}

impl WorktreeCleanup {
    pub fn new(
        pool: PgPool,
        bus: Arc<EventBus>,
        workspace_root: PathBuf,
        active_threads: Arc<dyn ActiveThreads>,
        cleanup_wake: Arc<Notify>,
    ) -> Self {
        let changes = crate::core::changes_projection::ChangesProjection::new(pool.clone());
        Self {
            pool,
            bus,
            free_disk: os_free_disk_probe(workspace_root.clone()),
            workspace_root,
            interval: CLEANUP_INTERVAL,
            soft_cap_bytes: FREE_DISK_SOFT_BYTES,
            hard_cap_bytes: FREE_DISK_HARD_BYTES,
            force_tier1_idle: FORCE_TIER_1_IDLE,
            stranded_grace: STRANDED_GRACE,
            temp_worktree_grace: TEMP_WORKTREE_GRACE,
            size_walk: walk_directory_size_bytes,
            changes,
            active_threads,
            cleanup_wake,
        }
    }

    pub fn spawn(
        pool: PgPool,
        bus: Arc<EventBus>,
        workspace_root: PathBuf,
        active_threads: Arc<dyn ActiveThreads>,
        cleanup_wake: Arc<Notify>,
    ) -> tokio::task::JoinHandle<()> {
        let worker = Self::new(pool, bus, workspace_root, active_threads, cleanup_wake);
        tokio::spawn(async move { worker.run_loop().await })
    }

    /// INVARIANT: Loop forever, waiting [`Self::interval`] or a wake between passes. The
    /// wait starts only when a pass returns, so passes never overlap: a pass
    /// stalled on the database just delays the next one. A wake during a pass
    /// is kept, and starts the next pass as soon as this one returns.
    async fn run_loop(self) {
        log_legacy!(
            "[WorktreeCleanup] starting (interval={:?}, tier1_idle={:?}, tier2_idle={:?}, soft_cap={} bytes, hard_cap={} bytes)",
            self.interval,
            TIER_1_IDLE,
            TIER_2_IDLE,
            self.soft_cap_bytes,
            self.hard_cap_bytes,
        );
        loop {
            self.run_once().await;
            tokio::select! {
                _ = tokio::time::sleep(self.interval) => {}
                _ = self.cleanup_wake.notified() => {}
            }
        }
    }

    pub async fn run_once(&self) {
        // INVARIANT: Read before every cleanup path, including reclaiming and temporary
        // trees. A broken external registry must never authorize deletion.
        let options = self.pool.connect_options();
        let exemptions = options
            .get_database()
            .ok_or("database unavailable")
            .map_err(|_| ())
            .and_then(|database| {
                crate::core::snapshot::store::Store::database_root(database).map_err(|_| ())
            })
            .and_then(|root| {
                crate::core::snapshot::worktree_exemptions::read(&root, &self.workspace_root)
                    .map_err(|_| ())
            });
        let exemptions = match exemptions {
            Ok(paths) => paths,
            Err(()) => {
                log_legacy!("[WorktreeCleanup] cannot verify restore exemptions; skipping cleanup");
                return;
            }
        };
        let dir = worktrees_dir(&self.workspace_root);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                log_legacy!(
                    "[WorktreeCleanup] cannot read worktrees dir {}: {}",
                    dir.display(),
                    e
                );
                return;
            }
        };

        let mut total_freed_under_hard: u64 = 0;

        for entry in entries.flatten() {
            let path = entry.path();
            // INVARIANT: Registry paths use the canonical workspace binding; entry paths
            // may come from a relative or symlinked workspace argument.
            if exemptions
                .iter()
                .any(|saved| saved.file_name() == path.file_name())
            {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with(RECLAIMING_PREFIX) {
                if is_safe_subpath(&dir, &path) && remove_reclaiming_dir(&path).await {
                    log_legacy!(
                        "[WorktreeCleanup] removed {}, left by an interrupted removal",
                        path.display()
                    );
                }
                continue;
            }
            let pre_size = measure_on_blocking_pool(&path, self.size_walk).await;

            if TEMP_WORKTREE_PREFIXES.iter().any(|p| name.starts_with(p)) {
                if let Some((freed, pressure)) =
                    self.try_temp_worktree(&dir, name, &path, pre_size).await
                {
                    total_freed_under_hard =
                        total_freed_under_hard.saturating_add(pressure.hard_reclaimed(Some(freed)));
                }
                continue;
            }

            let Some(short) = parse_thread_short(name) else {
                continue;
            };

            if !is_safe_subpath(&dir, &path) {
                log_legacy!(
                    "[WorktreeCleanup] refusing to act on path outside worktrees dir: {}",
                    path.display()
                );
                continue;
            }

            match lookup_thread_by_short(&self.pool, &short).await {
                // INVARIANT: The lookup could not be answered (a DB error, or an ambiguous
                // 8-hex prefix). Skip the entry outright: the `NotFound` arm
                // below routes to `try_orphan_path`, which is the one arm that
                // never consults `active_threads::is_active`, so treating an
                // unanswered probe as "orphan" would let a database blip delete
                // a live session's worktree.
                ShortThreadLookup::Unknown => continue,
                ShortThreadLookup::Found(thread_id) => {
                    // INVARIANT: A live Claude Code subprocess parked on `AskUserQuestion` emits
                    // no events while the user thinks, so `last_activity_age`
                    // can cross the Tier 0 grace while the session still needs
                    // its worktree. Skip every tier until the session ends.
                    if self.active_threads.is_active(thread_id).await {
                        log_legacy!(
                            "[WorktreeCleanup] skipping thread {} — live agent session active",
                            thread_id
                        );
                        continue;
                    }

                    if let Some(age) = last_activity_age(&self.pool, thread_id).await {
                        let pressure = self.disk_pressure();
                        if let Some(freed) = self
                            .try_stranded(thread_id, &dir, &path, pre_size, age)
                            .await
                        {
                            total_freed_under_hard = total_freed_under_hard
                                .saturating_add(pressure.hard_reclaimed(Some(freed)));
                            continue;
                        }

                        let mut gate = ReclaimGate::new(thread_id, age);
                        let removed = if self.archived(&mut gate).await {
                            self.try_archived_removal(&gate, &path, pre_size).await
                        } else {
                            self.try_removal_tiers(&mut gate, &path, pre_size).await
                        };
                        if let Some((freed, pressure)) = removed {
                            total_freed_under_hard = total_freed_under_hard
                                .saturating_add(pressure.hard_reclaimed(Some(freed)));
                            continue;
                        }
                        if let Some((freed, pressure)) = self.try_tier_1(&mut gate, &path).await {
                            total_freed_under_hard = total_freed_under_hard
                                .saturating_add(pressure.hard_reclaimed(Some(freed)));
                        }
                    }
                }
                ShortThreadLookup::NotFound => {
                    if let Some((freed, pressure)) =
                        self.try_orphan_path(&dir, &path, pre_size).await
                    {
                        total_freed_under_hard = total_freed_under_hard
                            .saturating_add(pressure.hard_reclaimed(Some(freed)));
                    }
                }
            }
        }

        if total_freed_under_hard > 0 {
            if let Some(free) = self.disk_pressure().free_bytes {
                self.emit_auto_cleanup_alert(free, total_freed_under_hard)
                    .await;
            }
        }
    }

    fn disk_pressure(&self) -> DiskPressure {
        DiskPressure::classify((self.free_disk)(), self.soft_cap_bytes, self.hard_cap_bytes)
    }

    async fn reclaim_pressure(
        &self,
        gate: &mut ReclaimGate,
        min_age: impl Fn(&DiskPressure) -> Duration,
    ) -> Option<DiskPressure> {
        let mut pressure = self.disk_pressure();
        if !pressure.under_soft {
            if !self.archived(gate).await {
                return None;
            }
            pressure = self.disk_pressure();
        }
        (gate.age >= min_age(&pressure)).then_some(pressure)
    }

    async fn archived(&self, gate: &mut ReclaimGate) -> bool {
        match gate.archived {
            Some(known) => known,
            None => *gate.archived.insert(self.is_archived(gate.thread_id).await),
        }
    }

    async fn try_removal_tiers(
        &self,
        gate: &mut ReclaimGate,
        worktree: &Path,
        pre_size: u64,
    ) -> Option<(u64, DiskPressure)> {
        if self
            .reclaim_pressure(gate, DiskPressure::zero_info_grace)
            .await
            .is_some()
        {
            if let Some(removed) = self.try_tier_0(gate, worktree, pre_size).await {
                return Some(removed);
            }
        }
        self.reclaim_pressure(gate, |_| TIER_2_IDLE).await?;
        self.try_tier_2(gate, worktree, pre_size).await
    }

    async fn try_archived_removal(
        &self,
        gate: &ReclaimGate,
        worktree: &Path,
        pre_size: u64,
    ) -> Option<(u64, DiskPressure)> {
        let thread_id = gate.thread_id;
        if gate.age < self.disk_pressure().archived_grace() {
            return None;
        }
        let keep = |reason: String| {
            log_legacy!(
                "[WorktreeCleanup] keeping archived thread {}'s worktree: {}",
                thread_id,
                reason
            );
        };
        let head = match self.check_archived_removal(thread_id, worktree).await {
            Ok(head) => head,
            Err(reason) => {
                keep(reason);
                return None;
            }
        };
        self.archived_removal_still_due(gate).await?;
        if let Err(reason) = self.snapshot_dirt(thread_id, worktree, &head).await {
            keep(reason);
            return None;
        }
        // INVARIANT: The snapshot strips build output and commits, which takes minutes.
        // Undo, Move to Current or a new session may have landed meanwhile.
        let pressure = self.archived_removal_still_due(gate).await?;
        let outcome = remove_worktree_and_optionally_delete_branch(
            worktree,
            Some(pre_size),
            BranchDisposal::Keep,
        )
        .await?;
        log_legacy!(
            "[WorktreeCleanup] archived removal freed {} bytes for thread {} (branch_deleted={})",
            outcome.freed_bytes,
            thread_id,
            outcome.branch_deleted
        );
        self.emit_cleaned(thread_id, 2, outcome.freed_bytes, outcome.branch_deleted)
            .await;
        Some((outcome.freed_bytes, pressure))
    }

    async fn archived_removal_still_due(&self, gate: &ReclaimGate) -> Option<DiskPressure> {
        let pressure = self.disk_pressure();
        let still = gate.age >= pressure.archived_grace()
            && self.is_archived(gate.thread_id).await
            && !self.active_threads.is_active(gate.thread_id).await;
        still.then_some(pressure)
    }

    async fn check_archived_removal(
        &self,
        thread_id: Uuid,
        worktree: &Path,
    ) -> Result<String, String> {
        if has_pending_fan_in(&self.pool, thread_id).await {
            return Err("the thread still owes a child fan-in".into());
        }
        match self.changes.pending_for_thread(thread_id).await {
            Ok(pending) if pending.is_empty() => {}
            Ok(_) => return Err("a change is still pending".into()),
            Err(e) => return Err(format!("the pending-change lookup failed: {e}")),
        }
        // INVARIANT: A tree whose `.git` is gone resolves upward to the enclosing repo.
        if !worktree_liveness(worktree).await.or_unknown(false) {
            return Err("git does not confirm it is its own work tree".into());
        }
        worktree_current_branch(worktree)
            .await
            .ok_or_else(|| "HEAD is detached, or git could not name its branch".to_string())
    }

    /// INVARIANT: Commit the tree's uncommitted work on `head`, which must be the
    /// thread's own branch. Build artifacts go first, so a repo that does not
    /// ignore them never gets them committed. `Err` says why the tree stays.
    async fn snapshot_dirt(
        &self,
        thread_id: Uuid,
        worktree: &Path,
        head: &str,
    ) -> Result<(), String> {
        let dirty = || async {
            match worktree_dirtiness(worktree).await {
                GitAnswer::Yes => Ok(true),
                GitAnswer::No => Ok(false),
                GitAnswer::Unknown => Err("git status could not answer".to_string()),
            }
        };
        if !dirty().await? {
            return Ok(());
        }
        prune_build_artifacts(worktree).await;
        if !dirty().await? {
            return Ok(());
        }
        let own = lookup_session_branch_for_thread(&self.pool, thread_id).await;
        if own.as_deref() != Some(head) {
            return Err(format!(
                "it has uncommitted changes on {head}, which is not the thread's own branch"
            ));
        }
        if let Some(nested) = untracked_repositories(worktree).await?.first() {
            return Err(format!(
                "{nested} is a git repository of its own, which a commit would not keep"
            ));
        }
        commit_worktree_or_err(worktree, SNAPSHOT_COMMIT_MESSAGE)
            .await
            .map_err(|e| format!("the snapshot commit failed: {e}"))?;
        if dirty().await? {
            return Err("changes remain after the snapshot, such as in a submodule".into());
        }
        log_legacy!(
            "[WorktreeCleanup] committed archived thread {}'s uncommitted changes on {}",
            thread_id,
            head
        );
        Ok(())
    }

    async fn try_tier_0(
        &self,
        gate: &mut ReclaimGate,
        worktree: &Path,
        pre_size: u64,
    ) -> Option<(u64, DiskPressure)> {
        let thread_id = gate.thread_id;
        if !is_finished_worktree(&self.pool, &self.changes, thread_id, worktree).await {
            return None;
        }
        let pressure = self
            .reclaim_pressure(gate, DiskPressure::zero_info_grace)
            .await?;
        // INVARIANT: A session may have started in this tree meanwhile.
        if self.active_threads.is_active(thread_id).await {
            return None;
        }

        let outcome = remove_worktree_and_optionally_delete_branch(
            worktree,
            Some(pre_size),
            BranchDisposal::Keep,
        )
        .await?;
        log_legacy!(
            "[WorktreeCleanup] tier-0 freed {} bytes for thread {} (branch_deleted={})",
            outcome.freed_bytes,
            thread_id,
            outcome.branch_deleted
        );
        self.emit_cleaned(thread_id, 0, outcome.freed_bytes, outcome.branch_deleted)
            .await;
        Some((outcome.freed_bytes, pressure))
    }

    async fn try_orphan_path(
        &self,
        worktrees_dir: &Path,
        worktree: &Path,
        pre_size: u64,
    ) -> Option<(u64, DiskPressure)> {
        if worktree_git_admin_missing(worktree) {
            if directory_age(worktree).unwrap_or(Duration::ZERO) < self.stranded_grace {
                return None;
            }
            let pressure = self.disk_pressure();
            let freed = remove_stranded_worktree(worktrees_dir, worktree, pre_size).await?;
            log_legacy!(
                "[WorktreeCleanup] stranded orphan-path freed {} bytes at {} (git admin dir missing)",
                freed,
                worktree.display()
            );
            return Some((freed, pressure));
        }

        let past_grace = |pressure: &DiskPressure| {
            directory_age(worktree).unwrap_or(Duration::ZERO) >= pressure.zero_info_grace()
        };
        if !past_grace(&self.disk_pressure()) {
            return None;
        }
        if is_worktree_dirty(worktree).await {
            return None;
        }
        let branch = crate::engine::git_ops::worktree_current_branch(worktree).await;
        if let Some(branch_name) = branch.as_deref() {
            let repo_root = resolve_repo_root_from_worktree(worktree).await?;
            if has_branch_commits(&repo_root, branch_name).await {
                return None;
            }
        }

        let pressure = self.disk_pressure();
        if !past_grace(&pressure) {
            return None;
        }

        let outcome = remove_worktree_and_optionally_delete_branch(
            worktree,
            Some(pre_size),
            BranchDisposal::WhenMerged,
        )
        .await?;
        log_legacy!(
            "[WorktreeCleanup] orphan-path freed {} bytes at {} (branch_deleted={})",
            outcome.freed_bytes,
            worktree.display(),
            outcome.branch_deleted
        );
        Some((outcome.freed_bytes, pressure))
    }

    /// INVARIANT: Stranded-worktree removal for a `thread-<8hex>` dir that resolves to a
    /// thread but whose git admin dir is gone (see [`worktree_git_admin_missing`]).
    /// Returns `None` — falling through to the normal git-based tier ladder —
    /// when the worktree is NOT stranded. The caller has already skipped active
    /// threads and computed `age` from the events table.
    ///
    /// Uses [`STRANDED_GRACE`] (fixed 1 h, no disk-pressure acceleration): git
    /// can't prove the tree is information-free here, so the floor is what
    /// rules out racing an in-flight `git worktree add`.
    async fn try_stranded(
        &self,
        thread_id: Uuid,
        worktrees_dir: &Path,
        worktree: &Path,
        pre_size: u64,
        age: Duration,
    ) -> Option<u64> {
        if !worktree_git_admin_missing(worktree) {
            return None;
        }
        if age < self.stranded_grace {
            return None;
        }
        let freed = remove_stranded_worktree(worktrees_dir, worktree, pre_size).await?;
        log_legacy!(
            "[WorktreeCleanup] stranded freed {} bytes for thread {} (git admin dir missing)",
            freed,
            thread_id
        );
        self.emit_cleaned(thread_id, 2, freed, false).await;
        Some(freed)
    }

    async fn try_temp_worktree(
        &self,
        worktrees_dir: &Path,
        name: &str,
        worktree: &Path,
        pre_size: u64,
    ) -> Option<(u64, DiskPressure)> {
        if !is_safe_subpath(worktrees_dir, worktree) {
            log_legacy!(
                "[WorktreeCleanup] refusing to act on temp path outside worktrees dir: {}",
                worktree.display()
            );
            return None;
        }
        let change_id = parse_temp_change_id(name)?;

        if directory_age(worktree).unwrap_or(Duration::ZERO) < self.temp_worktree_grace {
            return None;
        }

        match self.changes.get_by_id(change_id).await {
            Ok(Some(change)) if change.is_pending() => return None,
            Ok(_) => {}
            Err(e) => {
                log_legacy!(
                    "[WorktreeCleanup] temp sweep get_by_id({}): {} — skipping defensively",
                    change_id,
                    e
                );
                return None;
            }
        }

        // INVARIANT: Don't drop a tree with uncommitted edits (this also skips a *stranded*
        // temp dir, where `git status` errors and is treated as dirty — rare;
        // left for manual cleanup).
        if is_worktree_dirty(worktree).await {
            return None;
        }

        let pressure = self.disk_pressure();
        let outcome = remove_worktree_and_optionally_delete_branch(
            worktree,
            Some(pre_size),
            BranchDisposal::WhenMerged,
        )
        .await?;
        log_legacy!(
            "[WorktreeCleanup] temp worktree freed {} bytes at {} (branch_deleted={})",
            outcome.freed_bytes,
            worktree.display(),
            outcome.branch_deleted
        );
        Some((outcome.freed_bytes, pressure))
    }

    async fn try_tier_1(
        &self,
        gate: &mut ReclaimGate,
        worktree: &Path,
    ) -> Option<(u64, DiskPressure)> {
        let thread_id = gate.thread_id;
        let pressure = match self
            .reclaim_pressure(gate, |p| p.tier1_idle(self.force_tier1_idle))
            .await
        {
            Some(pressure) => pressure,
            None if gate.age >= RELEASE_ARTIFACTS_IDLE && self.nothing_pending(thread_id).await => {
                self.disk_pressure()
            }
            None => return None,
        };
        if self.active_threads.is_active(thread_id).await {
            return None;
        }
        let freed = prune_build_artifacts(worktree).await?;
        log_legacy!(
            "[WorktreeCleanup] tier-1 freed {} bytes for thread {}",
            freed,
            thread_id
        );
        self.emit_cleaned(thread_id, 1, freed, false).await;
        Some((freed, pressure))
    }

    /// INVARIANT: True when nothing waits on this thread's working copy: no pending
    /// change, no owed fan-in, and not saved. An unanswered lookup counts as
    /// pending, because this answer authorizes a delete.
    async fn nothing_pending(&self, thread_id: Uuid) -> bool {
        match self.changes.pending_for_thread(thread_id).await {
            Ok(pending) if pending.is_empty() => {}
            Ok(_) => return false,
            Err(e) => {
                log_legacy!(
                    "[WorktreeCleanup] pending_for_thread({}) failed: {}; keeping build artifacts",
                    thread_id,
                    e
                );
                return false;
            }
        }
        if has_pending_fan_in(&self.pool, thread_id).await {
            return false;
        }
        match thread_is_saved(&self.pool, thread_id).await {
            Ok(saved) => !saved,
            Err(e) => {
                log_legacy!(
                    "[WorktreeCleanup] is_saved lookup failed for thread {}: {}; keeping build artifacts",
                    thread_id,
                    e
                );
                false
            }
        }
    }

    async fn try_tier_2(
        &self,
        gate: &mut ReclaimGate,
        worktree: &Path,
        pre_size: u64,
    ) -> Option<(u64, DiskPressure)> {
        let thread_id = gate.thread_id;
        if has_pending_fan_in(&self.pool, thread_id).await {
            log_legacy!(
                "[WorktreeCleanup] tier-2 skipped for thread {} — outstanding child fan-in obligation",
                thread_id
            );
            return None;
        }
        match thread_is_saved(&self.pool, thread_id).await {
            Ok(true) => {
                return None;
            }
            Ok(false) => {}
            Err(e) => {
                log_legacy!(
                    "[WorktreeCleanup] is_saved lookup failed for thread {}: {} — skipping tier 2",
                    thread_id,
                    e
                );
                return None;
            }
        }

        if is_worktree_dirty(worktree).await {
            log_legacy!(
                "[WorktreeCleanup] tier-2 skipped for thread {} — worktree {} is dirty",
                thread_id,
                worktree.display()
            );
            return None;
        }

        let pressure = self.reclaim_pressure(gate, |_| TIER_2_IDLE).await?;
        // INVARIANT: A session may have started in this tree meanwhile.
        if self.active_threads.is_active(thread_id).await {
            return None;
        }

        let outcome = remove_worktree_and_optionally_delete_branch(
            worktree,
            Some(pre_size),
            BranchDisposal::Keep,
        )
        .await?;

        log_legacy!(
            "[WorktreeCleanup] tier-2 freed {} bytes for thread {} (branch_deleted={})",
            outcome.freed_bytes,
            thread_id,
            outcome.branch_deleted
        );

        self.emit_cleaned(thread_id, 2, outcome.freed_bytes, outcome.branch_deleted)
            .await;
        Some((outcome.freed_bytes, pressure))
    }

    async fn is_archived(&self, thread_id: Uuid) -> bool {
        let row: Result<Option<(String,)>, sqlx::Error> =
            sqlx::query_as("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&self.pool)
                .await;
        match row {
            Ok(Some((state,))) => state == "archived",
            Ok(None) => false,
            Err(e) => {
                log_legacy!(
                    "[WorktreeCleanup] is_archived lookup failed for thread {}: {} — keeping worktree",
                    thread_id,
                    e
                );
                false
            }
        }
    }

    async fn emit_cleaned(
        &self,
        thread_id: Uuid,
        tier: u8,
        freed_bytes: u64,
        branch_deleted: bool,
    ) {
        let event = ThreadEvent::WorktreeCleaned {
            tier,
            freed_bytes,
            branch_deleted,
        };
        self.bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event,
                    meta: EventMeta::NONE,
                },
                "[WorktreeCleanup] WorktreeCleaned",
            )
            .await;
    }

    async fn emit_auto_cleanup_alert(&self, free_bytes: u64, freed_bytes: u64) {
        log_legacy!(
            "[WorktreeCleanup] auto-cleanup reclaimed {:.1} GB (free now {:.1} GB), emitting NotificationCreated",
            freed_bytes as f64 / BYTES_PER_GB,
            free_bytes as f64 / BYTES_PER_GB,
        );
        self.bus
            .emit_or_log(
                disk_notification(
                    "Lucidos reclaimed disk space",
                    auto_cleanup_body(free_bytes, freed_bytes),
                ),
                "[WorktreeCleanup] auto-cleanup NotificationCreated",
            )
            .await;
    }
}

fn disk_notification(title: &str, message: String) -> BusEvent {
    BusEvent::System(SystemEvent::NotificationCreated {
        id: Uuid::new_v4().to_string(),
        title: title.to_string(),
        message,
        task_id: None,
        app_id: None,
        thread_id: None,
        event_id: None,
        tap: SettingsPage::DISK_USAGE.tap(),
        actor: None,
        audience: None,
        push_group: None,
    })
}

#[path = "worktree_cleanup_disk_monitor.rs"]
mod disk_monitor;
pub use disk_monitor::{DiskMonitor, LARGE_FOOTPRINT_BYTES};

#[path = "worktree_cleanup_ops.rs"]
mod ops;
pub(crate) use ops::*;
pub use ops::{
    directory_size_bytes, inventory_worktrees, is_worktree_dirty, prune_build_artifacts,
    remove_stranded_worktree, remove_worktree_and_optionally_delete_branch,
    resolve_repo_root_from_worktree, volume_reading, worktree_git_admin_missing, BranchDisposal,
    DiskThresholds,
};

#[path = "worktree_cleanup_zfs.rs"]
mod zfs;

#[path = "worktree_cleanup_recommended.rs"]
mod recommended;
pub use {recommended::run_recommended_cleanup, recommended::RecommendedCleanupOutcome};

#[cfg(test)]
#[path = "worktree_cleanup_tests/common.rs"]
mod common;

#[cfg(test)]
#[path = "worktree_cleanup_tests/tiers.rs"]
mod tiers_tests;

#[cfg(test)]
#[path = "worktree_cleanup_tests/archived.rs"]
mod archived_tests;

#[cfg(test)]
#[path = "worktree_cleanup_tests/parsers.rs"]
mod parsers_tests;

#[cfg(test)]
#[path = "worktree_cleanup_tests/stranded.rs"]
mod stranded_tests;

#[cfg(test)]
#[path = "worktree_cleanup_tests/disk_monitor.rs"]
mod disk_monitor_tests;

#[cfg(test)]
#[path = "worktree_cleanup_tests/release_artifacts.rs"]
mod release_artifacts_tests;

#[cfg(test)]
#[path = "worktree_cleanup_tests/recommended.rs"]
mod recommended_tests;
