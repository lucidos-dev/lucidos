//! HTTP consent endpoint for the chat MCP permission lane — the MCP counterpart
//! of `command_permission::submit_command_consent`. The `PermissionCard`
//! rendered for an `McpPermissionRequested` event POSTs here; the handler wakes
//! the in-process agentic loop blocked on the entry, records any "Always allow"
//! grant to `mcp-allowed-tools` (or the per-thread session set), and emits the
//! paired `McpPermissionResolved`.

use super::*;
use crate::engine::claude_code::AllowScope;
use crate::engine::mcp_permission::resolve_mcp_permission;

#[derive(Deserialize)]
pub(super) struct McpPermissionConsentRequest {
    pub request_id: String,
    pub allowed: bool,
    /// When `Some(_)` and `allowed`, the engine records the derived
    /// `Mcp(server:tool)` / `Mcp(server:*)` pattern so future matching calls
    /// skip the prompt: `session` into the in-memory per-thread allow set;
    /// `narrow` / `broad` into `<workspace>/.lucidos/mcp-allowed-tools`. Absent
    /// (Allow-once) records nothing. Unknown wire values 4xx via serde — match
    /// the engine's typed enum exactly.
    #[serde(default)]
    pub persist_scope: Option<AllowScope>,
}

/// POST /api/v1/mcp-permission/consent — resolve a chat MCP permission card.
/// Mirrors the command-guard consent endpoint; the waiter is the in-process
/// agentic loop (via the entry's broadcast).
///
/// Thin: resolving one is `mcp_permission::resolve_mcp_permission`, which a
/// spoken answer reaches too. This adds the actor and the wire shape.
pub(super) async fn submit_mcp_permission_consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<McpPermissionConsentRequest>,
) -> impl IntoResponse {
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    let answered = resolve_mcp_permission(
        &state.engine,
        body.request_id,
        body.allowed,
        body.persist_scope,
        actor,
        "[McpPermission] McpPermissionResolved",
    )
    .await;
    if !answered {
        // Already resolved (superseded / orphan-recovery / canceled) or unknown.
        return (
            StatusCode::NOT_FOUND,
            "No pending MCP permission with that ID",
        )
            .into_response();
    }
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
