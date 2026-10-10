//! Background worktree cleanup worker (Phase 10.2 + 10.3).
//!
//! Lucidos gives every CC thread a persistent git worktree under
//! `<workspace>/.lucidos/worktrees/thread-<short>`. Without bounded cleanup
//! these accumulate forever — every dependency `npm install` runs again, every
//! Cargo `target/` directory persists, and disk usage grows monotonically.
//!
//! Cleanup runs on [`CLEANUP_INTERVAL`] (15 minutes) and applies three tiers
//! per thread (Tier 0, Tier 1, Tier 2), plus sweeps for stranded, orphan-path
//! and temporary worktrees. The low-disk alert runs apart, in [`DiskMonitor`].
//!
//! **Retention gate (the load-bearing policy).** A worktree is the user's warm
//! working copy of a thread — reopening a thread with its worktree still on disk
//! is instant, while reclaiming it forces a cold ~15 GB rebuild and (if it races
//! a resume) can strand the tree so the next session lands in the workspace data
//! repo. So all reclamation tiers are **gated**. A worktree is removed only
//! once its thread is **archived** or free disk drops below
//! [`FREE_DISK_SOFT_BYTES`]. Archiving is the explicit "I'm done" signal, from
//! the user or the thread's agent (ADR 0310).
//!
//! While disk is comfortable and the thread is non-archived, the worktree is
//! never removed. Its build artifacts stay too, unless the thread has nothing
//! pending (Tier 1 below). The gate is `WorktreeCleanup::reclaim_pressure`; the
//! tier descriptions below state the additional per-tier conditions that apply
//! *once the gate opens*. Always-exempt regardless of the
//! gate: live sessions and *stranded* worktrees (broken git admin dir — removed
//! immediately because they hold nothing recoverable).
//!
//! **Fan-in retention (ADR 0011, B2).** The two *full-removal* tiers (Tier 0 and
//! Tier 2) additionally skip any thread with an outstanding parent↔child fan-in
//! obligation — a direct child still running (`active_children_count > 0`) or a
//! `ChildThreadCompleted` as its latest persisted event (a child completed but
//! the parent hasn't processed it yet). Reclaiming such a worktree would leave
//! the parent with nothing to resume into when it reacts to the completion (the
//! `276f5580` incident). See [`has_pending_fan_in`]. Tier 1 is
//! exempt — it strips only regenerable build artifacts, leaving the worktree and
//! the parent's ability to resume intact.
//!
//! - **Tier 0: full removal of zero-information worktrees.** When a thread has
//!   been idle longer than [`TIER_0_GRACE`] AND has no pending change, a clean
//!   `git status`, and a branch with no commits ahead of main, the whole
//!   worktree is removed (and the branch with it when fully merged). This is
//!   the tier that reclaims a thread's tree after Apply merged its work, and
//!   per ADR 0035 it is why no session teardown needs to reclaim anything.
//!   Emits `WorktreeCleaned { tier: 0, freed_bytes, branch_deleted }`.
//!
//! - **Tier 1: strip regenerable build artifacts.** Strips `target/`,
//!   `node_modules/` and `.lucidos/cache/`. The worktree, its source and its
//!   branch stay, so the next turn rebuilds only what is missing. It runs in
//!   two cases:
//!   - through the gate, once idle longer than [`TIER_1_IDLE`],
//!     [`FORCE_TIER_1_IDLE`] under soft pressure, or at once under hard;
//!   - outside the gate, once idle longer than [`RELEASE_ARTIFACTS_IDLE`], when
//!     the thread has nothing pending: no pending change, no owed fan-in, not
//!     saved (ADR 0311).
//!
//!   Emits `WorktreeCleaned { tier: 1, freed_bytes }`.
//!
//! - **Tier 2 — auto, safe when nothing depends on the working tree.** When
//!   a thread has been idle longer than [`TIER_2_IDLE`], `git status` is
//!   clean, the thread is not saved, and the on-disk path matches the
//!   deterministic `thread-<short>` shape (legacy random-suffix worktrees
//!   are skipped — we don't know which thread owned them), the entire
//!   worktree directory is removed. If the worktree's branch has no commits
//!   ahead of main (fully merged), the branch is also deleted (Phase 10.3).
//!   Emits `WorktreeCleaned { tier: 2, freed_bytes, branch_deleted }`.
//!
//! - **Free-disk pressure.** The worker reads free space on the volume hosting
//!   the worktrees dir at each decision, never once per cycle: a cycle can
//!   wait minutes on the database while free space changes by hundreds of GB.
//!   Below [`FREE_DISK_SOFT_BYTES`] (20 GB) the retention gate opens and the
//!   Tier 1 idle window shrinks from 24 h to [`FORCE_TIER_1_IDLE`] (1 h).
//!   Below [`FREE_DISK_HARD_BYTES`] (5 GB) the Tier 0 grace and the Tier 1
//!   window drop to zero. Each cycle that frees space then emits "Lucidos
//!   reclaimed disk space". The
//!   disk monitor wakes the worker when pressure worsens, so a cycle starts
//!   within a minute instead of at the next [`CLEANUP_INTERVAL`].
//!
//!   Active worktrees are always exempt. Routine Tier 1 and Tier 2 sweeps
//!   stay silent: only cleanup under hard pressure notifies.
//!
//! - **The low-disk alert** belongs to [`DiskMonitor`], a separate task that
//!   never waits on the database. Each crossing below the soft threshold emits
//!   one "Low disk space on your machine" `NotificationCreated`. Its body is
//!   framed around the volume and branches on Lucidos's footprint vs.
//!   [`LARGE_FOOTPRINT_BYTES`]. Every disk notification taps through to
//!   [`SettingsPage::DISK_USAGE`], and links it.
//!
//! ## What we *do not* touch
//!
//! - Active worktrees. A live agent session or a running background task skips
//!   every tier, checked through [`ActiveThreads`]: a session parked on a
//!   question emits no events, and a task builds after its session's turn.
//!   Past that, activity comes from the events table: a thread event newer
//!   than a tier's idle window keeps the worktree out of that tier.
//! - Legacy random-suffix worktrees (anything in `.lucidos/worktrees/` whose
//!   directory name doesn't match `thread-<8-hex>`). We can't safely map them
//!   back to a thread, so we leave them for manual pruning.
//! - Anything outside the workspace's `.lucidos/worktrees/` directory.
//!   `is_safe_subpath` validates the path before any `remove_dir_all`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use sqlx::PgPool;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::engine::agent_session::SpawnsInFlight;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::git_ops::{git_cmd, has_branch_commits, worktrees_dir, SHORT_THREAD_ID_LEN};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::tools::bash_background::BackgroundBashRegistry;
use crate::engine::types::AgentSession;
use crate::scheduler::notifications::SettingsPage;

/// Is something working in this thread's worktree right now? Event age cannot
/// say, because three kinds of work emit nothing while they use the tree:
/// - a session parked on `AskUserQuestion`;
/// - a spawn setting up the worktree before its session registers;
/// - a background task building after its session ended the turn.
///
/// Trait-erased so tests can fake it.
#[async_trait::async_trait]
pub trait ActiveThreads: Send + Sync {
    async fn is_active(&self, thread_id: Uuid) -> bool;
}

/// Asks the engine's three in-memory sources of work in a worktree: live agent
/// sessions, coding-agent spawns still starting, and running background tasks.
/// All die with the engine, so memory is the whole truth, and the probe never
/// waits on the database.
pub struct EngineActiveThreads {
    sessions: Arc<tokio::sync::Mutex<HashMap<Uuid, AgentSession>>>,
    spawns_in_flight: Arc<SpawnsInFlight>,
    background_tasks: BackgroundBashRegistry,
}

impl EngineActiveThreads {
    pub(crate) fn new(
        sessions: Arc<tokio::sync::Mutex<HashMap<Uuid, AgentSession>>>,
        spawns_in_flight: Arc<SpawnsInFlight>,
        background_tasks: BackgroundBashRegistry,
    ) -> Self {
        Self {
            sessions,
            spawns_in_flight,
            background_tasks,
        }
    }
}

#[async_trait::async_trait]
impl ActiveThreads for EngineActiveThreads {
    async fn is_active(&self, thread_id: Uuid) -> bool {
        if self.spawns_in_flight.contains(thread_id) {
            return true;
        }
        // Presence in the map is not liveness, `AgentSession::is_live` is. A
        // phantom left by a dropped run future would otherwise hold this
        // `true` forever and block reclamation of a long-dead tree.
        let session_live = self
            .sessions
            .lock()
            .await
            .get(&thread_id)
            .is_some_and(|s| s.is_live());
        session_live
            || self
                .background_tasks
                .has_running_for_thread(thread_id)
                .await
    }
}

/// Re-export of the canonical deterministic worktree path builder so callers
/// outside `engine::agent_session` (HTTP API handlers, this module) can
/// resolve a thread's worktree without depending on the crate-private
/// `agent_session` module.
pub(crate) use crate::engine::agent_session::resume::deterministic_worktree_path as deterministic_worktree_for;

/// Idle threshold for Tier 1 (build-artifact stripping). 24 hours.
pub const TIER_1_IDLE: Duration = Duration::from_secs(24 * 60 * 60);

/// Idle threshold for Tier 2 (full worktree removal). 30 days.
pub const TIER_2_IDLE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Soft free-disk threshold. Below it the disk monitor sends one low-disk
/// notification per pressure episode. The worker opens the retention gate for
/// non-archived threads and shrinks the Tier 1 idle window to
/// [`FORCE_TIER_1_IDLE`].
pub const FREE_DISK_SOFT_BYTES: u64 = 20 * 1024 * 1024 * 1024;

/// Hard free-disk threshold. Below it the Tier 0 grace and the Tier 1 idle
/// window drop to zero, and the worker tells the user what it reclaimed.
/// Active worktrees stay untouched.
pub const FREE_DISK_HARD_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Tier 1 idle window under soft disk pressure. 1 hour.
pub const FORCE_TIER_1_IDLE: Duration = Duration::from_secs(60 * 60);

/// How often the cleanup loop fires. 15 minutes — fast enough that
/// applied/clean worktrees (Tier 0) clear within ≤15min after their 1h
/// grace expires; slow enough that a quiet engine doesn't burn cycles.
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Grace window for Tier 0 (applied + clean + no commits ahead → full
/// removal). Long enough that the user can apply a change, read the diff,
/// and send a follow-up message that reuses the worktree before deletion;
/// short enough that the Disk Usage panel stays accurate. Drops to 0 under
/// hard disk pressure (`free_bytes < FREE_DISK_HARD_BYTES`).
pub const TIER_0_GRACE: Duration = Duration::from_secs(60 * 60);

/// Idle time after which a thread with nothing pending releases its build
/// artifacts through Tier 1, whatever the free disk (ADR 0311). The same
/// "applied, then idle an hour" window as [`TIER_0_GRACE`].
pub const RELEASE_ARTIFACTS_IDLE: Duration = TIER_0_GRACE;

/// Grace window before a *stranded* worktree (its git admin dir under
/// `.git/worktrees/<name>` is gone, so every git call fails) is removed.
/// Equal to [`TIER_0_GRACE`] (1 h) but — unlike Tier 0 — it does NOT drop to
/// 0 under disk pressure. Tier 0 can prove "zero information on disk" via git
/// (clean status, branch at main) before accelerating; a stranded worktree
/// can't be inspected by git at all, so the fixed floor is what rules out
/// nuking an in-flight `git worktree add` whose admin dir is mid-creation.
pub const STRANDED_GRACE: Duration = TIER_0_GRACE;

/// Grace window (by directory mtime) before an orphaned *temporary* worktree
/// (`harden-`/`apply-`/`merge-` left by a crashed apply/harden/merge flow) is
/// removed. Fixed 2 h, never accelerated under disk pressure: the primary
/// liveness gate is the change row's status (a still-`pending` change may be
/// retried and need its worktree); mtime is only a coarse backstop for the
/// "DB says resolved but cleanup never ran" window. The cost of deleting an
/// in-flight merge/harden worktree (the user re-does conflict resolution) far
/// outweighs the disk reclaimed by accelerating.
pub const TEMP_WORKTREE_GRACE: Duration = Duration::from_secs(2 * 60 * 60);

/// Directory-name prefixes for the temporary worktrees the apply/harden/merge
/// flows create under the worktrees dir. The background sweep collects these
/// when their change is resolved (see [`WorktreeCleanup::try_temp_worktree`]).
/// `cc-` (random-suffix recovery worktrees) is deliberately NOT here — those
/// are treated as legacy and skipped, same as any non-`thread-<8hex>` name.
const TEMP_WORKTREE_PREFIXES: &[&str] = &["harden-", "apply-", "merge-"];

const BYTES_PER_GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Body for the hard-threshold auto-cleanup report.
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

/// Subdirectories of a worktree that are always safe to delete on Tier 1 —
/// regenerable build artifacts. Order matters only for logging.
const TIER_1_PRUNE_DIRS: &[&str] = &["target", "node_modules", ".lucidos/cache"];

/// One worktree's inputs to [`WorktreeCleanup::reclaim_pressure`], which
/// every tier asks at the moment it would act.
struct ReclaimGate {
    thread_id: Uuid,
    age: Duration,
    /// The archive lookup, cached across the tiers of one worktree.
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

/// Worker handle returned by [`WorktreeCleanup::spawn`].
pub struct WorktreeCleanup {
    pool: PgPool,
    bus: Arc<EventBus>,
    workspace_root: PathBuf,
    interval: Duration,
    free_soft_bytes: u64,
    free_hard_bytes: u64,
    force_tier1_idle: Duration,
    /// Grace before a stranded worktree is removed. Defaults to
    /// [`STRANDED_GRACE`]; a struct field (like `force_tier1_idle`) so tests
    /// can drive removal of a freshly-created fixture without aging its mtime.
    stranded_grace: Duration,
    /// Grace before an orphaned temp worktree is removed. Defaults to
    /// [`TEMP_WORKTREE_GRACE`]; overridable in tests for the same reason.
    temp_worktree_grace: Duration,
    free_disk: FreeDiskProbe,
    /// Tier 0 needs `pending_for_thread`; constructed per-worker so the
    /// `spawn` signature stays pool-only.
    changes: crate::core::changes_projection::ChangesProjection,
    active_threads: Arc<dyn ActiveThreads>,
    /// Starts a cycle before the interval ends. The disk monitor holds the
    /// other end.
    cleanup_wake: Arc<Notify>,
}

impl WorktreeCleanup {
    /// Build a worker with production defaults (15-minute cycle, 20 GB soft / 5 GB hard free-disk thresholds).
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
            free_soft_bytes: FREE_DISK_SOFT_BYTES,
            free_hard_bytes: FREE_DISK_HARD_BYTES,
            force_tier1_idle: FORCE_TIER_1_IDLE,
            stranded_grace: STRANDED_GRACE,
            temp_worktree_grace: TEMP_WORKTREE_GRACE,
            changes,
            active_threads,
            cleanup_wake,
        }
    }

    /// Start the cleanup loop on a tokio task. The task lives for the engine's
    /// lifetime; the returned `JoinHandle` is kept by the caller for parity
    /// with other background spawns and so panics surface in tests.
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

    /// Loop forever, waiting [`Self::interval`] or a wake between passes. The
    /// wait starts only when a pass returns, so passes never overlap: a pass
    /// stalled on the database just delays the next one. A wake during a pass
    /// is kept, and starts the next pass as soon as this one returns.
    async fn run_loop(self) {
        log!(
            "[WorktreeCleanup] starting (interval={:?}, tier1_idle={:?}, tier2_idle={:?}, free_soft={} bytes, free_hard={} bytes)",
            self.interval,
            TIER_1_IDLE,
            TIER_2_IDLE,
            self.free_soft_bytes,
            self.free_hard_bytes,
        );
        loop {
            self.run_once().await;
            tokio::select! {
                _ = tokio::time::sleep(self.interval) => {}
                _ = self.cleanup_wake.notified() => {}
            }
        }
    }

    /// Single pass over the worktrees directory. Pulled out of [`run_loop`] so
    /// tests can drive cleanup deterministically without waiting an hour.
    /// See module-level docs for the free-disk tiering semantics.
    pub async fn run_once(&self) {
        let dir = worktrees_dir(&self.workspace_root);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                log!(
                    "[WorktreeCleanup] cannot read worktrees dir {}: {}",
                    dir.display(),
                    e
                );
                return;
            }
        };

        // Every pressure decision below reads `self.disk_pressure()` after the
        // database calls it depends on have returned. A reading taken before a
        // stalled query is stale by the time the query answers.
        let mut total_freed_under_hard: u64 = 0;

        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let pre_size = directory_size_bytes(&path);

            // Orphaned temporary worktrees (`harden-`/`apply-`/`merge-`) left by
            // a crashed apply/harden/merge flow. Checked before `parse_thread_short`
            // so `cc-<uuid>` and other non-`thread-` names still fall through to
            // the legacy skip below.
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
                log!(
                    "[WorktreeCleanup] refusing to act on path outside worktrees dir: {}",
                    path.display()
                );
                continue;
            }

            match lookup_thread_by_short(&self.pool, &short).await {
                // The lookup could not be answered (a DB error, or an ambiguous
                // 8-hex prefix). Skip the entry outright: the `NotFound` arm
                // below routes to `try_orphan_path`, which is the one arm that
                // never consults `active_threads::is_active`, so treating an
                // unanswered probe as "orphan" would let a database blip delete
                // a live session's worktree.
                ShortThreadLookup::Unknown => continue,
                ShortThreadLookup::Found(thread_id) => {
                    // A live Claude Code subprocess parked on `AskUserQuestion` emits
                    // no events while the user thinks, so `last_activity_age`
                    // crosses the tier-0 grace and we'd `git branch -D` the
                    // branch out from under it — destroying the recorded
                    // `branch_name` on the live session and silently breaking
                    // end-of-turn `ChangeProposed`. Skip every tier here; the
                    // next cleanup cycle picks up where this one left off
                    // once the session ends.
                    if self.active_threads.is_active(thread_id).await {
                        log!(
                            "[WorktreeCleanup] skipping thread {} — live agent session active",
                            thread_id
                        );
                        continue;
                    }

                    if let Some(age) = last_activity_age(&self.pool, thread_id).await {
                        // Stranded worktree (git admin dir gone): git-based tier
                        // checks all fail, and a stranded tree is broken whether
                        // or not disk is tight — remove it before the ladder and
                        // outside the retention gate below.
                        let pressure = self.disk_pressure();
                        if let Some(freed) = self
                            .try_stranded(thread_id, &dir, &path, pre_size, age)
                            .await
                        {
                            total_freed_under_hard = total_freed_under_hard
                                .saturating_add(pressure.hard_reclaimed(Some(freed)));
                            continue;
                        }

                        // Each tier asks the gate before its checks, as a cheap
                        // filter, and again right before it deletes anything.
                        let mut gate = ReclaimGate::new(thread_id, age);
                        if self
                            .reclaim_pressure(&mut gate, DiskPressure::zero_info_grace)
                            .await
                            .is_some()
                        {
                            if let Some((freed, pressure)) =
                                self.try_tier_0(&mut gate, &path, pre_size).await
                            {
                                total_freed_under_hard = total_freed_under_hard
                                    .saturating_add(pressure.hard_reclaimed(Some(freed)));
                                continue;
                            }
                        }
                        if self
                            .reclaim_pressure(&mut gate, |_| TIER_2_IDLE)
                            .await
                            .is_some()
                        {
                            if let Some((freed, pressure)) =
                                self.try_tier_2(&mut gate, &path, pre_size).await
                            {
                                total_freed_under_hard = total_freed_under_hard
                                    .saturating_add(pressure.hard_reclaimed(Some(freed)));
                                continue;
                            }
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

        // Only forced cleanup that actually reclaimed something reports.
        // Routine 24h Tier 1 / 30d Tier 2 sweeps stay silent.
        if total_freed_under_hard > 0 {
            if let Some(free) = self.disk_pressure().free_bytes {
                self.emit_auto_cleanup_alert(free, total_freed_under_hard)
                    .await;
            }
        }
    }

    /// Free disk right now, classified against this worker's thresholds.
    fn disk_pressure(&self) -> DiskPressure {
        DiskPressure::classify(
            (self.free_disk)(),
            self.free_soft_bytes,
            self.free_hard_bytes,
        )
    }

    /// The retention gate: `Some(pressure)` when this thread's worktree may be
    /// reclaimed now. A non-archived thread keeps its worktree while free disk
    /// is comfortable, so a resume never races a torn-down tree. Only Tier 1
    /// acts outside this gate, for a thread with nothing pending.
    /// Reclaim opens once the user ARCHIVED the thread (the explicit "done"
    /// signal) or free disk is below the soft threshold.
    ///
    /// On top of the gate, the worktree must have been idle for `min_age`,
    /// which some tiers derive from the same pressure reading. Under soft
    /// pressure the archive lookup is skipped entirely.
    async fn reclaim_pressure(
        &self,
        gate: &mut ReclaimGate,
        min_age: impl Fn(&DiskPressure) -> Duration,
    ) -> Option<DiskPressure> {
        let mut pressure = self.disk_pressure();
        if !pressure.under_soft {
            let archived = match gate.archived {
                Some(known) => known,
                None => *gate.archived.insert(self.is_archived(gate.thread_id).await),
            };
            if !archived {
                return None;
            }
            // Read again: the archive lookup may have waited on the database.
            pressure = self.disk_pressure();
        }
        (gate.age >= min_age(&pressure)).then_some(pressure)
    }

    /// Tier 0: full removal of zero-information worktrees (clean + branch at
    /// main HEAD + no pending change), typically after Apply merged the work.
    /// No saved-thread exemption: events stay in Postgres regardless, and the
    /// worktree itself carries nothing not in main.
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
        // The checks above may have waited minutes on the database or git.
        let pressure = self
            .reclaim_pressure(gate, DiskPressure::zero_info_grace)
            .await?;
        // A session may have started in this tree meanwhile.
        if self.active_threads.is_active(thread_id).await {
            return None;
        }

        let outcome = remove_worktree_and_optionally_delete_branch(
            worktree,
            Some(pre_size),
            BranchDisposal::WhenMerged,
        )
        .await?;
        log!(
            "[WorktreeCleanup] tier-0 freed {} bytes for thread {} (branch_deleted={})",
            outcome.freed_bytes,
            thread_id,
            outcome.branch_deleted
        );
        self.emit_cleaned(thread_id, 0, outcome.freed_bytes, outcome.branch_deleted)
            .await;
        Some((outcome.freed_bytes, pressure))
    }

    /// Orphan-path sweep: same destructive call as Tier 0 for `thread-<8hex>`
    /// dirs whose short id resolves to no thread (DB wipe, or aborted spawn
    /// that died before SessionStarted). Uses directory mtime instead of
    /// `last_activity_age` since no events exist to query, and skips the
    /// `WorktreeCleaned` emit because that event is keyed on `thread_id`.
    async fn try_orphan_path(
        &self,
        worktrees_dir: &Path,
        worktree: &Path,
        pre_size: u64,
    ) -> Option<(u64, DiskPressure)> {
        // Stranded orphan: the git admin dir is gone, so every git check below
        // fails (and `remove_worktree_and_optionally_delete_branch` can't
        // resolve a repo root). Remove the directory directly after the fixed
        // stranded grace — no event, since orphan paths carry no thread_id.
        if worktree_git_admin_missing(worktree) {
            if directory_age(worktree).unwrap_or(Duration::ZERO) < self.stranded_grace {
                return None;
            }
            let pressure = self.disk_pressure();
            let freed = remove_stranded_worktree(worktrees_dir, worktree, pre_size)?;
            log!(
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

        // The git checks above may have waited, and the grace depends on pressure.
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
        log!(
            "[WorktreeCleanup] orphan-path freed {} bytes at {} (branch_deleted={})",
            outcome.freed_bytes,
            worktree.display(),
            outcome.branch_deleted
        );
        Some((outcome.freed_bytes, pressure))
    }

    /// Stranded-worktree removal for a `thread-<8hex>` dir that resolves to a
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
        let freed = remove_stranded_worktree(worktrees_dir, worktree, pre_size)?;
        log!(
            "[WorktreeCleanup] stranded freed {} bytes for thread {} (git admin dir missing)",
            freed,
            thread_id
        );
        // tier 2 = entire worktree removed; branch_deleted is false because a
        // stranded dir has no resolvable repo to delete a branch from (and the
        // branch ref, if any, lives safely in the main repo regardless).
        self.emit_cleaned(thread_id, 2, freed, false).await;
        Some(freed)
    }

    /// Sweep an orphaned temporary worktree (`harden-`/`apply-`/`merge-`) left
    /// on disk by an apply/harden/merge flow that crashed or was interrupted
    /// between create and inline-remove. No `WorktreeCleaned` emit — these are
    /// keyed on a change id, not a thread.
    ///
    /// Liveness gates (all required): the dir name parses to a change id whose
    /// row is absent or NOT `pending` (a pending change may still be retried and
    /// need the worktree; a DB error is treated as in-use), the tree is clean,
    /// and it's
    /// been untouched past [`TEMP_WORKTREE_GRACE`]. Removal goes through
    /// [`remove_worktree_and_optionally_delete_branch`] so git's bookkeeping is
    /// cleaned and a fully-merged temp/thread branch is dropped.
    async fn try_temp_worktree(
        &self,
        worktrees_dir: &Path,
        name: &str,
        worktree: &Path,
        pre_size: u64,
    ) -> Option<(u64, DiskPressure)> {
        if !is_safe_subpath(worktrees_dir, worktree) {
            log!(
                "[WorktreeCleanup] refusing to act on temp path outside worktrees dir: {}",
                worktree.display()
            );
            return None;
        }
        let change_id = parse_temp_change_id(name)?;

        // Coarse mtime backstop first — short-circuits the common "temp dir of
        // an in-flight apply" case without a DB round-trip.
        if directory_age(worktree).unwrap_or(Duration::ZERO) < self.temp_worktree_grace {
            return None;
        }

        // Primary liveness gate: the change row's status.
        match self.changes.get_by_id(change_id).await {
            Ok(Some(change)) if change.is_pending() => return None,
            Ok(_) => {}
            Err(e) => {
                log!(
                    "[WorktreeCleanup] temp sweep get_by_id({}): {} — skipping defensively",
                    change_id,
                    e
                );
                return None;
            }
        }

        // Don't drop a tree with uncommitted edits (this also skips a *stranded*
        // temp dir, where `git status` errors and is treated as dirty — rare;
        // left for manual cleanup).
        if is_worktree_dirty(worktree).await {
            return None;
        }

        // Pressure plays no part in this sweep, only in the report.
        let pressure = self.disk_pressure();
        let outcome = remove_worktree_and_optionally_delete_branch(
            worktree,
            Some(pre_size),
            BranchDisposal::WhenMerged,
        )
        .await?;
        log!(
            "[WorktreeCleanup] temp worktree freed {} bytes at {} (branch_deleted={})",
            outcome.freed_bytes,
            worktree.display(),
            outcome.branch_deleted
        );
        Some((outcome.freed_bytes, pressure))
    }

    /// Tier 1: strip regenerable build artifacts, leaving the worktree, its
    /// source and its branch. Runs through the retention gate, or outside it
    /// for a thread with nothing pending idle past [`RELEASE_ARTIFACTS_IDLE`].
    /// Returns the bytes freed, or `None` if nothing was pruned.
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
        // The lookups above can wait minutes, and a session may start meanwhile.
        if self.active_threads.is_active(thread_id).await {
            return None;
        }
        let freed = prune_build_artifacts(worktree).await?;
        log!(
            "[WorktreeCleanup] tier-1 freed {} bytes for thread {}",
            freed,
            thread_id
        );
        self.emit_cleaned(thread_id, 1, freed, false).await;
        Some((freed, pressure))
    }

    /// True when nothing waits on this thread's working copy: no pending
    /// change, no owed fan-in, and not saved. An unanswered lookup counts as
    /// pending, because this answer authorizes a delete.
    async fn nothing_pending(&self, thread_id: Uuid) -> bool {
        match self.changes.pending_for_thread(thread_id).await {
            Ok(pending) if pending.is_empty() => {}
            Ok(_) => return false,
            Err(e) => {
                log!(
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
                log!(
                    "[WorktreeCleanup] is_saved lookup failed for thread {}: {}; keeping build artifacts",
                    thread_id,
                    e
                );
                false
            }
        }
    }

    /// Tier 2: remove the entire worktree directory if it's safe — clean
    /// `git status`, thread not saved, on-disk path matches the deterministic
    /// shape. Branch deletion is gated separately on "fully merged".
    ///
    /// `pre_size` is the directory size measured at the top of `run_once`;
    /// passing it through avoids walking the same tree twice (worktrees can
    /// be tens of GB).
    async fn try_tier_2(
        &self,
        gate: &mut ReclaimGate,
        worktree: &Path,
        pre_size: u64,
    ) -> Option<(u64, DiskPressure)> {
        let thread_id = gate.thread_id;
        // Don't reclaim a parent that still owes a child fan-in resume — it would
        // have nothing to resume into (ADR 0011, B2).
        if has_pending_fan_in(&self.pool, thread_id).await {
            log!(
                "[WorktreeCleanup] tier-2 skipped for thread {} — outstanding child fan-in obligation",
                thread_id
            );
            return None;
        }
        // Pinned threads are exempt — the user has indicated they care about
        // this thread and may come back to it.
        match thread_is_saved(&self.pool, thread_id).await {
            Ok(true) => {
                return None;
            }
            Ok(false) => {}
            Err(e) => {
                log!(
                    "[WorktreeCleanup] is_saved lookup failed for thread {}: {} — skipping tier 2",
                    thread_id,
                    e
                );
                return None;
            }
        }

        // Dirty worktrees keep their work — the user may have uncommitted
        // edits we'd silently lose.
        if is_worktree_dirty(worktree).await {
            log!(
                "[WorktreeCleanup] tier-2 skipped for thread {} — worktree {} is dirty",
                thread_id,
                worktree.display()
            );
            return None;
        }

        // The checks above may have waited minutes on the database or git.
        let pressure = self.reclaim_pressure(gate, |_| TIER_2_IDLE).await?;
        // A session may have started in this tree meanwhile.
        if self.active_threads.is_active(thread_id).await {
            return None;
        }

        let outcome = remove_worktree_and_optionally_delete_branch(
            worktree,
            Some(pre_size),
            BranchDisposal::WhenMerged,
        )
        .await?;

        log!(
            "[WorktreeCleanup] tier-2 freed {} bytes for thread {} (branch_deleted={})",
            outcome.freed_bytes,
            thread_id,
            outcome.branch_deleted
        );

        self.emit_cleaned(thread_id, 2, outcome.freed_bytes, outcome.branch_deleted)
            .await;
        Some((outcome.freed_bytes, pressure))
    }

    /// Whether the thread is archived (`archive_state = 'archived'`), by the
    /// user or by an agent (ADR 0310).
    /// Archiving is the explicit "I'm done with this" signal that lets the
    /// retention gate reclaim the worktree even while free disk is comfortable.
    /// On a DB error (or unknown thread) returns `false` — keep the worktree, as
    /// reclaiming on uncertain state is the unsafe direction.
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
                log!(
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

    /// Auto-cleanup action notification: hard pressure forced reclamation and
    /// we actually freed bytes. Fires per cycle that does work, so the user
    /// sees ongoing progress while disk recovers.
    async fn emit_auto_cleanup_alert(&self, free_bytes: u64, freed_bytes: u64) {
        log!(
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

/// A disk notification. Both kinds land on the same page, because that page
/// answers the question each of them raises: how much room is left on the
/// volume, and how much of it Lucidos holds.
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
    })
}

#[path = "worktree_cleanup_disk_monitor.rs"]
mod disk_monitor;
pub use disk_monitor::{DiskMonitor, LARGE_FOOTPRINT_BYTES};

#[path = "worktree_cleanup_ops.rs"]
mod ops;
pub(crate) use ops::*;

#[path = "worktree_cleanup_recommended.rs"]
mod recommended;
pub(crate) use recommended::{run_recommended_cleanup, RecommendedCleanupOutcome};

#[cfg(test)]
#[path = "worktree_cleanup_tests/common.rs"]
mod common;

#[cfg(test)]
#[path = "worktree_cleanup_tests/tiers.rs"]
mod tiers_tests;

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
