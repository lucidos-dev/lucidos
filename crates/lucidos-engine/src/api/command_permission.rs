//! HTTP consent endpoint for the command-guard permission lane (ADR 0002,
//! Phase 2) — the chat counterpart of `mcp::submit_mcp_consent`. The
//! `PermissionCard` rendered for a `CommandPermissionRequested` event POSTs
//! here; the handler wakes the in-process agentic loop blocked on the entry,
//! records any "Always allow" grant, and emits the paired
//! `CommandPermissionResolved`.

use super::*;
use crate::engine::claude_code::AllowScope;
use crate::engine::command_guard;
use crate::engine::command_permission::{
    emit_command_permission_resolved, record_command_allow_grant, DENIAL_REASON,
};
use crate::engine::thread_events::EventMeta;

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
pub(super) async fn submit_command_consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CommandConsentRequest>,
) -> impl IntoResponse {
    let entry = {
        let mut pending = state.engine.pending_command_permission.lock().unwrap();
        pending.take(&body.request_id)
    };
    let Some(entry) = entry else {
        // Already resolved (superseded / orphan-recovery / canceled) or unknown.
        return (
            StatusCode::NOT_FOUND,
            "No pending command permission with that ID",
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
    let persist_scope = if body.allowed { body.persist_scope } else { None };
    if let Some(scope) = persist_scope {
        let command = command_guard::command_text(&entry.tool_name, &entry.input).unwrap_or_default();
        record_command_allow_grant(&state.engine, entry.thread_id, &entry.tool_name, command, scope);
    }

    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    emit_command_permission_resolved(
        &state.engine.event_bus,
        entry.thread_id,
        body.request_id,
        body.allowed,
        reason,
        persist_scope,
        EventMeta::with_actor(actor),
        "[CommandPermission] CommandPermissionResolved",
    )
    .await;
    StatusCode::OK.into_response()
}

/// Route for the command-guard permission consent (ADR 0002) — chat mirror
/// of `/mcp/consent`.
pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/command-permission/consent",
        post(submit_command_consent),
    )
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
