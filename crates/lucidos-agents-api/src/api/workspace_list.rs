//! `GET /api/v1/workspaces`: every workspace this machine's gateway lists.
//!
//! The engine asks its gateway over loopback ([`crate::engine::gateway_control`]), so
//! the listing works for a sandboxed agent that cannot read the local token
//! (ADR 0645). A page served straight off an engine port reads it to reach a
//! peer on its own port, and `lucidos workspaces list` prints it.
//!
//! **It projects, it does not proxy.** Each row carries the slug, display
//! name, folder, engine port, health, autostart, origin and the machine-made
//! flag, and nothing else the gateway knows.
//!
//! Every failure is an error status, never an empty list: an empty list reads
//! as a machine with no workspaces, which hides a broken hop.

use axum::{http::StatusCode, routing::get, Json, Router};
use std::time::Duration;

use super::error::ApiError;
use super::AppState;
pub(super) use crate::engine::workspace_directory::{ListedWorkspace, GATEWAY_DEADLINE};

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/workspaces", get(list_workspaces))
}

async fn list_workspaces() -> Result<Json<serde_json::Value>, ApiError> {
    let Some(gateway_port) = crate::core::base_path::gateway_port() else {
        let detail = "this engine runs without a gateway, and the gateway keeps the list";
        crate::log_legacy!("[Workspaces] {}", detail);
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("Could not list workspaces: {detail}"),
        ));
    };
    let workspaces = list_from_gateway(&gateway_port, GATEWAY_DEADLINE).await?;
    Ok(Json(serde_json::json!({ "workspaces": workspaces })))
}

pub(super) async fn list_from_gateway(
    gateway_port: &str,
    deadline: Duration,
) -> Result<Vec<ListedWorkspace>, ApiError> {
    crate::engine::workspace_directory::list_from_gateway(gateway_port, deadline)
        .await
        .map_err(|e| ApiError::new(e.status, e.message))
}
