use super::*;
use crate::engine::agent_session::side_question::AgentSessionSideQuestionExt;

#[derive(Deserialize)]
pub(super) struct SideQuestionBody {
    thread_id: String,
    side_question_id: uuid::Uuid,
    question: String,
    #[serde(default)]
    image_hashes: Vec<String>,
}

#[derive(Deserialize)]
pub(super) struct SideQuestionDismissBody {
    thread_id: String,
    side_question_id: uuid::Uuid,
}

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

async fn side_question_target(
    state: &AppState,
    headers: &HeaderMap,
    thread_id: &str,
    verb: crate::engine::permission::Action,
) -> Result<(uuid::Uuid, crate::engine::thread_events::MessageOrigin), ApiError> {
    let thread_id =
        uuid::Uuid::parse_str(thread_id).map_err(|_| ApiError::bad_request("Invalid thread_id"))?;
    let actor = super::actor::require_user_actor(headers, &state.pool, None).await?;
    super::thread_reach::refuse_without_authority(
        &state.engine.event_bus,
        headers,
        Some(thread_id),
        verb,
    )
    .await
    .map_err(|e| ApiError::new(e.status_code(), e.to_string()))?;
    Ok((thread_id, actor))
}

pub(super) async fn ask_side_question(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SideQuestionBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (thread_id, actor) = side_question_target(
        &state,
        &headers,
        &body.thread_id,
        crate::engine::permission::Action::AskSideQuestion,
    )
    .await?;
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

pub(super) async fn dismiss_side_question(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SideQuestionDismissBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (thread_id, actor) = side_question_target(
        &state,
        &headers,
        &body.thread_id,
        crate::engine::permission::Action::DismissSideQuestion,
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

#[cfg(test)]
mod tests {
    const SOURCE: &str = include_str!("side_questions.rs");

    fn section(from: &str, to: &str) -> &'static str {
        let start = SOURCE.find(from).expect("start marker");
        let end = start + SOURCE[start..].find(to).expect("end marker");
        &SOURCE[start..end]
    }

    fn at(haystack: &str, needle: &str) -> usize {
        haystack
            .find(needle)
            .unwrap_or_else(|| panic!("missing {needle}"))
    }

    #[test]
    fn target_resolves_actor_then_thread_reach() {
        let target = section(
            "async fn side_question_target(",
            "/// `POST /api/v1/side-questions`",
        );
        assert!(at(target, "require_user_actor(") < at(target, "refuse_without_authority("));
    }

    #[test]
    fn handlers_resolve_the_target_before_the_engine_records() {
        let ask = section(
            "pub(super) async fn ask_side_question(",
            "/// `POST /api/v1/side-questions/dismiss`",
        );
        assert!(at(ask, "side_question_target(") < at(ask, ".ask_side_question("));
        let dismiss = section(
            "pub(super) async fn dismiss_side_question(",
            "pub(super) fn router()",
        );
        assert!(at(dismiss, "side_question_target(") < at(dismiss, ".dismiss_side_question("));
    }

    #[test]
    fn the_ask_runs_detached_from_the_request() {
        let ask = section(
            "pub(super) async fn ask_side_question(",
            "/// `POST /api/v1/side-questions/dismiss`",
        );
        assert!(at(ask, "tokio::spawn(") < at(ask, ".ask_side_question("));
    }
}
