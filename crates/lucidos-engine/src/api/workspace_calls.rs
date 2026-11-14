//! `POST /api/v1/workspace-calls/{target}/chat/stream`: a thread's own
//! subprocess spawns into another workspace through this engine.
//!
//! The subprocess cannot prove its thread to the target, since its origin token
//! verifies only here. So this engine verifies the token and asks its own spawn
//! gate for a verdict on the calling thread. It signs both for its gateway
//! (`engine::http::workspace_client`), which stamps the same claim for the
//! target. The target honors the verdict.
//!
//! A caller with no origin token is refused: it has no thread to vouch for.
//! It posts to the target directly instead, where it is unattested.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use uuid::Uuid;

use super::actor::{build_message_origin, subprocess_origin, SubprocessOrigin};
use super::error::ApiError;
use super::AppState;
use crate::engine::http::workspace_client::{
    post_through_gateway, ThroughGateway, WorkspaceCallCtx,
};
use crate::engine::thread_events::ActorMode;

/// The `reason` for a call that presented no origin token.
const REASON_ORIGIN_TOKEN_REQUIRED: &str = "origin_token_required";

/// The route, below `/api/v1`. It must match what
/// `lucidos_gateway_stamp::workspace_call_path` builds, since the CLI posts there.
const ROUTE: &str = "/workspace-calls/:target/chat/stream";

pub(super) fn router() -> Router<AppState> {
    Router::new().route(ROUTE, post(call_workspace))
}

async fn call_workspace(
    State(state): State<AppState>,
    Path(target): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let gateway_port = super::base_path::gateway_port();
    outbound_call(
        &state.pool,
        &state.engine.trigger_configs,
        gateway_port.as_deref(),
        &target,
        &headers,
        &body,
    )
    .await
}

/// The call itself, with the gateway port given, so a test never reaches the
/// gateway this process inherited.
async fn outbound_call(
    pool: &sqlx::PgPool,
    trigger_configs: &std::sync::RwLock<
        std::collections::HashMap<String, crate::triggers::TriggerConfig>,
    >,
    gateway_port: Option<&str>,
    target: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    let SubprocessOrigin::Subprocess {
        source_thread_id, ..
    } = subprocess_origin(headers)
    else {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "Only a Lucidos-spawned subprocess, holding its thread's origin token, can call \
             another workspace through this engine. Post to the target workspace directly.",
        )
        .with_reason(REASON_ORIGIN_TOKEN_REQUIRED)
        .into_response();
    };
    let body: serde_json::Value = match serde_json::from_slice(body) {
        Ok(value @ serde_json::Value::Object(_)) => value,
        _ => return ApiError::bad_request("The body must be a JSON object.").into_response(),
    };
    let Some(origin) =
        build_message_origin(headers, ActorMode::Agent, None, None, None, None, None)
    else {
        return ApiError::internal("A verified origin token built no origin.").into_response();
    };
    let verdict =
        crate::engine::agent_session::spawn_gate::spawn_verdict(pool, trigger_configs, origin)
            .await;
    let ctx = WorkspaceCallCtx {
        self_workspace: String::new(),
        source_thread_id,
        source_event_id: body["caller_event_id"]
            .as_str()
            .and_then(|id| id.parse::<Uuid>().ok()),
        mode: subprocess_mode(&body),
    };
    match post_through_gateway(gateway_port, target, body, &ctx, &verdict).await {
        Ok(ThroughGateway::Replied(reply)) => {
            let mut response = (reply.status, reply.body).into_response();
            if let Some(content_type) = reply.content_type {
                response
                    .headers_mut()
                    .insert(header::CONTENT_TYPE, content_type);
            }
            response
        }
        Ok(ThroughGateway::Unavailable { reason }) => ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "No gateway carries a call from this workspace to \"{target}\". Post to it \
                 directly: it will treat the call as unattested."
            ),
        )
        .with_reason(reason)
        .into_response(),
        Err(e) => ApiError::new(StatusCode::BAD_GATEWAY, e).into_response(),
    }
}

/// The mode a subprocess's call carries. The body names it, and the gateway
/// signs only the workspace, thread and verdict. So it is never `Human`: a
/// subprocess is not the person, and only a human message answers a card.
fn subprocess_mode(body: &serde_json::Value) -> ActorMode {
    match serde_json::from_value(body["mode"].clone()) {
        Ok(ActorMode::Engine) => ActorMode::Engine,
        _ => ActorMode::Agent,
    }
}

#[cfg(test)]
#[path = "workspace_calls_tests.rs"]
mod tests;
