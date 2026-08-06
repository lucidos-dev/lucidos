//! `POST /api/v1/threads/:thread_id/follow-up`: the HTTP surface of a child
//! follow-up.
//!
//! Thin by design. Every decision lives in
//! `engine::chat::child_follow_up`: the refusal ladder, the delivery sampling,
//! the derived coding-agent routing, and the ack. This handler resolves the
//! caller, calls the one engine function, and maps its typed error onto the
//! status `ChildFollowUpError::status_code` already declares. There is no
//! second delivery path, so a refusal the LLM tool reports and a refusal this
//! route reports can never disagree.
//!
//! ## What the body deliberately does not carry
//!
//! No `mode`, no `parent_thread_id`, no `use_coding_agent`, no `repo_id`, and
//! no caller thread id. Everything the engine can derive it derives, and the
//! two that matter are load-bearing rather than tidy:
//!
//! - **The caller** comes from the thread-bound origin token
//!   (`api::actor::subprocess_origin`), which a subprocess cannot re-point at
//!   another thread. A body field would hand back exactly the capability the
//!   binding removes.
//! - **Coding-agent-ness** comes from the child's own `thread_summaries` row.
//!   A mis-derived flag would send a coding-agent child down the Lucidos
//!   Agent's loop, whose `ResponseGenerated` terminal matches neither
//!   `should_callback` nor `should_decrement`, so the parent would never be
//!   woken and its `active_children_count` would never come down: silent in
//!   both dimensions.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::{ChildFollowUpError, FollowUpAck, FollowUpDelivery};

/// Body of a follow-up request.
///
/// `caller_workspace` is accepted only so it can be REFUSED explicitly: a
/// cross-workspace thread is always top-level, so it has no children, and a
/// silently-ignored field would surface as a confusing `NotYourChild` instead
/// of the real reason.
#[derive(Debug, Deserialize)]
pub(in crate::api) struct FollowUpRequest {
    /// What to say to the child. Lands in its conversation as a message from
    /// the parent.
    message: String,
    /// The parent's originating event, stamped onto the child's message-route
    /// panel so the follow-up links back to the exact tool call. Optional: the
    /// panel resolves the parent by id either way.
    #[serde(default)]
    event_id: Option<Uuid>,
    /// Preempt the child's in-flight turn instead of queueing behind it.
    /// Absent means `false`, so every caller that predates the flag keeps its
    /// behaviour. See `FollowUpUrgency`.
    #[serde(default)]
    urgent: Option<bool>,
    #[serde(default)]
    caller_workspace: Option<String>,
}

/// What the caller gets back the moment the message is on the child's
/// timeline. Never the child's result: the child's turn is not awaited, and
/// its outcome arrives later as an ordinary `ChildThreadCompleted` card on the
/// parent.
#[derive(Debug, Serialize)]
pub(in crate::api) struct FollowUpResponse {
    child_thread_id: Uuid,
    /// The child's human-meaningful handle. Callers name the child by this and
    /// never by uuid: no screen in Lucidos is labelled with a uuid.
    child_title: String,
    /// `running` | `interrupted` | `waiting-for-user-answer` | `revived`.
    /// Kebab-case because it is a public API parameter value (`CLAUDE.md`).
    delivered_to: &'static str,
    /// One sentence saying what that means, so a caller does not have to keep
    /// its own copy of the table.
    detail: &'static str,
}

/// The wire spelling of a delivery mode. Kept beside the response rather than
/// as a `Serialize` impl on `FollowUpDelivery` so the HTTP vocabulary
/// (kebab-case) cannot leak into the engine type, which the LLM tool also
/// renders and does so in prose.
fn delivered_to_wire(delivery: FollowUpDelivery) -> &'static str {
    match delivery {
        FollowUpDelivery::Running => "running",
        FollowUpDelivery::Interrupted => "interrupted",
        FollowUpDelivery::WaitingForUserAnswer => "waiting-for-user-answer",
        FollowUpDelivery::Revived => "revived",
    }
}

impl From<FollowUpAck> for FollowUpResponse {
    fn from(ack: FollowUpAck) -> Self {
        Self {
            child_thread_id: ack.child_thread_id,
            child_title: ack.child_title,
            delivered_to: delivered_to_wire(ack.delivered_to),
            detail: ack.delivered_to.describe(),
        }
    }
}

/// Map the engine's typed refusal onto the HTTP response. The status comes
/// from `ChildFollowUpError::status_code`, which lives beside the taxonomy, so
/// the mapping cannot drift from the ladder.
fn api_error(e: ChildFollowUpError) -> ApiError {
    let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    ApiError::new(status, e.to_string())
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

    // The caller is whoever the origin token says it is, and nothing else on
    // the request can influence that. `NotSubprocess`, or a subprocess with no
    // thread context (a scheduled script), has no thread whose children could
    // be looked up, so both are `NoCaller`.
    let caller_thread_id = match crate::api::actor::subprocess_origin(&headers) {
        crate::api::actor::SubprocessOrigin::Subprocess { source_thread_id } => source_thread_id,
        crate::api::actor::SubprocessOrigin::NotSubprocess => None,
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
