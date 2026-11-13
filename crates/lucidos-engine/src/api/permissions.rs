//! List and revoke *permissions* (ADR 0664): `GET /api/v1/permissions` and
//! `DELETE /api/v1/permissions/:permission_id`. Raise a *batch* with
//! `POST /api/v1/permissions/batches`.
//!
//! Revoking only takes power away, so any identified caller may revoke. Only
//! the owner's answer to a permission request grants one, never this route.
//! A batch is such a request: raising one grants nothing until the owner
//! picks Run all on it.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::actor::SubprocessOrigin;
use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::permission::batch::{raise_batch, BatchError, BatchItemRequest};
use crate::engine::permission::{self, RevokeOutcome};

#[derive(Deserialize)]
struct ListQuery {
    /// Include ended permissions too. Live ones only by default.
    #[serde(default)]
    all: bool,
}

async fn list_permissions(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let permissions = permission::list(&state.pool, !query.all)
        .await
        .map_err(ApiError::db)?;
    Ok(Json(serde_json::json!({ "permissions": permissions })))
}

async fn revoke_permission(
    State(state): State<AppState>,
    Path(permission_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let permission_id = Uuid::parse_str(&permission_id)
        .map_err(|_| ApiError::bad_request("Invalid permission_id"))?;
    match permission::revoke(
        &state.engine.event_bus,
        &state.engine.grants_dir(),
        permission_id,
        Some(actor),
    )
    .await
    .map_err(|e| ApiError::internal(format!("Could not revoke the permission: {e}")))?
    {
        RevokeOutcome::Revoked => Ok(Json(serde_json::json!({ "revoked": true }))),
        RevokeOutcome::AlreadyEnded => Ok(Json(
            serde_json::json!({ "revoked": false, "reason": "already_ended" }),
        )),
        RevokeOutcome::NotFound => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("No permission {permission_id}"),
        )),
    }
}

#[derive(Deserialize)]
struct RaiseBatchBody {
    items: Vec<BatchItemRequest>,
}

/// Raise a batch on the calling agent's own thread. It carries exact picks on
/// open cards in the sub-threads below it, and grants to triggers. Answers
/// `{"tool_use_id": ...}`, the batch card's id.
async fn raise_batch_card(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<RaiseBatchBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Json(body) = body.map_err(|e| {
        ApiError::bad_request(format!(
            "The body must be {{\"items\": [...]}}, each item {{\"kind\": \"card_answer\", \
             \"thread_id\": ..., \"option\": ...}} or {{\"kind\": \"trigger_grant\", \
             \"trigger\": ..., \"side_effects\": [...], \"coding_agents\": ...}} or \
             {{\"kind\": \"workspace_classification\", \"workspaces\": [...], \"origin\": ..., \
             \"evidence\": ...}}: {e}"
        ))
    })?;
    let SubprocessOrigin::Subprocess {
        source_thread_id: Some(raiser),
        ..
    } = crate::api::actor::subprocess_origin(&headers)
    else {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "An agent raises a batch on its own thread, so this needs a thread's agent.",
        ));
    };
    match raise_batch(
        &state.engine.event_bus,
        &state.engine.trigger_configs,
        raiser,
        &body.items,
    )
    .await
    {
        Ok(tool_use_id) => Ok(Json(serde_json::json!({ "tool_use_id": tool_use_id }))),
        Err(BatchError::Refused(why)) => Err(ApiError::bad_request(why)),
        Err(e @ BatchError::Unreadable(_)) => Err(ApiError::internal(e.to_string())),
    }
}

pub(super) fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/permissions", get(list_permissions))
        .route("/permissions/batches", post(raise_batch_card))
        .route("/permissions/:permission_id", delete(revoke_permission))
}
