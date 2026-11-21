//! The workspace broker's gateway half: `/~/api/v1/control/broker/*`.
//!
//! An engine calls these routes for one of its agent threads, after it spent
//! the user's Allow once for that verb and workspace. The gateway trusts no
//! caller but a local process, and enforces the verb split itself: delete and
//! Trash restore act only on a machine-made workspace, read under the
//! workspace lock. Stop, start, autostart and rename reuse the ordinary control
//! routes. On a machine-made workspace the engine calls those with no card.
//! There is no restart route here, by design (ADR 0671).

use crate::auth;
use crate::error::ApiError;
use crate::registry::{CreatedBy, WorkspaceOrigin};
use crate::server::GatewayState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn router() -> Router<GatewayState> {
    Router::new()
        .nest("/host-logs", crate::host_logs::router())
        .route("/workspaces", post(create))
        .route("/workspaces/:id/delete", post(delete))
        .route("/workspaces/:id/device-session", post(device_session))
        .route("/workspaces/:id/classify", post(classify))
        .route("/trash/:id/restore", post(restore))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    name: String,
    slug: Option<String>,
    #[serde(default)]
    autostart: bool,
    created_by: CreatedBy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallerBody {
    caller: CreatedBy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassifyBody {
    origin: WorkspaceOrigin,
}

/// A local engine only: no paired device, no agent's own origin token.
pub(crate) fn local_engine(
    local: Option<Extension<auth::AuthenticatedLocalProcess>>,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    if local.is_none() || headers.contains_key(auth::HEADER_AGENT_ORIGIN_TOKEN) {
        return Err(ApiError::forbidden(
            "the workspace broker serves only an engine on this machine",
        ));
    }
    Ok(())
}

pub(crate) fn valid_caller(caller: &CreatedBy) -> Result<(), ApiError> {
    if crate::registry::is_valid_id(&caller.workspace_id)
        && !caller.thread_id.is_empty()
        && caller.thread_id.len() <= 64
    {
        return Ok(());
    }
    Err(ApiError::bad_request(
        "the broker needs the calling workspace and thread",
    ))
}

fn valid_id(id: &str) -> Result<(), ApiError> {
    if crate::registry::is_valid_id(id) {
        return Ok(());
    }
    Err(ApiError::bad_request("invalid workspace id"))
}

async fn create(
    State(state): State<GatewayState>,
    local: Option<Extension<auth::AuthenticatedLocalProcess>>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Result<Json<Value>, ApiError> {
    local_engine(local, &headers).map_err(ApiError::unchanged)?;
    valid_caller(&body.created_by).map_err(ApiError::unchanged)?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("workspace name must not be empty").unchanged());
    }
    let slug = body
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| crate::registry::slugify(name), str::to_owned);
    let status = state
        .create_agent_workspace(name, &slug, body.autostart, body.created_by)
        .await?;
    Ok(Json(json!({ "workspace": status })))
}

async fn delete(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    local: Option<Extension<auth::AuthenticatedLocalProcess>>,
    headers: HeaderMap,
    Json(body): Json<CallerBody>,
) -> Result<StatusCode, ApiError> {
    local_engine(local, &headers).map_err(ApiError::unchanged)?;
    valid_id(&id).map_err(ApiError::unchanged)?;
    valid_caller(&body.caller).map_err(ApiError::unchanged)?;
    state
        .delete_machine_made_workspace(&id, body.caller)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn restore(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    local: Option<Extension<auth::AuthenticatedLocalProcess>>,
    headers: HeaderMap,
    Json(body): Json<CallerBody>,
) -> Result<StatusCode, ApiError> {
    local_engine(local, &headers).map_err(ApiError::unchanged)?;
    valid_caller(&body.caller).map_err(ApiError::unchanged)?;
    state.restore_machine_made_trash(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn classify(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    local: Option<Extension<auth::AuthenticatedLocalProcess>>,
    headers: HeaderMap,
    Json(body): Json<ClassifyBody>,
) -> Result<StatusCode, ApiError> {
    local_engine(local, &headers).map_err(ApiError::unchanged)?;
    valid_id(&id).map_err(ApiError::unchanged)?;
    state.classify_workspace(&id, body.origin).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceSessionBody {
    caller: CreatedBy,
    device_id: String,
    ttl_secs: Option<u64>,
    #[serde(default)]
    check_only: bool,
    creator_workspace: Option<String>,
}

async fn device_session(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    local: Option<Extension<auth::AuthenticatedLocalProcess>>,
    headers: HeaderMap,
    Json(body): Json<DeviceSessionBody>,
) -> Result<Json<Value>, ApiError> {
    local_engine(local, &headers).map_err(ApiError::unchanged)?;
    valid_id(&id).map_err(ApiError::unchanged)?;
    valid_caller(&body.caller).map_err(ApiError::unchanged)?;
    let device = &body.device_id;
    if !lucidos_gateway_stamp::is_valid_session_device_id(device) {
        return Err(ApiError::bad_request("invalid device id").unchanged());
    }
    let ttl = body
        .ttl_secs
        .unwrap_or(lucidos_gateway_stamp::DEFAULT_SESSION_SECS);
    if body
        .creator_workspace
        .as_deref()
        .is_some_and(|source| source != body.caller.workspace_id)
    {
        return Err(
            ApiError::forbidden("creator restriction must match the calling workspace").unchanged(),
        );
    }
    let (mint, durable_eligible) = state
        .grant_device_session(
            &id,
            device,
            ttl,
            body.check_only,
            &body.caller.workspace_id,
            body.creator_workspace.is_some(),
        )
        .await?;
    let Some((session, expires_at)) = mint else {
        return Ok(Json(
            json!({"checked": true, "durable_eligible": durable_eligible}),
        ));
    };
    Ok(Json(
        json!({ "session": session, "expires_at": expires_at }),
    ))
}

#[cfg(test)]
#[path = "broker_tests.rs"]
mod tests;
