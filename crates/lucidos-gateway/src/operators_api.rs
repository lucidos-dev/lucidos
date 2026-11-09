//! HTTP surface for operators, under `/~/api/v1/auth` (ADR 0623).
//!
//! Settings → People calls these from a paired device, and every engine this
//! gateway spawns reads `GET /operators` with the local token. Each change is
//! a [`crate::gateway_event_log`] event, and [`signal_engines`] tells every
//! routed engine to read again.
//!
//! Nothing here allows or refuses anything by operator. These routes sit
//! behind the same gate as the device list, and every paired device may call
//! them.

use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::{self, Authorization};
use crate::engine_hop::{self, Credential, EngineAddress};
use crate::error::ApiError;
use crate::gateway_event_log::{Directory, GatewayEvent, Refusal};
use crate::server::GatewayState;

/// The engine route that re-reads the operators.
const ENGINE_OPERATORS_CHANGED_PATH: &str = "/api/v1/operators/changed";

/// One engine hop. Nobody waits on it: a missed signal costs that engine its
/// next boot's read, never a wrong answer for long.
const ENGINE_SIGNAL_TIMEOUT: Duration = Duration::from_secs(5);

pub fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/operators", get(list).post(add))
        .route("/operators/:id", put(rename).delete(remove))
        .route("/operators/:id/emails", post(add_email))
        .route("/operators/:id/emails/:email", delete(remove_email))
        .route("/devices/:id/operator", put(move_device))
}

/// One person, with the devices they hold.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorRow {
    pub id: String,
    pub name: String,
    pub emails: Vec<String>,
    pub device_ids: Vec<String>,
}

/// A device relayed through a gateway pairing, shown as "Sam via A".
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayedRow {
    pub device_id: String,
    pub operator_id: String,
    pub name: String,
    pub via: String,
    /// The emails the other gateway holds for the person.
    #[serde(default)]
    pub emails: Vec<String>,
}

/// What `GET /operators` answers. Engines parse the same shape.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorsBody {
    pub operators: Vec<OperatorRow>,
    pub relayed: Vec<RelayedRow>,
}

impl OperatorsBody {
    pub fn of(directory: &Directory) -> Self {
        Self {
            operators: directory
                .operators
                .iter()
                .map(|o| OperatorRow {
                    id: o.id.clone(),
                    name: o.name.clone(),
                    emails: o.emails.clone(),
                    device_ids: directory
                        .devices_of(&o.id)
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                })
                .collect(),
            relayed: directory
                .relayed
                .iter()
                .map(|r| RelayedRow {
                    device_id: r.device_id.clone(),
                    operator_id: r.operator_id.clone(),
                    name: r.name.clone(),
                    via: r.via.clone(),
                    emails: r.emails.clone(),
                })
                .collect(),
        }
    }
}

async fn list(State(state): State<GatewayState>) -> Json<OperatorsBody> {
    Json(OperatorsBody::of(&state.directory()))
}

#[derive(Deserialize)]
struct NameBody {
    name: String,
}

#[derive(Deserialize)]
struct EmailBody {
    email: String,
}

#[derive(Deserialize)]
struct MoveBody {
    operator_id: String,
}

/// Record a decision and answer the operators as they now stand.
fn record_and_list(
    state: &GatewayState,
    decide: impl FnOnce(&Directory) -> Result<Vec<GatewayEvent>, Refusal>,
) -> Result<Json<OperatorsBody>, ApiError> {
    state
        .record(decide)
        .map_err(ApiError::internal)?
        .map_err(ApiError::bad_request)?;
    Ok(Json(OperatorsBody::of(&state.directory())))
}

async fn add(
    State(state): State<GatewayState>,
    Json(body): Json<NameBody>,
) -> Result<Json<OperatorsBody>, ApiError> {
    record_and_list(&state, |d| d.add_operator(&body.name))
}

async fn rename(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<NameBody>,
) -> Result<Json<OperatorsBody>, ApiError> {
    record_and_list(&state, |d| d.rename_operator(&id, &body.name))
}

/// Remove a person and revoke their devices. When the caller is one of them,
/// the same response clears its cookie, as revoking its own device does.
async fn remove(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let removes_self = match state.authorize(&headers) {
        Authorization::Device { id: caller, .. } => {
            state.directory().operator_of_device(&caller) == Some(id.as_str())
        }
        _ => false,
    };
    let body = record_and_list(&state, |d| d.remove_operator(&id))?;
    let mut response = body.into_response();
    if removes_self {
        if let Ok(value) = auth::cleared_credential_cookie(state.device_cookie_name()).parse() {
            response.headers_mut().insert(header::SET_COOKIE, value);
        }
    }
    Ok(response)
}

async fn add_email(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<EmailBody>,
) -> Result<Json<OperatorsBody>, ApiError> {
    record_and_list(&state, |d| d.add_email(&id, &body.email))
}

async fn remove_email(
    State(state): State<GatewayState>,
    Path((id, email)): Path<(String, String)>,
) -> Result<Json<OperatorsBody>, ApiError> {
    record_and_list(&state, |d| d.remove_email(&id, &email))
}

async fn move_device(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<MoveBody>,
) -> Result<Json<OperatorsBody>, ApiError> {
    record_and_list(&state, |d| d.move_device(&id, &body.operator_id))
}

/// Tell every routed engine the operators changed. Returns at once; outside a
/// runtime, which only a sync test is, it does nothing.
pub fn signal_engines(state: &GatewayState) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let engines = state.routed_engines();
    let token = state.local_token().to_string();
    handle.spawn(async move {
        futures::future::join_all(engines.iter().map(|engine| signal_engine(engine, &token))).await;
    });
}

async fn signal_engine(engine: &EngineAddress, local_token: &str) {
    let Ok(request) = Request::post(ENGINE_OPERATORS_CHANGED_PATH).body(Body::empty()) else {
        return;
    };
    let sent = engine_hop::send(engine, request, Credential::Local(local_token));
    match tokio::time::timeout(ENGINE_SIGNAL_TIMEOUT, sent).await {
        Ok(Ok(response)) if response.status().is_success() => {}
        // An engine that predates operators has no such route.
        Ok(Ok(response)) if response.status() == StatusCode::NOT_FOUND => {}
        Ok(Ok(response)) => crate::log!(
            "[Gateway] engine at {engine} refused the operators signal: HTTP {}",
            response.status().as_u16()
        ),
        Ok(Err(e)) => crate::log!("[Gateway] could not signal the engine at {engine}: {e}"),
        Err(_) => crate::log!("[Gateway] the engine at {engine} did not take the signal in time"),
    }
}

#[cfg(test)]
#[path = "operators_api_tests.rs"]
mod tests;
