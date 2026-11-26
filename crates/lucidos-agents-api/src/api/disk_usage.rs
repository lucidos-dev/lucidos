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
    run_recommended_cleanup, volume_reading, worktree_git_admin_missing, BranchDisposal,
    DiskThresholds, RecommendedCleanupOutcome, FREE_DISK_HARD_BYTES, FREE_DISK_SOFT_BYTES,
};

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

#[derive(Debug, Deserialize)]
pub struct BulkCleanupRequest {
    pub action: BulkCleanupAction,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BulkCleanupAction {
    Recommended,
}

static RECOMMENDED_CLEANUP_STATE: AtomicU8 = AtomicU8::new(PASS_IDLE);
const PASS_IDLE: u8 = 0;
const PASS_RUNNING: u8 = 1;
/// INVARIANT: The pass has ended and its terminal event is going out. A page re-reading
/// the summary on that event must see it finished. A new pass waits for the
/// event too, or the stale event would clear the new pass's cue.
const PASS_REPORTING: u8 = 2;

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

/// INVARIANT: POST /api/v1/disk-usage/cleanup: starts the recommended cleanup over every
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

#[derive(Debug, Deserialize)]
pub struct CleanupRequest {
    pub tier: u8,
}

pub(super) async fn cleanup_worktree(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<CleanupRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_uuid = Uuid::parse_str(&thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;
    if matches!(req.tier, 2 | 3) {
        super::thread_reach::refuse_without_authority(
            &state.engine.event_bus,
            &headers,
            Some(thread_uuid),
            crate::engine::permission::Action::Discard,
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

    // INVARIANT: Ask the background worker's liveness question first, through the same
    // probe (`ActiveThreads`). Tier 1 strips the build artifacts a running
    // build is using. Tiers 2 and 3 delete the directory the session runs in.
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
            if worktree_git_admin_missing(&worktree) {
                let dir = worktrees_dir(state.engine.workspace_path());
                let pre_size = directory_size_bytes(&worktree).await;
                let freed = remove_stranded_worktree(&dir, &worktree, pre_size)
                    .await
                    .ok_or_else(|| {
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
                    BranchDisposal::Keep,
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

pub(super) async fn summary(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let workspace = state.engine.workspace_path();
    let dir = worktrees_dir(workspace);
    let reading = volume_reading(&dir, workspace);
    let thresholds =
        DiskThresholds::for_reading(reading, FREE_DISK_SOFT_BYTES, FREE_DISK_HARD_BYTES);
    let workspace_data_bytes = directory_size_bytes(&workspace.join(crate::core::DATA_DIR)).await;

    Ok(Json(serde_json::json!({
        "free_bytes": reading.map(|r| r.free_bytes),
        "total_bytes": reading.map(|r| r.total_bytes),
        "workspace_data_bytes": workspace_data_bytes,
        "soft_threshold_bytes": thresholds.soft_bytes,
        "hard_threshold_bytes": thresholds.hard_bytes,
        "recommended_cleanup_running": recommended_cleanup_running(),
    })))
}

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

    mod guard_tests {
        fn production_src() -> String {
            crate::test_support::source_scan::read_production_source(
                &crate::test_support::source_scan::engine_source("api/disk_usage.rs"),
            )
        }

        #[test]
        fn the_cleanup_handler_keeps_the_branch_on_full_removal() {
            let src = production_src();
            let (_, call) = src
                .split_once("remove_worktree_and_optionally_delete_branch(")
                .expect("full cleanup removes the worktree");
            let args = call.split(".await").next().unwrap();
            assert!(args.contains("BranchDisposal::Keep"));
        }

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
                body.contains("Action::Discard"),
                "removing a worktree discards its changes, so it asks as Discard"
            );
        }
    }
}
