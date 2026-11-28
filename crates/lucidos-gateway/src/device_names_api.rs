//! Device identity on the gateway: every device's typed name and pairing
//! label, the one source every workspace engine resolves names from (ADR 0825).

use axum::extract::{Path, State};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::AuthenticatedLocalProcess;
use crate::error::ApiError;
use crate::gateway_event_log::{DeviceIdentity, ImportedDeviceName};
use crate::server::GatewayState;

pub const DEVICE_NAMES_PATH: &str = "/device-names";

/// INVARIANT: Reachable by a paired device and through the manage hop, like the
/// rest of B's devices (ADR 0656).
pub fn routes() -> Router<GatewayState> {
    Router::new()
        .route(DEVICE_NAMES_PATH, get(list))
        .route("/devices/:id/name", put(rename))
}

/// INVARIANT: A local process only. An engine hands over the names its
/// workspace held, once, and no device or hop may bulk-write names.
pub fn import_routes() -> Router<GatewayState> {
    Router::new().route("/device-names/import", post(import))
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceNamesBody {
    pub devices: Vec<DeviceIdentity>,
}

fn body(state: &GatewayState) -> Json<DeviceNamesBody> {
    Json(DeviceNamesBody {
        devices: state.directory().device_identities(),
    })
}

async fn list(State(state): State<GatewayState>) -> Json<DeviceNamesBody> {
    body(&state)
}

#[derive(Deserialize)]
struct RenameBody {
    name: Option<String>,
}

async fn rename(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(request): Json<RenameBody>,
) -> Result<Json<DeviceNamesBody>, ApiError> {
    state
        .record(|d| d.name_device(&id, request.name.as_deref()))
        .map_err(ApiError::internal)?
        .map_err(ApiError::bad_request)?;
    Ok(body(&state))
}

#[derive(Deserialize)]
struct ImportBody {
    devices: Vec<ImportedDeviceName>,
}

async fn import(
    State(state): State<GatewayState>,
    local: Option<Extension<AuthenticatedLocalProcess>>,
    Json(request): Json<ImportBody>,
) -> Result<Json<DeviceNamesBody>, ApiError> {
    if local.is_none() {
        return Err(ApiError::forbidden(
            "a device-name import needs a local process",
        ));
    }
    let imported = state
        .record(|d| Ok(d.import_device_names(&request.devices)))
        .map_err(ApiError::internal)?
        .map_err(ApiError::bad_request)?;
    if !imported.is_empty() {
        crate::log_legacy!(
            "[Gateway] imported {} device name(s) from a workspace",
            imported.len()
        );
    }
    Ok(body(&state))
}

#[cfg(test)]
#[path = "device_names_api_tests.rs"]
mod tests;
