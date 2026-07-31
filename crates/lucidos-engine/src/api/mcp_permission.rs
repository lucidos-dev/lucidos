//! HTTP consent endpoint for the chat MCP permission lane — the MCP counterpart
//! of `command_permission::submit_command_consent`. The `PermissionCard`
//! rendered for an `McpPermissionRequested` event POSTs here; the handler wakes
//! the in-process agentic loop blocked on the entry, records any "Always allow"
//! grant to `mcp-allowed-tools` (or the per-thread session set), and emits the
//! paired `McpPermissionResolved`.

use super::*;
use crate::engine::claude_code::AllowScope;
use crate::engine::mcp_permission::{
    emit_mcp_permission_resolved, record_mcp_allow_grant, DENIAL_REASON,
};
use crate::engine::thread_events::EventMeta;

#[derive(Deserialize)]
pub(super) struct McpPermissionConsentRequest {
    pub request_id: String,
    pub allowed: bool,
    /// When `Some(_)` and `allowed`, the engine records the derived
    /// `Mcp(server:tool)` / `Mcp(server:*)` pattern so future matching calls
    /// skip the prompt: `session` into the in-memory per-thread allow set;
    /// `narrow` / `broad` into `~/.lucidos/mcp-allowed-tools`. Absent
    /// (Allow-once) records nothing. Unknown wire values 4xx via serde — match
    /// the engine's typed enum exactly.
    #[serde(default)]
    pub persist_scope: Option<AllowScope>,
}

/// POST /api/v1/mcp-permission/consent — resolve a chat MCP permission card.
/// Mirrors the command-guard consent endpoint; the waiter is the in-process
/// agentic loop (via the entry's broadcast).
pub(super) async fn submit_mcp_permission_consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<McpPermissionConsentRequest>,
) -> impl IntoResponse {
    let entry = {
        let mut pending = state.engine.pending_mcp_permission.lock().unwrap();
        pending.take(&body.request_id)
    };
    let Some(entry) = entry else {
        // Already resolved (superseded / orphan-recovery / canceled) or unknown.
        return (
            StatusCode::NOT_FOUND,
            "No pending MCP permission with that ID",
        )
            .into_response();
    };

    // Wake the blocked loop (and any deduped waiters on the same broadcast).
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
        // `entry.tool_name` is the namespaced `mcp__<server>__<tool>` — recover
        // (server_id, tool) to derive the stored pattern. A malformed name (no
        // valid parse) records nothing.
        if let Some((server_id, tool)) =
            crate::mcp::McpManager::parse_mcp_tool_name(&entry.tool_name)
        {
            record_mcp_allow_grant(&state.engine, entry.thread_id, &server_id, &tool, scope);
        }
    }

    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    emit_mcp_permission_resolved(
        &state.engine.event_bus,
        entry.thread_id,
        body.request_id,
        body.allowed,
        reason,
        persist_scope,
        EventMeta::with_actor(actor),
        "[McpPermission] McpPermissionResolved",
    )
    .await;
    StatusCode::OK.into_response()
}

/// Route for the chat MCP permission consent — chat mirror of `/mcp/consent`.
pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/mcp-permission/consent",
        post(submit_mcp_permission_consent),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_permission_consent_deserializes_persist_scope_as_typed_enum() {
        let body: McpPermissionConsentRequest =
            serde_json::from_str(r#"{"request_id":"r","allowed":true,"persist_scope":"narrow"}"#)
                .unwrap();
        assert_eq!(body.persist_scope, Some(AllowScope::Narrow));

        let body: McpPermissionConsentRequest =
            serde_json::from_str(r#"{"request_id":"r","allowed":true,"persist_scope":"broad"}"#)
                .unwrap();
        assert_eq!(body.persist_scope, Some(AllowScope::Broad));

        let body: McpPermissionConsentRequest =
            serde_json::from_str(r#"{"request_id":"r","allowed":true,"persist_scope":"session"}"#)
                .unwrap();
        assert_eq!(body.persist_scope, Some(AllowScope::Session));

        let body: McpPermissionConsentRequest =
            serde_json::from_str(r#"{"request_id":"r","allowed":false}"#).unwrap();
        assert_eq!(body.persist_scope, None);

        // Unknown wire value rejected at the boundary (would 400 in axum).
        assert!(serde_json::from_str::<McpPermissionConsentRequest>(
            r#"{"request_id":"r","allowed":true,"persist_scope":"yolo"}"#
        )
        .is_err());
    }
}
