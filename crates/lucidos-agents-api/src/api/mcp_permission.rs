use super::*;
use crate::engine::mcp_permission::resolve_mcp_permission;
use crate::engine::permission::tool_grants::AnsweredBy;
use crate::engine::thread_events::AllowScope;

#[derive(Deserialize)]
pub(super) struct McpPermissionConsentRequest {
    pub request_id: String,
    pub allowed: bool,
    #[serde(default)]
    pub persist_scope: Option<AllowScope>,
}

pub(super) async fn submit_mcp_permission_consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<McpPermissionConsentRequest>,
) -> impl IntoResponse {
    let card_thread = state
        .engine
        .pending_mcp_permission
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
    let actor = super::actor::user_actor(&headers, None);
    let answered = resolve_mcp_permission(
        &state.engine,
        body.request_id,
        body.allowed,
        body.persist_scope,
        AnsweredBy::tap(actor),
        "[McpPermission] McpPermissionResolved",
    )
    .await;
    if !answered {
        return (
            StatusCode::NOT_FOUND,
            "No pending MCP permission with that ID",
        )
            .into_response();
    }
    StatusCode::OK.into_response()
}

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

        assert!(serde_json::from_str::<McpPermissionConsentRequest>(
            r#"{"request_id":"r","allowed":true,"persist_scope":"yolo"}"#
        )
        .is_err());
    }
}
