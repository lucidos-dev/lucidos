use super::*;
use crate::engine::agent_session::apply_now::ApplyNowExt;
use crate::engine::claude_code::control::ControlExt;
use crate::engine::claude_code::spawn::ClaudeCodeSpawnExt;

#[derive(Deserialize)]
pub(super) struct StopQuery {
    #[serde(default)]
    apply: bool,
    #[serde(default)]
    discard: bool,
    thread_id: Option<String>,
}

impl StopQuery {
    fn reason(&self) -> crate::engine::claude_code::StopReason {
        use crate::engine::claude_code::StopReason;
        match (self.apply, self.discard) {
            (true, _) => StopReason::Apply,
            (_, true) => StopReason::Discard,
            _ => StopReason::UserStop,
        }
    }
}

fn stop_verb(reason: crate::engine::claude_code::StopReason) -> crate::engine::permission::Action {
    use crate::engine::claude_code::StopReason;
    use crate::engine::permission::Action;
    match reason {
        StopReason::Apply => Action::Apply,
        StopReason::Discard => Action::Discard,
        _ => Action::Cancel,
    }
}

pub(super) async fn claude_code_stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<StopQuery>,
) -> Result<Json<super::CancelResponse>, ApiError> {
    let thread_id = super::parse_optional_uuid(query.thread_id.as_deref())?;
    // INVARIANT: Before the question card is cancel-stamped and before any hard stop, so
    // a refusal leaves the turn exactly as it found it. Apply and Discard reach
    // this route too, and each is gated as itself.
    super::thread_reach::refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        thread_id,
        stop_verb(query.reason()),
    )
    .await?;
    let actor = super::actor::user_actor(&headers, None);

    let question_resolved = if let Some(tid) = thread_id {
        crate::engine::agent_question::resolve_pending_question_as_canceled(
            &state.engine,
            tid,
            actor.clone(),
        )
        .await
    } else {
        false
    };

    // INVARIANT: Cancel (Stop) = Esc: route a real `UserStop` through CC's native interrupt
    // so the turn is interrupted but the session stays resumable — the next
    // message `--resume`s the same `cc_session_id` on the same branch. Apply /
    // Discard keep `stop_agent`: each carries its own terminator
    // (`ChangeApplied` / `ChangeDiscarded`) and must hard-stop, not interrupt.
    use crate::engine::claude_code::{SettleTerminal, StopReason, SESSION_ALREADY_WAITING};
    let reason = query.reason();
    match reason {
        // INVARIANT: `canceled = false` means nothing was interruptible — the client's
        // optimistic "canceling" state is stale and it must re-sync.
        //
        // The settle fallback inside `interrupt_agent` fires when no agent is
        // left to interrupt, the normal shape for a question parked overnight.
        // The cancel-stamp above is what set that row to `running`, so tell the
        // settle: it then emits the `ResponseCanceled` a live agent would have,
        // not a stale-settle abort. See `SettleTerminal`.
        StopReason::UserStop => match state
            .engine
            .interrupt_agent(
                thread_id,
                actor,
                if question_resolved {
                    SettleTerminal::CanceledQuestion
                } else {
                    SettleTerminal::StuckProjection
                },
            )
            .await
        {
            Ok(interrupted) => Ok(Json(super::CancelResponse {
                canceled: question_resolved || interrupted,
            })),
            Err(e) if e.to_string() == SESSION_ALREADY_WAITING => Ok(Json(super::CancelResponse {
                canceled: question_resolved,
            })),
            Err(e) => {
                crate::log_legacy!("[API] claude_code_stop ({:?}) failed: {}", reason, e);
                Err(StatusCode::NOT_FOUND.into())
            }
        },
        other => match state.engine.stop_agent(other, thread_id, actor).await {
            Ok(()) => Ok(Json(super::CancelResponse { canceled: true })),
            Err(e) => {
                if let Some(refused) = claim_refusal_error(&e.to_string()) {
                    return Err(refused);
                }
                crate::log_legacy!("[API] claude_code_stop ({:?}) failed: {}", reason, e);
                Err(StatusCode::NOT_FOUND.into())
            }
        },
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

fn claim_refusal_error(msg: &str) -> Option<ApiError> {
    crate::engine::claude_code::claim_refusal_reason(msg)
        .map(|reason| ApiError::new(StatusCode::CONFLICT, msg).with_reason(reason))
}

fn apply_now_error(msg: &str) -> ApiError {
    if let Some(refused) = claim_refusal_error(msg) {
        return refused;
    }
    if msg == crate::engine::MERGE_OWNED_BY_RESOLVER_MESSAGE {
        return ApiError::new(StatusCode::CONFLICT, msg).with_reason("resolving_conflicts");
    }
    if msg == crate::engine::claude_code::QUESTION_OPEN_MESSAGE {
        return ApiError::new(StatusCode::CONFLICT, msg).with_reason("question_open");
    }
    if msg == crate::engine::claude_code::QUESTION_UNKNOWN_MESSAGE {
        return ApiError::new(StatusCode::CONFLICT, msg).with_reason("question_unknown");
    }
    ApiError::internal(msg.to_string())
}

pub(super) async fn claude_code_apply_now(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ApplyNowQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let thread_id = uuid::Uuid::parse_str(&query.thread_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    // INVARIANT: Apply by another route, so it takes the same gate. Before the merge, so a
    // refusal lands nothing on main.
    super::thread_reach::refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(thread_id),
        crate::engine::permission::Action::Apply,
    )
    .await?;
    let actor = super::actor::user_actor(&headers, None);
    match state.engine.apply_now(thread_id, actor).await {
        Ok(summary_version) => Ok(Json(serde_json::json!({
            "status": "applying",
            "summary_version": summary_version,
        }))),
        Err(e) => {
            let msg = e.to_string();
            crate::log_legacy!("[API] apply_now failed for {}: {}", thread_id, msg);
            Err(apply_now_error(&msg))
        }
    }
}

#[cfg(test)]
mod apply_now_status_tests {
    use super::*;
    use crate::engine::session_claim_refusal::{
        APPLY_IN_PROGRESS_MESSAGE, DISCARD_IN_PROGRESS_MESSAGE, SESSION_STOPPING_MESSAGE,
    };

    fn refusal(msg: &str) -> (StatusCode, String, Option<&'static str>) {
        let e = apply_now_error(msg);
        (e.status, e.message, e.reason)
    }

    #[test]
    fn a_resolver_owned_merge_is_a_conflict_not_a_missing_session() {
        let msg = crate::engine::MERGE_OWNED_BY_RESOLVER_MESSAGE;
        assert_eq!(
            refusal(msg),
            (
                StatusCode::CONFLICT,
                msg.to_string(),
                Some("resolving_conflicts")
            )
        );
    }

    #[test]
    fn a_parked_question_is_a_conflict_not_a_missing_session() {
        use crate::engine::claude_code::{QUESTION_OPEN_MESSAGE, QUESTION_UNKNOWN_MESSAGE};
        for (msg, slug) in [
            (QUESTION_OPEN_MESSAGE, "question_open"),
            (QUESTION_UNKNOWN_MESSAGE, "question_unknown"),
        ] {
            assert_eq!(
                refusal(msg),
                (StatusCode::CONFLICT, msg.to_string(), Some(slug)),
                "{msg}"
            );
        }
    }

    #[test]
    fn a_claim_refusal_names_its_holder() {
        for (msg, slug) in [
            (APPLY_IN_PROGRESS_MESSAGE, "apply_in_progress"),
            (DISCARD_IN_PROGRESS_MESSAGE, "discard_in_progress"),
            (SESSION_STOPPING_MESSAGE, "session_stopping"),
        ] {
            assert_eq!(
                refusal(msg),
                (StatusCode::CONFLICT, msg.to_string(), Some(slug)),
                "{msg}"
            );
        }
        assert!(!DISCARD_IN_PROGRESS_MESSAGE.contains("apply"));
    }

    #[test]
    fn an_unrecognised_failure_names_its_cause() {
        let (status, message, reason) = refusal("Session channel closed");
        assert_eq!((status, reason), (StatusCode::INTERNAL_SERVER_ERROR, None));
        assert!(message.contains("Session channel closed"), "{message}");
    }
}

#[derive(Deserialize)]
pub(super) struct ControlRequestBody {
    thread_id: String,
    request: crate::engine::claude_code::RungPick,
}

pub(crate) const TURN_RUNNING_REASON: &str = "turn-running";

async fn refuse_unoffered_effort(
    engine: &SharedEngine,
    thread_id: uuid::Uuid,
    pick: &crate::engine::claude_code::RungPick,
) -> Result<(), String> {
    use crate::engine::claude_code::RungPick;
    let (model, effort) = match pick {
        RungPick::SetModel {
            model,
            effort: Some(effort),
        } => (Some(model.clone()), effort),
        RungPick::SetModel { effort: None, .. } => return Ok(()),
        RungPick::SetReasoningEffort { effort } => {
            (engine.cc_thread_settings(thread_id).await.0, effort)
        }
    };
    let agent = engine.thread_coding_agent(thread_id).await;
    match crate::runtime::validate_coding_agent_effort(agent, model.as_deref(), Some(effort))? {
        Some(_) => Ok(()),
        None => Err("reasoning_effort must name a level".to_string()),
    }
}

pub(super) async fn claude_code_control(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ControlRequestBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let thread_id = uuid::Uuid::parse_str(&body.thread_id)
        .map_err(|_| ApiError::bad_request("Invalid thread_id"))?;
    // INVARIANT: A thread-bound subprocess may drive only its own session or a
    // descendant's, never a sibling's. Runs before anything is recorded.
    super::thread_reach::refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(thread_id),
        crate::engine::permission::Action::Control,
    )
    .await
    .map_err(|e| ApiError::new(e.status_code(), e.to_string()))?;
    refuse_unoffered_effort(&state.engine, thread_id, &body.request)
        .await
        .map_err(ApiError::bad_request)?;
    let actor = super::actor::user_actor(&headers, None);
    let outcome = state
        .engine
        .record_rung_pick(thread_id, body.request, actor)
        .await
        .map_err(|e| ApiError::internal(format!("Could not record the pick: {e}")))?;
    match outcome {
        crate::engine::claude_code::RungPickOutcome::Recorded => {
            Ok(Json(serde_json::json!({ "ok": true })))
        }
        crate::engine::claude_code::RungPickOutcome::TurnRunning => Err(ApiError::new(
            StatusCode::CONFLICT,
            "The model and effort stay fixed while a turn runs. Pick again when it ends.",
        )
        .with_reason(TURN_RUNNING_REASON)),
    }
}

#[derive(Deserialize)]
pub(super) struct CommandsQuery {
    thread_id: Option<String>,
    repo_id: Option<String>,
    coding_agent: Option<crate::runtime::CodingAgent>,
}

pub(super) async fn claude_code_commands(
    State(state): State<AppState>,
    Query(query): Query<CommandsQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let tid = super::parse_optional_uuid(query.thread_id.as_deref())?;
    let coding_agent = match tid {
        Some(tid) => state.engine.thread_coding_agent(tid).await,
        None => query
            .coding_agent
            .unwrap_or(crate::runtime::CodingAgent::ClaudeCode),
    };
    let default_implementer =
        crate::runtime::intelligence_ladder::default_implementer(&state.pool, coding_agent)
            .await
            .map(|d| serde_json::json!({"model": d.model, "effort": d.effort}));
    let ladder_rungs = crate::runtime::intelligence_ladder::load(
        &state.pool,
        &state.engine.model_routes(),
        coding_agent.into(),
    )
    .await
    .on_intelligence_rungs();
    if coding_agent == crate::runtime::CodingAgent::Codex {
        let (current_model, current_effort) = match tid {
            Some(tid) => state.engine.cc_thread_settings(tid).await,
            None => (None, None),
        };
        let has_active_session = match tid {
            Some(tid) => {
                let guard = state.engine.agent_sessions.lock().await;
                guard.get(&tid).is_some_and(|s| s.is_live())
            }
            None => false,
        };
        return Ok(Json(serde_json::json!({
            "control_commands": crate::runtime::codex::codex_command_definitions(),
            "builtin_commands": [],
            "skill_commands": [],
            "current_model": current_model,
            "current_reasoning_effort": current_effort,
            "has_active_session": has_active_session,
            "default_implementer": default_implementer,
            "ladder_rungs": ladder_rungs,
        })));
    }
    let res = if let Some(tid) = tid {
        state.engine.cc_categorized_commands(tid).await
    } else {
        // INVARIANT: Compose-view: resolve repo_id (possibly empty/missing) to a path
        // and look up just that repo's cache. Never fall back to "first
        // cache entry" — that leaks skills from other repos into the menu.
        // For the default (no `repo_id`), use the engine's own repo_root —
        // the cache key is path-based, the engine registered itself with
        // exactly this path at startup, and bypassing the DB also covers
        // the recoverable case where the `repositories` row was truncated.
        let repo_path: std::path::PathBuf = match query.repo_id.as_deref() {
            Some(rid) if !rid.is_empty() => {
                let uuid = uuid::Uuid::parse_str(rid).map_err(|_| StatusCode::BAD_REQUEST)?;
                let repo = crate::core::repositories::RepositoryStore::get(&state.pool, uuid)
                    .await
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                    .ok_or(StatusCode::NOT_FOUND)?;
                repo.path.into()
            }
            _ => state.engine.repo_root().to_path_buf(),
        };
        state.engine.cc_commands_for_repo(&repo_path).await
    };
    Ok(Json(serde_json::json!({
        "control_commands": crate::runtime::claude_code::cc_command_definitions(),
        "builtin_commands": res.info.builtin_commands,
        "skill_commands": res.info.skill_commands,
        "current_model": res.current_model,
        "current_reasoning_effort": res.current_reasoning_effort,
        "has_active_session": res.has_active_session,
        "default_implementer": default_implementer,
        "ladder_rungs": ladder_rungs,
        "models_provenance": crate::runtime::cc_model_discovery::provenance(),
    })))
}

#[derive(Deserialize)]
pub(super) struct ThreadIdBody {
    thread_id: String,
}

pub(super) async fn claude_code_discard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ThreadIdBody>,
) -> Result<StatusCode, ApiError> {
    let thread_uuid = uuid::Uuid::parse_str(&body.thread_id)
        .map_err(|e| ApiError::bad_request(format!("Invalid thread_id: {}", e)))?;
    // INVARIANT: Discard by another route, so it takes the same gate. Before the branch
    // and worktree go.
    super::thread_reach::refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(thread_uuid),
        crate::engine::permission::Action::Discard,
    )
    .await?;
    let actor = super::actor::user_actor(&headers, None);
    state
        .engine
        .discard_cc_changes(thread_uuid, actor)
        .await
        .map_err(|e| {
            let message = e.to_string();
            claim_refusal_error(&message).unwrap_or_else(|| ApiError::internal(message))
        })?;
    Ok(StatusCode::OK)
}

pub(super) async fn claude_code_interrupt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<InterruptQuery>,
) -> Result<StatusCode, ApiError> {
    let thread_id = super::parse_optional_uuid(query.thread_id.as_deref())?;
    super::thread_reach::refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        thread_id,
        crate::engine::permission::Action::Cancel,
    )
    .await?;
    let actor = super::actor::user_actor(&headers, None);
    match state
        .engine
        .interrupt_agent(
            thread_id,
            actor,
            crate::engine::claude_code::SettleTerminal::StuckProjection,
        )
        .await
    {
        Ok(_) => Ok(StatusCode::OK),
        Err(e) => {
            crate::log_legacy!("[API] interrupt_agent failed: {}", e);
            Err(StatusCode::NOT_FOUND.into())
        }
    }
}

#[derive(Serialize)]
pub(super) struct AgentBinariesResponse {
    claude_code: crate::runtime::AgentBinaryStatus,
    codex: crate::runtime::AgentBinaryStatus,
}

pub(super) async fn coding_agent_binaries(
    State(state): State<AppState>,
) -> Result<Json<AgentBinariesResponse>, (StatusCode, String)> {
    use crate::core::prefs::{self, Optional, Pref};
    let read_override = |pref: &'static Pref<Optional>| {
        let pool = state.pool.clone();
        async move {
            pref.try_read(&pool).await.map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to read {}: {e}", pref.key()),
                )
            })
        }
    };
    let claude_override = read_override(&prefs::CODING_AGENT_CLAUDE_PATH).await?;
    let codex_override = read_override(&prefs::CODING_AGENT_CODEX_PATH).await?;
    let (claude_code, codex) = tokio::join!(
        crate::runtime::detect_agent_binary_with_version(
            crate::runtime::CodingAgent::ClaudeCode,
            claude_override.as_deref(),
        ),
        crate::runtime::detect_agent_binary_with_version(
            crate::runtime::CodingAgent::Codex,
            codex_override.as_deref(),
        ),
    );
    Ok(Json(AgentBinariesResponse { claude_code, codex }))
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/coding-agents/binaries", get(coding_agent_binaries))
        .route("/claude-code/stop", post(claude_code_stop))
        .route("/claude-code/interrupt", post(claude_code_interrupt))
        .route("/claude-code/control", post(claude_code_control))
        .route("/claude-code/commands", get(claude_code_commands))
        .route("/claude-code/apply-now", post(claude_code_apply_now))
        .route("/claude-code/discard", post(claude_code_discard))
}
