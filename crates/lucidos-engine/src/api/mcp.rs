use super::*;
use crate::engine::cc_permission::{PermissionEntry, DENIAL_REASON};
use crate::engine::claude_code::{append_allowed_tool_pattern, derive_allow_pattern, AllowScope};
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{EventMeta, ThreadEvent};

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
                crate::log!(
                    "[MCP] Failed to persist allow pattern {:?}: {}",
                    pattern,
                    e
                );
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

/// POST /api/v1/mcp/consent — Respond to an MCP tool consent request.
///
/// Two distinct flows share this endpoint:
///   1. CC permission prompts (deduped, multi-listener) — `pending_cc_permission`
///   2. Legacy agentic-loop MCP consent (single oneshot)   — `pending_mcp_consent`
///
/// CC's flow also owns the paired `CodingAgentPermissionResolved` event so it
/// fires once per click rather than once per deduped HTTP listener.
pub(super) async fn submit_mcp_consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<McpConsentResponse>,
) -> impl IntoResponse {
    let cc_entry = {
        let mut pending = state.engine.pending_cc_permission.lock().unwrap();
        pending.take(&body.request_id)
    };
    if let Some(entry) = cc_entry {
        let _ = entry.tx.send(body.allowed);
        let reason = if body.allowed {
            None
        } else {
            Some(DENIAL_REASON.to_string())
        };
        let persist_scope = if body.allowed { body.persist_scope } else { None };
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
        return StatusCode::OK.into_response();
    }

    let sender = {
        let mut pending = state.engine.pending_mcp_consent.lock().unwrap();
        pending.remove(&body.request_id)
    };
    match sender {
        Some(tx) => {
            let _ = tx.send(body.allowed);
            StatusCode::OK.into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            "No pending consent request with that ID",
        )
            .into_response(),
    }
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

/// GET /api/v1/mcp/servers — List MCP servers with status.
pub(super) async fn list_mcp_servers(State(state): State<AppState>) -> impl IntoResponse {
    match state.engine.mcp_manager.list_servers().await {
        Ok(servers) => Json(serde_json::json!({ "servers": servers })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed: {}", e)).into_response(),
    }
}

/// Routes for the `/mcp/*` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/mcp/consent", post(submit_mcp_consent))
        .route("/mcp/auto-approve", put(set_mcp_auto_approve))
        .route("/mcp/servers", get(list_mcp_servers))
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
}
