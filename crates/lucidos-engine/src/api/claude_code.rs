use super::*;

#[derive(Deserialize)]
pub(super) struct CancelQuery {
    #[serde(default)]
    apply: bool,
    #[serde(default)]
    discard: bool,
    thread_id: Option<String>,
}

pub(super) async fn claude_code_cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CancelQuery>,
) -> StatusCode {
    let thread_id = query
        .thread_id
        .as_deref()
        .and_then(|s| uuid::Uuid::parse_str(s).ok());
    // Stamp the user actor so any ChangeApplied / ChangeApplyFailed emitted by
    // the stale-session fallback (cancel?apply=true on a thread whose CC
    // already exited) carries the device that clicked Stop instead of
    // collapsing to "Lucidos Engine" via the actor-missing fallback.
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match state
        .engine
        .cancel_agent(query.apply, query.discard, thread_id, actor)
        .await
    {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            crate::log!("[API] cancel_agent failed: {}", e);
            StatusCode::NOT_FOUND
        }
    }
}

#[derive(Deserialize)]
pub(super) struct InterruptQuery {
    thread_id: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ApplyNowQuery {
    thread_id: String,
}

pub(super) async fn claude_code_apply_now(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ApplyNowQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let thread_id = uuid::Uuid::parse_str(&query.thread_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match state.engine.apply_now(thread_id, actor).await {
        Ok(_) => Ok(Json(serde_json::json!({ "status": "applying" }))),
        Err(e) => {
            let msg = e.to_string();
            crate::log!("[API] apply_now failed for {}: {}", thread_id, msg);
            if msg.contains("already in progress") {
                Err(StatusCode::CONFLICT)
            } else {
                Err(StatusCode::NOT_FOUND)
            }
        }
    }
}

#[derive(Deserialize)]
pub(super) struct ControlRequestBody {
    thread_id: String,
    request: crate::runtime::ControlRequest,
}

pub(super) async fn claude_code_control(
    State(state): State<AppState>,
    Json(body): Json<ControlRequestBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let thread_id = uuid::Uuid::parse_str(&body.thread_id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid thread_id" })),
        )
    })?;
    state
        .engine
        .send_agent_control_request(thread_id, body.request)
        .await
        .map(|_| Json(serde_json::json!({ "ok": true })))
        .map_err(|e| {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
        })
}

#[derive(Deserialize)]
pub(super) struct CommandsQuery {
    thread_id: Option<String>,
}

/// Return available CC commands: control subtypes (always) + categorized slash commands (if a session is active).
pub(super) async fn claude_code_commands(
    State(state): State<AppState>,
    Query(query): Query<CommandsQuery>,
) -> Json<serde_json::Value> {
    let tid = query
        .thread_id
        .as_deref()
        .and_then(|s| uuid::Uuid::parse_str(s).ok());
    let res = if let Some(tid) = tid {
        state.engine.cc_categorized_commands(tid).await
    } else {
        state.engine.cc_cached_commands().await
    };
    Json(serde_json::json!({
        "control_commands": crate::runtime::claude_code::cc_command_definitions(),
        "builtin_commands": res.info.builtin_commands,
        "skill_commands": res.info.skill_commands,
        "current_model": res.current_model,
        "current_reasoning_effort": res.current_reasoning_effort,
        "has_active_session": res.has_active_session,
    }))
}

#[derive(Deserialize)]
pub(super) struct ThreadIdBody {
    thread_id: String,
}

/// POST /api/claude-code/discard — discard CC changes without ending session
pub(super) async fn claude_code_discard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ThreadIdBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let thread_uuid = uuid::Uuid::parse_str(&body.thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    state
        .engine
        .discard_cc_changes(thread_uuid, actor)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::OK)
}

pub(super) async fn claude_code_interrupt(
    State(state): State<AppState>,
    Query(query): Query<InterruptQuery>,
) -> StatusCode {
    let thread_id = query
        .thread_id
        .as_deref()
        .and_then(|s| uuid::Uuid::parse_str(s).ok());
    match state.engine.interrupt_agent(thread_id).await {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            crate::log!("[API] interrupt_agent failed: {}", e);
            StatusCode::NOT_FOUND
        }
    }
}

#[derive(Deserialize)]
pub(super) struct AnswerQuestionBody {
    thread_id: String,
    tool_use_id: String,
    answer: crate::engine::thread_events::AnswerKind,
}

/// POST /api/claude-code/answer-question — answer a pending CC AskUserQuestion.
/// Emits `UserQuestionAnswered` and respawns CC with `--resume` and a matching
/// `tool_result`. Returns 409 when the question is missing or already answered.
pub(super) async fn claude_code_answer_question(
    State(state): State<AppState>,
    Json(body): Json<AnswerQuestionBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let thread_id = uuid::Uuid::parse_str(&body.thread_id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid thread_id" })),
        )
    })?;
    use crate::engine::agent_question::{answer_pending_question, AnswerResult};
    match answer_pending_question(&state.engine, thread_id, body.tool_use_id, body.answer).await {
        AnswerResult::Resumed => Ok(Json(serde_json::json!({ "ok": true }))),
        AnswerResult::Conflict(msg) => Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": msg })),
        )),
    }
}
