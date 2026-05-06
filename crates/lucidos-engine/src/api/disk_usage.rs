//! HTTP endpoints for the Settings → Disk Usage page (Phase 10.4).
//!
//! These endpoints surface the same machinery the background worktree
//! cleanup worker uses (`engine::worktree_cleanup`) but on demand, so the
//! user can see per-worktree disk usage and reclaim space at any time
//! instead of waiting for the hourly auto-cleanup tick.
//!
//! Endpoints:
//!
//! - `GET  /api/disk-usage/worktrees`               — inventory of all
//!   `<workspace>/.lucidos/worktrees/thread-<short>` directories paired
//!   with their thread metadata, sorted by size descending.
//! - `POST /api/disk-usage/worktrees/:thread_id/cleanup` with body
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
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use super::AppState;
use crate::engine::event_bus::BusEvent;
use crate::engine::git_ops::worktrees_dir;
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::worktree_cleanup::{
    available_disk_bytes, deterministic_worktree_for, directory_size_bytes, inventory_worktrees,
    is_worktree_dirty, prune_build_artifacts, remove_worktree_and_optionally_delete_branch,
    FREE_DISK_HARD_BYTES, FREE_DISK_SOFT_BYTES,
};

/// GET /api/disk-usage/worktrees — inventory of all known per-thread worktrees.
pub(super) async fn list_worktrees(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let rows = inventory_worktrees(state.engine.pool(), state.engine.workspace_path()).await;
    Ok(Json(serde_json::json!({ "worktrees": rows })))
}

/// Body for `POST /api/disk-usage/worktrees/:thread_id/cleanup`.
#[derive(Debug, Deserialize)]
pub struct CleanupRequest {
    /// 1 = strip build artifacts; 2 = remove worktree (clean only);
    /// 3 = remove worktree (allows dirty, gated by UI confirm).
    pub tier: u8,
}

/// POST /api/disk-usage/worktrees/:thread_id/cleanup — on-demand cleanup.
pub(super) async fn cleanup_worktree(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<CleanupRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_uuid = Uuid::parse_str(&thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    let worktree =
        deterministic_worktree_for(state.engine.workspace_path(), thread_uuid);
    if !worktree.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("No worktree on disk for thread {}", thread_uuid),
        ));
    }

    let (freed_bytes, branch_deleted) = match req.tier {
        1 => {
            let freed = prune_build_artifacts(&worktree).unwrap_or(0);
            (freed, false)
        }
        2 => {
            if is_worktree_dirty(&worktree).await {
                return Err((
                    StatusCode::CONFLICT,
                    "Worktree has uncommitted changes — use tier 3 to force-remove".to_string(),
                ));
            }
            let outcome = remove_worktree_and_optionally_delete_branch(&worktree, None)
                .await
                .ok_or_else(|| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Failed to remove worktree".to_string(),
                    )
                })?;
            (outcome.freed_bytes, outcome.branch_deleted)
        }
        3 => {
            let outcome = remove_worktree_and_optionally_delete_branch(&worktree, None)
                .await
                .ok_or_else(|| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Failed to remove worktree".to_string(),
                    )
                })?;
            (outcome.freed_bytes, outcome.branch_deleted)
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

/// GET /api/disk-usage/summary — free-disk stats + thresholds for the page header.
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
    let total_bytes = fs2::total_space(&dir).ok().or_else(|| fs2::total_space(workspace).ok());
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
