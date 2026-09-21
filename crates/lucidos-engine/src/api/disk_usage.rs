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
//! - `POST /api/v1/disk-usage/worktrees/:thread_id/cleanup` with body
//!   `{ "tier": 1 | 2 | 3 }`:
//!   - **Tier 1** strips regenerable build artifacts (`target/`,
//!     `node_modules/`, `.lucidos/cache/`). Worktree dir stays.
//!   - **Tier 2** removes the entire worktree directory; refuses on dirty
//!     worktrees so we don't silently drop uncommitted edits.
//!   - **Tier 3** is the same as Tier 2 but allows removing dirty
//!     worktrees — the UI gates this behind an explicit confirmation.
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
use uuid::Uuid;

use super::AppState;
use crate::engine::event_bus::BusEvent;
use crate::engine::git_ops::worktrees_dir;
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::worktree_cleanup::{
    available_disk_bytes, deterministic_worktree_for, directory_size_bytes, inventory_worktrees,
    is_worktree_dirty, prune_build_artifacts, remove_stranded_worktree,
    remove_worktree_and_optionally_delete_branch, worktree_git_admin_missing, ActiveThreads,
    AgentSessionsActiveThreads, BranchDisposal, FREE_DISK_HARD_BYTES, FREE_DISK_SOFT_BYTES,
};

/// GET /api/v1/disk-usage/worktrees — inventory of all known per-thread worktrees.
pub(super) async fn list_worktrees(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let rows = inventory_worktrees(state.engine.pool(), state.engine.workspace_path()).await;
    Ok(Json(serde_json::json!({ "worktrees": rows })))
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
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    let worktree = deterministic_worktree_for(state.engine.workspace_path(), thread_uuid);
    if !worktree.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("No worktree on disk for thread {}", thread_uuid),
        ));
    }

    // Ask the same question the background worker asks first, and for the same
    // reason (`worktree_cleanup.rs`): a live coding-agent session parked on
    // `AskUserQuestion` emits no events, so no age or dirtiness check can see
    // it. Tier 1 strips the build artifacts a running build is using. Tiers 2
    // and 3 delete the directory the subprocess runs in, and take its branch
    // with them. The worker's helpers say "the caller has already skipped
    // active", and this caller had not.
    if AgentSessionsActiveThreads::new(state.engine.agent_sessions.clone())
        .is_active(thread_uuid)
        .await
    {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "Thread {} has a live coding-agent session in this worktree. \
                 Stop it first, or wait for it to finish: cleaning up now would \
                 delete the tree it is working in.",
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
            let freed = prune_build_artifacts(&worktree).unwrap_or(0);
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
    let free_bytes = available_disk_bytes(&dir).or_else(|| available_disk_bytes(workspace));
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
    })))
}

/// Routes for the `/disk-usage/*` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/disk-usage/summary", get(summary))
        .route("/disk-usage/worktrees", get(list_worktrees))
        .route(
            "/disk-usage/worktrees/:thread_id/cleanup",
            post(cleanup_worktree),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// This file with its test module cut off. Uncut, a scan can match its own
    /// test fixtures and pass while production has lost the thing it pins.
    fn production_src() -> String {
        crate::test_support::source_scan::read_production_source(
            &crate::test_support::source_scan::src_root().join("api/disk_usage.rs"),
        )
    }

    /// A live session answers `is_active`, and nothing else here can.
    ///
    /// `AgentSession::is_live` is the liveness signal, not mere presence in the
    /// map: a phantom left by a dropped run future used to hold `true` forever
    /// and block reclamation of a tree whose subprocess was long gone.
    #[tokio::test]
    async fn an_empty_session_map_reports_no_live_thread() {
        let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let probe = AgentSessionsActiveThreads::new(sessions);
        assert!(!probe.is_active(uuid::Uuid::new_v4()).await);
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
}
