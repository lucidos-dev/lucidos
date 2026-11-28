use crate::engine::chat::child_follow_up::ChildFollowUpDispatchExt;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::chat::child_follow_up::{
    ChildFollowUpError, FollowUpAck, FollowUpDelivery, FollowUpReach,
};

#[derive(Debug, Deserialize)]
pub(in crate::api) struct FollowUpRequest {
    message: String,
    #[serde(default)]
    event_id: Option<Uuid>,
    #[serde(default)]
    urgent: Option<bool>,
    #[serde(default)]
    caller_workspace: Option<String>,
}

#[derive(Debug, Serialize)]
pub(in crate::api) struct FollowUpResponse {
    child_thread_id: Uuid,
    reach: FollowUpReach,
    child_title: String,
    delivered_to: &'static str,
    detail: &'static str,
}

fn delivered_to_wire(delivery: FollowUpDelivery) -> &'static str {
    match delivery {
        FollowUpDelivery::Running => "running",
        FollowUpDelivery::Interrupted => "interrupted",
        FollowUpDelivery::WaitingForUserAnswer => "waiting-for-user-answer",
        FollowUpDelivery::Held => "held",
        FollowUpDelivery::Revived => "revived",
    }
}

impl From<FollowUpAck> for FollowUpResponse {
    fn from(ack: FollowUpAck) -> Self {
        Self {
            child_thread_id: ack.child_thread_id,
            reach: ack.reach,
            child_title: ack.child_title,
            delivered_to: delivered_to_wire(ack.delivered_to),
            detail: ack.delivered_to.describe(),
        }
    }
}

fn api_error(e: ChildFollowUpError) -> ApiError {
    ApiError::with_code(e.status_code(), e.to_string())
}

pub(in crate::api) async fn follow_up_child(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<FollowUpRequest>,
) -> Result<Json<FollowUpResponse>, ApiError> {
    let child_thread_id = Uuid::parse_str(&thread_id)
        .map_err(|e| ApiError::bad_request(format!("Invalid thread id: {e}")))?;

    let message = body.message.trim();
    if message.is_empty() {
        return Err(ApiError::bad_request(
            "A child follow-up needs a non-empty message. It lands in the \
             child's conversation as a message from you.",
        ));
    }

    let caller_thread_id = match crate::core::subprocess_origin::subprocess_origin(&headers) {
        crate::core::subprocess_origin::SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } => source_thread_id,
        crate::core::subprocess_origin::SubprocessOrigin::NotSubprocess => None,
    };

    state
        .engine
        .follow_up_child_thread(
            caller_thread_id,
            child_thread_id,
            message,
            None,
            body.event_id,
            body.caller_workspace.as_deref(),
            crate::engine::FollowUpUrgency::from_flag(body.urgent),
        )
        .await
        .map(|ack| Json(ack.into()))
        .map_err(api_error)
}

#[cfg(test)]
#[path = "follow_up_tests.rs"]
mod tests;
