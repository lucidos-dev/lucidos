//! `/side-questions`: ask a *side question* beside a thread, and dismiss its
//! card. The ask and its outcome are recorded as side-question events no
//! agent reads (ADR 0320).

use super::*;

#[derive(Deserialize)]
pub(super) struct SideQuestionBody {
    thread_id: String,
    /// Named by the client, so its pending card and the recorded events
    /// reconcile by id.
    side_question_id: uuid::Uuid,
    question: String,
    /// Blobs the user attached, uploaded before the ask.
    #[serde(default)]
    image_hashes: Vec<String>,
}

#[derive(Deserialize)]
pub(super) struct SideQuestionDismissBody {
    thread_id: String,
    side_question_id: uuid::Uuid,
}

/// A side-question failure as its HTTP response.
fn side_question_error(
    failure: crate::engine::agent_session::side_question::SideQuestionFailure,
) -> ApiError {
    use crate::engine::agent_session::side_question::SideQuestionFailure;
    match failure {
        SideQuestionFailure::Refused(refusal) => ApiError::bad_request(refusal),
        SideQuestionFailure::UnknownImage(message) => ApiError::bad_request(message),
        SideQuestionFailure::AlreadyAsked => ApiError::new(
            StatusCode::CONFLICT,
            "A side question with this id is still running or was answered.",
        ),
        SideQuestionFailure::NotAsked => ApiError::new(
            StatusCode::NOT_FOUND,
            "No side question with this id was asked on this thread.",
        ),
        SideQuestionFailure::Failed(message) => ApiError::new(StatusCode::BAD_GATEWAY, message),
    }
}

/// The thread a side-question request names, once the caller is identified
/// and may reach it. Both steps run before anything is recorded.
async fn side_question_target(
    state: &AppState,
    headers: &HeaderMap,
    thread_id: &str,
    verb: super::thread_reach::ThreadReachVerb,
) -> Result<(uuid::Uuid, crate::engine::thread_events::MessageOrigin), ApiError> {
    let thread_id =
        uuid::Uuid::parse_str(thread_id).map_err(|_| ApiError::bad_request("Invalid thread_id"))?;
    let actor = super::actor::require_user_actor(headers, &state.pool, None).await?;
    super::thread_reach::refuse_without_authority(&state.pool, headers, Some(thread_id), verb)
        .await
        .map_err(|e| ApiError::new(e.status_code(), e.to_string()))?;
    Ok((thread_id, actor))
}

/// `POST /api/v1/side-questions`: answer a side question beside the thread's
/// work, in a Claude Code or a Lucidos Agent thread, with any attached images.
pub(super) async fn ask_side_question(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SideQuestionBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (thread_id, actor) = side_question_target(
        &state,
        &headers,
        &body.thread_id,
        super::thread_reach::ThreadReachVerb::AskSideQuestion,
    )
    .await?;
    // Detached, so a reload mid-ask still records the answer: dropping this
    // request would otherwise leave the card pending until a restart.
    let engine = state.engine.clone();
    let ask = tokio::spawn(async move {
        engine
            .ask_side_question(
                thread_id,
                body.side_question_id,
                &body.question,
                &body.image_hashes,
                actor,
            )
            .await
    });
    ask.await
        .map_err(|e| ApiError::internal(format!("The side question stopped: {e}")))?
        .map(|answer| Json(serde_json::json!({ "answer": answer })))
        .map_err(side_question_error)
}

/// `POST /api/v1/side-questions/dismiss`: record that the user dismissed a
/// side question's card. The ask stays, so the card can reopen.
pub(super) async fn dismiss_side_question(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SideQuestionDismissBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (thread_id, actor) = side_question_target(
        &state,
        &headers,
        &body.thread_id,
        super::thread_reach::ThreadReachVerb::DismissSideQuestion,
    )
    .await?;
    state
        .engine
        .dismiss_side_question(thread_id, body.side_question_id, actor)
        .await
        .map(|()| Json(serde_json::json!({ "ok": true })))
        .map_err(side_question_error)
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/side-questions", post(ask_side_question))
        .route("/side-questions/dismiss", post(dismiss_side_question))
}
