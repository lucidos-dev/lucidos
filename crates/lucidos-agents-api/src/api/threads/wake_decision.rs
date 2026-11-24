//! The *wake decision* route.
//!
//! A coding-agent child decides through `lucidos wake-parent yes|no`, which
//! posts here. A chat child decides through the `wake_parent` tool, in process,
//! and both record the same events.

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::api::AppState;
use crate::engine::thread_events::WakeMode;
use crate::engine::turn_end::wake_decision::{record_wake_decision, WakeDecision};
use crate::llm::tools::WAKE_ARG;

use super::actions::refuse_an_agent_from_another_thread;
use super::parse_thread_id;

/// The 403 for an agent naming another thread's id.
pub(super) const WAKE_DECISION_IS_THE_CALLERS: &str = "A thread can only decide whether its \
    own report wakes its parent, and the id in the path is not the calling thread. Drop the \
    id: `lucidos wake-parent` already acts on the thread you are running in.";

/// The body of a wake decision. `wake` is required: there is no default.
#[derive(serde::Deserialize)]
pub(in crate::api) struct WakeDecisionBody {
    wake: bool,
}

/// POST /api/v1/threads/:thread_id/wake-decision: the calling thread's agent
/// records whether this turn's report wakes its parent, `{"wake": true|false}`.
///
/// Acts on the calling thread only, as the tool does by taking no thread id.
/// A thread with no parent has no report to decide, so it gets a 409. A child
/// whose spawn declared a wake mode records nothing, and reads status
/// `declared` (ADR 0451).
pub(in crate::api) async fn record_thread_wake_decision(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<WakeDecisionBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_uuid = parse_thread_id(&thread_id)?;
    refuse_an_agent_from_another_thread(&headers, thread_uuid, WAKE_DECISION_IS_THE_CALLERS)?;
    let Json(body) = body.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!(
                "The body must be {{\"{WAKE_ARG}\": true}} or {{\"{WAKE_ARG}\": false}}: \
                 whether your parent must act on this turn's report now. {e}"
            ),
        )
    })?;
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None)
        .await
        .map_err(|e| (e.status, e.message))?;
    let row: Option<(Option<uuid::Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT parent_thread_id, wake_mode FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_uuid)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if let Some((Some(_), Some(mode))) = &row {
        if let Some(mode) = WakeMode::parse(mode) {
            return Ok(Json(serde_json::json!({
                "status": "declared",
                "message": WakeDecision::declared_ack(mode),
            })));
        }
    }
    match row.map(|(parent, _)| parent) {
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                format!("No thread {thread_id} in this workspace."),
            ))
        }
        Some(None) => {
            return Err((
                StatusCode::CONFLICT,
                "This thread has no parent thread to wake. Only a sub-thread decides whether \
                 its report wakes its parent."
                    .to_string(),
            ))
        }
        Some(Some(_)) => {}
    }
    let decision = WakeDecision::from_wake(body.wake);
    record_wake_decision(&state.engine.event_bus, thread_uuid, decision, Some(actor))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({
        "status": match decision {
            WakeDecision::Requested => "wake",
            WakeDecision::NotRequested => "file",
        },
        "message": decision.ack(),
    })))
}
