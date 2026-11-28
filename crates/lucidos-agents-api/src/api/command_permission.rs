use super::*;
use crate::engine::command_permission::resolve_command_permission;
use crate::engine::permission::tool_grants::AnsweredBy;
use crate::engine::thread_events::AllowScope;

#[derive(Deserialize)]
pub(super) struct CommandConsentRequest {
    pub request_id: String,
    pub allowed: bool,
    #[serde(default)]
    pub persist_scope: Option<AllowScope>,
    #[serde(default)]
    pub always_deny: bool,
}

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

async fn check_command_deny(
    State(state): State<AppState>,
    Json(body): Json<CommandDenyCheck>,
) -> Json<Option<String>> {
    use crate::engine::permission::standing_deny::{refusal_for, CommandKind};
    Json(refusal_for(&state.pool, CommandKind::Shell, &body.command).await)
}

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

        assert!(serde_json::from_str::<CommandConsentRequest>(
            r#"{"request_id":"r","allowed":true,"persist_scope":"yolo"}"#
        )
        .is_err());
    }
}
