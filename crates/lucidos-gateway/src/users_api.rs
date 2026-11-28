use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::{self, Authorization};
use crate::error::ApiError;
use crate::gateway_event_log::{Directory, GatewayEvent, Refusal};
use crate::server::GatewayState;

/// The signal route both engine generations accept. See "User rename wire aliases"
/// in docs/temporary-measures.md; switch to the canonical route before removing aliases.
const ENGINE_USERS_CHANGED_PATH: &str = "/api/v1/operators/changed";

pub fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/users", get(list).post(add))
        .route("/users/:id", put(rename).delete(remove))
        .route("/users/:id/emails", post(add_email))
        .route("/users/:id/emails/:email", delete(remove_email))
        .route("/devices/:id/user", put(move_device))
}

/// The wire contract older pages and engines still call. Stored events keep
/// their canonical names; only these responses use the older field names.
/// See "User rename wire aliases" in docs/temporary-measures.md.
pub fn legacy_routes() -> Router<GatewayState> {
    Router::new()
        .route("/operators", get(list).post(add))
        .route("/operators/:id", put(rename).delete(remove))
        .route("/operators/:id/emails", post(add_email))
        .route("/operators/:id/emails/:email", delete(remove_email))
        .route("/devices/:id/operator", put(move_device))
        .route_layer(axum::middleware::map_response(legacy_response))
}

async fn legacy_response(response: Response) -> Response {
    if !response.status().is_success() {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(body) = axum::body::to_bytes(body, usize::MAX).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if let Some(body) = value.as_object_mut() {
        if let Some(users) = body.remove("users") {
            body.insert("operators".into(), users);
        }
        if let Some(relayed) = body
            .get_mut("relayed")
            .and_then(serde_json::Value::as_array_mut)
        {
            for row in relayed
                .iter_mut()
                .filter_map(serde_json::Value::as_object_mut)
            {
                if let Some(id) = row.remove("user_id") {
                    row.insert("operator_id".into(), id);
                }
            }
        }
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, axum::body::Body::from(value.to_string()))
}

/// Accept either wire generation, refusing contradictory identity fields.
/// See "User rename wire aliases" in docs/temporary-measures.md.
pub(crate) fn requested_user<'a>(
    user: Option<&'a str>,
    legacy: Option<&'a str>,
) -> Result<Option<&'a str>, ApiError> {
    let nonblank = |value: &'a str| (!value.trim().is_empty()).then_some(value.trim());
    let user = user.and_then(nonblank);
    let legacy = legacy.and_then(nonblank);
    if matches!((user, legacy), (Some(a), Some(b)) if a != b) {
        return Err(ApiError::bad_request(
            "the user fields name different users",
        ));
    }
    Ok(user.or(legacy))
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserRow {
    pub id: String,
    pub name: String,
    pub emails: Vec<String>,
    pub device_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayedRow {
    pub device_id: String,
    pub user_id: String,
    pub name: String,
    pub via: String,
    #[serde(default)]
    pub emails: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsersBody {
    pub users: Vec<UserRow>,
    pub relayed: Vec<RelayedRow>,
}

impl UsersBody {
    pub fn of(directory: &Directory) -> Self {
        Self {
            users: directory
                .users
                .iter()
                .map(|o| UserRow {
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
                    user_id: r.user_id.clone(),
                    name: r.name.clone(),
                    via: r.via.clone(),
                    emails: r.emails.clone(),
                })
                .collect(),
        }
    }
}

async fn list(State(state): State<GatewayState>) -> Json<UsersBody> {
    Json(UsersBody::of(&state.directory()))
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
    user_id: Option<String>,
    operator_id: Option<String>,
}

fn record_and_list(
    state: &GatewayState,
    decide: impl FnOnce(&Directory) -> Result<Vec<GatewayEvent>, Refusal>,
) -> Result<Json<UsersBody>, ApiError> {
    state
        .record(decide)
        .map_err(ApiError::internal)?
        .map_err(ApiError::bad_request)?;
    Ok(Json(UsersBody::of(&state.directory())))
}

async fn add(
    State(state): State<GatewayState>,
    Json(body): Json<NameBody>,
) -> Result<Json<UsersBody>, ApiError> {
    record_and_list(&state, |d| d.add_user(&body.name))
}

async fn rename(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<NameBody>,
) -> Result<Json<UsersBody>, ApiError> {
    record_and_list(&state, |d| d.rename_user(&id, &body.name))
}

async fn remove(
    State(state): State<GatewayState>,
    connection: lucidos_gateway_tls::Connection,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let form = auth::CookieForm::of(&headers, &connection);
    let removes_self = match state.authorize(&headers, form) {
        Authorization::Device { id: caller, .. } => {
            state.directory().user_of_device(&caller) == Some(id.as_str())
        }
        _ => false,
    };
    let body = record_and_list(&state, |d| d.remove_user(&id))?;
    let mut response = body.into_response();
    if removes_self {
        if let Ok(value) = auth::cleared_credential_cookie(state.device_cookie_name(), form).parse()
        {
            response.headers_mut().insert(header::SET_COOKIE, value);
        }
    }
    Ok(response)
}

async fn add_email(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<EmailBody>,
) -> Result<Json<UsersBody>, ApiError> {
    record_and_list(&state, |d| d.add_email(&id, &body.email))
}

async fn remove_email(
    State(state): State<GatewayState>,
    Path((id, email)): Path<(String, String)>,
) -> Result<Json<UsersBody>, ApiError> {
    record_and_list(&state, |d| d.remove_email(&id, &email))
}

async fn move_device(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    Json(body): Json<MoveBody>,
) -> Result<Json<UsersBody>, ApiError> {
    let user = requested_user(body.user_id.as_deref(), body.operator_id.as_deref())?
        .ok_or_else(|| ApiError::bad_request("a device move needs a user"))?;
    record_and_list(&state, |d| d.move_device(&id, user))
}

pub fn signal_engines(state: &GatewayState) {
    crate::engine_signal::signal_engines(state, ENGINE_USERS_CHANGED_PATH, None, "users");
}

#[cfg(test)]
#[path = "users_api_tests.rs"]
mod tests;
