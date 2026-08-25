use super::*;

use crate::core::environment_variables::validate_name;
use crate::core::grants::{self, GrantFile};
use crate::core::{
    AuthType, CredentialStore, EnvironmentVariable, EnvironmentVariableStore, ModelStore,
    OAuthStore, PinnedAppStore, PreferenceStore,
};
use crate::llm::{supported_efforts, ProviderKind};

/// Response/request body for all three allowlist editors (`cc-allowed-tools`,
/// `agent-allowed-commands` and `mcp-allowed-tools`): the raw file text, one
/// pattern per line. The settings UI parses it into editable rows and
/// reserializes on save.
///
/// `pub(super)` on the field, not just the type. `api::mcp` owns the third
/// editor and reuses this shape, so all three answer with the same wire body.
#[derive(Serialize)]
pub(super) struct AllowlistResponse {
    pub(super) contents: String,
}

#[derive(Deserialize)]
pub(super) struct AllowlistRequest {
    pub(super) contents: String,
}

/// GET /api/v1/cc-allowed-tools — return the raw contents of
/// `<workspace>/.lucidos/cc-allowed-tools` so the settings UI can display them.
/// A missing file returns the seeded header (mirrors `cc_allowed_tools`).
pub(super) async fn get_cc_allowed_tools(
    State(state): State<AppState>,
) -> Result<Json<AllowlistResponse>, (StatusCode, String)> {
    let dir = state.engine.grants_dir();
    let contents = grants::read_raw(&dir, GrantFile::CodingAgentTools).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read cc-allowed-tools: {}", e),
        )
    })?;
    Ok(Json(AllowlistResponse { contents }))
}

/// Overwrite one grant file, and announce what the permission became.
///
/// **The write and the emit are one operation.** Each of the three editors
/// widens what an agent may run without a permission card. All three were a
/// bare `fs::write` leaving no trace, so `PUT /api/v1/cc-allowed-tools` with
/// `{"contents":"Bash(*)"}` was invisible in the events table.
///
/// One helper, rather than the pair copied into three handlers. Three copies
/// is how the emit came to be missing from all of them.
///
/// The patterns come from the body we just wrote, not from a read-back, so a
/// concurrent edit cannot make the event describe somebody else's write.
pub(super) async fn write_grant_file(
    state: &AppState,
    headers: &HeaderMap,
    file: GrantFile,
    contents: &str,
) -> Result<StatusCode, (StatusCode, String)> {
    grants::write_raw(&state.engine.grants_dir(), file, contents).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to write {}: {}", file.file_name(), e),
        )
    })?;
    let patterns = grants::parse_patterns(contents);
    state
        .engine
        .event_bus
        .emit_user_system(
            headers,
            &state.pool,
            "[Settings] PermissionGrantsChanged",
            |actor| crate::engine::event_bus::SystemEvent::PermissionGrantsChanged {
                grant_file: file,
                patterns,
                actor,
            },
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// PUT /api/v1/cc-allowed-tools — overwrite the file with the provided contents
/// (atomic). Newly spawned Claude Code subprocesses pick this up immediately; in-flight
/// subprocesses keep their frozen `--allowedTools` flag until they restart.
pub(super) async fn put_cc_allowed_tools(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AllowlistRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    write_grant_file(
        &state,
        &headers,
        GrantFile::CodingAgentTools,
        &body.contents,
    )
    .await
}

/// GET /api/v1/agent-allowed-commands — return the raw contents of
/// `<workspace>/.lucidos/agent-allowed-commands`, the Lucidos Agent command-guard
/// allowlist (ADR 0002). Missing file returns the seeded header. The chat
/// counterpart of `get_cc_allowed_tools`.
pub(super) async fn get_agent_allowed_commands(
    State(state): State<AppState>,
) -> Result<Json<AllowlistResponse>, (StatusCode, String)> {
    let dir = state.engine.grants_dir();
    let contents = grants::read_raw(&dir, GrantFile::AgentCommands).map_err(|e| {
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
    headers: HeaderMap,
    Json(body): Json<AllowlistRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    write_grant_file(&state, &headers, GrantFile::AgentCommands, &body.contents).await
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
    // Normalized at this boundary too, not just on the LLM tool path: typing
    // `oauth:google` into the Add Credential form must land on the same row as
    // typing `google`, or the form recreates the duplicate-credential incident
    // by hand. See `oauth::client_provider_name`.
    let service_name = if auth_type == AuthType::OauthClient {
        crate::core::oauth::client_provider_name(&request.service_name)
    } else {
        request.service_name.clone()
    };
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
    // `upsert` emits `Credential{Created,Updated}` itself, so this handler
    // resolves the device actor up front instead of going through
    // `emit_user_system`. The emit is not the caller's to make (see
    // `CredentialStore`'s type doc).
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match CredentialStore::upsert(
        &state.pool,
        &state.engine.event_bus,
        &service_name,
        &request.base_url,
        auth_type,
        &request.auth_value,
        request.auth_header.as_deref(),
        env_var_name,
        actor,
    )
    .await
    {
        Ok(_) => {
            // If this is an email password, also sync it into the email account
            // row. IMAP/SMTP read the password from `email_accounts`, not the
            // credentials table, so a failed sync must surface as an error (the
            // edit path in `update_credential` does the same) rather than report
            // a false success while email silently breaks.
            // The service name IS the account name now; the helper only differs
            // for a row the prefix migration had to leave alone.
            if auth_type == AuthType::EmailPassword {
                use crate::core::EmailStore;
                let account_name = EmailStore::account_name_for_credential(&service_name);
                if let Err(e) =
                    EmailStore::update_password(&state.pool, account_name, &request.auth_value)
                        .await
                {
                    return ApiResult::err(format!("Failed to update email password: {}", e));
                }
            }
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
    Query(query): Query<CredentialIdQuery>,
    headers: HeaderMap,
    Json(request): Json<UpdateCredentialRequest>,
) -> Json<ApiResult> {
    use crate::core::EmailStore;

    let id = query.id;
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

    // Same as the create path: the store owns the `CredentialUpdated` emit, so
    // resolve the device actor here and hand it over.
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match CredentialStore::update(
        &state.pool,
        &state.engine.event_bus,
        id,
        &base_url,
        auth_type,
        request.auth_header.as_deref(),
        new_secret,
        env_var_name,
        actor,
    )
    .await
    {
        // `update` hands back the row's service name, which for an
        // `email_password` credential IS the `email_accounts.name` (the `email:`
        // prefix that used to wrap it is gone). The id in the query cannot
        // supply it, which is why the store returns it.
        Ok(Some(service_name)) => {
            if auth_type == AuthType::EmailPassword {
                let account_name = EmailStore::account_name_for_credential(&service_name);
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
                        return ApiResult::err(format!("Failed to update email password: {}", e));
                    }
                }
            }
            ApiResult::ok()
        }
        Ok(None) => ApiResult::err("Credential not found".to_string()),
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

/// Delete a credential
pub(super) async fn delete_credential(
    State(state): State<AppState>,
    Query(query): Query<CredentialIdQuery>,
    headers: HeaderMap,
) -> Json<ApiResult> {
    // `delete` emits `CredentialDeleted` itself; resolve the device actor for it.
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match CredentialStore::delete(&state.pool, &state.engine.event_bus, query.id, actor).await {
        Ok(true) => ApiResult::ok(),
        Ok(false) => ApiResult::err("Credential not found".to_string()),
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
    let env_vars = EnvironmentVariableStore::list(&state.pool)
        .await
        .map_err(|e| {
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
    // The store emits `EnvironmentVariableSet` from inside its write path, so
    // this handler resolves the device actor and hands it over rather than
    // emitting afterwards (see `EnvironmentVariableStore`'s type doc).
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    if let Err(e) = EnvironmentVariableStore::upsert(
        &state.pool,
        &state.engine.event_bus,
        &request.name,
        &request.value,
        actor,
    )
    .await
    {
        return Ok(ApiResult::err(format!(
            "Failed to save environment variable: {}",
            e
        )));
    }
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
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match EnvironmentVariableStore::update(
        &state.pool,
        &state.engine.event_bus,
        &query.name,
        &request.value,
        actor,
    )
    .await
    {
        Ok(true) => Ok(ApiResult::ok()),
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
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match EnvironmentVariableStore::delete(&state.pool, &state.engine.event_bus, &query.name, actor)
        .await
    {
        Ok(true) => ApiResult::ok(),
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
        "vertex" | "anthropic" | "openai" | "openrouter" | "xai" | "opencode-free" | "local"
    )
}

const PROVIDER_ERR: &str =
    "Provider must be one of: vertex, anthropic, openai, openrouter, xai, opencode-free, local";

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
///
/// Each row carries the reasoning tiers its provider supports, derived here so
/// the picker offers exactly what `RoutingProvider` will send. Derived per
/// request rather than stored: it is a pure function of the row's provider and
/// id, so a re-providered model is right immediately and a user adding a local
/// model is never asked to declare tiers they cannot know.
pub(super) async fn list_models(
    State(state): State<AppState>,
) -> Result<Json<ModelsListResponse>, (StatusCode, String)> {
    let models = ModelStore::list(&state.pool).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list models: {}", e),
        )
    })?;
    let models = models
        .into_iter()
        .map(|model| ModelInfo {
            reasoning_efforts: supported_efforts(ProviderKind::parse(&model.provider), &model.id),
            model,
        })
        .collect();
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
    // The store emits `ModelCreated` from inside its write path (the in-memory
    // ModelRegistry reloads on it), so resolve the device actor and hand it over.
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match ModelStore::create(
        &state.pool,
        &state.engine.event_bus,
        id,
        label,
        &request.provider,
        sort_order,
        request.context_window,
        actor,
    )
    .await
    {
        Ok(_) => ApiResult::ok(),
        Err(e) => ApiResult::err(format!(
            "Failed to create model (id may already exist): {}",
            e
        )),
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
    // The store owns the `ModelUpdated` emit for both arms below.
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;

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
            &state.engine.event_bus,
            &existing.id,
            &existing.label,
            &existing.provider,
            existing.sort_order,
            enabled,
            context_window,
            actor,
        )
        .await
    } else {
        let label = request.label.unwrap_or_else(|| existing.label.clone());
        let provider = request
            .provider
            .unwrap_or_else(|| existing.provider.clone());
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
            &state.engine.event_bus,
            &existing.id,
            &label,
            &provider,
            sort_order,
            enabled,
            context_window,
            actor,
        )
        .await
    };

    match result {
        Ok(_) => ApiResult::ok(),
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
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match ModelStore::delete(&state.pool, &state.engine.event_bus, &existing.id, actor).await {
        Ok(_) => ApiResult::ok(),
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

    // `delete` emits `OAuthAccountDeleted` itself; resolve the device actor.
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match OAuthStore::delete(&state.pool, &state.engine.event_bus, id, actor).await {
        Ok(true) => ApiResult::ok(),
        Ok(false) => ApiResult::err("OAuth account not found"),
        Err(e) => ApiResult::err(format!("Failed to delete OAuth account: {}", e)),
    }
}

/// The *OAuth provider registry*: every provider whose endpoints Lucidos knows.
///
/// Drives two things on Settings > Accounts that were hardcoded or absent
/// before: the quick-provider buttons (previously a literal three-name array in
/// the frontend, which is why Dropbox had no button despite being fully
/// supported), and the Connect form's autofill. Nothing here is a secret, so the
/// rows are served verbatim; an unavailable registry answers an empty list and
/// the page falls back to its manual path.
pub(super) async fn list_known_oauth_providers(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let providers = crate::core::oauth_registry::load_providers(state.engine.system_knowhow_dir());
    Json(serde_json::json!({
        "providers": providers,
        // The exact loopback URI the flow will send, so the form can offer it
        // for copying into the provider's console. It has to be registered
        // character for character, and it is the engine's to state: only the
        // host form is configurable, never the port or path.
        "default_redirect_uri": crate::core::oauth::default_redirect_uri(),
    }))
}

/// Start an OAuth flow: prepares the authorization URL and spawns a background
/// listener for the callback. Returns `{ auth_url }` for the frontend to open.
pub(super) async fn reauthorize_oauth(
    State(state): State<AppState>,
    headers: HeaderMap,
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

    // The OAuth client has to exist AND be able to drive a flow before the
    // authorization starts. Both shortfalls resolve to the same answer: hand the
    // modal a credential request prefilled from the registry. Reaching
    // `prepare_oauth_flow` without them produces a bare "Missing auth_url in
    // OAuth credentials" toast one screen away from anything the user can act
    // on.
    let cred_service = crate::core::oauth::client_provider_name(&provider);
    let registry_row = crate::core::oauth_registry::find_provider(
        state.engine.system_knowhow_dir(),
        &cred_service,
    );
    let overrides = registry_row
        .as_ref()
        .map(crate::core::oauth::OAuthClientOverrides::from_registry)
        .unwrap_or_default();
    match CredentialStore::get_oauth_client(&state.pool, &cred_service).await {
        Ok(None) => return ApiResult::needs_credentials(&provider, &overrides),
        Err(e) => return ApiResult::err(format!("Failed to check credentials: {}", e)),
        Ok(Some(cred)) => {
            let missing = crate::core::oauth::missing_flow_fields(&cred.auth_value);
            if !missing.is_empty() {
                let client_id = serde_json::from_str::<serde_json::Value>(&cred.auth_value)
                    .ok()
                    .and_then(|v| v["client_id"].as_str().map(str::to_string));
                return ApiResult::needs_credential_repair(
                    &provider,
                    crate::core::oauth::oauth_client_repair_request(
                        &provider,
                        &overrides,
                        cred.id,
                        client_id.as_deref(),
                        &missing,
                    ),
                );
            }
        }
    }

    // The device clicking Connect is the one to bring back to the front when the
    // authorization lands, so it rides along to `OAuthAccountConnected`.
    let initiator = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match crate::core::oauth::prepare_oauth_flow(
        &state.pool,
        &state.engine.event_bus,
        &provider,
        &scopes,
        initiator,
    )
    .await
    {
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
        // The sender dropped without a result. Since the loopback callback port
        // has one owner and a new authorization supersedes the previous one
        // (`core::oauth::ACTIVE_CALLBACK_FLOW`), that is what this almost always
        // is, and it is something the user did on purpose rather than an
        // internal fault. Say so, and say how to get back.
        Ok(Err(_)) => ApiResult::err(crate::core::oauth::FLOW_SUPERSEDED_MSG),
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
    // `delete` emits the `PreferencesChanged` with `value: None` ("back to the
    // default") itself; resolve the device actor for it.
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match PreferenceStore::delete(&state.pool, &state.engine.event_bus, &key, actor).await {
        Ok(true) => ApiResult::ok(),
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
/// machine-global gateway bind, for Settings → Access → Network access.
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
        detected_tailscale_ip: crate::net_config::detect_tailscale_ipv4(),
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
        .apply_preference_write(
            crate::net_config::NETWORK_BIND_PREF_KEY,
            &value,
            None,
            actor,
        )
        .await
    {
        Ok(_) => Ok(ApiResult::ok()),
        Err(e) => Ok(ApiResult::err(format!("Failed to set network bind: {}", e))),
    }
}

// ===== Tailnet status (this workspace's Tailscale URL) =====

/// Bound on the MagicDNS reverse lookup. The resolver is local (`100.100.100.100`)
/// whenever we get this far, so this is a stall guard, not a budget. Mirrors
/// `REVERSE_DNS_TIMEOUT` in `crates/lucidos-app/src/mobile.rs`.
const MAGIC_DNS_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

/// Bound on the round trip that proves the serve URL reaches this workspace.
/// It leaves on the tailnet interface and comes straight back through the
/// gateway. So this is a stall guard for a hop that is local in practice.
const SERVE_VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// How long an answer is reused before the probes run again.
///
/// The endpoint needs no auth, like every other one here, and each miss costs a
/// resolver thread plus an outbound TLS connection. `magic_dns_name` ABANDONS
/// its worker at the deadline rather than joining it. So a caller in a loop
/// retires threads slower than it creates them, and an app iframe with a
/// polling bug is enough. This caps that at one probe per window.
///
/// Short on purpose. The Access page refetches right after an Expose run, and
/// that run takes far longer than this, so the refetch still sees the new URL.
const TAILNET_STATUS_TTL: std::time::Duration = std::time::Duration::from_secs(5);

/// The last answer and when it was produced. One engine serves one workspace,
/// so a process-wide slot is per-workspace by construction. In-memory cache,
/// which `CLAUDE.md` § Engine Statelessness allows: it is rebuilt by the next
/// miss and holds nothing a restart would lose.
static TAILNET_STATUS_CACHE: std::sync::Mutex<Option<(std::time::Instant, TailnetStatusResponse)>> =
    std::sync::Mutex::new(None);

/// What this machine's tailnet looks like from the engine, for Settings →
/// Access. Read over plain HTTP, so a phone browser gets the same answer the
/// packaged desktop app does.
///
/// Deliberately NOT folded into [`NetworkConfigResponse`]. Both Access panes
/// fetch that endpoint separately and read disjoint fields from it. That is
/// safe only while it stays the cheap local call `SettingsView` documents.
/// These two fields cost a reverse lookup and a network round trip, and the
/// bind editor reads neither.
#[derive(Serialize, Clone)]
pub(super) struct TailnetStatusResponse {
    /// `<machine>.<tailnet>.ts.net`, no scheme. `None` off a tailnet. Also
    /// `None` when MagicDNS is off, a per-tailnet setting that does not mean
    /// the machine is offline.
    magic_dns_name: Option<String>,
    /// The `https://<name>/<slug>/` URL, set **only** once a request to it was
    /// answered by this very engine. See [`get_tailnet_status`] for why a
    /// listener on 443 is not enough to publish it.
    workspace_serve_url: Option<String>,
}

/// GET /api/v1/tailnet-status: the MagicDNS name, and the HTTPS URL that
/// reaches THIS workspace over the tailnet.
///
/// **The URL is verified end to end, never inferred from a listener.** A TCP
/// probe of 443 proves that something serves HTTPS and says nothing about
/// which gateway. `system-knowhow/remote-access.md` documents a two-gateway
/// install: 443 fronts the packaged gateway, and the dev one takes 8443. So a
/// live 443 can belong to a gateway that never heard of this slug.
///
/// Publishing that URL would hand the user a link to somebody else's
/// workspace. So we fetch our own `health` through the candidate URL and
/// compare `workspace_path` with ours. A same-named workspace on the other
/// gateway lives at a different path.
///
/// Free when the machine is off a tailnet, and bounded when it is on one. This
/// answers a settings pane, and every probe on that path is bounded for the
/// reasons recorded in `crates/lucidos-app/src/mobile.rs`.
pub(super) async fn get_tailnet_status(
    State(state): State<AppState>,
) -> Json<TailnetStatusResponse> {
    if let Some(fresh) = cached_tailnet_status() {
        return Json(fresh);
    }
    let Some(addr) = lucidos_tailscale::tailnet_ipv4() else {
        // Not cached: this branch ran no probe, so repeating it costs nothing,
        // and a machine that just joined a tailnet answers on the next load.
        return Json(TailnetStatusResponse {
            magic_dns_name: None,
            workspace_serve_url: None,
        });
    };
    // A blocking resolver call, so it never runs on an async worker.
    let name = tokio::task::spawn_blocking(move || {
        lucidos_tailscale::magic_dns_name(addr, MAGIC_DNS_TIMEOUT)
    })
    .await
    .ok()
    .flatten();

    let workspace_serve_url = match (&name, super::base_path::workspace_id()) {
        // No name, or no slug: there is no candidate URL to verify. A slugless
        // engine is the direct-port dev mode, which no gateway path addresses.
        (Some(name), Some(slug)) => verified_workspace_serve_url(name, &slug, &state).await,
        _ => None,
    };
    let answer = TailnetStatusResponse {
        magic_dns_name: name,
        workspace_serve_url,
    };
    *lock_tailnet_cache() = Some((std::time::Instant::now(), answer.clone()));
    Json(answer)
}

/// The cached answer while it is inside [`TAILNET_STATUS_TTL`], else `None`.
fn cached_tailnet_status() -> Option<TailnetStatusResponse> {
    lock_tailnet_cache()
        .as_ref()
        .filter(|(at, _)| at.elapsed() < TAILNET_STATUS_TTL)
        .map(|(_, answer)| answer.clone())
}

/// Take the cache lock, ignoring poisoning. Nothing under it can be left
/// half-written: it holds one timestamped answer, replaced whole.
fn lock_tailnet_cache(
) -> std::sync::MutexGuard<'static, Option<(std::time::Instant, TailnetStatusResponse)>> {
    TAILNET_STATUS_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The candidate URL, if this engine is what answers at it.
///
/// `https` is hardcoded here, and that is not the intra-host scheme question
/// `.claude/rules/rust.md` governs. This is the tailnet endpoint `tailscale
/// serve` publishes, and it is HTTPS by construction. TLS is validated
/// normally for the same reason. Tailscale issues a real certificate for a
/// `.ts.net` name, and accepting an invalid one would throw away the proof
/// this function exists to produce.
async fn verified_workspace_serve_url(
    magic_dns_name: &str,
    slug: &str,
    state: &AppState,
) -> Option<String> {
    let url = format!("https://{magic_dns_name}/{slug}/");
    let client = reqwest::Client::builder()
        .timeout(SERVE_VERIFY_TIMEOUT)
        // A system proxy has no business intercepting a tailnet hop, and would
        // answer for a host it cannot reach.
        .no_proxy()
        // The candidate URL is exact, so there is nowhere legitimate to follow.
        // Left at the default, a hostile responder could bounce this probe at
        // `127.0.0.1` and make us reach a host it cannot.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let response = client
        .get(format!("{url}api/v1/health"))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.text().await.ok()?;
    let own_path = state.workspace_path.to_string_lossy();
    health_names_this_workspace(&body, &own_path).then_some(url)
}

/// Pure: does this `health` body prove the probe reached THIS engine?
///
/// `workspace_path` is the discriminator rather than `workspace`, which is only
/// the directory name and collides across gateways by design. Anything we
/// cannot parse is a no: an unverified URL is never published.
fn health_names_this_workspace(body: &str, own_workspace_path: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("workspace_path")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .is_some_and(|path| path == own_workspace_path)
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
    let actor =
        super::actor::user_actor_resolved(&headers, &state.pool, Some(&request.device_id)).await;
    match PinnedAppStore::pin(
        &state.pool,
        &state.engine.event_bus,
        &request.app_id,
        "main",
        &request.device_id,
        actor,
    )
    .await
    {
        Ok(_) => ApiResult::ok(),
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
    let actor =
        super::actor::user_actor_resolved(&headers, &state.pool, Some(&request.device_id)).await;
    match PinnedAppStore::unpin(
        &state.pool,
        &state.engine.event_bus,
        &request.app_id,
        "main",
        &request.device_id,
        actor,
    )
    .await
    {
        Ok(_) => ApiResult::ok(),
        Err(e) => ApiResult::err(format!("Failed to unpin app: {}", e)),
    }
}

// ===== Device Management Endpoints =====

pub(super) async fn register_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DeviceRegisterRequest>,
) -> Json<serde_json::Value> {
    // The frontend calls this on every page load to refresh `last_seen_at`, so
    // the upsert path is the steady state. `register` announces only a genuine
    // first-touch insert, so the events table does not grow by a row per
    // refresh (see `DeviceStore`'s type doc).
    let actor =
        super::actor::user_actor_resolved(&headers, &state.pool, Some(&request.device_id)).await;
    match crate::core::DeviceStore::register(
        &state.pool,
        &state.engine.event_bus,
        &request.device_id,
        request.user_agent.as_deref(),
        actor,
    )
    .await
    {
        Ok((device, _inserted)) => Json(serde_json::json!({ "success": true, "device": device })),
        Err(e) => Json(serde_json::json!({ "success": false, "error": e.to_string() })),
    }
}

/// Why this caller may not move a row onto `target`, or `None` when it may.
///
/// The rule is that a caller may only hand a row over to ITSELF. Behind the
/// *workspace gateway* the `x-lucidos-device-id` header is gateway-asserted
/// (ADR 0094's amendment), so requiring the body's target to match it is free.
/// Unchecked, a paired caller could move another device's row onto an id nobody
/// will ever present, taking its push subscription and preferences with it.
///
/// A missing or blank header is the loopback case and is not refused, matching
/// every other endpoint here.
///
/// The client asserts the id it is ADOPTING, which is what makes the rule
/// satisfiable at all: it still stores the old one when it asks. See
/// `handOverDevice` in `api/client/settings.ts`.
fn foreign_hand_over(headers: &HeaderMap, target: &str) -> Option<String> {
    let asserted = headers
        .get(super::actor::HEADER_DEVICE_ID)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    if asserted == target {
        return None;
    }
    Some(format!(
        "this caller is device {asserted} and cannot hand a row over to {target}"
    ))
}

/// Move a device's row to the id it now reports as.
///
/// Called once, early in boot, by a client whose id has changed under it. The
/// three OUTCOMES are all 200: `already-done` and `no-such-device` mean "stop
/// asking", and neither is a failure the user should see.
///
/// **A failed transaction is a 500, not a 200 carrying `success: false`.** The
/// client throws away its memory of the old id once this returns, so a failure
/// it cannot see is a row abandoned forever. The frontend's `json()` only
/// rejects on a non-2xx status, so the status IS the signal.
///
/// Who may ask is [`foreign_hand_over`]'s rule.
pub(super) async fn hand_over_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DeviceHandOverRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(reason) = foreign_hand_over(&headers, &request.device_id) {
        return Err(ApiError::bad_request(reason));
    }
    let actor =
        super::actor::user_actor_resolved(&headers, &state.pool, Some(&request.device_id)).await;
    let outcome = crate::core::DeviceStore::hand_over(
        &state.pool,
        &state.engine.event_bus,
        &request.old_device_id,
        &request.device_id,
        actor,
    )
    .await
    .map_err(|e| {
        log!("[Settings] Device hand-over failed: {}", e);
        ApiError::internal(format!(
            "could not hand device {} over to {}: {e}",
            request.old_device_id, request.device_id
        ))
    })?;
    Ok(Json(
        serde_json::json!({ "success": true, "outcome": outcome }),
    ))
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
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, Some(&device_id)).await;
    match crate::core::DeviceStore::rename(
        &state.pool,
        &state.engine.event_bus,
        &device_id,
        request.name.as_deref(),
        actor,
    )
    .await
    {
        Ok(true) => Json(serde_json::json!({ "success": true })),
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
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, Some(&device_id)).await;
    match crate::core::DeviceStore::set_push_enabled(
        &state.pool,
        &state.engine.event_bus,
        &device_id,
        request.push_enabled,
        actor,
    )
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
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, Some(&device_id)).await;
    match crate::core::DeviceStore::delete(&state.pool, &state.engine.event_bus, &device_id, actor)
        .await
    {
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
            return Json(serde_json::json!({ "success": false, "error": "`subject` is required" }))
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
            return Json(serde_json::json!({ "success": false, "error": "`account` is required" }))
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
        // Per-workspace engine network bind (Settings → Access → Network
        // access). The machine-global gateway bind + inherit toggle live on the
        // gateway control plane (`/~/api/v1/control/network-config`).
        .route(
            "/network-config",
            get(get_network_config).put(put_network_config),
        )
        // The MagicDNS name and the verified HTTPS URL for this workspace, for
        // the Connect URLs on that same page. Its own route rather than fields
        // on `/network-config`, which the bind editor also fetches and which
        // must stay a cheap local read.
        .route("/tailnet-status", get(get_tailnet_status))
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
        .route("/oauth/known-providers", get(list_known_oauth_providers))
        .route("/oauth/reauthorize", post(reauthorize_oauth))
        .route("/oauth/complete", post(complete_oauth))
        // Short-lived OAuth access-token for in-browser SDKs (e.g. Spotify
        // Web Playback SDK). Refresh token never leaves the engine.
        .route("/oauth/:provider/access-token", get(get_oauth_access_token))
        // Preferences endpoints
        .route(
            "/preferences",
            get(get_preferences)
                .put(set_preference)
                .delete(delete_preference),
        )
        // CC tool-permission allowlist (<workspace>/.lucidos/cc-allowed-tools)
        .route(
            "/cc-allowed-tools",
            get(get_cc_allowed_tools).put(put_cc_allowed_tools),
        )
        // Lucidos Agent command-guard allowlist
        // (<workspace>/.lucidos/agent-allowed-commands, ADR 0002)
        .route(
            "/agent-allowed-commands",
            get(get_agent_allowed_commands).put(put_agent_allowed_commands),
        )
        // Device endpoints
        .route("/devices/register", post(register_device))
        .route("/devices/hand-over", post(hand_over_device))
        .route("/devices", get(list_devices))
        .route("/devices/:device_id/name", put(rename_device))
        .route("/devices/:device_id/push", put(set_device_push))
        .route("/devices/:device_id", axum::routing::delete(delete_device))
        .route(
            "/pinned-apps",
            get(get_pinned_apps).post(pin_app).delete(unpin_app),
        )
        // Email endpoints
        .route("/email/send", post(send_email_confirmed))
}

#[cfg(test)]
mod tailnet_status_tests {
    use super::*;

    /// A `health` body shaped like the real one, for a given workspace path.
    fn health_body(workspace_path: &str) -> String {
        serde_json::json!({
            "status": "ok",
            "workspace": "dev",
            "workspace_path": workspace_path,
            "database_reachable": true,
        })
        .to_string()
    }

    #[test]
    fn the_url_publishes_only_when_our_own_engine_answered() {
        let own = "/Users/me/workspaces/dev";
        assert!(health_names_this_workspace(&health_body(own), own));
    }

    #[test]
    fn a_same_named_workspace_on_another_gateway_is_rejected() {
        // The failure this whole verification exists for. Two gateways can each
        // hold a workspace whose slug is `dev`, and only one of them is us.
        // They differ by path, which is why the path is the discriminator.
        let body = health_body("/Users/me/other-checkout/workspaces/dev");
        assert!(!health_names_this_workspace(
            &body,
            "/Users/me/workspaces/dev"
        ));
    }

    #[test]
    fn a_body_we_cannot_read_is_never_a_match() {
        // Anything unparseable is a no: an unverified URL is never published.
        // A 404 page from a gateway that does not know this slug lands here.
        let own = "/Users/me/workspaces/dev";
        assert!(!health_names_this_workspace("<html>not found</html>", own));
        assert!(!health_names_this_workspace("", own));
        assert!(!health_names_this_workspace("{}", own));
        assert!(!health_names_this_workspace(
            r#"{"workspace_path": 42}"#,
            own
        ));
    }

    #[test]
    fn the_directory_name_alone_does_not_prove_identity() {
        // `workspace` is only the last path segment, so it collides across
        // gateways by design. Reading it instead of `workspace_path` would
        // accept the very case the test above rejects.
        let body = health_body("/Users/me/other-checkout/workspaces/dev");
        assert!(!health_names_this_workspace(&body, "dev"));
    }

    #[test]
    fn the_verification_probe_never_disables_certificate_checking() {
        // Tailscale issues a real certificate for a `.ts.net` name, so there is
        // nothing to work around. Accepting an invalid one would throw away the
        // proof `verified_workspace_serve_url` exists to produce, and the
        // loopback carve-out in `.claude/rules/rust.md` does not reach here.
        let source = include_str!("settings.rs");
        let start = source
            .find("async fn verified_workspace_serve_url")
            .expect("the prober must exist");
        let body = &source[start..];
        let end = body.find("\n}\n").expect("the prober must end");
        assert!(
            !body[..end].contains("danger_accept_invalid_certs"),
            "the tailnet verification probe must validate TLS"
        );
    }
}

#[cfg(test)]
mod oauth_access_token_tests {
    use super::*;
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
        crate::test_support::seed_oauth_account(
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
        crate::test_support::seed_oauth_account(
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

    /// `valid_provider` must accept exactly the `ProviderKind` column values and
    /// reject anything else. The accepted list is spelled through `as_str`.
    /// Renaming a column value on the enum then fails here, rather than making
    /// the API reject rows the registry itself writes.
    #[test]
    fn valid_provider_accepts_known_and_rejects_unknown() {
        for kind in [
            ProviderKind::Vertex,
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::OpenRouter,
            ProviderKind::XAi,
            ProviderKind::OpenCodeFree,
            ProviderKind::Local,
        ] {
            let ok = kind.as_str();
            assert!(valid_provider(ok), "{ok} must be accepted");
            assert!(
                PROVIDER_ERR.contains(ok),
                "the error message must name {ok} as an option"
            );
        }
        for bad in ["", "Vertex", "openai ", "ollama", "bogus"] {
            assert!(!valid_provider(bad), "{bad:?} must be rejected");
        }
    }
}

#[cfg(test)]
mod hand_over_guard_tests {
    use super::*;

    fn headers_with(device_id: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(id) = device_id {
            h.insert(super::super::actor::HEADER_DEVICE_ID, id.parse().unwrap());
        }
        h
    }

    #[test]
    fn a_caller_may_move_its_own_row() {
        assert_eq!(
            foreign_hand_over(&headers_with(Some("dev-a")), "dev-a"),
            None
        );
    }

    #[test]
    fn a_caller_may_not_move_someone_elses_row() {
        // The reason names both, so the client's own log says which two ids
        // disagreed rather than only that something was refused.
        let reason = foreign_hand_over(&headers_with(Some("dev-a")), "dev-b")
            .expect("a caller that is not the target must be refused");
        assert!(reason.contains("dev-a"), "{reason}");
        assert!(reason.contains("dev-b"), "{reason}");
    }

    #[test]
    fn the_client_asserting_the_id_it_adopts_is_the_case_that_must_pass() {
        // What `handOverDevice` sends. Asserting the id it still STORES is the
        // shape that stranded the migration, so pin both directions.
        assert_eq!(
            foreign_hand_over(&headers_with(Some("paired-device")), "paired-device"),
            None
        );
        assert!(
            foreign_hand_over(&headers_with(Some("minted-locally")), "paired-device").is_some()
        );
    }

    #[test]
    fn no_header_is_the_loopback_case_and_is_allowed() {
        assert_eq!(foreign_hand_over(&headers_with(None), "dev-a"), None);
    }

    #[test]
    fn a_blank_header_asserts_nothing_and_is_allowed() {
        assert_eq!(foreign_hand_over(&headers_with(Some("   ")), "dev-a"), None);
    }
}
