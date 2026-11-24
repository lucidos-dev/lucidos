//! HTTP consent endpoint for the command-guard permission lane (ADR 0002,
//! Phase 2) — the chat counterpart of `mcp::submit_mcp_consent`. The
//! `PermissionCard` rendered for a `CommandPermissionRequested` event POSTs
//! here; the handler wakes the in-process agentic loop blocked on the entry,
//! records any "Always allow" grant, and emits the paired
//! `CommandPermissionResolved`.

use super::*;
use crate::engine::command_permission::resolve_command_permission;
use crate::engine::permission::tool_grants::AnsweredBy;
use crate::engine::thread_events::AllowScope;

#[derive(Deserialize)]
pub(super) struct CommandConsentRequest {
    pub request_id: String,
    pub allowed: bool,
    /// When `Some(_)` and `allowed`, the engine records the derived pattern as
    /// a *tool grant* so future matching commands skip the prompt: `session`
    /// held by the thread, `narrow` / `broad` standing. Absent (Allow once)
    /// records nothing. Unknown wire values 4xx via serde, matching the
    /// engine's typed enum exactly.
    #[serde(default)]
    pub persist_scope: Option<AllowScope>,
    /// Always deny: with `allowed` false, the engine also stores the card's
    /// deny line as a *standing deny*. Ignored on an allow.
    #[serde(default)]
    pub always_deny: bool,
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
    let card_thread = state
        .engine
        .pending_command_permission
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
    let answered = resolve_command_permission(
        &state.engine,
        body.request_id,
        body.allowed,
        body.persist_scope,
        always_deny,
        AnsweredBy::tap(actor),
        "[CommandPermission] CommandPermissionResolved",
    )
    .await;
    let answered = match answered {
        Ok(answered) => answered,
        Err(error) => return (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
    };
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

#[derive(Deserialize)]
struct CommandDenyCheck {
    command: String,
}

/// Pre-execution check for provider tools that can run without an approval
/// request. It never grants a command or dispatches one to the judge.
async fn check_command_deny(
    State(state): State<AppState>,
    Json(body): Json<CommandDenyCheck>,
) -> Json<Option<String>> {
    use crate::engine::permission::standing_deny::{refusal_for, CommandKind};
    Json(refusal_for(&state.pool, CommandKind::Shell, &body.command).await)
}

/// Route for the command-guard permission consent (ADR 0002) — chat mirror
/// of `/mcp/consent`.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/command-permission/consent", post(submit_command_consent))
        .route("/internal/command-deny-check", post(check_command_deny))
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
        assert!(!body.always_deny);

        let body: CommandConsentRequest =
            serde_json::from_str(r#"{"request_id":"r","allowed":false,"always_deny":true}"#)
                .unwrap();
        assert!(body.always_deny);

        // Unknown wire value rejected at the boundary (would 400 in axum).
        assert!(serde_json::from_str::<CommandConsentRequest>(
            r#"{"request_id":"r","allowed":true,"persist_scope":"yolo"}"#
        )
        .is_err());
    }
}
