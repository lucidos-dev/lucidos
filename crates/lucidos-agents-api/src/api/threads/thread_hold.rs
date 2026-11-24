//! `/api/v1/threads/:thread_id/hold`: *Hold* and *Resume*.
//!
//! - `POST {"reason": "..."}` puts the thread and its sub-threads on hold.
//!   The owner, Home, the thread's own agent, or an agent above it may.
//! - `DELETE` resumes it. The owner may, and so may the thread's own agent
//!   inside a turn the owner's device message started. A sub-thread a Hold
//!   reached resumes from its root only.
//!
//! Both answer `{"held": bool, "changed": bool}` and refuse an unknown thread
//! with 404. The retired `/automation-pause` route is a one-release
//! alias (see `docs/temporary-measures.md`).

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::core::subprocess_origin::SubprocessOrigin;
use crate::engine::thread_hold::{place_hold, resume_hold, HoldChange, HoldError};

#[derive(serde::Deserialize)]
pub(in crate::api) struct HoldBody {
    reason: String,
}

#[derive(serde::Serialize)]
pub(in crate::api) struct HoldResponse {
    held: bool,
    changed: bool,
}

/// What an agent reads when it may not resume.
const AGENT_RESUME_REFUSAL: &str = "Only the owner resumes a thread on hold. Its own agent \
     may resume it only inside a turn the owner's own message started there. Ask the owner \
     to press Resume on the hold strip.";

fn thread_uuid(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|e| ApiError::bad_request(format!("Invalid thread id: {e}")))
}

fn api_error(e: HoldError) -> ApiError {
    match e {
        HoldError::NotFound => ApiError::not_found(e.to_string()),
        HoldError::Refused(why) => ApiError::bad_request(why),
        HoldError::Internal(why) => ApiError::internal(why),
    }
}

pub(in crate::api) async fn hold(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<HoldBody>, JsonRejection>,
) -> Result<Json<HoldResponse>, ApiError> {
    let thread_id = thread_uuid(&thread_id)?;
    let Json(body) = body.map_err(|e| {
        ApiError::bad_request(format!("The body must be {{\"reason\": \"...\"}}: {e}"))
    })?;
    let mut holder = None;
    if let SubprocessOrigin::Subprocess {
        source_thread_id, ..
    } = crate::core::subprocess_origin::subprocess_origin(&headers)
    {
        let reaches = match source_thread_id {
            Some(caller) => {
                crate::api::thread_reach::hold_reach(&state.pool, caller, thread_id).await?
            }
            None => false,
        };
        if !reaches {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "An agent may hold only its own thread or a thread below it. \
                 Home may hold any thread in its workspace.",
            ));
        }
        holder = source_thread_id;
    }
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let change = place_hold(&state.engine, thread_id, &body.reason, holder, Some(actor))
        .await
        .map_err(api_error)?;
    Ok(Json(HoldResponse {
        held: true,
        changed: change == HoldChange::Recorded,
    }))
}

pub(in crate::api) async fn resume(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<HoldResponse>, ApiError> {
    let thread_id = thread_uuid(&thread_id)?;
    match crate::core::subprocess_origin::subprocess_origin(&headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } => {
            let own_turn = source_thread_id == Some(thread_id)
                && crate::engine::standing_instruction::owner_opened_current_turn(
                    &state.pool,
                    thread_id,
                )
                .await;
            if !own_turn {
                return Err(ApiError::new(StatusCode::FORBIDDEN, AGENT_RESUME_REFUSAL));
            }
        }
        _ => crate::api::actor::require_attested_owner(&headers, AGENT_RESUME_REFUSAL)?,
    }
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let change = resume_hold(&state.engine, thread_id, Some(actor))
        .await
        .map_err(api_error)?;
    Ok(Json(HoldResponse {
        held: false,
        changed: change == HoldChange::Recorded,
    }))
}

#[cfg(test)]
#[path = "thread_hold_tests.rs"]
mod tests;
