//! HTTP endpoints for the Settings → Disk Usage page (Phase 10.4).
//!
//! These endpoints surface the same machinery the background worktree
//! cleanup worker uses (`engine::worktree_cleanup`) but on demand, so the
//! user can see per-worktree disk usage and reclaim space at any time
//! instead of waiting for the hourly auto-cleanup tick.
//!
//! Endpoints:
//!
//! - `GET  /api/v1/disk-usage/worktrees`               — inventory of all
//!   `<workspace>/.lucidos/worktrees/thread-<short>` directories paired
//!   with their thread metadata, sorted by size descending.
//! - `POST /api/v1/disk-usage/cleanup` with body `{ "action": "recommended" }`
//!   runs the recommended cleanup over every worktree: a finished worktree is
//!   removed, every other one loses its build artifacts, and live or pinned
//!   threads are skipped (`engine::worktree_cleanup::run_recommended_cleanup`).
//!   It answers 202 at once and reports through `RecommendedCleanupStarted`,
//!   then `RecommendedCleanupCompleted` or `RecommendedCleanupFailed`.
//!   One pass runs at a time; a second request gets a 409.
//! - `POST /api/v1/disk-usage/worktrees/:thread_id/cleanup` with body
//!   `{ "tier": 1 | 2 | 3 }`:
//!   - **Tier 1** strips regenerable build artifacts (`target/`,
//!     `node_modules/`, `.lucidos/cache/`). Worktree dir stays.
//!   - **Tier 2** removes the entire worktree directory; refuses on dirty
//!     worktrees so we don't silently drop uncommitted edits.
//!   - **Tier 3** is the same as Tier 2 but allows removing dirty
//!     worktrees — the UI gates this behind an explicit confirmation.
//!
//!   Tiers 2 and 3 take the `thread_reach` Discard gate, so an agent can
//!   remove only its own subtree's worktrees unless the owner stands behind it.
//!
//! On successful cleanup the endpoint emits `WorktreeCleaned` so the rest
//! of the system (status badges, threshold accounting) sees the same
//! event the background worker emits.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::atomic::{AtomicU8, Ordering};
use uuid::Uuid;

use super::AppState;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::git_ops::worktrees_dir;
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::worktree_cleanup::{
    deterministic_worktree_for, directory_size_bytes, inventory_worktrees, is_worktree_dirty,
    prune_build_artifacts, remove_stranded_worktree, remove_worktree_and_optionally_delete_branch,
    run_recommended_cleanup, volume_free_bytes, worktree_git_admin_missing, BranchDisposal,
    RecommendedCleanupOutcome, FREE_DISK_HARD_BYTES, FREE_DISK_SOFT_BYTES,
};

/// GET /api/v1/disk-usage/worktrees — inventory of all known per-thread worktrees.
pub(super) async fn list_worktrees(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let active_threads = state.engine.worktree_cleanup_active_threads();
    let rows = inventory_worktrees(
        state.engine.pool(),
        state.engine.workspace_path(),
        active_threads.as_ref(),
    )
    .await;
    Ok(Json(serde_json::json!({ "worktrees": rows })))
}

/// Body for `POST /api/v1/disk-usage/cleanup`.
#[derive(Debug, Deserialize)]
pub struct BulkCleanupRequest {
    pub action: BulkCleanupAction,
}

/// The one bulk action so far. Kebab-case on the wire.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BulkCleanupAction {
    Recommended,
}

/// Where the one recommended pass stands. Idle is the only state that may start
/// a pass, so a double tap or a second device cannot run two over the same trees.
static RECOMMENDED_CLEANUP_STATE: AtomicU8 = AtomicU8::new(PASS_IDLE);
const PASS_IDLE: u8 = 0;
/// The pass is walking worktrees. The summary reports only this as running.
const PASS_RUNNING: u8 = 1;
/// The pass has ended and its terminal event is going out. A page re-reading
/// the summary on that event must see it finished. A new pass waits for the
/// event too, or the stale event would clear the new pass's cue.
const PASS_REPORTING: u8 = 2;

/// The one running pass. Dropping it returns the state to idle, panics included.
struct RunningPass;

impl RunningPass {
    fn try_start() -> Option<Self> {
        RECOMMENDED_CLEANUP_STATE
            .compare_exchange(PASS_IDLE, PASS_RUNNING, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self)
    }

    fn reporting(&self) {
        RECOMMENDED_CLEANUP_STATE.store(PASS_REPORTING, Ordering::SeqCst);
    }
}

impl Drop for RunningPass {
    fn drop(&mut self) {
        RECOMMENDED_CLEANUP_STATE.store(PASS_IDLE, Ordering::SeqCst);
    }
}

fn recommended_cleanup_running() -> bool {
    RECOMMENDED_CLEANUP_STATE.load(Ordering::SeqCst) == PASS_RUNNING
}

/// POST /api/v1/disk-usage/cleanup: starts the recommended cleanup over every
/// worktree and returns 202 at once.
///
/// A pass takes seconds per worktree, so a large one runs for minutes. WebKit
/// drops a request that answers nothing for about a minute, so the outcome
/// arrives as `RecommendedCleanupCompleted` or `RecommendedCleanupFailed`.
pub(super) async fn cleanup_all(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<BulkCleanupRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    let BulkCleanupAction::Recommended = req.action;
    let actor = super::actor::user_actor(&headers, None);
    let Some(running) = RunningPass::try_start() else {
        return Err((
            StatusCode::CONFLICT,
            "A recommended cleanup is already running. Wait for it to finish.".to_string(),
        ));
    };
    let engine = state.engine.clone();
    engine
        .event_bus
        .emit_or_log(
            BusEvent::System(SystemEvent::RecommendedCleanupStarted {
                actor: actor.clone(),
            }),
            "[DiskUsage] RecommendedCleanupStarted",
        )
        .await;
    tokio::spawn(async move {
        let pass_engine = engine.clone();
        let pass = tokio::spawn(async move {
            let active_threads = pass_engine.worktree_cleanup_active_threads();
            run_recommended_cleanup(
                pass_engine.pool(),
                &pass_engine.event_bus,
                pass_engine.workspace_path(),
                active_threads.as_ref(),
                actor,
            )
            .await
        });
        let ended = recommended_cleanup_ended(pass.await);
        running.reporting();
        engine
            .event_bus
            .emit_or_log(
                BusEvent::System(ended),
                "[DiskUsage] recommended cleanup ended",
            )
            .await;
        drop(running);
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "started" })),
    ))
}

/// The one event that ends a pass. A panicked pass still reports, as a failure.
fn recommended_cleanup_ended(
    pass: Result<RecommendedCleanupOutcome, tokio::task::JoinError>,
) -> SystemEvent {
    match pass {
        Ok(outcome) => SystemEvent::RecommendedCleanupCompleted {
            removed_count: outcome.removed_count,
            cleaned_count: outcome.cleaned_count,
            freed_bytes: outcome.freed_bytes,
        },
        Err(e) => SystemEvent::RecommendedCleanupFailed {
            error: format!("The recommended cleanup stopped: {e}"),
        },
    }
}

/// Body for `POST /api/v1/disk-usage/worktrees/:thread_id/cleanup`.
#[derive(Debug, Deserialize)]
pub struct CleanupRequest {
    /// 1 = strip build artifacts; 2 = remove worktree (clean only);
    /// 3 = remove worktree (allows dirty, gated by UI confirm).
    pub tier: u8,
}

/// POST /api/v1/disk-usage/worktrees/:thread_id/cleanup — on-demand cleanup.
pub(super) async fn cleanup_worktree(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<CleanupRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_uuid = Uuid::parse_str(&thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;
    // Removing a worktree discards the thread's work, so tiers 2 and 3 take the
    // Discard gate before the disk is read. Tier 1 strips only regenerable output.
    if matches!(req.tier, 2 | 3) {
        super::thread_reach::refuse_without_authority(
            &state.pool,
            &headers,
            Some(thread_uuid),
            super::thread_reach::ThreadReachVerb::Discard,
        )
        .await
        .map_err(|e| (e.status_code(), e.to_string()))?;
    }
    let actor = super::actor::user_actor(&headers, None);
    let worktree = deterministic_worktree_for(state.engine.workspace_path(), thread_uuid);
    if !worktree.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("No worktree on disk for thread {}", thread_uuid),
        ));
    }

    // Ask the background worker's liveness question first, through the same
    // probe (`ActiveThreads`). Tier 1 strips the build artifacts a running
    // build is using. Tiers 2 and 3 delete the directory the work runs in, and
    // take its branch with them. The worker's helpers say "the caller has
    // already skipped active", and this caller had not.
    if state
        .engine
        .worktree_cleanup_active_threads()
        .is_active(thread_uuid)
        .await
    {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "Thread {} has a coding-agent session starting or running, or a \
                 background task running, in this worktree. Stop it first, or wait \
                 for it to finish: cleaning up now would delete the tree it is \
                 working in.",
                thread_uuid
            ),
        ));
    }

    if req.tier == 2 && is_worktree_dirty(&worktree).await {
        return Err((
            StatusCode::CONFLICT,
            "Worktree has uncommitted changes — use tier 3 to force-remove".to_string(),
        ));
    }

    let (freed_bytes, branch_deleted) = match req.tier {
        1 => {
            let freed = prune_build_artifacts(&worktree).await.unwrap_or(0);
            (freed, false)
        }
        2 | 3 => {
            // Stranded worktree (git admin dir gone): the git-based helper
            // can't resolve a repo root and would 500. Remove the directory
            // directly — the inventory already surfaces these rows as dirty, so
            // the UI reaches here via tier 3 (force).
            if worktree_git_admin_missing(&worktree) {
                let dir = worktrees_dir(state.engine.workspace_path());
                let pre_size = directory_size_bytes(&worktree);
                let freed =
                    remove_stranded_worktree(&dir, &worktree, pre_size).ok_or_else(|| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "Failed to remove stranded worktree".to_string(),
                        )
                    })?;
                (freed, false)
            } else {
                let outcome = remove_worktree_and_optionally_delete_branch(
                    &worktree,
                    None,
                    BranchDisposal::WhenMerged,
                )
                .await
                .ok_or_else(|| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Failed to remove worktree".to_string(),
                    )
                })?;
                (outcome.freed_bytes, outcome.branch_deleted)
            }
        }
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Invalid tier {} (must be 1, 2, or 3)", other),
            ));
        }
    };

    // Emit WorktreeCleaned so other consumers (status badges, threshold accounting)
    // see the same event the background worker would emit. Tier 3 is reported as
    // tier 2 in the event because both result in the same on-disk outcome
    // (worktree gone) — the event variant only distinguishes "artifact strip"
    // from "full removal".
    let event_tier: u8 = if req.tier == 1 { 1 } else { 2 };
    state
        .engine
        .event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id: thread_uuid,
                event: ThreadEvent::WorktreeCleaned {
                    tier: event_tier,
                    freed_bytes,
                    branch_deleted,
                },
                meta: EventMeta::with_actor(actor),
            },
            "[DiskUsage] WorktreeCleaned",
        )
        .await;

    Ok(Json(serde_json::json!({
        "tier": req.tier,
        "freed_bytes": freed_bytes,
        "branch_deleted": branch_deleted,
    })))
}

/// GET /api/v1/disk-usage/summary — free-disk stats + thresholds for the page header.
///
/// `free_bytes` and `total_bytes` are `null` when the OS disk-info call fails
/// (rare; typically only when the workspace volume is unreadable). Frontend
/// computes per-worktree-total locally from the `/disk-usage/worktrees` rows —
/// we don't duplicate that walk here.
///
/// `workspace_data_bytes` walks `<workspace>/data/` (artifacts, postgres,
/// apps, knowhow, …) so the chart can show it as a distinct segment instead
/// of letting it disappear into "Other apps".
pub(super) async fn summary(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let workspace = state.engine.workspace_path();
    let dir = worktrees_dir(workspace);
    let free_bytes = volume_free_bytes(&dir, workspace);
    let total_bytes = fs2::total_space(&dir)
        .ok()
        .or_else(|| fs2::total_space(workspace).ok());
    // The walk can hit a multi-GB postgres data dir with many files — keep it
    // off the tokio worker thread so other requests aren't stalled.
    let data_dir = workspace.join(crate::core::DATA_DIR);
    let workspace_data_bytes =
        match tokio::task::spawn_blocking(move || directory_size_bytes(&data_dir)).await {
            Ok(bytes) => bytes,
            Err(e) => {
                crate::log!("[DiskUsage] data dir walk task failed: {}", e);
                0
            }
        };

    Ok(Json(serde_json::json!({
        "free_bytes": free_bytes,
        "total_bytes": total_bytes,
        "workspace_data_bytes": workspace_data_bytes,
        "soft_threshold_bytes": FREE_DISK_SOFT_BYTES,
        "hard_threshold_bytes": FREE_DISK_HARD_BYTES,
        "recommended_cleanup_running": recommended_cleanup_running(),
    })))
}

/// Routes for the `/disk-usage/*` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/disk-usage/summary", get(summary))
        .route("/disk-usage/worktrees", get(list_worktrees))
        .route("/disk-usage/cleanup", post(cleanup_all))
        .route(
            "/disk-usage/worktrees/:thread_id/cleanup",
            post(cleanup_worktree),
        )
}

#[cfg(test)]
mod tests {
    /// This file with its test module cut off. Uncut, a scan can match its own
    /// test fixtures and pass while production has lost the thing it pins.
    fn production_src() -> String {
        crate::test_support::source_scan::read_production_source(
            &crate::test_support::source_scan::src_root().join("api/disk_usage.rs"),
        )
    }

    /// The handler asks the worker's own probe. A hand-built one drifts from
    /// what the worker counts as live.
    #[test]
    fn the_cleanup_handler_asks_the_workers_liveness_probe() {
        let src = production_src();
        assert!(
            src.contains("worktree_cleanup_active_threads()"),
            "cleanup_worktree must ask the engine's shared liveness probe"
        );
        assert!(
            !src.contains("ActiveThreads::new("),
            "no hand-built liveness probe in the handler"
        );
    }

    /// One test walks the whole cycle: the state is a process-wide static.
    #[test]
    fn a_pass_reads_finished_while_it_reports_and_blocks_a_new_one_until_done() {
        let pass = super::RunningPass::try_start().expect("idle at the start");
        assert!(super::recommended_cleanup_running());
        assert!(
            super::RunningPass::try_start().is_none(),
            "one pass at a time"
        );

        pass.reporting();
        assert!(
            !super::recommended_cleanup_running(),
            "a summary read on the terminal event sees the pass finished"
        );
        assert!(
            super::RunningPass::try_start().is_none(),
            "no new pass until the terminal event is out"
        );

        drop(pass);
        let next = super::RunningPass::try_start().expect("idle once the pass reported");
        drop(next);
    }

    #[tokio::test]
    async fn a_finished_pass_reports_its_outcome() {
        let pass = tokio::spawn(async {
            super::RecommendedCleanupOutcome {
                removed_count: 3,
                cleaned_count: 2,
                freed_bytes: 1024,
            }
        });
        match super::recommended_cleanup_ended(pass.await) {
            super::SystemEvent::RecommendedCleanupCompleted {
                removed_count,
                cleaned_count,
                freed_bytes,
            } => assert_eq!((removed_count, cleaned_count, freed_bytes), (3, 2, 1024)),
            other => panic!("expected RecommendedCleanupCompleted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_panicked_pass_still_reports_as_a_failure() {
        fn walk_that_blows_up() -> super::RecommendedCleanupOutcome {
            panic!("worktree walk blew up")
        }
        let pass = tokio::spawn(async { walk_that_blows_up() });
        match super::recommended_cleanup_ended(pass.await) {
            super::SystemEvent::RecommendedCleanupFailed { error } => {
                assert!(error.contains("stopped"), "{error}")
            }
            other => panic!("expected RecommendedCleanupFailed, got {other:?}"),
        }
    }

    /// The liveness gate runs BEFORE the tier match, so every tier is covered.
    ///
    /// A gate inside one tier arm leaves the other two destroying a live
    /// session's tree. `cleanup_worktree` states what each tier removes.
    #[test]
    fn the_cleanup_handler_refuses_a_live_session_before_it_picks_a_tier() {
        let src = production_src();
        let at = src
            .find("pub(super) async fn cleanup_worktree")
            .expect("cleanup_worktree is still here");
        let body = &src[at..];
        let gate = body
            .find("is_active(thread_uuid)")
            .expect("cleanup_worktree must ask whether a coding-agent session is live");
        let tiers = body
            .find("match req.tier")
            .expect("cleanup_worktree still dispatches on the tier");
        assert!(
            gate < tiers,
            "the live-session refusal must come before the tier match, or tier 1 \
             still strips a running build's artifacts"
        );
    }

    /// Tiers 2 and 3 remove another thread's worktree, so they take the reach
    /// gate before the handler reads the disk. A gate after the dirty check
    /// would still tell an out-of-reach agent whether the tree holds edits.
    #[test]
    fn the_cleanup_handler_asks_the_reach_gate_before_it_removes_anything() {
        let src = production_src();
        let at = src
            .find("pub(super) async fn cleanup_worktree")
            .expect("cleanup_worktree is still here");
        let body = &src[at..];
        let gate = body
            .find("refuse_without_authority(")
            .expect("cleanup_worktree must ask the thread_reach gate");
        for later in ["worktree.exists()", "is_worktree_dirty(", "match req.tier"] {
            let at = body
                .find(later)
                .unwrap_or_else(|| panic!("cleanup_worktree still calls {later}"));
            assert!(gate < at, "the reach gate must come before {later}");
        }
        assert!(
            body.contains("ThreadReachVerb::Discard"),
            "removing a worktree discards its changes, so it asks as Discard"
        );
    }
}
