use super::*;

#[derive(Deserialize)]
pub(super) struct StopQuery {
    /// `apply=true` — user clicked Apply Now. The resulting change auto-applies
    /// after CC terminates; no `ResponseCanceled` (ChangeApplied is the terminator).
    #[serde(default)]
    apply: bool,
    /// `discard=true` — user clicked Discard. Change is dropped; no
    /// `ResponseCanceled` (ChangeDiscarded is the terminator).
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

/// `POST /api/v1/claude-code/stop` — stop a running Claude Code session.
///
/// Three modes via query params:
///   - default: real Cancel/Stop click — emits `ResponseCanceled(UserStop)` if
///     CC was actively working, nothing if CC was already idle.
///   - `apply=true`: Apply Now — change auto-applies after CC terminates.
///   - `discard=true`: Discard — change is dropped.
///
/// Archiving uses a different code path (`POST /api/v1/threads/archive` →
/// `stop_agent(StopReason::Archive, ...)`) because it also emits `ThreadArchived`.
pub(super) async fn claude_code_stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<StopQuery>,
) -> Result<StatusCode, StatusCode> {
    let thread_id = super::parse_optional_uuid(query.thread_id.as_deref())?;
    // Stamp the user actor so any ChangeApplied / ChangeApplyFailed emitted by
    // the stale-session fallback (stop?apply=true on a thread whose CC
    // already exited) carries the device that clicked the button instead of
    // collapsing to "Lucidos Engine" via the actor-missing fallback.
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;

    // Resolve any pending question card before CC ends — otherwise its answer
    // buttons would dangle after the session goes away.
    if let Some(tid) = thread_id {
        crate::engine::agent_question::resolve_pending_question_as_canceled(
            &state.engine,
            tid,
            actor.clone(),
        )
        .await;
    }

    match state
        .engine
        .stop_agent(query.reason(), thread_id, actor)
        .await
    {
        Ok(_) => Ok(StatusCode::OK),
        Err(e) => {
            crate::log!("[API] stop_agent failed: {}", e);
            Err(StatusCode::NOT_FOUND)
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
    headers: HeaderMap,
    Json(body): Json<ControlRequestBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let thread_id = uuid::Uuid::parse_str(&body.thread_id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid thread_id" })),
        )
    })?;
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    state
        .engine
        .send_agent_control_request(thread_id, body.request, actor)
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
    /// Compose-view repo selector. Empty string ("") = the workspace's default
    /// "Lucidos" repo, mirroring the frontend's `selectedRepoId` convention.
    /// Missing = same as empty string.
    repo_id: Option<String>,
}

/// Return available CC commands: control subtypes (always) + categorized slash commands (if a session is active).
pub(super) async fn claude_code_commands(
    State(state): State<AppState>,
    Query(query): Query<CommandsQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let tid = super::parse_optional_uuid(query.thread_id.as_deref())?;
    let res = if let Some(tid) = tid {
        state.engine.cc_categorized_commands(tid).await
    } else {
        // Compose-view: resolve repo_id (possibly empty/missing) to a path
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
    })))
}

#[derive(Deserialize)]
pub(super) struct ThreadIdBody {
    thread_id: String,
}

/// POST /api/v1/claude-code/discard — discard CC changes without ending session
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
    headers: HeaderMap,
    Query(query): Query<InterruptQuery>,
) -> Result<StatusCode, StatusCode> {
    let thread_id = super::parse_optional_uuid(query.thread_id.as_deref())?;
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match state.engine.interrupt_agent(thread_id, actor).await {
        Ok(_) => Ok(StatusCode::OK),
        Err(e) => {
            crate::log!("[API] interrupt_agent failed: {}", e);
            Err(StatusCode::NOT_FOUND)
        }
    }
}

