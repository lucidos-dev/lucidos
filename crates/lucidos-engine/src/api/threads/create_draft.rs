//! `POST /api/v1/threads/drafts`: an agent creates a draft for the owner
//! (ADR 0665). The body is a [`CreateDraftRequest`]. The answer is the draft as
//! `GET /api/v1/threads/drafts?thread_id=` reads it, plus `created`.
//!
//! Only an agent inside a thread may call it, since the draft's id derives from
//! the calling thread. A person starts a draft from the composer instead.

use axum::{
    extract::{rejection::JsonRejection, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::api::actor::SubprocessOrigin;
use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::agent_drafts::{CreateDraftRequest, CreatedDraft, DraftRefusal};

pub(in crate::api) async fn create_draft(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<CreateDraftRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<CreatedDraft>), ApiError> {
    let SubprocessOrigin::Subprocess {
        source_thread_id: Some(caller),
        ..
    } = crate::api::actor::subprocess_origin(&headers)
    else {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Only an agent inside a thread creates a draft here. A person starts one \
             from the composer.",
        ));
    };
    let Json(request) =
        body.map_err(|e| ApiError::bad_request(format!("Invalid draft request: {e}")))?;
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let created = state
        .engine
        .create_agent_draft(request, caller, actor)
        .await
        .map_err(|refusal| match refusal {
            DraftRefusal::Invalid(reason) => {
                ApiError::bad_request(format!("The draft was not created: {reason}"))
                    .with_reason("draft_invalid")
            }
            DraftRefusal::Internal(error) => ApiError::internal(error),
        })?;
    let status = if created.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(created)))
}
