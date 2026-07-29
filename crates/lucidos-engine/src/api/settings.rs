use super::*;

use crate::core::{
    AuthType, CredentialStore, EnvironmentVariable, EnvironmentVariableStore, ModelStore,
    OAuthStore, PinnedAppStore, PreferenceStore,
};
use crate::core::environment_variables::validate_name;
use crate::engine::claude_code::{read_allowed_tools_file, write_allowed_tools_file};
use crate::engine::command_permission::{
    read_agent_allowed_commands_file, write_agent_allowed_commands_file,
};
use crate::engine::event_bus::{BusEvent, SystemEvent};

/// Response/request body for both allowlist editors (`cc-allowed-tools` and
/// `agent-allowed-commands`) — the raw file text, one pattern per line. The
/// settings UI parses it into editable rows and reserializes on save.
#[derive(Serialize)]
pub(super) struct AllowlistResponse {
    contents: String,
}

#[derive(Deserialize)]
pub(super) struct AllowlistRequest {
    contents: String,
}

/// GET /api/v1/cc-allowed-tools — return the raw contents of
/// `~/.lucidos/cc-allowed-tools` so the settings UI can display them. Missing
/// file returns the seeded header (mirrors `cc_allowed_tools` semantics).
pub(super) async fn get_cc_allowed_tools(
    State(state): State<AppState>,
) -> Result<Json<AllowlistResponse>, (StatusCode, String)> {
    let dir = state.engine.user_dir().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "User directory not configured".to_string(),
    ))?;
    let contents = read_allowed_tools_file(dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read cc-allowed-tools: {}", e),
        )
    })?;
    Ok(Json(AllowlistResponse { contents }))
}

/// PUT /api/v1/cc-allowed-tools — overwrite the file with the provided contents
/// (atomic). Newly spawned Claude Code subprocesses pick this up immediately; in-flight
/// subprocesses keep their frozen `--allowedTools` flag until they restart.
pub(super) async fn put_cc_allowed_tools(
    State(state): State<AppState>,
    Json(body): Json<AllowlistRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let dir = state.engine.user_dir().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "User directory not configured".to_string(),
    ))?;
    write_allowed_tools_file(dir, &body.contents).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to write cc-allowed-tools: {}", e),
        )
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/agent-allowed-commands — return the raw contents of
/// `~/.lucidos/agent-allowed-commands`, the Lucidos Agent command-guard
/// allowlist (ADR 0002). Missing file returns the seeded header. The chat
/// counterpart of `get_cc_allowed_tools`.
pub(super) async fn get_agent_allowed_commands(
    State(state): State<AppState>,
) -> Result<Json<AllowlistResponse>, (StatusCode, String)> {
    let dir = state.engine.user_dir().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "User directory not configured".to_string(),
    ))?;
    let contents = read_agent_allowed_commands_file(dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read agent-allowed-commands: {}", e),
        )
    })?;
    Ok(Json(AllowlistResponse { contents }))
}

/// PUT /api/v1/agent-allowed-commands — overwrite the file (atomic). The command
/// guard reads it fresh on each prompt, so an edit takes effect on the next
/// gated command — no restart.
pub(super) async fn put_agent_allowed_commands(
    State(state): State<AppState>,
    Json(body): Json<AllowlistRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let dir = state.engine.user_dir().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "User directory not configured".to_string(),
    ))?;
    write_agent_allowed_commands_file(dir, &body.contents).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to write agent-allowed-commands: {}", e),
        )
    })?;
    Ok(StatusCode::NO_CONTENT)
}

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
    headers: HeaderMap,
    Json(request): Json<CreateCredentialRequest>,
) -> Json<ApiResult> {
    let auth_type = AuthType::parse(&request.auth_type);
    // Optional custom env var name — validated like a user env var (valid shape +
    // not engine-reserved). Empty/whitespace → None (default CRED_<NAME>).
    let env_var_name = request
        .env_var_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(name) = env_var_name {
        if let Err(rejection) = validate_name(name) {
            return ApiResult::err(rejection.message(name));
        }
    }
    match CredentialStore::upsert(
        &state.pool,
        &request.service_name,
        &request.base_url,
        auth_type,
        &request.auth_value,
        request.auth_header.as_deref(),
        env_var_name,
    )
    .await
    {
        Ok(_) => {
            // If this is an email password, also sync it into the email account
            // row. IMAP/SMTP read the password from `email_accounts`, not the
            // credentials table, so a failed sync must surface as an error (the
            // edit path in `update_credential` does the same) rather than report
            // a false success while email silently breaks.
            if auth_type == AuthType::EmailPassword {
                if let Some(account_name) = request.service_name.strip_prefix("email:") {
                    use crate::core::EmailStore;
                    if let Err(e) =
                        EmailStore::update_password(&state.pool, account_name, &request.auth_value)
                            .await
                    {
                        return ApiResult::err(format!("Failed to update email password: {}", e));
                    }
                }
            }
            state
                .engine
                .event_bus
                .emit_user_system(&headers, &state.pool, "[Settings] CredentialCreated", |actor| {
                    SystemEvent::CredentialCreated {
                        service_name: request.service_name.clone(),
                        auth_type,
                        actor,
                    }
                })
                .await;
            ApiResult::ok()
        }
        Err(e) => ApiResult::err(format!("Failed to save credential: {}", e)),
    }
}

/// Update an existing credential's editable fields. For `email_password`
/// credentials the `email_accounts` row (server settings + password) is kept in
/// sync, because IMAP/SMTP read from `email_accounts`, not the credentials
/// table — the create path does the same, and the edit path must match it.
pub(super) async fn update_credential(
    State(state): State<AppState>,
    Query(query): Query<ServiceQuery>,
    headers: HeaderMap,
    Json(request): Json<UpdateCredentialRequest>,
) -> Json<ApiResult> {
    use crate::core::EmailStore;

    let service = query.service;
    let auth_type = AuthType::parse(&request.auth_type);

    // Empty string means "keep the current secret" — same contract the
    // frontend relies on when the user edits only non-secret fields.
    let new_secret = request.auth_value.as_deref().filter(|s| !s.is_empty());

    // For email credentials the canonical base_url mirrors the SMTP host
    // (matches `configure_email`'s `smtp://<host>`), derived from the edited
    // server settings when present.
    let base_url = match (auth_type, &request.email) {
        (AuthType::EmailPassword, Some(email)) => format!("smtp://{}", email.smtp_host),
        _ => request.base_url.clone(),
    };

    // Optional custom env var name — validated; empty/whitespace clears it back
    // to the default CRED_<NAME>.
    let env_var_name = request
        .env_var_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(name) = env_var_name {
        if let Err(rejection) = validate_name(name) {
            return ApiResult::err(rejection.message(name));
        }
    }

    match CredentialStore::update(
        &state.pool,
        &service,
        &base_url,
        auth_type,
        request.auth_header.as_deref(),
        new_secret,
        env_var_name,
    )
    .await
    {
        Ok(true) => {
            if auth_type == AuthType::EmailPassword {
                if let Some(account_name) = service.strip_prefix("email:") {
                    if let Some(email) = &request.email {
                        if let Err(e) = EmailStore::upsert(
                            &state.pool,
                            account_name,
                            &email.email_address,
                            &email.imap_host,
                            email.imap_port,
                            &email.smtp_host,
                            email.smtp_port,
                            &email.username,
                            email.use_tls,
                            email.require_send_confirmation,
                        )
                        .await
                        {
                            return ApiResult::err(format!("Failed to update email account: {}", e));
                        }
                    }
                    if let Some(value) = new_secret {
                        if let Err(e) =
                            EmailStore::update_password(&state.pool, account_name, value).await
                        {
                            return ApiResult::err(format!(
                                "Failed to update email password: {}",
                                e
                            ));
                        }
                    }
                }
            }
            state
                .engine
                .event_bus
                .emit_user_system(&headers, &state.pool, "[Settings] CredentialUpdated", |actor| {
                    SystemEvent::CredentialUpdated {
                        service_name: service.clone(),
                        actor,
                    }
                })
                .await;
            ApiResult::ok()
        }
        Ok(false) => ApiResult::err(format!("Credential '{}' not found", service)),
        Err(e) => ApiResult::err(format!("Failed to update credential: {}", e)),
    }
}

/// Get an email account's server settings (no password) so the settings UI can
/// pre-fill the edit form for an `email_password` credential.
pub(super) async fn get_email_account(
    State(state): State<AppState>,
    Query(query): Query<NameQuery>,
) -> Result<Json<crate::core::EmailAccountInfo>, (StatusCode, String)> {
    match crate::core::EmailStore::get_info(&state.pool, &query.name).await {
        Ok(Some(info)) => Ok(Json(info)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            format!("Email account '{}' not found", query.name),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get email account: {}", e),
        )),
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
    headers: HeaderMap,
) -> Json<ApiResult> {
    let service = query.service;
    match CredentialStore::delete(&state.pool, &service).await {
        Ok(true) => {
            state
                .engine
                .event_bus
                .emit_user_system(&headers, &state.pool, "[Settings] CredentialDeleted", |actor| {
                    SystemEvent::CredentialDeleted {
                        service_name: service.clone(),
                        actor,
                    }
                })
                .await;
            ApiResult::ok()
        }
        Ok(false) => ApiResult::err(format!("Credential '{}' not found", service)),
        Err(e) => ApiResult::err(format!("Failed to delete credential: {}", e)),
    }
}

// ===== Environment Variable Endpoints =====
//
// User-managed non-secret env vars (Settings → System → Environment variables).
// Mirrors the credential CRUD shape, but the value IS broadcast in the
// `EnvironmentVariableSet` event since these are deliberately not secret.

#[derive(Serialize)]
pub(super) struct EnvVarsListResponse {
    env_vars: Vec<EnvironmentVariable>,
}

#[derive(Deserialize)]
pub(super) struct CreateEnvVarRequest {
    name: String,
    value: String,
}

#[derive(Deserialize)]
pub(super) struct UpdateEnvVarRequest {
    value: String,
}

/// GET /api/v1/env-vars — list all user environment variables (name + value).
pub(super) async fn list_env_vars(
    State(state): State<AppState>,
) -> Result<Json<EnvVarsListResponse>, (StatusCode, String)> {
    let env_vars = EnvironmentVariableStore::list(&state.pool).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list environment variables: {}", e),
        )
    })?;
    Ok(Json(EnvVarsListResponse { env_vars }))
}

/// POST /api/v1/env-vars — create (or replace) a variable. Validates the name
/// shape and rejects engine-reserved names with 400.
pub(super) async fn create_env_var(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateEnvVarRequest>,
) -> Result<Json<ApiResult>, (StatusCode, String)> {
    if let Err(rejection) = validate_name(&request.name) {
        return Err((StatusCode::BAD_REQUEST, rejection.message(&request.name)));
    }
    if let Err(e) =
        EnvironmentVariableStore::upsert(&state.pool, &request.name, &request.value).await
    {
        return Ok(ApiResult::err(format!(
            "Failed to save environment variable: {}",
            e
        )));
    }
    state
        .engine
        .event_bus
        .emit_user_system(
            &headers,
            &state.pool,
            "[Settings] EnvironmentVariableSet",
            |actor| SystemEvent::EnvironmentVariableSet {
                name: request.name.clone(),
                value: request.value.clone(),
                actor,
            },
        )
        .await;
    Ok(ApiResult::ok())
}

/// PUT /api/v1/env-vars?name=NAME — update an existing variable's value.
pub(super) async fn update_env_var(
    State(state): State<AppState>,
    Query(query): Query<NameQuery>,
    headers: HeaderMap,
    Json(request): Json<UpdateEnvVarRequest>,
) -> Result<Json<ApiResult>, (StatusCode, String)> {
    // A stored name is already valid, but re-checking keeps the contract honest
    // if an out-of-band row ever sneaks in.
    if let Err(rejection) = validate_name(&query.name) {
        return Err((StatusCode::BAD_REQUEST, rejection.message(&query.name)));
    }
    match EnvironmentVariableStore::update(&state.pool, &query.name, &request.value).await {
        Ok(true) => {
            state
                .engine
                .event_bus
                .emit_user_system(
                    &headers,
                    &state.pool,
                    "[Settings] EnvironmentVariableSet",
                    |actor| SystemEvent::EnvironmentVariableSet {
                        name: query.name.clone(),
                        value: request.value.clone(),
                        actor,
                    },
                )
                .await;
            Ok(ApiResult::ok())
        }
        Ok(false) => Ok(ApiResult::err(format!(
            "Environment variable '{}' not found",
            query.name
        ))),
        Err(e) => Ok(ApiResult::err(format!(
            "Failed to update environment variable: {}",
            e
        ))),
    }
}

/// DELETE /api/v1/env-vars?name=NAME — remove a variable.
pub(super) async fn delete_env_var(
    State(state): State<AppState>,
    Query(query): Query<NameQuery>,
    headers: HeaderMap,
) -> Json<ApiResult> {
    match EnvironmentVariableStore::delete(&state.pool, &query.name).await {
        Ok(true) => {
            state
                .engine
                .event_bus
                .emit_user_system(
                    &headers,
                    &state.pool,
                    "[Settings] EnvironmentVariableDeleted",
                    |actor| SystemEvent::EnvironmentVariableDeleted {
                        name: query.name.clone(),
                        actor,
                    },
                )
                .await;
            ApiResult::ok()
        }
        Ok(false) => ApiResult::err(format!("Environment variable '{}' not found", query.name)),
        Err(e) => ApiResult::err(format!("Failed to delete environment variable: {}", e)),
    }
}

// ===== Model Registry Endpoints =====

/// Provider values the registry accepts. Kept in lockstep with
/// `crate::llm::model_registry::ProviderKind`.
fn valid_provider(p: &str) -> bool {
    matches!(
        p,
        "vertex" | "anthropic" | "openai" | "openrouter" | "local"
    )
}

const PROVIDER_ERR: &str =
    "Provider must be one of: vertex, anthropic, openai, openrouter, local";

const CONTEXT_WINDOW_ERR: &str =
    "context_window must be a positive number of tokens (omit it to infer from the model id)";

/// A declared context window must be positive — a zero or negative value would
/// produce a zero (or, once cast, an enormous) trim budget. Absent is fine: the
/// engine falls back to the id-shape guess.
fn valid_context_window(w: Option<i32>) -> bool {
    w.is_none_or(|w| w > 0)
}

/// GET /api/v1/models — the full registry (enabled + disabled). The chat picker
/// filters to `enabled`; the Settings → Models manager shows all.
pub(super) async fn list_models(
    State(state): State<AppState>,
) -> Result<Json<ModelsListResponse>, (StatusCode, String)> {
    let models = ModelStore::list(&state.pool).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list models: {}", e),
        )
    })?;
    Ok(Json(ModelsListResponse { models }))
}

/// POST /api/v1/models — add a user model.
pub(super) async fn create_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateModelRequest>,
) -> Json<ApiResult> {
    let id = request.id.trim();
    if id.is_empty() {
        return ApiResult::err("Model id cannot be empty");
    }
    // Label is optional — an absent/empty label defaults to the id (mirrors the
    // `manage_models` LLM handler and the `lucidos models add` CLI, whose --label
    // is optional). The Settings UI always supplies one.
    let label = match request.label.trim() {
        l if !l.is_empty() => l,
        _ => id,
    };
    if !valid_provider(&request.provider) {
        return ApiResult::err(PROVIDER_ERR);
    }
    if !valid_context_window(request.context_window) {
        return ApiResult::err(CONTEXT_WINDOW_ERR);
    }
    // User models sort after the builtins by default.
    let sort_order = request.sort_order.unwrap_or(1000);
    match ModelStore::create(
        &state.pool,
        id,
        label,
        &request.provider,
        sort_order,
        request.context_window,
    )
    .await
    {
        Ok(model) => {
            state
                .engine
                .event_bus
                .emit_user_system(&headers, &state.pool, "[Settings] ModelCreated", |actor| {
                    SystemEvent::ModelCreated {
                        id: model.id.clone(),
                        label: model.label.clone(),
                        provider: model.provider.clone(),
                        actor,
                    }
                })
                .await;
            ApiResult::ok()
        }
        Err(e) => ApiResult::err(format!("Failed to create model (id may already exist): {}", e)),
    }
}

/// PUT /api/v1/models?id= — edit a model. Builtins keep their identity (id,
/// label, provider, sort_order) but accept `enabled` and `context_window`.
/// User models update any provided field.
pub(super) async fn update_model(
    State(state): State<AppState>,
    Query(query): Query<ModelIdQuery>,
    headers: HeaderMap,
    Json(request): Json<UpdateModelRequest>,
) -> Json<ApiResult> {
    let existing = match ModelStore::get(&state.pool, &query.id).await {
        Ok(Some(m)) => m,
        Ok(None) => return ApiResult::err(format!("Model '{}' not found", query.id)),
        Err(e) => return ApiResult::err(format!("Failed to load model: {}", e)),
    };

    let result = if existing.is_builtin() {
        // Builtins keep their IDENTITY — label / provider / sort_order are
        // engine-owned. `context_window` is not identity: it's a factual
        // property of the model that the vendor can raise, and whose seeded
        // value can simply be wrong. Refusing it would strand a builtin on a
        // bad window with no way to correct it (and would silently no-op the
        // documented `lucidos models update --id z-ai/glm-5.2 --context-window`).
        let enabled = request.enabled.unwrap_or(existing.enabled);
        let context_window = request.context_window.unwrap_or(existing.context_window);
        if !valid_context_window(context_window) {
            return ApiResult::err(CONTEXT_WINDOW_ERR);
        }
        ModelStore::update(
            &state.pool,
            &existing.id,
            &existing.label,
            &existing.provider,
            existing.sort_order,
            enabled,
            context_window,
        )
        .await
    } else {
        let label = request.label.unwrap_or_else(|| existing.label.clone());
        let provider = request.provider.unwrap_or_else(|| existing.provider.clone());
        if !valid_provider(&provider) {
            return ApiResult::err(PROVIDER_ERR);
        }
        let sort_order = request.sort_order.unwrap_or(existing.sort_order);
        let enabled = request.enabled.unwrap_or(existing.enabled);
        // Absent keeps the stored window; an explicit `null` clears it back to
        // the id-shape fallback (see `UpdateModelRequest::context_window`).
        let context_window = request.context_window.unwrap_or(existing.context_window);
        if !valid_context_window(context_window) {
            return ApiResult::err(CONTEXT_WINDOW_ERR);
        }
        ModelStore::update(
            &state.pool,
            &existing.id,
            &label,
            &provider,
            sort_order,
            enabled,
            context_window,
        )
        .await
    };

    match result {
        Ok(_) => {
            state
                .engine
                .event_bus
                .emit_user_system(&headers, &state.pool, "[Settings] ModelUpdated", |actor| {
                    SystemEvent::ModelUpdated {
                        id: existing.id.clone(),
                        actor,
                    }
                })
                .await;
            ApiResult::ok()
        }
        Err(e) => ApiResult::err(format!("Failed to update model: {}", e)),
    }
}

/// DELETE /api/v1/models?id= — delete a user model. Builtins are not deletable
/// (disable instead), since deleting one could orphan a saved `chat_model` pref.
pub(super) async fn delete_model(
    State(state): State<AppState>,
    Query(query): Query<ModelIdQuery>,
    headers: HeaderMap,
) -> Json<ApiResult> {
    let existing = match ModelStore::get(&state.pool, &query.id).await {
        Ok(Some(m)) => m,
        Ok(None) => return ApiResult::err(format!("Model '{}' not found", query.id)),
        Err(e) => return ApiResult::err(format!("Failed to load model: {}", e)),
    };
    if existing.is_builtin() {
        return ApiResult::err("Builtin models cannot be deleted — disable it instead");
    }
    match ModelStore::delete(&state.pool, &existing.id).await {
        Ok(_) => {
            state
                .engine
                .event_bus
                .emit_user_system(&headers, &state.pool, "[Settings] ModelDeleted", |actor| {
                    SystemEvent::ModelDeleted {
                        id: existing.id.clone(),
                        actor,
                    }
                })
                .await;
            ApiResult::ok()
        }
        Err(e) => ApiResult::err(format!("Failed to delete model: {}", e)),
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
    headers: HeaderMap,
) -> Json<ApiResult> {
    let id: Uuid = match query.id.parse() {
        Ok(id) => id,
        Err(_) => return ApiResult::err("Invalid account ID"),
    };

    match OAuthStore::delete(&state.pool, id).await {
        Ok(true) => {
            state
                .engine
                .event_bus
                .emit_user_system(&headers, &state.pool, "[Settings] OAuthAccountDeleted", |actor| {
                    SystemEvent::OAuthAccountDeleted {
                        account_id: id.to_string(),
                        actor,
                    }
                })
                .await;
            ApiResult::ok()
        }
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
    // Route through the single write chokepoint (engine/preferences.rs) so the
    // Settings UI gets the same side-effects the set_preference tool does:
    // language/timezone refresh the engine's in-memory locale + emit
    // LanguageSet/TimezoneSet, push syncs devices.push_enabled, everything else
    // emits PreferencesChanged. The HTTP path is intentionally permissive about
    // the key (the human edits internal keys here) — the catalog gate lives in
    // the tool handler only.
    let actor =
        super::actor::user_actor_resolved(&headers, &state.pool, request.device_id.as_deref())
            .await;
    match state
        .engine
        .apply_preference_write(
            &query.key,
            &request.value,
            request.device_id.as_deref(),
            actor,
        )
        .await
    {
        Ok(_) => ApiResult::ok(),
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
            state
                .engine
                .event_bus
                .emit_user_system(&headers, &state.pool, "[Settings] PreferencesChanged", |actor| {
                    crate::engine::event_bus::SystemEvent::PreferencesChanged {
                        key: key.clone(),
                        value: None,
                        actor,
                    }
                })
                .await;
            ApiResult::ok()
        }
        Ok(false) => ApiResult::err(format!("Preference '{}' not found", key)),
        Err(e) => ApiResult::err(format!("Failed to delete preference: {}", e)),
    }
}

// ===== Network access (per-workspace engine bind) =====

/// GET response for the per-workspace Network access pane. `engine_bind` is this
/// workspace's own `network_bind` preference; `inherit` + `gateway_bind` are read
/// from the machine-global `~/.lucidos/network.toml` so the pane can grey out the
/// engine field (showing the inherited value) when engines inherit the gateway
/// bind. `detected_tailscale_ip` is a best-effort hint for the IP field.
#[derive(Serialize)]
pub(super) struct NetworkConfigResponse {
    engine_bind: String,
    inherit: bool,
    gateway_bind: String,
    detected_tailscale_ip: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct SetNetworkConfigRequest {
    engine_bind: String,
}

/// GET /api/v1/network-config — the per-workspace engine bind + the inherited
/// machine-global gateway bind, for Settings → System → Network access.
pub(super) async fn get_network_config(
    State(state): State<AppState>,
) -> Result<Json<NetworkConfigResponse>, (StatusCode, String)> {
    let engine_bind = PreferenceStore::get(&state.pool, crate::net_config::NETWORK_BIND_PREF_KEY)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read network bind preference: {}", e),
            )
        })?
        .unwrap_or_else(|| "loopback".to_string());
    let net = crate::net_config::read_network_toml();
    Ok(Json(NetworkConfigResponse {
        engine_bind,
        inherit: net.engine_inherit,
        gateway_bind: net.gateway_bind.unwrap_or_else(|| "loopback".to_string()),
        detected_tailscale_ip: crate::net_config::detect_tailscale_ipv4().await,
    }))
}

/// PUT /api/v1/network-config — set this workspace's engine bind. Validated
/// server-side (loopback / all / a parseable IP); takes effect on the next
/// engine restart (a live socket cannot be re-bound).
pub(super) async fn put_network_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SetNetworkConfigRequest>,
) -> Result<Json<ApiResult>, (StatusCode, String)> {
    if let Err(msg) = crate::net_config::validate_bind_input(&request.engine_bind) {
        return Err((StatusCode::BAD_REQUEST, msg));
    }
    // Normalize keyword case so the stored value is canonical (the resolver is
    // case-insensitive, but a clean value reads better in the timeline / file).
    let value = match request.engine_bind.trim().to_ascii_lowercase().as_str() {
        "loopback" => "loopback".to_string(),
        "all" => "all".to_string(),
        _ => request.engine_bind.trim().to_string(),
    };
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    // Route through the single preference write chokepoint so it emits
    // PreferencesChanged like every other settings write.
    match state
        .engine
        .apply_preference_write(crate::net_config::NETWORK_BIND_PREF_KEY, &value, None, actor)
        .await
    {
        Ok(_) => Ok(ApiResult::ok()),
        Err(e) => Ok(ApiResult::err(format!("Failed to set network bind: {}", e))),
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

/// GET /api/v1/pinned-apps?device_id=X — list pinned apps for a device
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

/// POST /api/v1/pinned-apps — pin an app for a device. Idempotent in the DB;
/// only emits `PinnedAppPinned` when the row was actually inserted (so
/// re-clicks of an already-pinned tile don't append events).
pub(super) async fn pin_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PinAppRequest>,
) -> Json<ApiResult> {
    match PinnedAppStore::pin(&state.pool, &request.app_id, "main", &request.device_id).await {
        Ok(true) => {
            let actor =
                super::actor::user_actor_resolved(&headers, &state.pool, Some(&request.device_id))
                    .await;
            state
                .engine
                .event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::PinnedAppPinned {
                        app_id: request.app_id.clone(),
                        device_id: request.device_id.clone(),
                        actor,
                    }),
                    "[Settings] PinnedAppPinned",
                )
                .await;
            ApiResult::ok()
        }
        Ok(false) => ApiResult::ok(),
        Err(e) => ApiResult::err(format!("Failed to pin app: {}", e)),
    }
}

/// DELETE /api/v1/pinned-apps — unpin an app for a device. Idempotent in the
/// DB; only emits `PinnedAppUnpinned` when a row was actually removed.
pub(super) async fn unpin_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PinAppRequest>,
) -> Json<ApiResult> {
    match PinnedAppStore::unpin(&state.pool, &request.app_id, "main", &request.device_id).await {
        Ok(true) => {
            let actor =
                super::actor::user_actor_resolved(&headers, &state.pool, Some(&request.device_id))
                    .await;
            state
                .engine
                .event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::PinnedAppUnpinned {
                        app_id: request.app_id.clone(),
                        device_id: request.device_id.clone(),
                        actor,
                    }),
                    "[Settings] PinnedAppUnpinned",
                )
                .await;
            ApiResult::ok()
        }
        Ok(false) => ApiResult::ok(),
        Err(e) => ApiResult::err(format!("Failed to unpin app: {}", e)),
    }
}

// ===== Device Management Endpoints =====

pub(super) async fn register_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DeviceRegisterRequest>,
) -> Json<serde_json::Value> {
    match crate::core::DeviceStore::register(
        &state.pool,
        &request.device_id,
        request.user_agent.as_deref(),
    )
    .await
    {
        Ok((device, inserted)) => {
            // Frontend calls this endpoint on every page load to refresh
            // `last_seen_at`, so the upsert path is the steady state — only
            // emit `DeviceRegistered` for genuine first-touch inserts.
            // Otherwise the events table would grow by one row per refresh.
            if inserted {
                let actor = super::actor::user_actor_resolved(
                    &headers,
                    &state.pool,
                    Some(&request.device_id),
                )
                .await;
                state
                    .engine
                    .event_bus
                    .emit_or_log(
                        BusEvent::System(SystemEvent::DeviceRegistered {
                            device_id: request.device_id.clone(),
                            user_agent: request.user_agent.clone(),
                            actor,
                        }),
                        "[Settings] DeviceRegistered",
                    )
                    .await;
            }
            Json(serde_json::json!({ "success": true, "device": device }))
        }
        Err(e) => Json(serde_json::json!({ "success": false, "error": e.to_string() })),
    }
}

pub(super) async fn list_devices(
    State(state): State<AppState>,
) -> Result<Json<DevicesListResponse>, (StatusCode, Json<serde_json::Value>)> {
    match crate::core::DeviceStore::list(&state.pool).await {
        Ok(devices) => Ok(Json(DevicesListResponse { devices })),
        Err(e) => {
            log!("[Settings] Failed to list devices: {}", e);
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
    headers: HeaderMap,
    Json(request): Json<DeviceRenameRequest>,
) -> Json<serde_json::Value> {
    match crate::core::DeviceStore::rename(&state.pool, &device_id, request.name.as_deref()).await {
        Ok(true) => {
            state
                .engine
                .event_bus
                .emit_user_system(&headers, &state.pool, "[Settings] DeviceRenamed", |actor| {
                    SystemEvent::DeviceRenamed {
                        device_id: device_id.clone(),
                        name: request.name.clone(),
                        actor,
                    }
                })
                .await;
            Json(serde_json::json!({ "success": true }))
        }
        Ok(false) => Json(serde_json::json!({ "success": false, "error": "Device not found" })),
        Err(e) => Json(serde_json::json!({ "success": false, "error": e.to_string() })),
    }
}

pub(super) async fn set_device_push(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DevicePushRequest>,
) -> Json<serde_json::Value> {
    match crate::core::DeviceStore::set_push_enabled(&state.pool, &device_id, request.push_enabled)
        .await
    {
        Ok(true) => {
            state
                .engine
                .event_bus
                .emit_user_system(&headers, &state.pool, "[Settings] DevicePushChanged", |actor| {
                    SystemEvent::DevicePushChanged {
                        device_id: device_id.clone(),
                        push_enabled: request.push_enabled,
                        actor,
                    }
                })
                .await;
            Json(serde_json::json!({ "success": true }))
        }
        Ok(false) => Json(serde_json::json!({ "success": false, "error": "Device not found" })),
        Err(e) => Json(serde_json::json!({ "success": false, "error": e.to_string() })),
    }
}

pub(super) async fn delete_device(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    match crate::core::DeviceStore::delete(&state.pool, &device_id).await {
        Ok(true) => {
            state
                .engine
                .event_bus
                .emit_user_system(&headers, &state.pool, "[Settings] DeviceDeleted", |actor| {
                    SystemEvent::DeviceDeleted {
                        device_id: device_id.clone(),
                        actor,
                    }
                })
                .await;
            Ok(Json(serde_json::json!({ "success": true })))
        }
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
    headers: HeaderMap,
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
    if to.is_empty() {
        return Json(
            serde_json::json!({ "success": false, "error": "`to` is required and must be a non-empty array" }),
        );
    }
    let subject = match body["subject"].as_str() {
        Some(s) => s,
        None => {
            return Json(
                serde_json::json!({ "success": false, "error": "`subject` is required" }),
            )
        }
    };
    let body_text = match body["body"].as_str() {
        Some(s) => s,
        None => {
            return Json(serde_json::json!({ "success": false, "error": "`body` is required" }))
        }
    };
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
    let account_name = match body["account"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Json(
                serde_json::json!({ "success": false, "error": "`account` is required" }),
            )
        }
    };

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
        Ok(_) => {
            let attachment_count = attachments.len();
            state
                .engine
                .event_bus
                .emit_user_system(&headers, &state.pool, "[Email] EmailSent", |actor| {
                    crate::engine::event_bus::SystemEvent::EmailSent {
                        account: account.name.clone(),
                        to: to.clone(),
                        cc: cc.clone(),
                        bcc: bcc.clone(),
                        subject: subject.to_string(),
                        attachment_count,
                        actor,
                    }
                })
                .await;
            Json(serde_json::json!({ "success": true }))
        }
        Err(e) => {
            log!("[Email] Failed to send email to {}: {}", to.join(", "), e);
            Json(serde_json::json!({ "success": false, "error": format!("{}", e) }))
        }
    }
}

// ===== OAuth Access Token Endpoint =====

/// Response for `GET /api/v1/oauth/{provider}/access-token`. Carries the
/// short-lived bearer token plus the upstream-reported expiry. The
/// `refresh_token` is intentionally NOT included — keeping it engine-side
/// is the whole point of this endpoint.
#[derive(Serialize)]
pub(super) struct OAuthAccessTokenResponse {
    pub access_token: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Core lookup + auto-refresh for `GET /api/v1/oauth/{provider}/access-token`.
/// Pulled out of the axum handler so it can be unit-tested without HTTP plumbing.
pub(super) async fn fetch_oauth_access_token(
    pool: &PgPool,
    provider: &str,
) -> Result<OAuthAccessTokenResponse, (StatusCode, String)> {
    use crate::core::oauth::{get_account_with_fresh_token, AccountLookupError};
    let account = get_account_with_fresh_token(pool, provider)
        .await
        .map_err(|e| match e {
            AccountLookupError::NotConnected => (
                StatusCode::NOT_FOUND,
                format!("Provider '{}' not connected", provider),
            ),
            AccountLookupError::DbError(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load OAuth account for '{}': {}", provider, err),
            ),
            AccountLookupError::RefreshFailed(err) => (
                StatusCode::BAD_GATEWAY,
                format!("OAuth token refresh failed for '{}': {}", provider, err),
            ),
        })?;
    Ok(OAuthAccessTokenResponse {
        access_token: account.access_token,
        expires_at: account.token_expiry,
    })
}

/// `GET /api/v1/oauth/{provider}/access-token` — returns a short-lived
/// access token for an in-browser SDK (e.g. the Spotify Web Playback SDK)
/// without exposing the refresh token. Auto-refreshes when the stored
/// token is expired or expiring within 60s.
pub(super) async fn get_oauth_access_token(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> Result<Json<OAuthAccessTokenResponse>, (StatusCode, String)> {
    fetch_oauth_access_token(&state.pool, &provider)
        .await
        .map(Json)
}

/// Routes for the settings-owned surfaces: credentials, model registry,
/// OAuth accounts, preferences, tool allowlists, devices, pinned apps, and
/// email send.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        // Credentials endpoints
        .route(
            "/credentials",
            get(list_credentials)
                .post(create_credential)
                .put(update_credential)
                .delete(delete_credential),
        )
        .route("/credential-value", get(get_credential_value))
        .route("/email-account", get(get_email_account))
        // User-managed non-secret environment variables
        // (Settings → System → Environment variables).
        .route(
            "/env-vars",
            get(list_env_vars)
                .post(create_env_var)
                .put(update_env_var)
                .delete(delete_env_var),
        )
        // Per-workspace engine network bind (Settings → System → Network
        // access). The machine-global gateway bind + inherit toggle live on the
        // gateway control plane (`/~/api/v1/control/network-config`).
        .route(
            "/network-config",
            get(get_network_config).put(put_network_config),
        )
        // Model registry — the DB-backed chat model list (Settings → Models).
        // Drives the Lucidos Agent picker + RoutingProvider provider selection.
        .route(
            "/models",
            get(list_models)
                .post(create_model)
                .put(update_model)
                .delete(delete_model),
        )
        // OAuth account endpoints
        .route(
            "/oauth/accounts",
            get(list_oauth_accounts).delete(delete_oauth_account),
        )
        .route("/oauth/reauthorize", post(reauthorize_oauth))
        .route("/oauth/complete", post(complete_oauth))
        // Short-lived OAuth access-token for in-browser SDKs (e.g. Spotify
        // Web Playback SDK). Refresh token never leaves the engine.
        .route(
            "/oauth/:provider/access-token",
            get(get_oauth_access_token),
        )
        // Preferences endpoints
        .route(
            "/preferences",
            get(get_preferences)
                .put(set_preference)
                .delete(delete_preference),
        )
        // CC tool-permission allowlist (~/.lucidos/cc-allowed-tools)
        .route(
            "/cc-allowed-tools",
            get(get_cc_allowed_tools).put(put_cc_allowed_tools),
        )
        // Lucidos Agent command-guard allowlist (~/.lucidos/agent-allowed-commands, ADR 0002)
        .route(
            "/agent-allowed-commands",
            get(get_agent_allowed_commands).put(put_agent_allowed_commands),
        )
        // Device endpoints
        .route("/devices/register", post(register_device))
        .route("/devices", get(list_devices))
        .route("/devices/:device_id/name", put(rename_device))
        .route("/devices/:device_id/push", put(set_device_push))
        .route(
            "/devices/:device_id",
            axum::routing::delete(delete_device),
        )
        .route(
            "/pinned-apps",
            get(get_pinned_apps)
                .post(pin_app)
                .delete(unpin_app),
        )
        // Email endpoints
        .route("/email/send", post(send_email_confirmed))
}

#[cfg(test)]
mod oauth_access_token_tests {
    use super::*;
    use crate::core::OAuthStore;
    use chrono::{Duration, Utc};

    #[tokio::test]
    async fn returns_404_when_provider_not_connected() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;

        let result = fetch_oauth_access_token(&pool, "spotify").await;

        match result {
            Err((status, msg)) => {
                assert_eq!(status, StatusCode::NOT_FOUND);
                assert!(
                    msg.contains("spotify"),
                    "error should name the provider, got: {}",
                    msg
                );
            }
            Ok(_) => panic!("expected 404, got OK"),
        }

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn returns_access_token_and_expires_at_when_connected() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let expiry = Utc::now() + Duration::seconds(3600);
        OAuthStore::insert(
            &pool,
            "spotify",
            Some("user@example.com"),
            None,
            "BQAlive-access-token",
            Some("AQrefresh-token"),
            Some(expiry),
            "user-read-playback-state user-modify-playback-state",
        )
        .await
        .unwrap();

        let resp = fetch_oauth_access_token(&pool, "spotify")
            .await
            .expect("should return a token");

        assert_eq!(resp.access_token, "BQAlive-access-token");
        let returned_expiry = resp.expires_at.expect("expires_at must be set");
        assert!(
            (returned_expiry - expiry).num_seconds().abs() < 2,
            "expires_at should match the stored expiry"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn response_serialization_omits_refresh_token() {
        // Belt-and-braces: even if someone adds a refresh_token field later,
        // the on-the-wire JSON shape must stay { access_token, expires_at }.
        let resp = OAuthAccessTokenResponse {
            access_token: "tok".into(),
            expires_at: Some(Utc::now()),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("access_token").is_some());
        assert!(json.get("expires_at").is_some());
        assert!(
            json.get("refresh_token").is_none(),
            "refresh_token must never be serialized: {:?}",
            json
        );
    }

    #[tokio::test]
    async fn attempts_refresh_when_stored_token_is_expired() {
        // When the stored access token is already expired but no client
        // credentials are configured, refresh_oauth_if_needed errors out
        // and the handler maps that to BAD_GATEWAY. This proves the
        // refresh code path runs (rather than returning the stale token).
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let past = Utc::now() - Duration::seconds(120);
        OAuthStore::insert(
            &pool,
            "spotify",
            Some("user@example.com"),
            None,
            "stale-token",
            Some("AQrefresh-token"),
            Some(past),
            "user-read-playback-state",
        )
        .await
        .unwrap();

        let result = fetch_oauth_access_token(&pool, "spotify").await;

        match result {
            Err((status, _msg)) => {
                assert_eq!(
                    status,
                    StatusCode::BAD_GATEWAY,
                    "expired token + missing client creds must surface as 502, not return the stale token"
                );
            }
            Ok(resp) => panic!(
                "expected refresh to be attempted, got stale token back: {:?}",
                resp.access_token
            ),
        }

        crate::test_support::teardown_test_db(&db_name).await;
    }
}

#[cfg(test)]
mod provider_validation_tests {
    use super::*;

    /// `valid_provider` must accept exactly the five `ProviderKind` values and
    /// reject anything else — kept in lockstep with
    /// `crate::llm::model_registry::ProviderKind`.
    #[test]
    fn valid_provider_accepts_known_and_rejects_unknown() {
        for ok in ["vertex", "anthropic", "openai", "openrouter", "local"] {
            assert!(valid_provider(ok), "{ok} must be accepted");
        }
        for bad in ["", "Vertex", "openai ", "ollama", "bogus"] {
            assert!(!valid_provider(bad), "{bad:?} must be rejected");
        }
    }
}
