use super::*;

use crate::core::{AuthType, CredentialStore, OAuthStore, PinnedAppStore, PreferenceStore};

// ===== Credential Endpoints =====

pub(super) async fn list_credentials(
    State(state): State<AppState>,
) -> Result<Json<CredentialsListResponse>, (StatusCode, String)> {
    let credentials = CredentialStore::list(&state.pool).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list credentials: {}", e),
        )
    })?;

    Ok(Json(CredentialsListResponse { credentials }))
}

/// Create or update a credential
pub(super) async fn create_credential(
    State(state): State<AppState>,
    Json(request): Json<CreateCredentialRequest>,
) -> Json<ApiResult> {
    let auth_type = AuthType::parse(&request.auth_type);
    match CredentialStore::upsert(
        &state.pool,
        &request.service_name,
        &request.base_url,
        auth_type,
        &request.auth_value,
        request.auth_header.as_deref(),
    )
    .await
    {
        Ok(_) => {
            // If this is an email password, also update the email account
            if auth_type == AuthType::EmailPassword {
                if let Some(account_name) = request.service_name.strip_prefix("email:") {
                    use crate::core::EmailStore;
                    if let Err(e) =
                        EmailStore::update_password(&state.pool, account_name, &request.auth_value)
                            .await
                    {
                        log!(
                            "[Email] Failed to update email password for '{}': {}",
                            account_name,
                            e
                        );
                    }
                }
            }
            ApiResult::ok()
        }
        Err(e) => ApiResult::err(format!("Failed to save credential: {}", e)),
    }
}

/// Update a credential's auth value
pub(super) async fn update_credential(
    State(state): State<AppState>,
    Query(query): Query<ServiceQuery>,
    Json(request): Json<UpdateCredentialRequest>,
) -> Json<ApiResult> {
    let service = query.service;
    match CredentialStore::update_value(&state.pool, &service, &request.auth_value).await {
        Ok(true) => ApiResult::ok(),
        Ok(false) => ApiResult::err(format!("Credential '{}' not found", service)),
        Err(e) => ApiResult::err(format!("Failed to update credential: {}", e)),
    }
}

/// Get a credential's auth value (for copying client ID/secret)
pub(super) async fn get_credential_value(
    State(state): State<AppState>,
    Query(query): Query<ServiceQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let service = query.service;
    match CredentialStore::get(&state.pool, &service).await {
        Ok(Some(cred)) => Ok(Json(serde_json::json!({
            "auth_type": cred.auth_type.to_string(),
            "auth_value": cred.auth_value,
        }))),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            format!("Credential '{}' not found", service),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get credential: {}", e),
        )),
    }
}

/// Delete a credential
pub(super) async fn delete_credential(
    State(state): State<AppState>,
    Query(query): Query<ServiceQuery>,
) -> Json<ApiResult> {
    let service = query.service;
    match CredentialStore::delete(&state.pool, &service).await {
        Ok(true) => ApiResult::ok(),
        Ok(false) => ApiResult::err(format!("Credential '{}' not found", service)),
        Err(e) => ApiResult::err(format!("Failed to delete credential: {}", e)),
    }
}

// ===== OAuth Account Endpoints =====

/// List all OAuth accounts (without tokens)
pub(super) async fn list_oauth_accounts(
    State(state): State<AppState>,
) -> Result<Json<OAuthAccountsListResponse>, (StatusCode, String)> {
    let accounts = OAuthStore::list(&state.pool).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list OAuth accounts: {}", e),
        )
    })?;
    Ok(Json(OAuthAccountsListResponse { accounts }))
}

/// Delete an OAuth account
pub(super) async fn delete_oauth_account(
    State(state): State<AppState>,
    Query(query): Query<OAuthAccountQuery>,
) -> Json<ApiResult> {
    let id: Uuid = match query.id.parse() {
        Ok(id) => id,
        Err(_) => return ApiResult::err("Invalid account ID"),
    };

    match OAuthStore::delete(&state.pool, id).await {
        Ok(true) => ApiResult::ok(),
        Ok(false) => ApiResult::err("OAuth account not found"),
        Err(e) => ApiResult::err(format!("Failed to delete OAuth account: {}", e)),
    }
}

/// Start an OAuth flow: prepares the authorization URL and spawns a background
/// listener for the callback. Returns `{ auth_url }` for the frontend to open.
pub(super) async fn reauthorize_oauth(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Json<ApiResult> {
    let provider = match body["provider"].as_str() {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => return ApiResult::err("provider is required"),
    };
    let scopes = match body["scopes"].as_str() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return ApiResult::err("scopes is required"),
    };

    // Check if client credentials exist before starting the OAuth flow
    let cred_service = format!("oauth:{}", provider);
    match CredentialStore::get(&state.pool, &cred_service).await {
        Ok(None) => return ApiResult::needs_credentials(&provider),
        Err(e) => return ApiResult::err(format!("Failed to check credentials: {}", e)),
        Ok(Some(_)) => {}
    }

    match crate::core::oauth::prepare_oauth_flow(&state.pool, &provider, &scopes).await {
        Ok(prepared) => {
            let auth_url = prepared.auth_url.clone();
            // Store the receiver so /oauth/complete can await it
            state
                .pending_oauth_flows
                .lock()
                .unwrap()
                .insert(provider, prepared.result_rx);
            ApiResult::with_auth_url(auth_url)
        }
        Err(e) => ApiResult::err(format!("OAuth flow failed: {}", e)),
    }
}

/// Wait for a pending OAuth flow to complete (called after the frontend opens the auth URL).
/// Blocks until the background listener receives the callback and finishes token exchange.
pub(super) async fn complete_oauth(
    State(state): State<AppState>,
    Query(query): Query<ProviderQuery>,
) -> Json<ApiResult> {
    let provider = query.provider;

    let rx = state.pending_oauth_flows.lock().unwrap().remove(&provider);
    let rx = match rx {
        Some(rx) => rx,
        None => return ApiResult::err(format!("No pending OAuth flow for {}", provider)),
    };

    // Wait up to 120s for the background task
    match tokio::time::timeout(std::time::Duration::from_secs(120), rx).await {
        Ok(Ok(Ok(_))) => ApiResult::ok(),
        Ok(Ok(Err(e))) => ApiResult::err(format!("OAuth flow failed: {}", e)),
        Ok(Err(_)) => ApiResult::err("OAuth flow task was dropped"),
        Err(_) => ApiResult::err("OAuth flow timed out after 120 seconds"),
    }
}

// ===== Preferences Endpoints =====

/// Get all preferences (optionally merged with device-specific overrides)
pub(super) async fn get_preferences(
    State(state): State<AppState>,
    Query(query): Query<PreferencesQuery>,
) -> Result<Json<PreferencesResponse>, (StatusCode, String)> {
    let preferences = if let Some(ref device_id) = query.device_id {
        PreferenceStore::get_all_for_device(&state.pool, device_id).await
    } else {
        PreferenceStore::get_all(&state.pool).await
    }
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load preferences: {}", e),
        )
    })?;

    Ok(Json(PreferencesResponse { preferences }))
}

/// Set a preference (optionally per-device)
pub(super) async fn set_preference(
    State(state): State<AppState>,
    Query(query): Query<KeyQuery>,
    headers: HeaderMap,
    Json(request): Json<SetPreferenceRequest>,
) -> Json<ApiResult> {
    let key = query.key;
    let result = if let Some(ref device_id) = request.device_id {
        PreferenceStore::set_for_device(&state.pool, &key, &request.value, device_id).await
    } else {
        PreferenceStore::set(&state.pool, &key, &request.value).await
    };
    match result {
        Ok(()) => {
            let actor = super::actor::user_actor_resolved(&headers, &state.pool, request.device_id.as_deref(),
            )
            .await;
            state
                .engine
                .event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::System(
                        crate::engine::event_bus::SystemEvent::PreferencesChanged {
                            key: key.clone(),
                            value: Some(request.value.clone()),
                            actor,
                        },
                    ),
                    "[Settings] PreferencesChanged",
                )
                .await;
            ApiResult::ok()
        }
        Err(e) => ApiResult::err(format!("Failed to set preference: {}", e)),
    }
}

/// Delete a preference
pub(super) async fn delete_preference(
    State(state): State<AppState>,
    Query(query): Query<KeyQuery>,
    headers: HeaderMap,
) -> Json<ApiResult> {
    let key = query.key;
    match PreferenceStore::delete(&state.pool, &key).await {
        Ok(true) => {
            let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
            state
                .engine
                .event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::System(
                        crate::engine::event_bus::SystemEvent::PreferencesChanged {
                            key: key.clone(),
                            value: None,
                            actor,
                        },
                    ),
                    "[Settings] PreferencesChanged",
                )
                .await;
            ApiResult::ok()
        }
        Ok(false) => ApiResult::err(format!("Preference '{}' not found", key)),
        Err(e) => ApiResult::err(format!("Failed to delete preference: {}", e)),
    }
}

// ===== Pinned App UIs Endpoints =====

#[derive(Deserialize)]
pub(super) struct PinnedAppsQuery {
    device_id: String,
}

#[derive(Serialize)]
pub(super) struct PinnedAppsResponse {
    entries: Vec<crate::core::PinnedAppUi>,
}

#[derive(Deserialize)]
pub(super) struct PinAppRequest {
    app_id: String,
    device_id: String,
}

/// GET /api/pinned-apps?device_id=X — list pinned apps for a device
pub(super) async fn get_pinned_apps(
    State(state): State<AppState>,
    Query(query): Query<PinnedAppsQuery>,
) -> Result<Json<PinnedAppsResponse>, (StatusCode, String)> {
    let entries = PinnedAppStore::list_for_device(&state.pool, &query.device_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load pinned apps: {}", e),
            )
        })?;
    Ok(Json(PinnedAppsResponse { entries }))
}

/// POST /api/pinned-apps — pin an app for a device
pub(super) async fn pin_app(
    State(state): State<AppState>,
    Json(request): Json<PinAppRequest>,
) -> Json<ApiResult> {
    match PinnedAppStore::pin(&state.pool, &request.app_id, "main", &request.device_id).await {
        Ok(()) => ApiResult::ok(),
        Err(e) => ApiResult::err(format!("Failed to pin app: {}", e)),
    }
}

/// DELETE /api/pinned-apps — unpin an app for a device
pub(super) async fn unpin_app(
    State(state): State<AppState>,
    Json(request): Json<PinAppRequest>,
) -> Json<ApiResult> {
    match PinnedAppStore::unpin(&state.pool, &request.app_id, "main", &request.device_id).await {
        Ok(_) => ApiResult::ok(),
        Err(e) => ApiResult::err(format!("Failed to unpin app: {}", e)),
    }
}

// ===== Device Management Endpoints =====

pub(super) async fn register_device(
    State(state): State<AppState>,
    Json(request): Json<DeviceRegisterRequest>,
) -> Json<serde_json::Value> {
    match crate::core::DeviceStore::register(
        &state.pool,
        &request.device_id,
        request.user_agent.as_deref(),
    )
    .await
    {
        Ok(device) => Json(serde_json::json!({ "success": true, "device": device })),
        Err(e) => Json(serde_json::json!({ "success": false, "error": e.to_string() })),
    }
}

pub(super) async fn list_devices(
    State(state): State<AppState>,
) -> Result<Json<DevicesListResponse>, (StatusCode, Json<serde_json::Value>)> {
    match crate::core::DeviceStore::list(&state.pool).await {
        Ok(devices) => Ok(Json(DevicesListResponse { devices })),
        Err(e) => {
            log!("Failed to list devices: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            ))
        }
    }
}

pub(super) async fn rename_device(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Json(request): Json<DeviceRenameRequest>,
) -> Json<serde_json::Value> {
    match crate::core::DeviceStore::rename(&state.pool, &device_id, request.name.as_deref()).await {
        Ok(true) => Json(serde_json::json!({ "success": true })),
        Ok(false) => Json(serde_json::json!({ "success": false, "error": "Device not found" })),
        Err(e) => Json(serde_json::json!({ "success": false, "error": e.to_string() })),
    }
}

pub(super) async fn set_device_push(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Json(request): Json<DevicePushRequest>,
) -> Json<serde_json::Value> {
    match crate::core::DeviceStore::set_push_enabled(&state.pool, &device_id, request.push_enabled)
        .await
    {
        Ok(true) => Json(serde_json::json!({ "success": true })),
        Ok(false) => Json(serde_json::json!({ "success": false, "error": "Device not found" })),
        Err(e) => Json(serde_json::json!({ "success": false, "error": e.to_string() })),
    }
}

pub(super) async fn delete_device(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    match crate::core::DeviceStore::delete(&state.pool, &device_id).await {
        Ok(true) => Ok(Json(serde_json::json!({ "success": true }))),
        Ok(false) => Err((StatusCode::NOT_FOUND, "Device not found".to_string())),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to delete device: {}", e),
        )),
    }
}

// ===== Email Endpoints =====

pub(super) async fn send_email_confirmed(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    use crate::core::email::{EmailClient, EmailStore};

    let to: Vec<String> = body["to"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let subject = body["subject"].as_str().unwrap_or("");
    let body_text = body["body"].as_str().unwrap_or("");
    let cc: Vec<String> = body
        .get("cc")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let bcc: Vec<String> = body
        .get("bcc")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let reply_to = body.get("reply_to_message_id").and_then(|v| v.as_str());
    let account_name = body["account"].as_str().unwrap_or("");

    let account = match EmailStore::get(&state.pool, account_name).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return Json(serde_json::json!({ "success": false, "error": "Account not found" }))
        }
        Err(e) => return Json(serde_json::json!({ "success": false, "error": format!("{}", e) })),
    };

    // Resolve OAuth token if linked
    let oauth_token = if let Some(oauth_id) = account.oauth_account_id {
        match OAuthStore::get_by_id(&state.pool, oauth_id).await {
            Ok(Some(mut oauth_account)) => {
                match crate::core::oauth::refresh_oauth_if_needed(&state.pool, &mut oauth_account)
                    .await
                {
                    Ok(()) => {}
                    Err(e) => log!(
                        "[Email] OAuth token refresh failed for {}: {}",
                        oauth_account.provider,
                        e
                    ),
                }
                Some(oauth_account.access_token)
            }
            _ => None,
        }
    } else {
        None
    };

    let to_str = to.join(", ");
    let cc_str = if cc.is_empty() {
        None
    } else {
        Some(cc.join(", "))
    };
    let bcc_str = if bcc.is_empty() {
        None
    } else {
        Some(bcc.join(", "))
    };

    // Read attachments from workspace
    let attachment_paths: Vec<String> = body
        .get("attachments")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let attachments = match crate::core::email::EmailAttachment::read_from_workspace(
        &state.workspace_path,
        &attachment_paths,
    ) {
        Ok(a) => a,
        Err(e) => return Json(serde_json::json!({ "success": false, "error": e })),
    };

    match EmailClient::send_email(
        &account,
        &to_str,
        subject,
        body_text,
        cc_str.as_deref(),
        bcc_str.as_deref(),
        reply_to,
        oauth_token.as_deref(),
        &attachments,
    )
    .await
    {
        Ok(_) => Json(serde_json::json!({ "success": true })),
        Err(e) => {
            log!("[Email] Failed to send email to {}: {}", to.join(", "), e);
            Json(serde_json::json!({ "success": false, "error": format!("{}", e) }))
        }
    }
}
