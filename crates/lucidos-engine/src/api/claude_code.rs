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

/// Which clause-4 verb this route is, for the authority gate. One route, three
/// verbs: the query params pick which button was pressed, and the caller reads
/// a refusal naming the one it tried.
fn stop_verb(
    reason: crate::engine::claude_code::StopReason,
) -> super::thread_reach::ThreadReachVerb {
    use super::thread_reach::ThreadReachVerb;
    use crate::engine::claude_code::StopReason;
    match reason {
        StopReason::Apply => ThreadReachVerb::Apply,
        StopReason::Discard => ThreadReachVerb::Discard,
        _ => ThreadReachVerb::Cancel,
    }
}

/// `POST /api/v1/claude-code/stop` — stop a running Claude Code session.
///
/// Three modes via query params:
///   - default (`UserStop`): real Cancel/Stop click. Routed through CC's native
///     **interrupt (Esc)** via `interrupt_agent` — the turn is interrupted but
///     the session stays resumable (the next message `--resume`s the same
///     `cc_session_id` on the same branch). Emits `ResponseCanceled(UserStop)` +
///     `CodingAgentIdled` if CC was working; a no-op (HTTP 200) if CC was
///     already idle. NOT a hard kill — that lost all conversation context.
///   - `apply=true`: Apply Now — `stop_agent` hard-stops, then the change
///     auto-applies (`ChangeApplied` is the terminator).
///   - `discard=true`: Discard — `stop_agent` hard-stops, change dropped
///     (`ChangeDiscarded` is the terminator).
///
/// Archiving uses a different code path (`POST /api/v1/threads/archive` →
/// `stop_agent(StopReason::Archive, ...)`) because it also emits `ThreadArchived`.
pub(super) async fn claude_code_stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<StopQuery>,
) -> Result<Json<super::CancelResponse>, ApiError> {
    let thread_id = super::parse_optional_uuid(query.thread_id.as_deref())?;
    // Before the question card is cancel-stamped and before any hard stop, so
    // a refusal leaves the turn exactly as it found it. Apply and Discard reach
    // this route too, and each is gated as itself.
    super::thread_reach::refuse_without_authority(
        &state.pool,
        &headers,
        thread_id,
        stop_verb(query.reason()),
    )
    .await?;
    // Stamp the user actor so any ChangeApplied / ChangeApplyFailed emitted by
    // the stale-session fallback (stop?apply=true on a thread whose CC
    // already exited) carries the device that clicked the button instead of
    // collapsing to "Lucidos Engine" via the actor-missing fallback.
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;

    // Resolve any pending question card before CC ends — otherwise its answer
    // buttons would dangle after the session goes away. A resolved card counts
    // toward `canceled`: it IS a status-changing event the client will receive.
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

    // Cancel (Stop) = Esc: route a real `UserStop` through CC's native interrupt
    // so the turn is interrupted but the session stays resumable — the next
    // message `--resume`s the same `cc_session_id` on the same branch. Apply /
    // Discard keep `stop_agent`: each carries its own terminator
    // (`ChangeApplied` / `ChangeDiscarded`) and must hard-stop, not interrupt.
    use crate::engine::claude_code::{SettleTerminal, StopReason, SESSION_ALREADY_WAITING};
    let reason = query.reason();
    match reason {
        // `canceled = false` means nothing was interruptible — the client's
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
            // Cancel click racing an already-finished turn — nothing to
            // interrupt. Report `canceled` = whether the question card resolved
            // (the only status-changing effect this click could have had).
            Err(e) if e.to_string() == SESSION_ALREADY_WAITING => Ok(Json(super::CancelResponse {
                canceled: question_resolved,
            })),
            Err(e) => {
                crate::log!("[API] claude_code_stop ({:?}) failed: {}", reason, e);
                Err(StatusCode::NOT_FOUND.into())
            }
        },
        // Apply / Discard: the terminator is `ChangeApplied` / `ChangeDiscarded`,
        // and callers don't read `canceled` — report `true` on success.
        other => match state.engine.stop_agent(other, thread_id, actor).await {
            Ok(()) => Ok(Json(super::CancelResponse { canceled: true })),
            Err(e) => {
                crate::log!("[API] claude_code_stop ({:?}) failed: {}", reason, e);
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

/// Classify an `apply_now` error into its HTTP status.
///
/// `404` is not a generic failure here, it is a SIGNAL: the frontend
/// (`endClaudeCodeAndApply`) reads it as "no live coding-agent session" and
/// falls back to applying the thread's pending changes one at a time. `409` is
/// its "already applying" branch, which keeps the spinner and arms the safety
/// timeout. So every refusal that is not literally "there is no session" has to
/// be `409`, or the UI runs the wrong fallback.
///
/// The merge-ownership refusal is matched by identity against the const rather
/// than by another substring sniff, so rewording the message cannot silently
/// reclassify it as "no session".
fn apply_now_error_status(msg: &str) -> StatusCode {
    if msg.contains("already in progress") || msg == crate::engine::MERGE_OWNED_BY_RESOLVER_MESSAGE
    {
        StatusCode::CONFLICT
    } else {
        StatusCode::NOT_FOUND
    }
}

pub(super) async fn claude_code_apply_now(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ApplyNowQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let thread_id = uuid::Uuid::parse_str(&query.thread_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    // Apply by another route, so it takes the same gate. Before the merge, so a
    // refusal lands nothing on main.
    super::thread_reach::refuse_without_authority(
        &state.pool,
        &headers,
        Some(thread_id),
        super::thread_reach::ThreadReachVerb::Apply,
    )
    .await?;
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match state.engine.apply_now(thread_id, actor).await {
        Ok(_) => Ok(Json(serde_json::json!({ "status": "applying" }))),
        Err(e) => {
            let msg = e.to_string();
            crate::log!("[API] apply_now failed for {}: {}", thread_id, msg);
            Err(apply_now_error_status(&msg).into())
        }
    }
}

#[cfg(test)]
mod apply_now_status_tests {
    use super::*;

    /// The refusal added with the merge-ownership guard must reach the
    /// frontend's "already applying" branch. As a 404 it would instead hit the
    /// "no live session" fallback, which loops over the thread's pending
    /// changes calling `applyChange` on each: the wrong fallback for a thread
    /// whose merge a resolver already owns.
    #[test]
    fn a_resolver_owned_merge_is_a_conflict_not_a_missing_session() {
        assert_eq!(
            apply_now_error_status(crate::engine::MERGE_OWNED_BY_RESOLVER_MESSAGE),
            StatusCode::CONFLICT
        );
    }

    /// The pre-existing concurrent-apply refusal keeps its 409.
    #[test]
    fn a_concurrent_apply_stays_a_conflict() {
        assert_eq!(
            apply_now_error_status("Apply is already in progress for this thread"),
            StatusCode::CONFLICT
        );
    }

    /// Anything else still reads as "no live session" so the frontend can fall
    /// back to applying the pending changes directly.
    #[test]
    fn an_unrecognised_failure_still_signals_no_live_session() {
        assert_eq!(
            apply_now_error_status("Session channel closed"),
            StatusCode::NOT_FOUND
        );
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
    /// Compose-view backend selector (`claude-code` | `codex`). Only
    /// consulted when no `thread_id` is given — an existing thread's backend
    /// comes from its `thread_summaries` row, not the client.
    coding_agent: Option<crate::runtime::CodingAgent>,
}

/// Return available coding-agent commands: control subtypes (always, per
/// backend) + categorized slash commands (CC only, if a session is active).
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
    if coding_agent == crate::runtime::CodingAgent::Codex {
        // Codex has no slash commands / skills surface — only the control
        // menu. Model/effort still come from the session settings events so
        // the picker shows the thread's current selection.
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
        })));
    }
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
    // Discard by another route, so it takes the same gate. Before the branch
    // and worktree go.
    super::thread_reach::refuse_without_authority(
        &state.pool,
        &headers,
        Some(thread_uuid),
        super::thread_reach::ThreadReachVerb::Discard,
    )
    .await
    .map_err(|e| (e.status_code(), e.to_string()))?;
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
) -> Result<StatusCode, ApiError> {
    let thread_id = super::parse_optional_uuid(query.thread_id.as_deref())?;
    // Cancel by another route, so it takes the same gate.
    super::thread_reach::refuse_without_authority(
        &state.pool,
        &headers,
        thread_id,
        super::thread_reach::ThreadReachVerb::Cancel,
    )
    .await?;
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    // No question card is cancel-stamped on this route, so a `running` row it
    // settles is one this request did not write.
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
            crate::log!("[API] interrupt_agent failed: {}", e);
            Err(StatusCode::NOT_FOUND.into())
        }
    }
}

/// Response of `GET /api/v1/coding-agents/binaries` — effective CLI binary
/// resolution per coding agent, plus each resolved binary's self-reported
/// version, for the Settings → Coding Agents surface. Live detection (see
/// `runtime::detect_agent_binary_with_version`); the override itself is the
/// `coding_agent_*_path` preference written via `PUT /api/v1/preferences`.
#[derive(Serialize)]
pub(super) struct AgentBinariesResponse {
    claude_code: crate::runtime::AgentBinaryStatus,
    codex: crate::runtime::AgentBinaryStatus,
}

pub(super) async fn coding_agent_binaries(
    State(state): State<AppState>,
) -> Result<Json<AgentBinariesResponse>, (StatusCode, String)> {
    let read_override = |key: &'static str| {
        let pool = state.pool.clone();
        async move {
            crate::core::PreferenceStore::get(&pool, key)
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to read {key}: {e}"),
                    )
                })
                .map(|v| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
        }
    };
    let claude_override = read_override(crate::core::PREF_CODING_AGENT_CLAUDE_PATH).await?;
    let codex_override = read_override(crate::core::PREF_CODING_AGENT_CODEX_PATH).await?;
    // Concurrently: each agent's detection now runs `<binary> --version`, so
    // sequencing them would make the section pay both probes back to back.
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

/// Routes for the `/claude-code/*` surface.
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
