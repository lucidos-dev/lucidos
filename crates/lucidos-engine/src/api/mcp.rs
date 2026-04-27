use super::*;
use crate::engine::cc_permission::DENIAL_REASON;
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{EventMeta, ThreadEvent};

#[derive(Deserialize)]
pub(super) struct McpConsentResponse {
    pub request_id: String,
    pub allowed: bool,
}

/// POST /api/mcp/consent — Respond to an MCP tool consent request.
///
/// Two distinct flows share this endpoint:
///   1. CC permission prompts (deduped, multi-listener) — `pending_cc_permission`
///   2. Legacy agentic-loop MCP consent (single oneshot)   — `pending_mcp_consent`
///
/// CC's flow also owns the paired `CodingAgentPermissionResolved` event so it
/// fires once per click rather than once per deduped HTTP listener.
pub(super) async fn submit_mcp_consent(
    State(state): State<AppState>,
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
                    },
                    meta: EventMeta::NONE,
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

/// PUT /api/mcp/auto-approve — Set auto-approve for an MCP server.
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

/// GET /api/mcp/servers — List MCP servers with status.
pub(super) async fn list_mcp_servers(State(state): State<AppState>) -> impl IntoResponse {
    match state.engine.mcp_manager.list_servers().await {
        Ok(servers) => Json(serde_json::json!({ "servers": servers })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed: {}", e)).into_response(),
    }
}
