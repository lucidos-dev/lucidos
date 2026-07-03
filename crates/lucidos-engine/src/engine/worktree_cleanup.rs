//! Background worktree cleanup worker (Phase 10.2 + 10.3).
//!
//! Lucidos gives every CC thread a persistent git worktree under
//! `<workspace>/.lucidos/worktrees/thread-<short>`. Without bounded cleanup
//! these accumulate forever — every dependency `npm install` runs again, every
//! Cargo `target/` directory persists, and disk usage grows monotonically.
//!
//! Cleanup runs on a long timer (default 1 hour) and applies two tiers per
//! thread, plus one global threshold alert.
//!
//! **Retention gate (the load-bearing policy).** A worktree is the user's warm
//! working copy of a thread — reopening a thread with its worktree still on disk
//! is instant, while reclaiming it forces a cold ~15 GB rebuild and (if it races
//! a resume) can strand the tree so the next session lands in the workspace data
//! repo. So all reclamation tiers are **gated**: a thread's worktree is touched
//! only when the user has **archived** the thread (the explicit "I'm done"
//! signal) OR free disk has dropped below [`FREE_DISK_SOFT_BYTES`]. While disk is
//! comfortable and the thread is non-archived, the worktree is kept fully warm
//! regardless of idle age — no stripping, no removal. The gate is applied in
//! `run_once`; the tier descriptions below state the additional per-tier
//! conditions that apply *once the gate opens*. Always-exempt regardless of the
//! gate: live sessions and *stranded* worktrees (broken git admin dir — removed
//! immediately because they hold nothing recoverable).
//!
//! **Fan-in retention (ADR 0011, B2).** The two *full-removal* tiers (Tier 0 and
//! Tier 2) additionally skip any thread with an outstanding parent↔child fan-in
//! obligation — a direct child still running (`active_children_count > 0`) or a
//! `ChildThreadCompleted` as its latest persisted event (a child completed but
//! the parent hasn't processed it yet). Reclaiming such a worktree would leave
//! the parent with nothing to resume into when it reacts to the completion (the
//! `276f5580` incident). See [`WorktreeCleanup::has_pending_fan_in`]. Tier 1 is
//! exempt — it strips only regenerable build artifacts, leaving the worktree and
//! the parent's ability to resume intact.
//!
//! - **Tier 1 — strip regenerable build artifacts.** When a thread has been idle longer than
//!   [`TIER_1_IDLE`] AND its worktree contains a `target/`, `node_modules/`,
//!   or `.lucidos/cache/` directory, those directories are stripped. The
//!   worktree itself stays so the next CC turn re-installs only the missing
//!   bits. Emits `WorktreeCleaned { tier: 1, freed_bytes }`.
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
//! - **Free-disk monitoring.** Each cycle the worker probes available space
//!   on the volume hosting the worktrees dir. On the transition from
//!   above-soft to below [`FREE_DISK_SOFT_BYTES`] (20 GB) it emits a one-shot
//!   "Low disk space on your machine" `NotificationCreated`; the alert re-arms
//!   once disk recovers above soft. The body is deliberately framed around
//!   the volume (not Lucidos) and branches on Lucidos's own footprint vs.
//!   [`LARGE_FOOTPRINT_BYTES`] so the suggestion matches reality — small
//!   footprint says "look elsewhere on your machine", large footprint
//!   suggests Settings → Disk Usage. Below [`FREE_DISK_HARD_BYTES`] (5 GB) it
//!   ALSO widens its Tier 1 idle window from 24 h to [`FORCE_TIER_1_IDLE`]
//!   (1 h) so build artifacts from recently-idle worktrees get reclaimed
//!   aggressively, and emits "Lucidos reclaimed disk space" with the bytes
//!   reclaimed each cycle that actually freed space. Active and saved
//!   worktrees are always exempt. Routine 24h Tier 1 / 30d Tier 2 sweeps
//!   stay silent — only disk-pressure cleanup notifies.
//!
//! ## What we *do not* touch
//!
//! - Active worktrees. Activity is detected by querying the events table:
//!   any thread event newer than [`TIER_2_IDLE`] keeps the worktree out of
//!   Tier 2. The cleanup worker therefore needs no in-memory `agent_sessions`
//!   handle — events are the source of truth and the in-memory map is just a
//!   cache of currently-running spawns. A spawn always emits at least
//!   `SessionStarted` / `MessageReceived` / `CodingAgentTextStreamed` shortly
//!   after start, well within the 30-day idle window.
//! - Legacy random-suffix worktrees (anything in `.lucidos/worktrees/` whose
//!   directory name doesn't match `thread-<8-hex>`). We can't safely map them
//!   back to a thread, so we leave them for manual pruning.
//! - Anything outside the workspace's `.lucidos/worktrees/` directory.
//!   `prune_path` validates the path before any `remove_dir_all`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::agent_session::resume::THREAD_WORKTREE_ID_LEN;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::git_ops::{git_cmd, has_branch_commits, worktrees_dir};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::types::AgentSession;

/// `last_activity_age` lies across long `AskUserQuestion` waits — CC is alive
/// on stdin but emits no events. Probing `agent_sessions` is the only reliable
/// liveness signal. Trait-erased so tests can fake without building a real
/// `AgentSession`.
#[async_trait::async_trait]
pub trait ActiveThreads: Send + Sync {
    async fn is_active(&self, thread_id: Uuid) -> bool;
}

pub struct AgentSessionsActiveThreads {
    sessions: Arc<tokio::sync::Mutex<HashMap<Uuid, AgentSession>>>,
}

impl AgentSessionsActiveThreads {
    pub fn new(sessions: Arc<tokio::sync::Mutex<HashMap<Uuid, AgentSession>>>) -> Self {
        Self { sessions }
    }
}

#[async_trait::async_trait]
impl ActiveThreads for AgentSessionsActiveThreads {
    async fn is_active(&self, thread_id: Uuid) -> bool {
        self.sessions.lock().await.contains_key(&thread_id)
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

/// Soft free-disk threshold. When the volume hosting the workspace falls
/// below this, the worker emits a `NotificationCreated` once per cycle so the
/// user knows worktrees are competing for space. No automatic cleanup beyond
/// the normal Tier 1 / Tier 2 idle sweeps.
pub const FREE_DISK_SOFT_BYTES: u64 = 20 * 1024 * 1024 * 1024;

/// Hard free-disk threshold. When free space drops below this, the worker
/// widens its Tier 1 idle window from `TIER_1_IDLE` (24 h) to
/// `FORCE_TIER_1_IDLE` (1 h) so build artifacts get reclaimed aggressively.
/// Active and saved worktrees are still untouched. Also emits a stronger
/// notification telling the user what we just did.
pub const FREE_DISK_HARD_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Tier 1 idle window when disk pressure forces aggressive cleanup. 1 hour.
pub const FORCE_TIER_1_IDLE: Duration = Duration::from_secs(60 * 60);

/// How often the cleanup loop fires. 15 minutes — fast enough that
/// applied/clean worktrees (Tier 0) clear within ≤15min after their 1h
/// grace expires; slow enough that a quiet engine doesn't burn cycles.
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Grace window for Tier 0 (applied + clean + no commits ahead → full
/// removal). Long enough that the user can apply a change, read the diff,
/// and send a follow-up message that reuses the worktree before deletion;
/// short enough that the Disk Usage panel stays accurate. Drops to 0 under
/// disk pressure (`free_bytes < FREE_DISK_HARD_BYTES`), matching how Tier 1
/// already accelerates from 24h to 1h under the same condition.
pub const TIER_0_GRACE: Duration = Duration::from_secs(60 * 60);

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

/// Threshold above which Lucidos's own worktree footprint is "meaningful"
/// in the disk-low notification. Below this, the heads-up message tells the
/// user the pressure is from their machine overall (other apps), not Lucidos
/// — so the framing matches reality. 5 GB is roughly the size of one CC
/// session's `target/` after a Cargo build, so anything noticeably above
/// "one fresh worktree" gets the cleanup-suggestion variant.
pub const LARGE_FOOTPRINT_BYTES: u64 = 5 * 1024 * 1024 * 1024;

const BYTES_PER_GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Subdirectories of a worktree that are always safe to delete on Tier 1 —
/// regenerable build artifacts. Order matters only for logging.
const TIER_1_PRUNE_DIRS: &[&str] = &["target", "node_modules", ".lucidos/cache"];

/// In-memory state used to dedupe disk-pressure notifications across cycles.
/// Lives only on the worker handle: on engine restart the state resets, which
/// at worst causes one extra "getting low" notification — acceptable per the
/// engine-statelessness rules.
#[derive(Default)]
struct AlertState {
    /// Whether the previous tick observed `free_bytes < free_soft_bytes`.
    /// Used to fire the "low" notification only on the above_soft → below_soft
    /// transition, not on every tick we remain below.
    was_below_soft: bool,
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
    /// Boundary used by [`emit_disk_low_alert`] to pick the
    /// look-elsewhere vs. clean-from-Settings body variant.
    large_footprint_bytes: u64,
    alerts: Mutex<AlertState>,
    /// Tier 0 needs `pending_for_thread`; constructed per-worker so the
    /// `spawn` signature stays pool-only.
    changes: crate::core::changes_projection::ChangesProjection,
    active_threads: Arc<dyn ActiveThreads>,
}

impl WorktreeCleanup {
    /// Build a worker with production defaults (15-minute cycle, 20 GB soft / 5 GB hard free-disk thresholds).
    pub fn new(
        pool: PgPool,
        bus: Arc<EventBus>,
        workspace_root: PathBuf,
        active_threads: Arc<dyn ActiveThreads>,
    ) -> Self {
        let changes = crate::core::changes_projection::ChangesProjection::new(pool.clone());
        Self {
            pool,
            bus,
            workspace_root,
            interval: CLEANUP_INTERVAL,
            free_soft_bytes: FREE_DISK_SOFT_BYTES,
            free_hard_bytes: FREE_DISK_HARD_BYTES,
            force_tier1_idle: FORCE_TIER_1_IDLE,
            stranded_grace: STRANDED_GRACE,
            temp_worktree_grace: TEMP_WORKTREE_GRACE,
            large_footprint_bytes: LARGE_FOOTPRINT_BYTES,
            alerts: Mutex::new(AlertState::default()),
            changes,
            active_threads,
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
    ) -> tokio::task::JoinHandle<()> {
        let worker = Self::new(pool, bus, workspace_root, active_threads);
        tokio::spawn(async move { worker.run_loop().await })
    }

    /// Loop forever, sleeping [`Self::interval`] between passes.
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
            tokio::time::sleep(self.interval).await;
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

        let free_bytes = available_disk_bytes(&dir)
            .or_else(|| available_disk_bytes(&self.workspace_root));
        let under_hard = free_bytes.is_some_and(|b| b < self.free_hard_bytes);
        let under_soft = free_bytes.is_some_and(|b| b < self.free_soft_bytes);
        let tier1_idle = if under_hard { self.force_tier1_idle } else { TIER_1_IDLE };

        let mut total_freed_under_hard: u64 = 0;
        // Sum of every recognised worktree's on-disk size — the "Lucidos
        // worktree footprint" we put in the disk-low notification so the
        // user can see how much of the volume is actually Lucidos vs. the
        // rest of their machine.
        let mut lucidos_footprint_bytes: u64 = 0;

        // Tier 0 / orphan-path grace: 1h normally, 0 under disk pressure.
        // Same threshold for both since the safety story is identical
        // (provably zero information on disk).
        let zero_info_grace = if under_hard { Duration::ZERO } else { TIER_0_GRACE };

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
                if let Some(freed) = self.try_temp_worktree(&dir, name, &path, pre_size).await {
                    if under_hard {
                        total_freed_under_hard = total_freed_under_hard.saturating_add(freed);
                    }
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
                Some(thread_id) => {
                    // Footprint accounting MUST stay above the active-session
                    // skip below — `inventory_worktrees` counts active worktrees
                    // too, and the disk-low alert's "Lucidos uses X GB" framing
                    // breaks if a live session's bytes silently disappear.
                    lucidos_footprint_bytes =
                        lucidos_footprint_bytes.saturating_add(pre_size);

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
                        if let Some(freed) = self
                            .try_stranded(thread_id, &dir, &path, pre_size, age)
                            .await
                        {
                            if under_hard {
                                total_freed_under_hard =
                                    total_freed_under_hard.saturating_add(freed);
                            }
                            continue;
                        }

                        // Retention gate: keep a non-archived thread's worktree
                        // fully warm while free disk is comfortable, so reopening
                        // the thread reuses the worktree instead of paying a cold
                        // ~15 GB rebuild (and never races a resume into a torn-down
                        // tree). Reclaim — Tier 0/1/2 — only once the user has
                        // ARCHIVED the thread (the explicit "done" signal) or free
                        // disk drops below the soft threshold. `||` short-circuits,
                        // so the archive lookup is skipped entirely under pressure.
                        let may_reclaim = under_soft || self.is_archived(thread_id).await;
                        if may_reclaim {
                            if age >= zero_info_grace {
                                if let Some(freed) =
                                    self.try_tier_0(thread_id, &path, pre_size).await
                                {
                                    if under_hard {
                                        total_freed_under_hard =
                                            total_freed_under_hard.saturating_add(freed);
                                    }
                                    continue;
                                }
                            }
                            if age >= TIER_2_IDLE
                                && self
                                    .try_tier_2(thread_id, &path, pre_size)
                                    .await
                                    .is_some()
                            {
                                continue;
                            }
                            if age >= tier1_idle {
                                if let Some(freed) = self.try_tier_1(thread_id, &path).await {
                                    if under_hard {
                                        total_freed_under_hard =
                                            total_freed_under_hard.saturating_add(freed);
                                    }
                                }
                            }
                        }
                    }
                }
                None => {
                    // Orphan worktrees are excluded from the footprint to
                    // match `inventory_worktrees` (Settings → Disk Usage).
                    if let Some(freed) =
                        self.try_orphan_path(&dir, &path, pre_size, zero_info_grace).await
                    {
                        if under_hard {
                            total_freed_under_hard =
                                total_freed_under_hard.saturating_add(freed);
                        }
                    }
                }
            }
        }

        if let Some(free) = free_bytes {
            // Heads-up notification fires only on the transition into the
            // below-soft state — not every tick — so the user isn't pinged
            // hourly while the disk stays low. Probe failures (`free_bytes`
            // is `None`) skip this whole block, intentionally freezing
            // `was_below_soft` so a transient probe failure doesn't reset
            // the dedup state.
            let just_crossed_soft = {
                let mut state = self.alerts.lock().unwrap();
                let crossed = under_soft && !state.was_below_soft;
                state.was_below_soft = under_soft;
                crossed
            };
            if just_crossed_soft {
                self.emit_disk_low_alert(free, lucidos_footprint_bytes).await;
            }
            // Action notification: only when forced cleanup actually ran AND
            // reclaimed something. Routine 24h Tier 1 / 30d Tier 2 sweeps stay
            // silent.
            if under_hard && total_freed_under_hard > 0 {
                self.emit_auto_cleanup_alert(free, total_freed_under_hard).await;
            }
        }
    }

    /// Tier 0: full removal of zero-information worktrees (clean + branch at
    /// main HEAD + no pending change), typically after Apply merged the work.
    /// No saved-thread exemption: events stay in Postgres regardless, and the
    /// worktree itself carries nothing not in main.
    async fn try_tier_0(
        &self,
        thread_id: Uuid,
        worktree: &Path,
        pre_size: u64,
    ) -> Option<u64> {
        // Don't reclaim a parent that still owes a child fan-in resume — it would
        // have nothing to resume into (ADR 0011, B2).
        if self.has_pending_fan_in(thread_id).await {
            log!(
                "[WorktreeCleanup] tier-0 skipped for thread {} — outstanding child fan-in obligation",
                thread_id
            );
            return None;
        }
        // On DB error, treat as if pending changes exist — skipping tier-0 cleanup
        // is safer than deleting a worktree whose pending-state we can't verify.
        match self.changes.pending_for_thread(thread_id).await {
            Ok(v) if v.is_empty() => {}
            Ok(_) => return None,
            Err(e) => {
                crate::log!(
                    "[WorktreeCleanup] tier-0 pending_for_thread({}): {} — \
                     skipping cleanup defensively",
                    thread_id,
                    e
                );
                return None;
            }
        }
        if is_worktree_dirty(worktree).await {
            return None;
        }
        let branch = crate::engine::git_ops::worktree_current_branch(worktree).await;
        let branch_name = branch.as_deref()?;
        let repo_root = resolve_repo_root_from_worktree(worktree).await?;
        if has_branch_commits(&repo_root, branch_name).await {
            return None;
        }

        let outcome =
            remove_worktree_and_optionally_delete_branch(worktree, Some(pre_size)).await?;
        log!(
            "[WorktreeCleanup] tier-0 freed {} bytes for thread {} (branch_deleted={})",
            outcome.freed_bytes,
            thread_id,
            outcome.branch_deleted
        );
        self.emit_cleaned(thread_id, 0, outcome.freed_bytes, outcome.branch_deleted)
            .await;
        Some(outcome.freed_bytes)
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
        mtime_grace: Duration,
    ) -> Option<u64> {
        // Stranded orphan: the git admin dir is gone, so every git check below
        // fails (and `remove_worktree_and_optionally_delete_branch` can't
        // resolve a repo root). Remove the directory directly after the fixed
        // stranded grace — no event, since orphan paths carry no thread_id.
        if worktree_git_admin_missing(worktree) {
            if directory_age(worktree).unwrap_or(Duration::ZERO) < self.stranded_grace {
                return None;
            }
            let freed = remove_stranded_worktree(worktrees_dir, worktree, pre_size)?;
            log!(
                "[WorktreeCleanup] stranded orphan-path freed {} bytes at {} (git admin dir missing)",
                freed,
                worktree.display()
            );
            return Some(freed);
        }

        if directory_age(worktree).unwrap_or(Duration::ZERO) < mtime_grace {
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

        let outcome =
            remove_worktree_and_optionally_delete_branch(worktree, Some(pre_size)).await?;
        log!(
            "[WorktreeCleanup] orphan-path freed {} bytes at {} (branch_deleted={})",
            outcome.freed_bytes,
            worktree.display(),
            outcome.branch_deleted
        );
        Some(outcome.freed_bytes)
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
    ) -> Option<u64> {
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
            Ok(Some(change)) if change.status == "pending" => return None,
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

        let outcome =
            remove_worktree_and_optionally_delete_branch(worktree, Some(pre_size)).await?;
        log!(
            "[WorktreeCleanup] temp worktree freed {} bytes at {} (branch_deleted={})",
            outcome.freed_bytes,
            worktree.display(),
            outcome.branch_deleted
        );
        Some(outcome.freed_bytes)
    }

    /// Tier 1: strip regenerable build artifacts. Safe even when the thread
    /// is still considered "active enough" to keep the worktree around.
    /// Returns the total bytes freed (best-effort), or `None` if nothing was
    /// pruned (so the caller knows not to count it against the threshold).
    async fn try_tier_1(&self, thread_id: Uuid, worktree: &Path) -> Option<u64> {
        let freed = prune_build_artifacts(worktree)?;
        log!(
            "[WorktreeCleanup] tier-1 freed {} bytes for thread {}",
            freed,
            thread_id
        );
        self.emit_cleaned(thread_id, 1, freed, false).await;
        Some(freed)
    }

    /// Tier 2: remove the entire worktree directory if it's safe — clean
    /// `git status`, thread not saved, on-disk path matches the deterministic
    /// shape. Branch deletion is gated separately on "fully merged".
    ///
    /// `pre_size` is the directory size measured at the top of `run_once`;
    /// passing it through avoids walking the same tree twice (worktrees can
    /// be tens of GB).
    async fn try_tier_2(&self, thread_id: Uuid, worktree: &Path, pre_size: u64) -> Option<u64> {
        // Don't reclaim a parent that still owes a child fan-in resume — it would
        // have nothing to resume into (ADR 0011, B2).
        if self.has_pending_fan_in(thread_id).await {
            log!(
                "[WorktreeCleanup] tier-2 skipped for thread {} — outstanding child fan-in obligation",
                thread_id
            );
            return None;
        }
        // Pinned threads are exempt — the user has indicated they care about
        // this thread and may come back to it.
        match self.is_saved(thread_id).await {
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

        let outcome =
            remove_worktree_and_optionally_delete_branch(worktree, Some(pre_size)).await?;

        log!(
            "[WorktreeCleanup] tier-2 freed {} bytes for thread {} (branch_deleted={})",
            outcome.freed_bytes,
            thread_id,
            outcome.branch_deleted
        );

        self.emit_cleaned(thread_id, 2, outcome.freed_bytes, outcome.branch_deleted)
            .await;
        Some(outcome.freed_bytes)
    }

    async fn is_saved(&self, thread_id: Uuid) -> Result<bool, sqlx::Error> {
        let row: Option<(bool,)> = sqlx::query_as(
            "SELECT is_saved FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(p,)| p).unwrap_or(false))
    }

    /// Whether the user has archived this thread (`archive_state = 'archived'`).
    /// Archiving is the explicit "I'm done with this" signal that lets the
    /// retention gate reclaim the worktree even while free disk is comfortable.
    /// On a DB error (or unknown thread) returns `false` — keep the worktree, as
    /// reclaiming on uncertain state is the unsafe direction.
    async fn is_archived(&self, thread_id: Uuid) -> bool {
        let row: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
            "SELECT archive_state FROM thread_summaries WHERE thread_id = $1",
        )
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

    /// True iff `thread_id` has an outstanding parent↔child fan-in obligation
    /// that requires keeping its worktree alive (ADR 0011, weakness B2):
    ///   (a) `active_children_count > 0` — a direct child is still running; the
    ///       parent will resume when it finishes, so it needs its worktree.
    ///   (b) the thread's latest persisted event is a `ChildThreadCompleted` —
    ///       a child has completed but the parent hasn't processed it yet (the
    ///       exact incident window); this is the same predicate B1's boot sweep
    ///       (`refire_unprocessed_child_completions`) selects on.
    ///
    /// Removing the worktree in either window would leave the parent with nothing
    /// to resume into. The count guards the "children still running" window; the
    /// card guards the "completed-but-not-processed" window — both are needed
    /// because the child's terminal event decrements `active_children_count` in
    /// the same transaction that persists `ChildThreadCompleted`, so the count is
    /// already 0 by the time the parent owes a resume. On a DB error returns
    /// `true` (keep the worktree) — reclaiming on unverifiable state is the
    /// unsafe direction, matching `try_tier_0`'s pending-change stance.
    async fn has_pending_fan_in(&self, thread_id: Uuid) -> bool {
        // (a) direct children still running.
        match sqlx::query_scalar::<_, i64>(
            "SELECT active_children_count::bigint FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(Some(count)) if count > 0 => return true,
            Ok(_) => {}
            Err(e) => {
                log!(
                    "[WorktreeCleanup] active_children_count lookup failed for thread {}: {} — keeping worktree",
                    thread_id,
                    e
                );
                return true;
            }
        }
        // (b) a completed-but-unprocessed child completion is the thread's last
        // persisted word (no resume emitted a later event).
        match sqlx::query_scalar::<_, String>(
            "SELECT event_type FROM events \
             WHERE aggregate = 'thread' AND aggregate_id = $1::text \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(Some(event_type)) => event_type == "ChildThreadCompleted",
            Ok(None) => false,
            Err(e) => {
                log!(
                    "[WorktreeCleanup] latest-event lookup failed for thread {}: {} — keeping worktree",
                    thread_id,
                    e
                );
                true
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

    /// Heads-up that free disk has crossed the soft threshold. Fires once per
    /// pressure episode (re-armed on recovery above soft).
    ///
    /// The body is framed around the user's machine, not Lucidos: the trigger
    /// is system-wide free space, and Lucidos's own footprint is usually a
    /// small slice of that. Body branches on the footprint so the suggestion
    /// matches reality (point at Settings only when cleaning would actually
    /// help).
    async fn emit_disk_low_alert(&self, free_bytes: u64, lucidos_bytes: u64) {
        let free_gb = free_bytes as f64 / BYTES_PER_GB;
        let lucidos_gb = lucidos_bytes as f64 / BYTES_PER_GB;
        let title = "Low disk space on your machine".to_string();
        let message = if lucidos_bytes >= self.large_footprint_bytes {
            format!(
                "Only {:.1} GB free on the volume hosting your Lucidos workspace. \
                 Lucidos worktrees use {:.1} GB — clean idle ones from Settings → Disk Usage to reclaim space. \
                 New coding-agent sessions may fail to spawn until disk is freed.",
                free_gb, lucidos_gb,
            )
        } else {
            format!(
                "Only {:.1} GB free on the volume hosting your Lucidos workspace. \
                 Lucidos itself uses just {:.1} GB — most of the pressure is from other apps on your machine. \
                 New coding-agent sessions may fail to spawn until you free space elsewhere.",
                free_gb, lucidos_gb,
            )
        };
        log!(
            "[WorktreeCleanup] crossed below soft threshold ({:.1} GB free, Lucidos {:.1} GB) — emitting disk-low NotificationCreated",
            free_gb,
            lucidos_gb,
        );
        self.emit_notification(title, message, "disk-low").await;
    }

    /// Auto-cleanup action notification: hard pressure forced reclamation and
    /// we actually freed bytes. Fires per cycle that does work, so the user
    /// sees ongoing progress while disk recovers. Title attributes the action
    /// to Lucidos (it's helpful to know who did it), but the body still names
    /// the system-wide free space first so the user understands the trigger
    /// is the volume, not Lucidos eating disk.
    async fn emit_auto_cleanup_alert(&self, free_bytes: u64, freed_bytes: u64) {
        let free_gb = free_bytes as f64 / BYTES_PER_GB;
        let freed_gb = freed_bytes as f64 / BYTES_PER_GB;
        let title = "Lucidos reclaimed disk space".to_string();
        let message = format!(
            "Your machine is critically low on disk ({:.1} GB free). Lucidos reclaimed {:.1} GB by stripping build artifacts from idle Claude Code worktrees. \
             Close saved threads or remove unused worktrees from Settings → Disk Usage to reclaim more.",
            free_gb, freed_gb,
        );
        log!(
            "[WorktreeCleanup] auto-cleanup reclaimed {:.1} GB (free now {:.1} GB) — emitting NotificationCreated",
            freed_gb,
            free_gb,
        );
        self.emit_notification(title, message, "auto-cleanup").await;
    }

    async fn emit_notification(&self, title: String, message: String, log_tag: &str) {
        let id = Uuid::new_v4().to_string();
        self.bus
            .emit_or_log(
                BusEvent::System(SystemEvent::NotificationCreated {
                    id,
                    title,
                    message,
                    task_id: None,
                    app_id: None,
                    thread_id: None,
                    event_id: None,
                    tap: crate::scheduler::notifications::Tap::Modal,
                    actor: None,
                }),
                &format!("[WorktreeCleanup] {} NotificationCreated", log_tag),
            )
            .await;
    }
}

#[path = "worktree_cleanup_ops.rs"]
mod ops;
pub(crate) use ops::*;

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
