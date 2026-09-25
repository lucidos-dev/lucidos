//! `POST /api/v1/threads/:thread_id/detach`: move a child thread to top level
//! (ADR 0278).
//!
//! Thin by design, like the follow-up route. The ladder and the emit live in
//! `engine::chat::child_detach`. This handler decides WHO is asking and maps
//! the typed refusal onto its status.
//!
//! Who is asking comes from the verified origin token, never from the body:
//!
//! - **A token-bearing caller** (an agent, or the CLI inside an agent session)
//!   moves only its own direct children.
//! - **A caller with no token** is the user's device or the local API. It moves
//!   any thread that has a parent, as the Archive button archives any thread.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde::Serialize;
use uuid::Uuid;

use crate::api::actor::SubprocessOrigin;
use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::thread_events::EventMeta;
use crate::engine::{ChildDetachError, DetachAck, DetachCaller};

#[derive(Debug, Serialize)]
pub(in crate::api) struct DetachResponse {
    child_thread_id: Uuid,
    child_title: String,
    former_parent_thread_id: Uuid,
}

impl From<DetachAck> for DetachResponse {
    fn from(ack: DetachAck) -> Self {
        Self {
            child_thread_id: ack.child_thread_id,
            child_title: ack.child_title,
            former_parent_thread_id: ack.former_parent_id,
        }
    }
}

fn api_error(e: ChildDetachError) -> ApiError {
    ApiError::with_code(e.status_code(), e.to_string())
}

pub(in crate::api) async fn detach_thread(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<DetachResponse>, ApiError> {
    let target = Uuid::parse_str(&thread_id)
        .map_err(|e| ApiError::bad_request(format!("Invalid thread id: {e}")))?;
    let caller = match crate::api::actor::subprocess_origin(&headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } => DetachCaller::Agent(source_thread_id),
        SubprocessOrigin::NotSubprocess => DetachCaller::User,
    };
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;

    state
        .engine
        .detach_child_thread(caller, target, EventMeta::with_actor(actor))
        .await
        .map(|ack| Json(ack.into()))
        .map_err(api_error)
}
