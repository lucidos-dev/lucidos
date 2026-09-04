//! HTTP consent endpoint for the command-guard permission lane (ADR 0002,
//! Phase 2) — the chat counterpart of `mcp::submit_mcp_consent`. The
//! `PermissionCard` rendered for a `CommandPermissionRequested` event POSTs
//! here; the handler wakes the in-process agentic loop blocked on the entry,
//! records any "Always allow" grant, and emits the paired
//! `CommandPermissionResolved`.

use super::*;
use crate::engine::claude_code::AllowScope;
use crate::engine::command_permission::resolve_command_permission;

#[derive(Deserialize)]
pub(super) struct CommandConsentRequest {
    pub request_id: String,
    pub allowed: bool,
    /// When `Some(_)` and `allowed`, the engine records the derived pattern so
    /// future matching commands skip the prompt: `session` into the in-memory
    /// per-thread allow set; `narrow` / `broad` into `~/.lucidos/
    /// agent-allowed-commands`. Absent (Allow-once) records nothing. Unknown
    /// wire values 4xx via serde — match the engine's typed enum exactly.
    #[serde(default)]
    pub persist_scope: Option<AllowScope>,
}

/// POST /api/v1/command-permission/consent — resolve a command-guard
/// permission card. Mirrors the MCP consent endpoint, but the waiter is the
/// in-process agentic loop (via the entry's broadcast) rather than an MCP HTTP
/// handler, so there is no legacy oneshot fallback.
///
/// Thin: resolving one is `command_permission::resolve_command_permission`,
/// which a spoken answer reaches too. This adds the actor and the wire shape.
pub(super) async fn submit_command_consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CommandConsentRequest>,
) -> impl IntoResponse {
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    let answered = resolve_command_permission(
        &state.engine,
        body.request_id,
        body.allowed,
        body.persist_scope,
        actor,
        "[CommandPermission] CommandPermissionResolved",
    )
    .await;
    if !answered {
        // Already resolved (superseded / orphan-recovery / canceled) or unknown.
        return (
            StatusCode::NOT_FOUND,
            "No pending command permission with that ID",
        )
            .into_response();
    }
    StatusCode::OK.into_response()
}

/// Route for the command-guard permission consent (ADR 0002) — chat mirror
/// of `/mcp/consent`.
pub(super) fn router() -> Router<AppState> {
    Router::new().route("/command-permission/consent", post(submit_command_consent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_consent_deserializes_persist_scope_as_typed_enum() {
        let body: CommandConsentRequest =
            serde_json::from_str(r#"{"request_id":"r","allowed":true,"persist_scope":"narrow"}"#)
                .unwrap();
        assert_eq!(body.persist_scope, Some(AllowScope::Narrow));

        let body: CommandConsentRequest =
            serde_json::from_str(r#"{"request_id":"r","allowed":true,"persist_scope":"session"}"#)
                .unwrap();
        assert_eq!(body.persist_scope, Some(AllowScope::Session));

        let body: CommandConsentRequest =
            serde_json::from_str(r#"{"request_id":"r","allowed":false}"#).unwrap();
        assert_eq!(body.persist_scope, None);

        // Unknown wire value rejected at the boundary (would 400 in axum).
        assert!(serde_json::from_str::<CommandConsentRequest>(
            r#"{"request_id":"r","allowed":true,"persist_scope":"yolo"}"#
        )
        .is_err());
    }
}
