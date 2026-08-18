use super::error::ApiError;
use super::settings::{AllowlistRequest, AllowlistResponse};
use super::*;
use crate::core::mcp_servers::validate_server_id;
use crate::core::{McpServer, McpServerStore};
use crate::engine::cc_permission::{PermissionEntry, DENIAL_REASON};
use crate::engine::claude_code::{append_allowed_tool_pattern, derive_allow_pattern, AllowScope};
use crate::engine::event_bus::BusEvent;
use crate::engine::mcp_permission::{read_mcp_allowed_tools_file, write_mcp_allowed_tools_file};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::mcp::{McpCostTotals, McpStartOutcome, McpStopOutcome};

/// Dispatch an "Always allow" grant to the right storage. `Narrow` / `Broad`
/// append to `~/.lucidos/cc-allowed-tools` (CC reads it on next spawn);
/// `Session` records into the per-thread in-memory allow set the engine
/// checks before each prompt. Returns silently for tools whose scope yields
/// no derivable pattern (e.g. `Edit` with `Broad` — `BROAD_ALLOW_INEFFECTIVE`).
fn record_allow_grant(state: &AppState, entry: &PermissionEntry, scope: AllowScope) {
    let Some(pattern) = derive_allow_pattern(&entry.tool_name, &entry.input, scope) else {
        return;
    };
    match scope {
        AllowScope::Session => {
            let mut pending = state.engine.pending_cc_permission.lock().unwrap();
            pending.allow_session(entry.thread_id, pattern);
        }
        AllowScope::Narrow | AllowScope::Broad => {
            if let Err(e) = append_allowed_tool_pattern(state.engine.user_dir(), &pattern) {
                crate::log!("[MCP] Failed to persist allow pattern {:?}: {}", pattern, e);
            }
        }
    }
}

#[derive(Deserialize)]
pub(super) struct McpConsentResponse {
    pub request_id: String,
    pub allowed: bool,
    /// When `Some(_)` and `allowed == true`, the engine derives a pattern
    /// from the original prompt's tool_name + input and remembers it so
    /// future identical-pattern requests skip the prompt. Where the pattern
    /// is recorded depends on scope:
    ///   * `narrow` / `broad` — appended to `~/.lucidos/cc-allowed-tools`
    ///     and handed to CC via `--allowedTools` on every spawn.
    ///   * `session` — inserted into the engine's in-memory per-thread
    ///     allow set; lost on engine restart but works for tools/paths CC
    ///     itself never auto-approves (notably `.claude/` and `.git/`).
    ///
    /// Absent (the Allow-once path) records nothing. Unknown wire values
    /// cause a 4xx via serde — match the engine's typed enum exactly.
    #[serde(default)]
    pub persist_scope: Option<AllowScope>,
}

/// POST /api/v1/mcp/consent — Respond to a coding-agent (Claude Code / Codex)
/// permission prompt. Resolves the deduped, multi-listener entry in
/// `pending_cc_permission` and owns the paired `CodingAgentPermissionResolved`
/// event so it fires once per click rather than once per deduped HTTP listener.
///
/// The chat MCP permission lane has its own endpoint
/// (`/api/v1/mcp-permission/consent`); this one is coding-agent-only.
pub(super) async fn submit_mcp_consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<McpConsentResponse>,
) -> impl IntoResponse {
    let cc_entry = {
        let mut pending = state.engine.pending_cc_permission.lock().unwrap();
        pending.take(&body.request_id)
    };
    let Some(entry) = cc_entry else {
        return (
            StatusCode::NOT_FOUND,
            "No pending consent request with that ID",
        )
            .into_response();
    };
    let _ = entry.tx.send(body.allowed);
    let reason = if body.allowed {
        None
    } else {
        Some(DENIAL_REASON.to_string())
    };
    let persist_scope = if body.allowed {
        body.persist_scope
    } else {
        None
    };
    if let Some(scope) = persist_scope {
        record_allow_grant(&state, &entry, scope);
    }
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    state
        .engine
        .event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id: entry.thread_id,
                event: ThreadEvent::CodingAgentPermissionResolved {
                    request_id: body.request_id,
                    allowed: body.allowed,
                    reason,
                    persist_scope,
                },
                meta: EventMeta::with_actor(actor),
            },
            "[MCP] CodingAgentPermissionResolved",
        )
        .await;
    StatusCode::OK.into_response()
}

#[derive(Deserialize)]
pub(super) struct McpAutoApproveRequest {
    pub server_id: String,
    pub auto_approve: bool,
}

/// PUT /api/v1/mcp/auto-approve — Set auto-approve for an MCP server.
pub(super) async fn set_mcp_auto_approve(
    State(state): State<AppState>,
    Json(body): Json<McpAutoApproveRequest>,
) -> impl IntoResponse {
    match state
        .engine
        .mcp_manager
        .set_auto_approve(&body.server_id, body.auto_approve)
        .await
    {
        Ok(msg) => Json(serde_json::json!({ "success": true, "message": msg })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed: {}", e)).into_response(),
    }
}

/// The model a new Lucidos Agent thread would run with: the account's
/// `chat_model` preference, else the provider default.
///
/// Mirrors the chat path's own order, so the page states the window the request
/// packer will size against. `LlmProvider::default_model()` alone is NOT that
/// window. It is `LUCIDOS_MODEL` or the compiled-in `DEFAULT_CHAT_MODEL`, fixed
/// at boot, so it drops the `[1m]` marker a saved preference carries. The page
/// reported a 1M model's tools as a share of 200k.
async fn resolved_chat_model(pool: &PgPool, provider_default: &str) -> String {
    crate::core::PreferenceStore::user_chat_settings(pool)
        .await
        .0
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| provider_default.to_string())
}

/// GET /api/v1/mcp/servers: list MCP servers with status and context cost.
///
/// Cost is computed here rather than client-side, through the same
/// `tool_definitions_chars` and `estimate_tokens_from_chars` the request packer
/// and the Context Viewer use. A second ratio in the frontend is what once
/// reported a 205k prompt as 361k.
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

    // The window the resolved model actually has, so the page can state the
    // share of it these tools occupy.
    let model =
        resolved_chat_model(&state.pool, state.engine.current_provider().default_model()).await;
    let context_window = state.engine.context_window_for(&model);

    Ok(Json(serde_json::json!({
        "servers": servers,
        "totals": totals,
        "model": model,
        "context_window": context_window,
    })))
}

/// The registry row for `id`, or a 404.
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

/// The error for a server whose STORED id cannot ride a wire tool name, or
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

/// The error for a start that reached the process and failed there.
///
/// 502, because the failure is the upstream server's: a missing binary, a bad
/// command, an app that is not running. A 500 would read as an engine bug and
/// hide the one line that says what to fix.
fn start_failed_error(id: &str, e: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        format!("MCP server '{id}' failed to start: {e}"),
    )
}

/// POST /api/v1/mcp/servers/:id/start: start a registered server.
pub(super) async fn start_mcp_server(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let server = load_server(&state, &id).await?;
    if let Some(e) = unusable_id_error(&server) {
        return Err(e);
    }

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

/// POST /api/v1/mcp/servers/:id/stop: stop a running server.
///
/// Stopping one that is already stopped is a 200, not an error: the caller
/// asked for a state and got it. Only an unknown id is a 404.
pub(super) async fn stop_mcp_server(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    load_server(&state, &id).await?;

    let outcome = state
        .engine
        .mcp_manager
        .stop_server(&id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to stop MCP server '{id}': {e}")))?;

    Ok(Json(serde_json::json!({
        "running": false,
        "was_running": matches!(outcome, McpStopOutcome::Stopped { .. }),
    })))
}

/// DELETE /api/v1/mcp/servers/:id: remove a server, stopping it first.
pub(super) async fn remove_mcp_server(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // The store emits `McpServerRemoved` from inside its write path, so the
    // device actor is resolved up front and passed down rather than going
    // through `emit_user_system`.
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
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
pub(super) struct McpDisabledToolsRequest {
    /// The full set to store, by WIRE name (`mcp__<server>__<tool>`). A
    /// replacement, not a delta, so a stale client cannot re-enable a tool it
    /// never knew about.
    disabled_tools: Vec<String>,
}

/// PUT /api/v1/mcp/servers/:id/disabled-tools: set which tools are switched
/// off.
///
/// Names are not checked against the manifest. A stopped server may have none
/// to check against, and a name matching no tool simply filters nothing.
pub(super) async fn set_mcp_disabled_tools(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<McpDisabledToolsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    let stored = state
        .engine
        .mcp_manager
        .set_disabled_tools(&id, &body.disabled_tools, actor)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to update MCP server '{id}': {e}")))?
        .ok_or_else(|| ApiError::not_found(format!("MCP server '{id}' not found")))?;

    Ok(Json(serde_json::json!({ "disabled_tools": stored })))
}

/// GET /api/v1/mcp-allowed-tools: the raw `~/.lucidos/mcp-allowed-tools`, the
/// chat MCP permission allowlist. The MCP counterpart of the
/// `cc-allowed-tools` pair in `api/settings.rs`; a missing file returns the
/// seeded header.
pub(super) async fn get_mcp_allowed_tools(
    State(state): State<AppState>,
) -> Result<Json<AllowlistResponse>, ApiError> {
    let dir = state.engine.user_dir().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "User directory not configured",
        )
    })?;
    let contents = read_mcp_allowed_tools_file(dir)
        .map_err(|e| ApiError::internal(format!("Failed to read mcp-allowed-tools: {e}")))?;
    Ok(Json(AllowlistResponse { contents }))
}

/// PUT /api/v1/mcp-allowed-tools: overwrite the file (atomic). The gate reads
/// it fresh on each prompt, so an edit takes effect on the next gated MCP call.
pub(super) async fn put_mcp_allowed_tools(
    State(state): State<AppState>,
    Json(body): Json<AllowlistRequest>,
) -> Result<StatusCode, ApiError> {
    let dir = state.engine.user_dir().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "User directory not configured",
        )
    })?;
    write_mcp_allowed_tools_file(dir, &body.contents)
        .map_err(|e| ApiError::internal(format!("Failed to write mcp-allowed-tools: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Routes for the `/mcp/*` surface, plus the `mcp-allowed-tools` editor.
///
/// The per-server verbs take the id as a path param. `auto-approve` keeps its
/// body form so the client that already calls it does not break.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/mcp/consent", post(submit_mcp_consent))
        .route("/mcp/auto-approve", put(set_mcp_auto_approve))
        .route("/mcp/servers", get(list_mcp_servers))
        .route("/mcp/servers/:id", delete(remove_mcp_server))
        .route("/mcp/servers/:id/start", post(start_mcp_server))
        .route("/mcp/servers/:id/stop", post(stop_mcp_server))
        .route(
            "/mcp/servers/:id/disabled-tools",
            put(set_mcp_disabled_tools),
        )
        .route(
            "/mcp-allowed-tools",
            get(get_mcp_allowed_tools).put(put_mcp_allowed_tools),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

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

        // Unknown wire value rejected at the boundary (would 400 in axum).
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

    /// The three per-server failures map to three different statuses, and the
    /// page renders three different things. Collapsing the unusable id into the
    /// start failure is what left the reported server showing a generic error
    /// the user could only retry.
    #[test]
    fn the_three_per_server_failures_stay_distinct() {
        // A usable id raises nothing, so the request proceeds to the process.
        assert!(unusable_id_error(&server_with_id("backstage")).is_none());

        // A stored id that cannot ride a wire tool name is 422, not 502: no
        // retry will ever help, and the message has to say so.
        let unusable =
            unusable_id_error(&server_with_id("back.stage")).expect("a dotted id is unusable");
        assert_eq!(unusable.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(unusable.message.contains("back.stage"));
        assert!(
            unusable.message.contains("remove it"),
            "the message has to name the only action that works: {}",
            unusable.message
        );

        // A start that reached the process and failed is 502, carrying the
        // process error verbatim so the user can see what to fix.
        let failed = start_failed_error("slack", "Failed to spawn MCP server 'npx': not found");
        assert_eq!(failed.status, StatusCode::BAD_GATEWAY);
        assert!(failed.message.contains("not found"));
        assert!(failed.message.contains("slack"));

        // And an unknown id is a 404, which is what stops a DELETE on nothing
        // reporting success.
        let missing = ApiError::not_found(format!("MCP server '{}' not found", "gone"));
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
    }

    /// An over-long id is refused for a different reason than a dotted one, and
    /// both are the same class of unusable. Each message names its own cause.
    #[test]
    fn an_over_long_id_is_unusable_for_its_own_stated_reason() {
        let long = "a".repeat(crate::core::mcp_servers::MAX_SERVER_ID_LEN + 1);
        let e = unusable_id_error(&server_with_id(&long)).expect("too long to carry");
        assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(e.message.contains("characters"), "{}", e.message);
    }

    /// The disabled set is a replacement, so the body has to carry the whole
    /// selection. An empty array is a valid selection (nothing off), not a
    /// missing field.
    #[test]
    fn disabled_tools_body_takes_a_full_set() {
        let body: McpDisabledToolsRequest =
            serde_json::from_str(r#"{"disabled_tools":["mcp__slack__post_message"]}"#).unwrap();
        assert_eq!(body.disabled_tools, vec!["mcp__slack__post_message"]);

        let cleared: McpDisabledToolsRequest =
            serde_json::from_str(r#"{"disabled_tools":[]}"#).unwrap();
        assert!(cleared.disabled_tools.is_empty());

        // Omitting it entirely is rejected rather than read as "disable
        // nothing", which would silently re-enable every tool.
        assert!(serde_json::from_str::<McpDisabledToolsRequest>("{}").is_err());
    }

    /// The saved preference wins over the boot-time provider default, and the
    /// `[1m]` marker it carries is what decides the window.
    ///
    /// The page named the provider default instead. A user on `…@default[1m]`
    /// was told their tools ate a share of 200k, against a request the packer
    /// sizes at 1M.
    #[tokio::test]
    async fn the_page_names_the_saved_chat_model_not_the_boot_default() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());

        // Unset: the provider default is all there is to report.
        assert_eq!(
            resolved_chat_model(&pool, "claude-opus-5@default").await,
            "claude-opus-5@default"
        );

        crate::core::PreferenceStore::set(
            &pool,
            &bus,
            crate::core::PREF_CHAT_MODEL,
            "claude-opus-5@default[1m]",
            None,
        )
        .await
        .expect("save the chat model preference");

        assert_eq!(
            resolved_chat_model(&pool, "claude-opus-5@default").await,
            "claude-opus-5@default[1m]"
        );

        // Blank is unset, not a model id: a stored empty string must not name
        // the page's model as "".
        crate::core::PreferenceStore::set(&pool, &bus, crate::core::PREF_CHAT_MODEL, "  ", None)
            .await
            .expect("blank the chat model preference");
        assert_eq!(
            resolved_chat_model(&pool, "claude-opus-5@default").await,
            "claude-opus-5@default"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }
}
