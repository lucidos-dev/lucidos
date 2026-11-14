//! `/api/v1/threads/:thread_id/automation-pause`: the *automation pause*.
//! `POST {"reason": "..."}` pauses a coding-agent thread's automatic steps.
//! The owner, the thread's own agent, or an agent above it may pause, since a
//! pause only restricts. `DELETE` unpauses, and only the owner may. Both answer
//! `{"paused": bool, "changed": bool}`, and refuse an unknown thread with 404
//! and a thread that is not a coding-agent thread with 400.

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use crate::api::actor::SubprocessOrigin;
use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::automation_pause::{
    pause_automation, require_pausable_thread, unpause_automation, PauseChange, PauseError,
};
use crate::engine::thread_events::AutomationUnpauseCause;

#[derive(serde::Deserialize)]
pub(in crate::api) struct PauseBody {
    reason: String,
}

#[derive(serde::Serialize)]
pub(in crate::api) struct PauseResponse {
    paused: bool,
    changed: bool,
}

fn thread_uuid(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|e| ApiError::bad_request(format!("Invalid thread id: {e}")))
}

fn api_error(e: PauseError) -> ApiError {
    match e {
        PauseError::NotFound => ApiError::not_found(e.to_string()),
        PauseError::Refused(why) => ApiError::bad_request(why),
        PauseError::Internal(why) => ApiError::internal(why),
    }
}

pub(in crate::api) async fn pause(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<PauseBody>, JsonRejection>,
) -> Result<Json<PauseResponse>, ApiError> {
    let thread_id = thread_uuid(&thread_id)?;
    let Json(body) = body.map_err(|e| {
        ApiError::bad_request(format!("The body must be {{\"reason\": \"...\"}}: {e}"))
    })?;
    if let SubprocessOrigin::Subprocess {
        source_thread_id, ..
    } = crate::api::actor::subprocess_origin(&headers)
    {
        let reaches = match source_thread_id {
            Some(caller) => {
                crate::api::thread_reach::authorize_thread_reach(&state.pool, caller, thread_id)
                    .await?
            }
            None => false,
        };
        if !reaches {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "An agent may pause only its own thread or a thread below it.",
            ));
        }
    }
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let change = pause_automation(
        &state.pool,
        &state.engine.event_bus,
        thread_id,
        &body.reason,
        Some(actor),
    )
    .await
    .map_err(api_error)?;
    Ok(Json(PauseResponse {
        paused: true,
        changed: change == PauseChange::Recorded,
    }))
}

pub(in crate::api) async fn unpause(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<PauseResponse>, ApiError> {
    let thread_id = thread_uuid(&thread_id)?;
    crate::api::actor::require_attested_owner(
        &headers,
        "Only the owner unpauses a thread. The owner paused its automatic steps, so an \
         agent cannot lift that, whoever asks it to.",
        crate::api::owner_door::OnUnattested::Refuse,
    )?;
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    require_pausable_thread(&state.pool, thread_id)
        .await
        .map_err(api_error)?;
    let change = unpause_automation(
        &state.pool,
        &state.engine.event_bus,
        thread_id,
        AutomationUnpauseCause::Unpause,
        Some(actor.clone()),
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    if change == PauseChange::Recorded {
        let engine = state.engine.clone();
        tokio::spawn(async move {
            engine
                .release_paused_steps(thread_id, None, Some(actor))
                .await
        });
    }
    Ok(Json(PauseResponse {
        paused: false,
        changed: change == PauseChange::Recorded,
    }))
}
