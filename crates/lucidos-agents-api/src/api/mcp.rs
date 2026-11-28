use super::error::ApiError;
use super::settings::{AllowlistRequest, AllowlistResponse};
use super::*;
use crate::core::grants::GrantFile;
use crate::core::mcp_servers::validate_server_id;
use crate::core::{McpServer, McpServerStore};
use crate::engine::cc_permission::resolve_coding_agent_permission;
use crate::engine::permission::tool_grants::AnsweredBy;
use crate::engine::thread_events::AllowScope;
use crate::engine::thread_events::{EventMeta, McpEnablementSource};
use crate::mcp::{McpCostTotals, McpStartOutcome, McpStopOutcome};
use axum::routing::patch;

#[derive(Deserialize)]
pub(super) struct McpConsentResponse {
    pub request_id: String,
    pub allowed: bool,
    /// INVARIANT: When `Some(_)` and `allowed == true`, the engine derives a pattern
    /// from the original prompt's tool_name + input and remembers it so
    /// future identical-pattern requests skip the prompt. Where the pattern
    /// is recorded as a *tool grant* depends on scope:
    ///   * `narrow` / `broad`: a standing grant, handed to CC via
    ///     `--allowedTools` on every spawn AND read by the engine's own gate
    ///     on every prompt.
    ///   * `session`: a grant the thread holds. The one scope reaching tools
    ///     and paths CC never auto-approves.
    ///
    /// Absent (the Allow-once path) records nothing. Unknown wire values
    /// cause a 4xx via serde — match the engine's typed enum exactly.
    #[serde(default)]
    pub persist_scope: Option<AllowScope>,
    #[serde(default)]
    pub always_deny: bool,
}

pub(super) async fn submit_mcp_consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<McpConsentResponse>,
) -> impl IntoResponse {
    let card_thread = state
        .engine
        .pending_cc_permission
        .lock()
        .unwrap()
        .thread_of(&body.request_id);
    if let Err(e) = super::thread_reach::refuse_permission_answer(
        &state.engine.event_bus,
        &headers,
        card_thread,
        body.allowed,
        body.persist_scope,
    )
    .await
    {
        return e.into_response();
    }
    let always_deny = body.always_deny && !body.allowed;
    if always_deny {
        if let Err(e) = super::thread_reach::require_owner_for_standing_deny(&headers) {
            return e.into_response();
        }
    }
    let actor = super::actor::user_actor(&headers, None);
    let answered = resolve_coding_agent_permission(
        &state.engine,
        body.request_id,
        body.allowed,
        body.persist_scope,
        always_deny,
        AnsweredBy::tap(actor),
        "[MCP] CodingAgentPermissionResolved",
    )
    .await;
    let answered = match answered {
        Ok(answered) => answered,
        Err(error) => return (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
    };
    if !answered {
        return (
            StatusCode::NOT_FOUND,
            "No pending consent request with that ID",
        )
            .into_response();
    }
    StatusCode::OK.into_response()
}

#[derive(Deserialize)]
pub(super) struct McpAutoApproveRequest {
    pub server_id: String,
    pub auto_approve: bool,
}

/// INVARIANT: PUT /api/v1/mcp/auto-approve — Set auto-approve for an MCP server.
///
/// The store emits `McpServerUpdated` from inside its write path. So this
/// handler resolves the device actor and hands it over, the way every other
/// mutating endpoint in this router does.
pub(super) async fn set_mcp_auto_approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<McpAutoApproveRequest>,
) -> impl IntoResponse {
    if let Err(refusal) = super::thread_reach::require_owner_for_permission_grant(&headers) {
        return refusal.into_response();
    }
    let actor = super::actor::user_actor(&headers, None);
    match state
        .engine
        .mcp_manager
        .set_auto_approve(&body.server_id, body.auto_approve, actor)
        .await
    {
        Ok(msg) => Json(serde_json::json!({ "success": true, "message": msg })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed: {}", e)).into_response(),
    }
}

/// INVARIANT: GET /api/v1/mcp/servers: list MCP servers with status and context cost.
///
/// Cost is computed here rather than client-side, through the same
/// `tool_definitions_chars` and `estimate_tokens_from_chars` the request packer
/// and the Context Viewer use. A second ratio in the frontend is what once
/// reported a 205k prompt as 361k.
///
/// `budget` states the worst case, one thread that enabled every server: what
/// its request sends once the MCP tool ceiling applies, and what the ceiling
/// drops. Its share must stay chars against the request's own char budget, the
/// unit and total the packer fills.
pub(super) async fn list_mcp_servers(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let servers = state
        .engine
        .mcp_manager
        .list_servers()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list MCP servers: {e}")))?;
    let totals = McpCostTotals::of(&servers);

    let (model, context_window) = state.engine.chat_model_and_window().await;
    let budget_model = state.engine.request_budget_model_for(&model, None);
    let budget = state
        .engine
        .mcp_manager
        .budget(&budget_model, context_window)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to cost MCP servers: {e}")))?;

    Ok(Json(serde_json::json!({
        "servers": servers,
        "totals": totals,
        "model": model,
        "context_window": context_window,
        "budget": budget,
    })))
}

/// INVARIANT: The registry row for `id`, or a 404.
///
/// Every per-server verb loads the row first. That is what separates "no such
/// server" from a start that really failed. It also hands `start_loaded` the
/// row, so a removal between the check and the start cannot turn a 404 into a
/// 502.
async fn load_server(state: &AppState, id: &str) -> Result<McpServer, ApiError> {
    McpServerStore::get(&state.pool, id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to read MCP server '{id}': {e}")))?
        .ok_or_else(|| ApiError::not_found(format!("MCP server '{id}' not found")))
}

/// INVARIANT: The error for a server whose STORED id cannot ride a wire tool name, or
/// `None` when it can.
///
/// 422 rather than the 502 a failed start gets, and the distinction is the
/// whole point. Nothing about this server will ever work, so the page has to
/// say the id is unusable and offer Remove, not show a retryable failure.
/// `validate_server_id` runs at registration too, but a stored row is not proof
/// that it passed: this is the row the incident left behind.
fn unusable_id_error(server: &McpServer) -> Option<ApiError> {
    validate_server_id(&server.id).err().map(|e| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("{e}. This server cannot be started; remove it and register it again."),
        )
    })
}

fn start_failed_error(id: &str, e: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        format!("MCP server '{id}' failed to start: {e}"),
    )
}

/// INVARIANT: Record the owner's click as approval of `server`'s current launch line, so
/// the start gate lets it spawn whatever the command guard setting.
///
/// Only the owner's registered device approves. Any other caller's start
/// still runs, through the start gate: an agent's `lucidos mcp start`, the
/// local token, or an app an agent wrote. Otherwise one of them could approve
/// a line the guard never showed the user.
async fn approve_launch_by_owner_click(
    manager: &crate::mcp::McpManager,
    pool: &sqlx::PgPool,
    event_bus: &crate::engine::event_bus::EventBus,
    headers: &HeaderMap,
    server: &McpServer,
) -> Result<(), ApiError> {
    let Ok(owner) = super::actor::require_owner_device(
        headers,
        pool,
        event_bus,
        None,
        "approve the MCP server launch",
    )
    .await
    else {
        return Ok(());
    };
    manager
        .approve_launch(server, Some(owner))
        .await
        .map_err(|e| {
            ApiError::internal(format!("Failed to approve MCP server '{}': {e}", server.id))
        })
}

pub(super) async fn start_mcp_server(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    super::actor::require_user_actor(&headers, &state.pool, None).await?;
    let server = load_server(&state, &id).await?;
    if let Some(e) = unusable_id_error(&server) {
        return Err(e);
    }
    approve_launch_by_owner_click(
        &state.engine.mcp_manager,
        &state.pool,
        &state.engine.event_bus,
        &headers,
        &server,
    )
    .await?;

    let outcome = state
        .engine
        .mcp_manager
        .start_loaded(&server)
        .await
        .map_err(|e| start_failed_error(&id, e))?;

    Ok(Json(serde_json::json!({
        "running": true,
        "already_running": matches!(outcome, McpStartOutcome::AlreadyRunning { .. }),
        "tool_count": outcome.tool_count(),
    })))
}

pub(super) async fn stop_mcp_server(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    let outcome = state
        .engine
        .mcp_manager
        .stop_everywhere(&id, Some(actor))
        .await
        .map_err(|e| ApiError::internal(format!("Failed to stop MCP server '{id}': {e}")))?
        .ok_or_else(|| ApiError::not_found(format!("MCP server '{id}' not found")))?;

    Ok(Json(serde_json::json!({
        "running": false,
        "was_running": matches!(outcome, McpStopOutcome::Stopped { .. }),
    })))
}

async fn require_thread(state: &AppState, thread_id: Uuid) -> Result<(), ApiError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1)")
            .bind(thread_id)
            .fetch_one(&state.pool)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to read thread {thread_id}: {e}")))?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found(format!("Thread {thread_id} not found")))
    }
}

pub(super) async fn enable_thread_mcp_server(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((thread_id, id)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    require_thread(&state, thread_id).await?;
    let server = load_server(&state, &id).await?;
    if let Some(e) = unusable_id_error(&server) {
        return Err(e);
    }
    approve_launch_by_owner_click(
        &state.engine.mcp_manager,
        &state.pool,
        &state.engine.event_bus,
        &headers,
        &server,
    )
    .await?;

    let outcome = state
        .engine
        .mcp_manager
        .enable_for_thread(
            thread_id,
            &id,
            McpEnablementSource::User,
            EventMeta::with_actor(Some(actor)),
        )
        .await
        .map_err(|e| start_failed_error(&id, e))?;

    Ok(Json(serde_json::json!({
        "enabled": true,
        "already_running": matches!(outcome, McpStartOutcome::AlreadyRunning { .. }),
        "tool_count": outcome.tool_count(),
    })))
}

pub(super) async fn disable_thread_mcp_server(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((thread_id, id)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
    require_thread(&state, thread_id).await?;
    load_server(&state, &id).await?;

    let was_enabled = state
        .engine
        .mcp_manager
        .disable_for_thread(
            thread_id,
            &id,
            McpEnablementSource::User,
            EventMeta::with_actor(Some(actor)),
        )
        .await
        .map_err(|e| ApiError::internal(format!("Failed to disable MCP server '{id}': {e}")))?;

    Ok(Json(serde_json::json!({
        "enabled": false,
        "was_enabled": was_enabled,
    })))
}

pub(super) async fn remove_mcp_server(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // INVARIANT: The store emits `McpServerRemoved` from inside its write path, so the
    // device actor is resolved up front and passed down.
    let actor = Some(super::actor::require_user_actor(&headers, &state.pool, None).await?);
    let removed = state
        .engine
        .mcp_manager
        .remove_server(&id, actor)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to remove MCP server '{id}': {e}")))?;

    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("MCP server '{id}' not found")))
    }
}

#[derive(Deserialize)]
pub(super) struct McpToolSwitchRequest {
    wire_name: String,
    disabled: bool,
}

pub(super) async fn set_mcp_disabled_tools(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<McpToolSwitchRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = super::actor::user_actor(&headers, None);
    let stored = state
        .engine
        .mcp_manager
        .set_tool_disabled(&id, &body.wire_name, body.disabled, actor)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to update MCP server '{id}': {e}")))?
        .ok_or_else(|| ApiError::not_found(format!("MCP server '{id}' not found")))?;

    Ok(Json(serde_json::json!({ "disabled_tools": stored })))
}

pub(super) async fn get_mcp_allowed_tools(
    State(state): State<AppState>,
) -> Result<Json<AllowlistResponse>, ApiError> {
    super::settings::read_grant_lane(&state, GrantFile::McpTools)
        .await
        .map_err(|(status, message)| ApiError::new(status, message))
}

pub(super) async fn put_mcp_allowed_tools(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AllowlistRequest>,
) -> Result<StatusCode, ApiError> {
    super::settings::write_grant_lane(&state, &headers, GrantFile::McpTools, &body.contents)
        .await
        .map_err(|(status, message)| ApiError::new(status, message))
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/mcp/consent", post(submit_mcp_consent))
        .route("/mcp/auto-approve", put(set_mcp_auto_approve))
        .route("/mcp/servers", get(list_mcp_servers))
        .route("/mcp/servers/:id", delete(remove_mcp_server))
        .route("/mcp/servers/:id/start", post(start_mcp_server))
        .route("/mcp/servers/:id/stop", post(stop_mcp_server))
        .route(
            "/threads/:thread_id/mcp-servers/:id/enable",
            post(enable_thread_mcp_server),
        )
        .route(
            "/threads/:thread_id/mcp-servers/:id/disable",
            post(disable_thread_mcp_server),
        )
        .route(
            "/mcp/servers/:id/disabled-tools",
            patch(set_mcp_disabled_tools),
        )
        .route(
            "/mcp-allowed-tools",
            get(get_mcp_allowed_tools).put(put_mcp_allowed_tools),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_the_owners_click_approves_a_launch_line() {
        use crate::core::subprocess_origin::{
            init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
            HEADER_DEVICE_ID,
        };
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        crate::core::DeviceStore::register(&pool, &bus, "owner-phone", None, None)
            .await
            .unwrap();
        let server = McpServerStore::register(
            &pool,
            &bus,
            "srv",
            "srv",
            "/usr/local/bin/srv",
            &[],
            &Default::default(),
            None,
        )
        .await
        .unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let manager =
            crate::mcp::McpManager::new(pool.clone(), bus.clone(), workspace.path().into());

        init_agent_origin_secret("mcp-click-test-secret".to_string());
        let mut agent = HeaderMap::new();
        let token = mint_agent_origin_token(Some(Uuid::new_v4()), 0, None).unwrap();
        agent.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
        approve_launch_by_owner_click(&manager, &pool, &bus, &agent, &server)
            .await
            .unwrap();
        assert!(!manager.is_launch_approved(&server).await.unwrap());

        let mut typed = HeaderMap::new();
        typed.insert(HEADER_DEVICE_ID, "owner-phone".parse().unwrap());
        approve_launch_by_owner_click(&manager, &pool, &bus, &typed, &server)
            .await
            .unwrap();
        assert!(
            !manager.is_launch_approved(&server).await.unwrap(),
            "a typed device id with no gateway stamp is not the owner's click"
        );

        let owner = crate::api::gateway_stamp::test_support::stamped_device_headers("owner-phone");
        approve_launch_by_owner_click(&manager, &pool, &bus, &owner, &server)
            .await
            .unwrap();
        assert!(manager.is_launch_approved(&server).await.unwrap());

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[test]
    fn mcp_consent_response_deserializes_persist_scope_as_typed_enum() {
        let body: McpConsentResponse =
            serde_json::from_str(r#"{"request_id":"r","allowed":true,"persist_scope":"narrow"}"#)
                .unwrap();
        assert_eq!(body.persist_scope, Some(AllowScope::Narrow));

        let body: McpConsentResponse =
            serde_json::from_str(r#"{"request_id":"r","allowed":true,"persist_scope":"broad"}"#)
                .unwrap();
        assert_eq!(body.persist_scope, Some(AllowScope::Broad));

        let body: McpConsentResponse =
            serde_json::from_str(r#"{"request_id":"r","allowed":true,"persist_scope":"session"}"#)
                .unwrap();
        assert_eq!(body.persist_scope, Some(AllowScope::Session));

        let body: McpConsentResponse =
            serde_json::from_str(r#"{"request_id":"r","allowed":true}"#).unwrap();
        assert_eq!(body.persist_scope, None);

        assert!(serde_json::from_str::<McpConsentResponse>(
            r#"{"request_id":"r","allowed":true,"persist_scope":"yolo"}"#
        )
        .is_err());
    }

    fn server_with_id(id: &str) -> McpServer {
        McpServer {
            id: id.to_string(),
            name: "Srv".to_string(),
            command: "cmd".to_string(),
            args: Vec::new(),
            env: std::collections::HashMap::new(),
            auto_approve: false,
            created_at: chrono::Utc::now(),
            tools: Vec::new(),
            tools_observed_at: None,
            disabled_tools: Vec::new(),
        }
    }

    #[test]
    fn the_three_per_server_failures_stay_distinct() {
        assert!(unusable_id_error(&server_with_id("backstage")).is_none());

        let unusable =
            unusable_id_error(&server_with_id("back.stage")).expect("a dotted id is unusable");
        assert_eq!(unusable.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(unusable.message.contains("back.stage"));
        assert!(
            unusable.message.contains("remove it"),
            "the message has to name the only action that works: {}",
            unusable.message
        );

        let failed = start_failed_error("slack", "Failed to spawn MCP server 'npx': not found");
        assert_eq!(failed.status, StatusCode::BAD_GATEWAY);
        assert!(failed.message.contains("not found"));
        assert!(failed.message.contains("slack"));

        let missing = ApiError::not_found(format!("MCP server '{}' not found", "gone"));
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn an_over_long_id_is_unusable_for_its_own_stated_reason() {
        let long = "a".repeat(crate::core::mcp_servers::MAX_SERVER_ID_LEN + 1);
        let e = unusable_id_error(&server_with_id(&long)).expect("too long to carry");
        assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(e.message.contains("characters"), "{}", e.message);
    }

    #[test]
    fn a_tool_switch_names_one_tool_and_its_direction() {
        let body: McpToolSwitchRequest =
            serde_json::from_str(r#"{"wire_name":"mcp__slack__post_message","disabled":true}"#)
                .unwrap();
        assert_eq!(body.wire_name, "mcp__slack__post_message");
        assert!(body.disabled);

        assert!(serde_json::from_str::<McpToolSwitchRequest>(r#"{"wire_name":"x"}"#).is_err());
        assert!(serde_json::from_str::<McpToolSwitchRequest>(r#"{"disabled":false}"#).is_err());
    }

    #[tokio::test]
    async fn the_page_names_the_saved_chat_model_not_the_boot_default() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());

        assert_eq!(
            crate::core::PreferenceStore::resolved_chat_model(&pool, "claude-opus-5").await,
            "claude-opus-5"
        );

        crate::core::PreferenceStore::set(
            &pool,
            &bus,
            crate::core::prefs::CHAT_MODEL.key(),
            "claude-opus-5[1m]",
            None,
        )
        .await
        .expect("save the chat model preference");

        assert_eq!(
            crate::core::PreferenceStore::resolved_chat_model(&pool, "claude-opus-5").await,
            "claude-opus-5[1m]"
        );

        let key = crate::core::prefs::CHAT_MODEL.key();
        crate::core::PreferenceStore::set(&pool, &bus, key, "  ", None)
            .await
            .expect("blank the chat model preference");
        assert_eq!(
            crate::core::PreferenceStore::resolved_chat_model(&pool, "claude-opus-5").await,
            "claude-opus-5"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }
}
