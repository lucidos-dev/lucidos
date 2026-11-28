use crate::engine::chat::child_detach::ChildDetachExt;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde::Serialize;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::core::subprocess_origin::SubprocessOrigin;
use crate::engine::chat::child_detach::{ChildDetachError, DetachAck, DetachCaller};
use crate::engine::thread_events::EventMeta;

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
    let caller = match crate::core::subprocess_origin::subprocess_origin(&headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } => DetachCaller::Agent(source_thread_id),
        SubprocessOrigin::NotSubprocess => DetachCaller::User,
    };
    let actor = crate::api::actor::user_actor(&headers, None);

    state
        .engine
        .detach_child_thread(caller, target, EventMeta::with_actor(actor))
        .await
        .map(|ack| Json(ack.into()))
        .map_err(api_error)
}
