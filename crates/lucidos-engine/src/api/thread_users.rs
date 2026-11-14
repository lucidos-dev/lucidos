//! The thread users routes (ADR 0623): read who drives and watches a root
//! thread, and Take, Release, Watch or Unwatch it as the calling device's
//! user. Take over is Take on a root thread someone else drives.
//!
//! Any thread id names its root thread, through the family root. A role is
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
use crate::engine::thread_users::{self, ThreadUsers};

/// The 409 for a caller no user holds: a role belongs to a user.
pub(super) const NO_USER: &str = "This device belongs to no user yet, and a role in a \
    thread belongs to a user. Add the device to a user in Settings, Users.";

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
        .route("/threads/users", get(list))
        .route("/threads/:thread_id/users", get(read_roles))
        .route("/threads/:thread_id/users/take", post(take))
        .route("/threads/:thread_id/users/release", post(release))
        .route("/threads/:thread_id/users/watch", post(watch))
        .route("/threads/:thread_id/users/unwatch", post(unwatch))
}

fn roles_body(root: Uuid, roles: &ThreadUsers) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "root_thread_id": root,
        "driver": roles.driver,
        "watchers": roles.watchers,
    }))
}

/// The root thread of a thread id, or a 404 for an unknown thread.
async fn root_of(state: &AppState, raw: &str) -> Result<Uuid, ApiError> {
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

/// GET /api/v1/threads/users: the open root threads, newest activity first, each
/// marked `yours` when the caller's user drives or watches it.
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snapshot = crate::gateway_users::current();
    let user = crate::api::actor::user_actor(&headers, None)
        .and_then(|actor| snapshot.user_of_origin(&actor));
    let items = thread_users::list(&state.pool, &snapshot, user.as_ref())
        .await
        .map_err(ApiError::db)?;
    Ok(Json(serde_json::json!({
        "items": items,
        "user": user,
    })))
}

/// GET /api/v1/threads/:thread_id/users: the root thread's driver and watchers.
async fn read_roles(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let root = root_of(&state, &thread_id).await?;
    let snapshot = crate::gateway_users::current();
    let roles = thread_users::roles(&state.pool, root, &snapshot)
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
    let root = root_of(&state, &thread_id).await?;
    let snapshot = crate::gateway_users::current();
    let Some(who) = snapshot.user_of_origin(&actor) else {
        return Err(ApiError::new(StatusCode::CONFLICT, NO_USER).with_reason("no_user"));
    };
    let roles = thread_users::roles(&state.pool, root, &snapshot)
        .await
        .map_err(ApiError::db)?;
    let drives = roles.driver.as_ref().is_some_and(|d| d.id == who.id);
    let watches = roles.watchers.iter().any(|w| w.id == who.id);
    let event = match change {
        RoleChange::Take if !drives => Some(ThreadEvent::WorkItemDriverTaken),
        RoleChange::Release if drives => Some(ThreadEvent::WorkItemDriverReleased),
        RoleChange::Watch if !watches => Some(ThreadEvent::WorkItemWatched { user_id: None }),
        RoleChange::Unwatch if watches => Some(ThreadEvent::WorkItemUnwatched),
        _ => None,
    };
    let Some(event) = event else {
        return Ok(roles_body(root, &roles));
    };
    thread_users::record(&state.engine.event_bus, root, event, Some(actor))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if let (RoleChange::Take, Some(from)) = (change, roles.driver.clone()) {
        let notice = thread_users::UserNotice::TakenOver(thread_users::Takeover { from, to: who });
        // The take stands either way, so a failed notice only logs.
        if let Err(e) = thread_users::notify(&state.engine, root, notice, &snapshot).await {
            log!(
                "[ThreadUsers] Could not tell the driver of {} it was taken over: {}",
                root,
                e
            );
        }
    }
    let roles = thread_users::roles(&state.pool, root, &snapshot)
        .await
        .map_err(ApiError::db)?;
    Ok(roles_body(root, &roles))
}

/// POST /api/v1/threads/:thread_id/users/take: the caller's user drives the
/// root thread, from whoever drove it before.
async fn take(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_role(state, headers, thread_id, RoleChange::Take).await
}

/// POST /api/v1/threads/:thread_id/users/release: the caller's user stops
/// driving. A caller who does not drive it changes nothing.
async fn release(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_role(state, headers, thread_id, RoleChange::Release).await
}

/// POST /api/v1/threads/:thread_id/users/watch: the caller's user watches.
async fn watch(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_role(state, headers, thread_id, RoleChange::Watch).await
}

/// POST /api/v1/threads/:thread_id/users/unwatch: the caller's user stops
/// watching, from any of their devices.
async fn unwatch(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_role(state, headers, thread_id, RoleChange::Unwatch).await
}
