//! Settings, 1Password: the workspace's service account, which reads every
//! *secret reference*.
//!
//! The token is a credential of type `onepassword_service_account`. These
//! routes own it, not the generic credential routes. So a new `op` path keeps
//! the stored token, and the token never goes back out. Every route is
//! host-only (`app_reach`), and the three that write or use the token take
//! only the owner at a registered device.

use super::error::ApiError;
use super::secret_reveal::{reveal_request_allowed, RefererRule};
use super::*;
use crate::core::secret_reference::op::{list_vaults, ServiceAccount, VaultCheck, OP_TIMEOUT};
use crate::core::secret_reference::ResolveFailure;
use crate::core::{AuthType, CredentialStore};
use crate::engine::thread_events::MessageOrigin;

/// The service name a service account saved here is stored under.
const SERVICE_NAME: &str = "1password-service-account";

/// What Settings shows about the service account. Never the token.
#[derive(Serialize)]
pub(super) struct OnepasswordStatus {
    saved: bool,
    cli_path: Option<String>,
}

/// `PUT /api/v1/onepassword`. A blank `token` keeps the stored one.
#[derive(Deserialize)]
pub(super) struct SaveOnepasswordRequest {
    #[serde(default)]
    token: String,
    #[serde(default)]
    cli_path: String,
}

const SETTINGS_ONLY: &str = "The 1Password service account is managed from Settings only";

/// The gate on every route that writes or uses the token. Only the owner at
/// a registered device passes: an agent that could pick the `op` path would
/// receive the token, and one that could swap the account would choose every
/// referenced value.
async fn require_owner(state: &AppState, headers: &HeaderMap) -> Result<MessageOrigin, ApiError> {
    if !reveal_request_allowed(headers, RefererRule::Required) {
        log!("[1Password] refused a service account request from an app document");
        return Err(ApiError::new(StatusCode::FORBIDDEN, SETTINGS_ONLY));
    }
    crate::api::actor::require_owner_device_for(headers, &state.pool, None, SETTINGS_ONLY).await
}

async fn stored_account(pool: &PgPool) -> Result<Option<ServiceAccount>, ApiError> {
    CredentialStore::onepassword_service_account(pool)
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read the 1Password service account: {e}"),
            )
        })
}

/// `GET /api/v1/onepassword`: whether a service account is saved, and the
/// `op` path it names.
async fn get_onepassword(
    State(state): State<AppState>,
) -> Result<Json<OnepasswordStatus>, ApiError> {
    let account = stored_account(&state.pool).await?;
    Ok(Json(OnepasswordStatus {
        saved: account.is_some(),
        cli_path: account.and_then(|account| account.cli_path),
    }))
}

/// `PUT /api/v1/onepassword`: save the token, the `op` path, or both.
async fn save_onepassword(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SaveOnepasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let actor = require_owner(&state, &headers).await?;
    let token = match request.token.trim() {
        "" => stored_account(&state.pool)
            .await?
            .map(|account| account.token)
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "Paste the service account token first",
                )
            })?,
        pasted => pasted.to_string(),
    };
    let cli_path = Some(request.cli_path.trim().to_string()).filter(|path| !path.is_empty());
    let payload = serde_json::to_string(&ServiceAccount { token, cli_path })
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::core::credentials::check_secret_references(
        &state.pool,
        AuthType::OnepasswordServiceAccount,
        &payload,
    )
    .await
    .map_err(|reason| ApiError::new(StatusCode::BAD_REQUEST, reason))?;
    CredentialStore::upsert(
        &state.pool,
        &state.engine.event_bus,
        SERVICE_NAME,
        &[],
        AuthType::OnepasswordServiceAccount,
        &payload,
        None,
        None,
        Some(actor),
    )
    .await
    .map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save the 1Password service account: {e}"),
        )
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/onepassword`: remove every service account, which ends
/// every secret reference.
async fn delete_onepassword(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let actor = require_owner(&state, &headers).await?;
    let failed = |e: sqlx::Error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to remove the 1Password service account: {e}"),
        )
    };
    let accounts = CredentialStore::list(&state.pool)
        .await
        .map_err(failed)?
        .into_iter()
        .filter(|credential| credential.auth_type == AuthType::OnepasswordServiceAccount);
    for account in accounts {
        CredentialStore::delete(
            &state.pool,
            &state.engine.event_bus,
            account.id,
            Some(actor.clone()),
        )
        .await
        .map_err(failed)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/onepassword/test`: run `op` with the saved service account
/// and name the vaults it reaches.
async fn test_onepassword(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<VaultCheck>, ApiError> {
    require_owner(&state, &headers).await?;
    let account = stored_account(&state.pool).await?.ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "No 1Password service account is saved",
        )
    })?;
    list_vaults(&account, OP_TIMEOUT)
        .await
        .map(Json)
        .map_err(|failure| {
            let status = match failure {
                ResolveFailure::Definitive(_) => StatusCode::UNPROCESSABLE_ENTITY,
                ResolveFailure::Transport(_) => StatusCode::BAD_GATEWAY,
            };
            ApiError::new(status, failure.to_string())
        })
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/onepassword",
            get(get_onepassword)
                .put(save_onepassword)
                .delete(delete_onepassword),
        )
        .route("/onepassword/test", post(test_onepassword))
}
