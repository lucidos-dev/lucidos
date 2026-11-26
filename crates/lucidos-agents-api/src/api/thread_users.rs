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

pub(super) const NO_USER: &str = "This device belongs to no user yet, and a role in a \
    thread belongs to a user. Add the device to a user in Settings, Users.";

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
        RoleChange::Take if !drives => Some(ThreadEvent::RootThreadDriverTaken),
        RoleChange::Release if drives => Some(ThreadEvent::RootThreadDriverReleased),
        RoleChange::Watch if !watches => Some(ThreadEvent::RootThreadWatched { user_id: None }),
        RoleChange::Unwatch if watches => Some(ThreadEvent::RootThreadUnwatched),
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
        if let Err(e) = thread_users::notify(&state.engine, root, notice, &snapshot).await {
            log_legacy!(
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

async fn take(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_role(state, headers, thread_id, RoleChange::Take).await
}

async fn release(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_role(state, headers, thread_id, RoleChange::Release).await
}

async fn watch(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_role(state, headers, thread_id, RoleChange::Watch).await
}

async fn unwatch(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_role(state, headers, thread_id, RoleChange::Unwatch).await
}
