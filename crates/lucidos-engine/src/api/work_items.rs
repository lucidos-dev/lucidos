//! The work item role routes (ADR 0623): read who drives and watches a work
//! item, and Take, Release, Watch or Unwatch it as the calling device's
//! person. Take over is Take on a work item someone else drives.
//!
//! Any thread id names its work item, through the family root. A role is
//! never authority, so no route refuses a caller because of one. A repeat
//! that changes nothing records nothing.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use uuid::Uuid;

use super::error::ApiError;
use super::AppState;
use crate::engine::thread_events::ThreadEvent;
use crate::engine::work_item::{self, WorkItemRoles};

/// The 409 for a caller no operator holds: a role belongs to a person.
pub(super) const NO_PERSON: &str = "This device belongs to no person yet, and a work item \
    role belongs to a person. Add the device to a person in Settings, People.";

/// One role change a caller can make.
#[derive(Debug, Clone, Copy)]
enum RoleChange {
    Take,
    Release,
    Watch,
    Unwatch,
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/work-items", get(list))
        .route("/work-items/:thread_id", get(read_roles))
        .route("/work-items/:thread_id/take", post(take))
        .route("/work-items/:thread_id/release", post(release))
        .route("/work-items/:thread_id/watch", post(watch))
        .route("/work-items/:thread_id/unwatch", post(unwatch))
}

fn roles_body(root: Uuid, roles: &WorkItemRoles) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "root_thread_id": root,
        "driver": roles.driver,
        "watchers": roles.watchers,
    }))
}

/// The work item root of a thread id, or a 404 for an unknown thread.
async fn work_item_root(state: &AppState, raw: &str) -> Result<Uuid, ApiError> {
    let thread_id = Uuid::parse_str(raw)
        .map_err(|e| ApiError::bad_request(format!("Invalid thread_id: {e}")))?;
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1)")
            .bind(thread_id)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::db)?;
    if !known {
        return Err(ApiError::not_found(format!(
            "No thread {thread_id} in this workspace."
        )));
    }
    crate::engine::thread_family::family_root(&state.pool, thread_id)
        .await
        .map_err(ApiError::db)
}

/// GET /api/v1/work-items: the open work items, newest activity first, each
/// marked `yours` when the caller's person drives or watches it.
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snapshot = crate::gateway_operators::current();
    let person = crate::api::actor::user_actor(&headers, None)
        .and_then(|actor| snapshot.operator_of_origin(&actor));
    let items = work_item::list(&state.pool, &snapshot, person.as_ref())
        .await
        .map_err(ApiError::db)?;
    Ok(Json(serde_json::json!({
        "items": items,
        "person": person,
    })))
}

/// GET /api/v1/work-items/:thread_id: the work item's driver and watchers.
async fn read_roles(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let root = work_item_root(&state, &thread_id).await?;
    let snapshot = crate::gateway_operators::current();
    let roles = work_item::roles(&state.pool, root, &snapshot)
        .await
        .map_err(ApiError::db)?;
    Ok(roles_body(root, &roles))
}

async fn change_role(
    state: AppState,
    headers: HeaderMap,
    thread_id: String,
    change: RoleChange,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let root = work_item_root(&state, &thread_id).await?;
    let snapshot = crate::gateway_operators::current();
    let Some(who) = snapshot.operator_of_origin(&actor) else {
        return Err(ApiError::new(StatusCode::CONFLICT, NO_PERSON).with_reason("no_person"));
    };
    let roles = work_item::roles(&state.pool, root, &snapshot)
        .await
        .map_err(ApiError::db)?;
    let drives = roles.driver.as_ref().is_some_and(|d| d.id == who.id);
    let watches = roles.watchers.iter().any(|w| w.id == who.id);
    let event = match change {
        RoleChange::Take if !drives => Some(ThreadEvent::WorkItemDriverTaken),
        RoleChange::Release if drives => Some(ThreadEvent::WorkItemDriverReleased),
        RoleChange::Watch if !watches => Some(ThreadEvent::WorkItemWatched),
        RoleChange::Unwatch if watches => Some(ThreadEvent::WorkItemUnwatched),
        _ => None,
    };
    let Some(event) = event else {
        return Ok(roles_body(root, &roles));
    };
    work_item::record(&state.engine.event_bus, root, event, Some(actor))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if let (RoleChange::Take, Some(from)) = (change, roles.driver.clone()) {
        let notice = work_item::WorkItemNotice::TakenOver(work_item::Takeover { from, to: who });
        // The take stands either way, so a failed notice only logs.
        if let Err(e) = work_item::notify(&state.engine, root, notice, &snapshot).await {
            log!(
                "[WorkItems] Could not tell the driver of {} it was taken over: {}",
                root,
                e
            );
        }
    }
    let roles = work_item::roles(&state.pool, root, &snapshot)
        .await
        .map_err(ApiError::db)?;
    Ok(roles_body(root, &roles))
}

/// POST /api/v1/work-items/:thread_id/take: the caller's person drives the
/// work item, from whoever drove it before.
async fn take(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_role(state, headers, thread_id, RoleChange::Take).await
}

/// POST /api/v1/work-items/:thread_id/release: the caller's person stops
/// driving. A caller who does not drive it changes nothing.
async fn release(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_role(state, headers, thread_id, RoleChange::Release).await
}

/// POST /api/v1/work-items/:thread_id/watch: the caller's person watches.
async fn watch(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_role(state, headers, thread_id, RoleChange::Watch).await
}

/// POST /api/v1/work-items/:thread_id/unwatch: the caller's person stops
/// watching, from any of their devices.
async fn unwatch(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_role(state, headers, thread_id, RoleChange::Unwatch).await
}
