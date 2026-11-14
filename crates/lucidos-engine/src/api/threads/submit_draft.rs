//! `POST /api/v1/threads/:thread_id/submit-draft`: an agent sends the owner's
//! saved draft exactly as saved (ADR 0651). The body names a revision, never
//! content. The answer is 202 with the receipt: accepted, not yet run.
//!
//! Only an agent inside a thread may call it. The engine resolves the owner's
//! authority itself: a standing instruction or a live Allow once card.

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::actor::SubprocessOrigin;
use crate::api::error::ApiError;
use crate::api::AppState;
use crate::core::draft_submission::{DraftSubmissionReceipt, SubmitDraftRequest};
use crate::engine::draft_submission::{refusal_report, RefusalAudience};

/// Unknown fields fail closed, so no caller can add text, images or settings.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::api) struct SubmitDraftBody {
    expected_revision: i64,
    expected_epoch: i64,
    /// Names this send. Omitted, the engine mints one and the receipt echoes it.
    #[serde(default)]
    retry_key: Option<Uuid>,
}

pub(in crate::api) async fn submit_draft(
    State(state): State<AppState>,
    Path(thread_id): Path<Uuid>,
    headers: HeaderMap,
    body: Result<Json<SubmitDraftBody>, JsonRejection>,
) -> Result<(StatusCode, Json<DraftSubmissionReceipt>), ApiError> {
    let caller = submitting_agent(&headers)?;
    let Json(body) =
        body.map_err(|e| ApiError::bad_request(format!("Invalid submit-draft request: {e}")))?;
    let request = SubmitDraftRequest {
        thread_id,
        expected_revision: body.expected_revision,
        expected_epoch: body.expected_epoch,
        retry_key: body.retry_key.unwrap_or_else(Uuid::new_v4),
    };
    match state
        .engine
        .submit_saved_draft_as_agent(caller, request)
        .await
    {
        Ok(receipt) => Ok((StatusCode::ACCEPTED, Json(receipt))),
        Err(error) => {
            let audience = refusal_audience(&state.pool, caller).await;
            let report = refusal_report(&*error, thread_id, audience);
            Err(ApiError::with_code(report.status, report.message).with_reason(report.reason))
        }
    }
}

/// A chat agent and a coding agent reach this route alike, through the CLI.
/// Each refusal names a next step the caller's own kind of thread has. A
/// failed lookup keeps the refusal and names the step every agent has.
async fn refusal_audience(pool: &sqlx::PgPool, caller: Uuid) -> RefusalAudience {
    let coding: Result<Option<bool>, _> =
        sqlx::query_scalar("SELECT is_coding_agent FROM thread_summaries WHERE thread_id = $1")
            .bind(caller)
            .fetch_optional(pool)
            .await;
    match coding {
        Ok(Some(true)) => RefusalAudience::CodingAgent,
        Ok(_) => RefusalAudience::ChatAgent,
        Err(e) => {
            log!("[SubmitDraft] Could not read calling thread {caller}: {e}");
            RefusalAudience::ChatAgent
        }
    }
}

/// The calling agent's thread, as its origin token's MAC proves it. A device,
/// the local token and a threadless subprocess are refused.
fn submitting_agent(headers: &HeaderMap) -> Result<Uuid, ApiError> {
    match crate::api::actor::subprocess_origin(headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id: Some(caller),
            ..
        } => Ok(caller),
        _ => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Only an agent inside a thread submits a saved draft here. The owner sends \
             their own draft from the composer.",
        )),
    }
}

#[cfg(test)]
#[path = "submit_draft_tests.rs"]
pub(crate) mod tests;
