//! Background worktree cleanup worker (Phase 10.2 + 10.3).
//!
//! Lucidos gives every CC thread a persistent git worktree under
//! `<workspace>/.lucidos/worktrees/thread-<short>`. Without bounded cleanup
//! these accumulate forever — every dependency `npm install` runs again, every
//! Cargo `target/` directory persists, and disk usage grows monotonically.
//!
//! Cleanup runs on a long timer (default 1 hour) and applies two tiers per
//! thread, plus one global threshold alert:
//!
//! - **Tier 1 — auto, always safe.** When a thread has been idle longer than
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

                    // A live CC subprocess parked on `AskUserQuestion` emits
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
                None => {
                    // Orphan worktrees are excluded from the footprint to
                    // match `inventory_worktrees` (Settings → Disk Usage).
                    if let Some(freed) =
                        self.try_orphan_path(&path, pre_size, zero_info_grace).await
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
        if !self.changes.pending_for_thread(thread_id).await.is_empty() {
            return None;
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
        worktree: &Path,
        pre_size: u64,
        mtime_grace: Duration,
    ) -> Option<u64> {
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
                 New Claude Code sessions may fail to spawn until disk is freed.",
                free_gb, lucidos_gb,
            )
        } else {
            format!(
                "Only {:.1} GB free on the volume hosting your Lucidos workspace. \
                 Lucidos itself uses just {:.1} GB — most of the pressure is from other apps on your machine. \
                 New Claude Code sessions may fail to spawn until you free space elsewhere.",
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
                }),
                &format!("[WorktreeCleanup] {} NotificationCreated", log_tag),
            )
            .await;
    }
}

/// Recognize a deterministic worktree directory name (`thread-<8-hex>`) and
/// return the lowercase 8-char short id. Returns `None` for legacy /
/// random-suffix names so the caller skips them.
///
/// The short id is used as a prefix lookup against the `events.aggregate_id`
/// column to recover the full thread `Uuid`. We don't reconstruct a Uuid by
/// zero-padding because the original Uuid is random across all 32 hex chars
/// and the padded form would never match.
fn parse_thread_short(dir_name: &str) -> Option<String> {
    let stripped = dir_name.strip_prefix("thread-")?;
    if stripped.len() != THREAD_WORKTREE_ID_LEN {
        return None;
    }
    if !stripped.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(stripped.to_ascii_lowercase())
}

/// True iff `child` is strictly inside `parent` after canonicalization. Used to
/// guard `remove_dir_all` against symlink escapes and accidental top-level
/// deletes. Falls back to a literal prefix check if canonicalization fails
/// (e.g. the path no longer exists), which is fine for the worktree-removal
/// path because the failure case is "child doesn't exist" → no harm done.
fn is_safe_subpath(parent: &Path, child: &Path) -> bool {
    let parent_canon = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
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
fn directory_age(path: &Path) -> Option<Duration> {
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
async fn resolve_repo_root_from_worktree(worktree: &Path) -> Option<PathBuf> {
    let out = git_cmd(&["rev-parse", "--git-common-dir"], worktree).await.ok()?;
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

/// True iff the worktree has any uncommitted changes (tracked or untracked).
pub(crate) async fn is_worktree_dirty(worktree: &Path) -> bool {
    match git_cmd(&["status", "--porcelain"], worktree).await {
        Ok(o) if o.status.success() => !o.stdout.is_empty(),
        Ok(o) => {
            // `git status` failing usually means the path is no longer a git
            // worktree (e.g. someone deleted `.git/worktrees/<name>` out of
            // band). Treat as dirty so we don't silently nuke it.
            log!(
                "[WorktreeCleanup] git status failed in {}: {} — treating as dirty",
                worktree.display(),
                String::from_utf8_lossy(&o.stderr).trim()
            );
            true
        }
        Err(e) => {
            log!(
                "[WorktreeCleanup] git status errored in {}: {} — treating as dirty",
                worktree.display(),
                e
            );
            true
        }
    }
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
/// main) delete the branch. Reused by the background cleanup worker (Tier 2)
/// and by the user-facing close + disk-usage endpoints.
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

/// One row in the disk-usage inventory served by `/api/disk-usage/worktrees`.
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
            Some(id) => id,
            None => continue,
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

/// Resolve a `thread-<8-hex>` directory's short id back to a full `Uuid`
/// by prefix-matching against the events table. The 8-char prefix is
/// effectively unique across the per-workspace thread space (collision
/// probability ~ N²/2³², so for ~10k threads/workspace the expected
/// collisions are ≪ 1). On the rare collision — or any sqlx error —
/// returns `None` so the caller skips the worktree. Refusing to act is
/// always safer than acting on the wrong thread.
async fn lookup_thread_by_short(pool: &sqlx::PgPool, short: &str) -> Option<Uuid> {
    // The id column is uuid; cast to text for the prefix LIKE. We match
    // against `aggregate_id` (the canonical per-event thread id) instead
    // of the legacy `thread_id` column to stay aligned with the rest of
    // the codebase.
    let pattern = format!("{}%", short);
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT aggregate_id::uuid FROM events \
         WHERE aggregate = 'thread' AND aggregate_id LIKE $1 \
         LIMIT 2",
    )
    .bind(pattern)
    .fetch_all(pool)
    .await
    .ok()?;
    if rows.len() != 1 {
        None
    } else {
        Some(rows[0].0)
    }
}

/// Time since the most recent thread event, or `None` when no events exist
/// (a stranded worktree) or the lookup fails. The cleanup worker treats
/// `None` as "don't act" rather than guessing.
async fn last_activity_age(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<Duration> {
    let last = lookup_last_activity(pool, thread_id).await?;
    let delta = Utc::now().signed_duration_since(last);
    // Negative deltas (clock skew) round to zero so we don't accidentally
    // treat a freshly-emitted event as ancient.
    Some(delta.to_std().unwrap_or(Duration::ZERO))
}

async fn lookup_thread_summary(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> (Option<String>, bool) {
    let row: Option<(Option<String>, bool)> = sqlx::query_as(
        "SELECT title, is_saved FROM thread_summaries WHERE thread_id = $1",
    )
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

async fn lookup_last_activity(
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

#[cfg(test)]
#[path = "worktree_cleanup_tests.rs"]
mod tests;
