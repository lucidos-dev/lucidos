use super::actor::build_message_origin;
use super::*;
use crate::engine::thread_events::ActorMode;
use crate::engine::thread_state::ThreadState;
use crate::engine::InjectedPrompt;
use std::collections::VecDeque;

/// Re-process orphaned injections sequentially until the chain settles.
///
/// `process_message_with_steps` for a re-processed orphan can itself produce
/// orphans (a follow-up MessageReceived arrived during the re-process loop
/// but landed on `injection_rx` after the loop's final `try_recv()`). The
/// caller used to fire-and-forget a `tokio::spawn` per orphan, throwing away
/// `ProcessResult` and the inner `orphaned_injections` with it — so an
/// orphan-of-orphan was silently lost, leaving its MR without a response and
/// stamping the NEXT reply with the prior orphan's `request_event_id`
/// (observed on real thread 9b5a05aa as a chat exchange where the second
/// follow-up's MR went unanswered while the third turn's response bound
/// onto the wrong MR). Iterating here keeps the chain bounded by the
/// available orphans and serializes per-thread (`register_thread_queued`
/// already serializes anyway, so a queue costs nothing extra).
async fn process_orphan_chain(
    engine: SharedEngine,
    thread_id: Uuid,
    initial_orphans: Vec<InjectedPrompt>,
) {
    drain_orphan_queue(initial_orphans, |orphan| {
        let engine = engine.clone();
        async move {
            match engine
                .process_message_with_steps(
                    &orphan.text,
                    None,
                    None,
                    None,
                    None,
                    orphan.images.as_deref(),
                    None,
                    None,
                    None,
                    Some(thread_id),
                    None,
                    None,
                    None,
                    None,
                    orphan.spawning_event_id,
                    orphan.mode,
                    None,
                    orphan.event_id,
                    None,
                    None,
                )
                .await
            {
                Ok(res) => res.orphaned_injections,
                Err(e) => {
                    log!(
                        "[Chat] Failed to re-process orphaned injection for thread {}: {}",
                        thread_id,
                        e
                    );
                    Vec::new()
                }
            }
        }
    })
    .await;
}

/// Iterate over `initial`, calling `process_one` per item; any items the
/// processor returns are appended to the queue and processed in turn. Pure
/// queue-drain logic, extracted from `process_orphan_chain` so the
/// orphan-of-orphan invariant ("if A's re-process produces orphan B, B is
/// processed too") is unit-testable without a real engine.
async fn drain_orphan_queue<F, Fut>(initial: Vec<InjectedPrompt>, mut process_one: F)
where
    F: FnMut(InjectedPrompt) -> Fut,
    Fut: std::future::Future<Output = Vec<InjectedPrompt>>,
{
    let mut queue: VecDeque<InjectedPrompt> = initial.into();
    while let Some(item) = queue.pop_front() {
        let new_orphans = process_one(item).await;
        queue.extend(new_orphans);
    }
}

/// Convert API request contexts into engine file context string.
fn resolve_file_ctx(
    file_context: Option<&super::FileContext>,
    repo_file_context: Option<&super::RepoFileContext>,
) -> Option<String> {
    file_context.map(|ctx| ctx.path.clone()).or_else(|| {
        repo_file_context.map(|ctx| {
            if let Some((start, end)) = ctx.lines {
                format!("[repo:{}] {}:{}-{}", ctx.repo_id, ctx.path, start, end)
            } else {
                format!("[repo:{}] {}", ctx.repo_id, ctx.path)
            }
        })
    })
}

/// Parsed spawn / caller UUIDs returned by `validate_mode_and_spawn`.
///
/// Carrying parsed `caller_*` UUIDs out of validation lets the handler avoid
/// re-parsing (which would silently drop on regression — see CLAUDE.md
/// "no silent defaults").
#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedSpawn {
    parent_thread_id: Option<Uuid>,
    spawning_event_id: Option<Uuid>,
    caller_thread_id: Option<Uuid>,
    caller_event_id: Option<Uuid>,
}

/// Validate `mode` against the spawning / caller context from a `ChatRequest`.
///
/// `mode` is mandatory on the API: callers must explicitly state who originated
/// the message. The mapping is enforced so the source of a thread is never
/// silently inferred:
///
/// - `mode = Human` — must NOT supply `parent_thread_id` or `spawning_event_id`.
///   Human-originated threads have no spawning context. May supply
///   `caller_workspace` (e.g. a human curl from another workspace).
/// - `mode = Agent | Engine` — provenance is REQUIRED. Either `parent_thread_id`
///   (same-workspace spawn with callback) OR `caller_workspace`
///   (cross-workspace origin, fire-and-forget) must be present.
///
/// Cross-workspace `caller_*` fields are mutually exclusive with same-workspace
/// `parent_thread_id` / `spawning_event_id`: they describe incompatible
/// relationships (origin without callback vs. parent with callback). A
/// `caller_thread_id` or `caller_event_id` without `caller_workspace` is
/// malformed.
///
/// Returns `Err(StatusCode::BAD_REQUEST)` if the constraint is violated or any
/// UUID is malformed — failing fast beats silently dropping the parent link.
fn validate_mode_and_spawn(request: &ChatRequest) -> Result<ValidatedSpawn, StatusCode> {
    // Cross-workspace caller_* fields are mutually exclusive with same-workspace
    // parent_thread_id / spawning_event_id. They describe incompatible
    // relationships: caller_* = origin (no callback), parent_* = parent (callback).
    let has_caller = request.caller_workspace.is_some()
        || request.caller_thread_id.is_some()
        || request.caller_event_id.is_some();

    if has_caller && request.caller_workspace.is_none() {
        // caller_thread_id / caller_event_id without caller_workspace is malformed.
        return Err(StatusCode::BAD_REQUEST);
    }

    let has_parent =
        request.parent_thread_id.is_some() || request.spawning_event_id.is_some();

    if has_caller && has_parent {
        return Err(StatusCode::BAD_REQUEST);
    }

    let caller_thread_id = parse_optional_uuid(request.caller_thread_id.as_deref())?;
    let caller_event_id = parse_optional_uuid(request.caller_event_id.as_deref())?;
    let parent_thread_id = parse_optional_uuid(request.parent_thread_id.as_deref())?;
    let spawning_event_id = parse_optional_uuid(request.spawning_event_id.as_deref())?;

    match request.mode {
        ActorMode::Human => {
            if parent_thread_id.is_some() || spawning_event_id.is_some() {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
        ActorMode::Agent | ActorMode::Engine => {
            // Agent/Engine mode requires EITHER parent_thread_id (same-workspace
            // spawn with callback) OR caller_workspace (cross-workspace origin).
            // Without either, no provenance — reject.
            if parent_thread_id.is_none() && request.caller_workspace.is_none() {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }

    Ok(ValidatedSpawn {
        parent_thread_id,
        spawning_event_id,
        caller_thread_id,
        caller_event_id,
    })
}

/// Wire-format string for `ThreadType::CodingAgent` (`thread_summaries.source`
/// and the `channel` payload field). Mirrored here to avoid pulling the engine
/// enum across the API boundary just for this single comparison; the source of
/// truth is the `#[serde(rename = "claude_code")]` on `ThreadType::CodingAgent`.
const CC_SOURCE: &str = "claude_code";

/// A thread is locked to the (mode, repo) it picked on its first message.
/// Subsequent follow-ups must match — switching either makes the executor card
/// disagree with the commands menu (the menu collapses to one repo) and breaks
/// the assumption that the thread's worktree branch lives in one repo.
///
/// `existing_source` is the thread's `thread_summaries.source`; `None` means
/// no row yet (new thread — nothing to lock against). `existing_repo_id` is
/// the bound `cc_repo_id`, only meaningful when the source is `claude_code`.
pub(super) fn validate_thread_continuity(
    existing_source: Option<&str>,
    existing_repo_id: Option<&str>,
    requested_use_cc: Option<bool>,
    requested_repo_id: Option<&str>,
) -> Result<(), (StatusCode, String)> {
    let Some(source) = existing_source else {
        return Ok(());
    };
    let existing_is_cc = source == CC_SOURCE;
    let requested_is_cc = requested_use_cc == Some(true);
    if existing_is_cc != requested_is_cc {
        let from = if existing_is_cc { "Claude Code" } else { "Lucidos" };
        let to = if requested_is_cc { "Claude Code" } else { "Lucidos" };
        return Err((
            StatusCode::CONFLICT,
            format!("Thread is locked to {from} mode; cannot switch to {to}"),
        ));
    }
    if existing_is_cc {
        if let (Some(req_repo), Some(existing_repo)) = (requested_repo_id, existing_repo_id) {
            if req_repo != existing_repo {
                return Err((
                    StatusCode::CONFLICT,
                    format!(
                        "Thread is locked to repo {existing_repo}; cannot switch to {req_repo}"
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(super) async fn chat(
    State(state): State<AppState>,
    Json(request): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, StatusCode> {
    if let Some(ref model) = request.model {
        log!("[Chat] Using model: {}", model);
    }

    let validated = validate_mode_and_spawn(&request)?;
    let ValidatedSpawn {
        parent_thread_id,
        spawning_event_id,
        ..
    } = validated;
    let mode = request.mode;

    let app_ctx = request.app_context;
    let file_ctx = resolve_file_ctx(
        request.file_context.as_ref(),
        request.repo_file_context.as_ref(),
    );
    let url_ctx = request.url_context;

    // Reject malformed thread_id with 400 instead of silently starting a new
    // thread (CLAUDE.md "no silent defaults").
    let thread_id = parse_optional_uuid(request.thread_id.as_deref())?;
    Ok(
        match state
            .engine
            .process_message_with_steps(
                &request.message,
                request.model.as_deref(),
                app_ctx,
                file_ctx,
                request.reasoning_effort.as_deref(),
                request.images.as_deref(),
                request.device_id.as_deref(),
                None,
                request.event_id.as_deref(),
                thread_id,
                None,
                None,
                url_ctx,
                parent_thread_id,
                spawning_event_id,
                mode,
                request.cc_model.as_deref(),
                None,
                request.title.as_deref(),
                None,
            )
            .await
        {
            Ok(result) => Json(ChatResponse {
                response: result.response,
                steps: result.steps,
            }),
            Err(e) => Json(ChatResponse {
                response: format!("[ERROR] {}", e),
                steps: vec![],
            }),
        },
    )
}

#[derive(Serialize)]
pub(super) struct ChatSubmitResponse {
    event_id: String,
}

/// POST endpoint for chat with progress updates.
/// Returns immediately with a message_id. All progress events are sent
/// via the global SSE stream as ThreadEvent events.
pub(super) async fn chat_submit(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(mut request): Json<ChatRequest>,
) -> Result<Json<ChatSubmitResponse>, StatusCode> {
    // Capture frontend origin from the browser Origin header so the LLM
    // system prompt can show the user-facing Lucidos URL. Skip cross-workspace
    // calls (a cross-workspace caller — `caller_workspace` set in body) so a
    // server-to-server caller can't poison the cache with an unrelated host.
    if request.caller_workspace.is_none() {
        if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
            let mut stored = state.engine.frontend_origin.lock().unwrap();
            if stored.is_none() {
                *stored = Some(origin.to_string());
            }
        }
    }
    let ValidatedSpawn {
        parent_thread_id,
        spawning_event_id,
        caller_thread_id,
        caller_event_id,
    } = validate_mode_and_spawn(&request)?;
    let mode = request.mode;
    let engine_clone = state.engine.clone();
    let message = request.message.clone();
    let model = request.model.clone();
    let reasoning_effort = request.reasoning_effort.clone();
    let device_id = request.device_id.clone();

    // A cross-workspace caller (`caller_workspace` set in body) takes
    // precedence over device_id in build_message_origin — skip the device-name
    // lookup when present to save a roundtrip on the cross-workspace path.
    // Lookups are otherwise independent, so run them concurrently when both
    // apply.
    let workspace_caller_present = request.caller_workspace.is_some();
    let device_lookup = async {
        match device_id.as_deref() {
            Some(did) if !workspace_caller_present => {
                crate::core::DeviceStore::display_name(state.engine.pool(), did).await
            }
            _ => None,
        }
    };
    let parent_lookup = async {
        match parent_thread_id {
            Some(ptid) => state
                .engine
                .event_store()
                .get_thread_title(ptid)
                .await
                .unwrap_or(None),
            None => None,
        }
    };
    let (device_label, parent_thread_title) = tokio::join!(device_lookup, parent_lookup);

    let caller = request
        .caller_workspace
        .as_ref()
        .map(|workspace| crate::api::actor::CallerOrigin {
            workspace: workspace.clone(),
            thread_id: caller_thread_id,
            event_id: caller_event_id,
            // For a cross-workspace POST, the body `mode` describes the upstream
            // (calling workspace's) actor — that's exactly what CallerOrigin.mode wants.
            mode,
        });

    let origin = build_message_origin(
        &headers,
        mode,
        device_id.as_deref(),
        device_label,
        parent_thread_id,
        parent_thread_title,
        spawning_event_id,
        caller,
    );

    let app_ctx = request.app_context;
    let file_ctx = resolve_file_ctx(
        request.file_context.as_ref(),
        request.repo_file_context.as_ref(),
    );
    let url_ctx = request.url_context;
    // Resolve `image_hashes` to ChatImage by reading + base64-encoding the
    // blobs once per send. Mutually exclusive with `images`; the latter
    // (legacy base64 body) is still accepted and runs through the same
    // compression pipeline. After Phase 4 ships, every frontend send takes
    // the hash path and the legacy body becomes dead.
    let chat_images = if let Some(hashes) = request.image_hashes.take() {
        let mut resolved = Vec::with_capacity(hashes.len());
        for hash in &hashes {
            match crate::core::blobs::read_blob_as_base64(state.engine.workspace_path(), hash) {
                Some((data, mime_type)) => resolved.push(ChatImage { base64: data, mime_type }),
                None => log!(
                    "[Chat] image_hashes: blob {} missing on disk, dropping from message",
                    hash
                ),
            }
        }
        Some(resolved)
    } else {
        request.images.take().map(super::compress_images)
    };
    let use_claude_code = request.use_claude_code;
    let cc_model = request.cc_model;
    let event_id = request.event_id;
    // Reject malformed ids with 400 instead of silently dropping them and
    // starting a fresh thread / unscoped change (CLAUDE.md "no silent defaults").
    let thread_id = parse_optional_uuid(request.thread_id.as_deref())?;
    let conflict_change_id = parse_optional_uuid(request.conflict_change_id.as_deref())?;
    // `repo_id` accepts either a UUID or a registered repo name (the
    // `lucidos spawn-thread --repo <name>` CLI sends names). UUIDs pass
    // through unchanged — they match `cc_repo_id` storage in
    // `thread_summaries` directly, and downstream code already handles
    // unknown UUIDs (the tests seeding random UUIDs depend on that). Names
    // resolve to the registered UUID; an unknown name surfaces as a clean
    // 400 here instead of a 500 from deep inside worktree creation.
    let repo_id = match request.repo_id.as_deref() {
        Some(s) if !s.is_empty() && Uuid::parse_str(s).is_err() => {
            match crate::core::repositories::RepositoryStore::get_by_name(state.engine.pool(), s).await {
                Ok(Some(repo)) => Some(repo.id.to_string()),
                Ok(None) => {
                    log!("[Chat] Unknown repo name '{}' in chat_submit", s);
                    return Err(StatusCode::BAD_REQUEST);
                }
                Err(e) => {
                    log!("[Chat] Failed to look up repo '{}': {}", s, e);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
                }
            }
        }
        _ => request.repo_id,
    };

    // Lock thread to its first (mode, repo). Switching either mid-thread
    // makes the executor card and commands menu disagree (different repo's
    // skills, branch can't follow across repos). Frontend should already
    // disable the selectors for existing threads — this is the backend
    // backstop. See `validate_thread_continuity`.
    //
    // Skip the lock for `state='composing'`: a draft's `source` reflects the
    // last compose-mode toggle (the PUT compose CASE writes 'chat' or
    // 'claude_code' to it). Toggling back across modes before the first send
    // races the debounced PUT; reading the lagged source as authoritative
    // here surfaces as a 409 ("Thread is locked to X mode") on Send. The
    // lock only applies once the thread has actually been sent.
    if let Some(tid) = thread_id {
        let row: Option<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT state, source, cc_repo_id FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(tid)
        .fetch_optional(state.engine.pool())
        .await
        .map_err(|e| {
            log!("[Chat] thread_summaries lookup failed for {}: {}", tid, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        if let Some((state_str, source, existing_repo)) = row {
            let existing_state = ThreadState::from_db_str(&state_str).map_err(|e| {
                log!("[Chat] thread_summaries.state for {} invalid: {}", tid, e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            if !existing_state.can_change_mode() {
                if let Err((sc, msg)) = validate_thread_continuity(
                    Some(&source),
                    existing_repo.as_deref(),
                    use_claude_code,
                    repo_id.as_deref(),
                ) {
                    log!("[Chat] Reject follow-up on thread {}: {}", tid, msg);
                    return Err(sc);
                }
            }
        }
    }
    let title = request.title;
    // Generate an event_id for tracking progress events
    let response_event_id = event_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    // Spawn task to process message — all events flow through EventBus now.
    // The JoinHandle is monitored so panics emit ResponseFailed + SessionEnded
    // instead of silently dropping the thread into a stuck "running" state.
    let result_started_at = state.started_at;
    let engine_for_panic = state.engine.clone();
    let thread_id_for_panic = thread_id;
    let actor_for_apply = origin.clone();
    let handle = tokio::spawn(async move {
        let result = engine_clone
            .process_message_with_steps(
                &message,
                model.as_deref(),
                app_ctx,
                file_ctx,
                reasoning_effort.as_deref(),
                chat_images.as_deref(),
                device_id.as_deref(),
                use_claude_code,
                event_id.as_deref(),
                thread_id,
                conflict_change_id,
                repo_id.as_deref(),
                url_ctx,
                parent_thread_id,
                spawning_event_id,
                mode,
                cc_model.as_deref(),
                None,
                title.as_deref(),
                origin,
            )
            .await;

        // Post-processing: auto-apply changes and broadcast updates
        match result {
            Ok(ref res) => {
                if res.proposed_change {
                    if res.auto_apply {
                        let pending = engine_clone.changes().list_pending().await;
                        if let Some(change) =
                            pending.iter().find(|c| c.request_id == res.request_id)
                        {
                            match engine_clone
                                .apply_change(change.id, actor_for_apply.clone())
                                .await
                            {
                                Ok(result) => {
                                    log!("[Chat] Auto-applied change: {}", result.message);
                                }
                                Err(e) => {
                                    log!("[Chat] Failed to auto-apply change: {}", e);
                                    engine_clone
                                        .event_bus
                                        .emit_or_log(
                                            crate::engine::event_bus::BusEvent::System(
                                                crate::engine::event_bus::SystemEvent::Toast {
                                                    message: format!(
                                                        "Failed to apply change: {}",
                                                        e
                                                    ),
                                                    level: "error".to_string(),
                                                },
                                            ),
                                            "[Chat] auto-apply error Toast",
                                        )
                                        .await;
                                }
                            }
                        }
                    }
                    let proj = engine_clone.changes();
                    let mut pending = proj.list_pending().await;
                    let mut applied = proj.list_recently_applied(15, None).await;
                    let restart = proj.requires_restart_since(result_started_at).await;
                    let (r1, r2) = tokio::join!(
                        crate::core::changes::enrich_thread_titles(
                            engine_clone.pool(),
                            &mut pending,
                        ),
                        crate::core::changes::enrich_thread_titles(
                            engine_clone.pool(),
                            &mut applied,
                        ),
                    );
                    if let Err(e) = r1 {
                        log!("[Chat] enrich pending titles: {}", e);
                    }
                    if let Err(e) = r2 {
                        log!("[Chat] enrich applied titles: {}", e);
                    }
                    engine_clone
                        .event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::System(
                                crate::engine::event_bus::SystemEvent::ChangesUpdated {
                                    total_pending: pending.len(),
                                    pending,
                                    applied,
                                    restart_required: restart,
                                },
                            ),
                            "[Chat] ChangesUpdated",
                        )
                        .await;
                }

                // Re-submit orphaned injections (follow-ups that arrived after
                // the processing loop exited but before cleanup finished). See
                // `process_orphan_chain` for the chain-drain invariant.
                if !res.orphaned_injections.is_empty() {
                    let engine = engine_clone.clone();
                    let tid = res.thread_id;
                    let orphans = res.orphaned_injections.clone();
                    tokio::spawn(async move {
                        process_orphan_chain(engine, tid, orphans).await;
                    });
                }
            }
            Err(ref e) => {
                log!("[Chat] Chat error: {}", e);
                // Signal frontend to exit "running" state with error
                if let Some(tid) = thread_id {
                    if let Err(emit_err) = engine_clone
                        .event_bus
                        .emit(crate::engine::event_bus::BusEvent::Thread {
                            thread_id: tid,
                            event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                error: e.to_string(),
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        })
                        .await
                    {
                        log!("[Chat] Failed to emit ResponseFailed: {}", emit_err);
                    }
                }
            }
        }
    });

    // Monitor the spawned task — if it panics, emit ResponseFailed + SessionEnded
    // and clean up the CC session so the thread doesn't get stuck in "running" state.
    if let Some(tid) = thread_id_for_panic {
        LucidosEngine::monitor_cc_task(engine_for_panic, tid, handle);
    } else {
        tokio::spawn(async move {
            if let Err(join_err) = handle.await {
                if join_err.is_panic() {
                    log!("[Chat] Task panicked (no thread context): {:?}", join_err);
                }
            }
        });
    }

    Ok(Json(ChatSubmitResponse {
        event_id: response_event_id,
    }))
}

#[derive(Deserialize)]
pub(super) struct CancelChatQuery {
    thread_id: Option<String>,
}

pub(super) async fn cancel_chat(
    State(state): State<AppState>,
    Query(query): Query<CancelChatQuery>,
) -> Result<StatusCode, StatusCode> {
    match parse_optional_uuid(query.thread_id.as_deref())? {
        Some(uuid) => {
            state.engine.cancel_thread(uuid);
        }
        None => state.engine.cancel_all_threads(),
    }
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
pub(super) struct InjectRequest {
    thread_id: String,
    message: String,
    event_id: Option<String>,
}

/// POST /api/chat/inject — inject a user prompt into an active agentic loop.
/// The injected text is picked up between tool iterations, allowing mid-flight corrections.
/// Returns 409 if the thread is not currently active.
pub(super) async fn inject_prompt(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<InjectRequest>,
) -> Result<StatusCode, StatusCode> {
    let thread_id = Uuid::parse_str(&request.thread_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    if request.message.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Reject malformed event_id with 400 instead of silently dropping the
    // injection's correlation handle (CLAUDE.md "no silent defaults").
    let event_id = parse_optional_uuid(request.event_id.as_deref())?;

    // Stamp the user actor so the injected MessageReceived event carries the
    // device that submitted it (mutating-endpoint actor rule).
    let origin = super::actor::user_actor_resolved(&headers, &state.pool, None).await;

    if state.engine.inject_prompt(
        thread_id,
        request.message,
        event_id,
        crate::engine::thread_events::ActorMode::Human,
        None,
        origin,
    ) {
        Ok(StatusCode::OK)
    } else {
        Err(StatusCode::CONFLICT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_req(mode: ActorMode) -> ChatRequest {
        ChatRequest {
            message: "hi".into(),
            model: None,
            app_context: None,
            file_context: None,
            url_context: None,
            repo_file_context: None,
            reasoning_effort: None,
            images: None,
            image_hashes: None,
            device_id: None,
            use_claude_code: None,
            cc_model: None,
            event_id: None,
            thread_id: None,
            conflict_change_id: None,
            repo_id: None,
            title: None,
            mode,
            parent_thread_id: None,
            spawning_event_id: None,
            caller_workspace: None,
            caller_thread_id: None,
            caller_event_id: None,
        }
    }

    #[test]
    fn human_mode_with_no_spawn_context_is_valid() {
        let req = base_req(ActorMode::Human);
        let v = validate_mode_and_spawn(&req).unwrap();
        assert_eq!(v.parent_thread_id, None);
        assert_eq!(v.spawning_event_id, None);
    }

    #[test]
    fn human_mode_with_parent_thread_id_returns_400() {
        let mut req = base_req(ActorMode::Human);
        req.parent_thread_id = Some(Uuid::new_v4().to_string());
        assert_eq!(
            validate_mode_and_spawn(&req),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn human_mode_with_spawning_event_id_returns_400() {
        let mut req = base_req(ActorMode::Human);
        req.spawning_event_id = Some(Uuid::new_v4().to_string());
        assert_eq!(
            validate_mode_and_spawn(&req),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn agent_mode_with_parent_and_spawning_event_is_valid() {
        let parent_uuid = Uuid::new_v4();
        let event_uuid = Uuid::new_v4();
        let mut req = base_req(ActorMode::Agent);
        req.parent_thread_id = Some(parent_uuid.to_string());
        req.spawning_event_id = Some(event_uuid.to_string());
        let v = validate_mode_and_spawn(&req).unwrap();
        assert_eq!(v.parent_thread_id, Some(parent_uuid));
        assert_eq!(v.spawning_event_id, Some(event_uuid));
    }

    #[test]
    fn agent_mode_without_parent_thread_id_returns_400() {
        let req = base_req(ActorMode::Agent);
        assert_eq!(
            validate_mode_and_spawn(&req),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn engine_mode_without_parent_thread_id_returns_400() {
        let req = base_req(ActorMode::Engine);
        assert_eq!(
            validate_mode_and_spawn(&req),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn engine_mode_with_caller_workspace_is_valid() {
        let mut req = base_req(ActorMode::Engine);
        req.caller_workspace = Some("personal".into());
        assert!(validate_mode_and_spawn(&req).is_ok());
    }

    #[test]
    fn invalid_parent_uuid_returns_400() {
        let mut req = base_req(ActorMode::Agent);
        req.parent_thread_id = Some("not-a-uuid".into());
        assert_eq!(
            validate_mode_and_spawn(&req),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn invalid_spawning_event_uuid_returns_400() {
        let mut req = base_req(ActorMode::Agent);
        req.parent_thread_id = Some(Uuid::new_v4().to_string());
        req.spawning_event_id = Some("not-a-uuid".into());
        assert_eq!(
            validate_mode_and_spawn(&req),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn chat_request_requires_mode() {
        let body = serde_json::json!({
            "message": "spawned task",
            "use_claude_code": true,
        });
        let result: Result<ChatRequest, _> = serde_json::from_value(body);
        assert!(result.is_err(), "ChatRequest must require mode field");
    }

    #[test]
    fn chat_request_mode_field_deserializes() {
        let body = serde_json::json!({
            "message": "hi",
            "mode": "human",
        });
        let req: ChatRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.mode, ActorMode::Human);
    }

    #[test]
    fn caller_workspace_with_parent_thread_id_returns_400() {
        let mut req = base_req(ActorMode::Agent);
        req.caller_workspace = Some("personal".into());
        req.parent_thread_id = Some(Uuid::new_v4().to_string());
        assert_eq!(
            validate_mode_and_spawn(&req),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn caller_workspace_with_spawning_event_id_returns_400() {
        let mut req = base_req(ActorMode::Agent);
        req.caller_workspace = Some("personal".into());
        req.spawning_event_id = Some(Uuid::new_v4().to_string());
        assert_eq!(
            validate_mode_and_spawn(&req),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn caller_thread_id_without_caller_workspace_returns_400() {
        // caller_thread_id only meaningful in conjunction with caller_workspace —
        // an orphan caller id is a malformed request, not a silent drop.
        let mut req = base_req(ActorMode::Human);
        req.caller_thread_id = Some(Uuid::new_v4().to_string());
        assert_eq!(
            validate_mode_and_spawn(&req),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn caller_event_id_without_caller_workspace_returns_400() {
        let mut req = base_req(ActorMode::Human);
        req.caller_event_id = Some(Uuid::new_v4().to_string());
        assert_eq!(
            validate_mode_and_spawn(&req),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn caller_thread_id_invalid_uuid_returns_400() {
        let mut req = base_req(ActorMode::Agent);
        req.caller_workspace = Some("personal".into());
        req.caller_thread_id = Some("not-a-uuid".into());
        assert_eq!(
            validate_mode_and_spawn(&req),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn caller_event_id_invalid_uuid_returns_400() {
        let mut req = base_req(ActorMode::Agent);
        req.caller_workspace = Some("personal".into());
        req.caller_event_id = Some("not-a-uuid".into());
        assert_eq!(
            validate_mode_and_spawn(&req),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn caller_workspace_with_only_workspace_field_is_valid() {
        // Caller workspace alone is fine — caller_thread_id / caller_event_id
        // are optional. Common case: human curl from another workspace.
        let mut req = base_req(ActorMode::Human);
        req.caller_workspace = Some("personal".into());
        let v = validate_mode_and_spawn(&req).unwrap();
        assert_eq!(v.parent_thread_id, None);
        assert_eq!(v.spawning_event_id, None);
    }

    #[test]
    fn caller_workspace_with_all_three_fields_is_valid() {
        let mut req = base_req(ActorMode::Agent);
        req.caller_workspace = Some("personal".into());
        req.caller_thread_id = Some(Uuid::new_v4().to_string());
        req.caller_event_id = Some(Uuid::new_v4().to_string());
        assert!(validate_mode_and_spawn(&req).is_ok());
    }

    // -- validate_thread_continuity ---------------------------------------

    #[test]
    fn continuity_new_thread_is_always_ok() {
        // No existing summary => new thread, anything goes
        assert!(validate_thread_continuity(None, None, None, None).is_ok());
        assert!(validate_thread_continuity(None, None, Some(true), Some("repo-a")).is_ok());
    }

    #[test]
    fn continuity_chat_thread_with_chat_followup_is_ok() {
        assert!(validate_thread_continuity(Some("chat"), None, None, None).is_ok());
        assert!(validate_thread_continuity(Some("chat"), None, Some(false), None).is_ok());
    }

    #[test]
    fn continuity_chat_thread_rejects_cc_followup() {
        let err = validate_thread_continuity(Some("chat"), None, Some(true), Some("repo-a"))
            .unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert!(err.1.contains("Lucidos"));
        assert!(err.1.contains("Claude Code"));
    }

    #[test]
    fn continuity_cc_thread_rejects_chat_followup() {
        let err = validate_thread_continuity(Some("claude_code"), Some("repo-a"), None, None)
            .unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
        let err = validate_thread_continuity(
            Some("claude_code"),
            Some("repo-a"),
            Some(false),
            None,
        )
        .unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
    }

    #[test]
    fn continuity_cc_thread_with_matching_repo_is_ok() {
        assert!(validate_thread_continuity(
            Some("claude_code"),
            Some("repo-a"),
            Some(true),
            Some("repo-a"),
        )
        .is_ok());
    }

    #[test]
    fn continuity_cc_thread_rejects_different_repo() {
        let err = validate_thread_continuity(
            Some("claude_code"),
            Some("repo-a"),
            Some(true),
            Some("repo-b"),
        )
        .unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert!(err.1.contains("repo-a"));
        assert!(err.1.contains("repo-b"));
    }

    #[test]
    fn continuity_cc_thread_with_no_request_repo_is_ok() {
        // Request omits repo_id => frontend will inherit from the thread.
        // Don't 409 just because the field is missing.
        assert!(validate_thread_continuity(
            Some("claude_code"),
            Some("repo-a"),
            Some(true),
            None,
        )
        .is_ok());
    }

    #[test]
    fn continuity_cc_thread_with_no_existing_repo_is_ok() {
        // First CC session bound but cc_repo_id wasn't recorded (e.g. older
        // event before SessionStarted carried repo_id). Don't gate on a
        // missing existing value — just let the request through.
        assert!(validate_thread_continuity(
            Some("claude_code"),
            None,
            Some(true),
            Some("repo-b"),
        )
        .is_ok());
    }

    #[test]
    fn continuity_trigger_thread_treated_as_chat() {
        // Trigger threads aren't claude_code, so use_claude_code=true is a
        // mode switch and must be rejected.
        let err = validate_thread_continuity(Some("trigger"), None, Some(true), None).unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
    }

    // ── drain_orphan_queue ──

    fn make_orphan(text: &str) -> InjectedPrompt {
        InjectedPrompt {
            text: text.to_string(),
            event_id: Some(Uuid::new_v4()),
            mode: ActorMode::Human,
            spawning_event_id: None,
            images: None,
            origin: None,
            kind: crate::engine::InjectedPromptKind::UserText,
        }
    }

    #[tokio::test]
    async fn drain_orphan_queue_processes_each_initial_orphan_in_order() {
        let initial = vec![make_orphan("a"), make_orphan("b"), make_orphan("c")];
        let processed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let processed_clone = processed.clone();
        drain_orphan_queue(initial, move |orphan| {
            let processed = processed_clone.clone();
            async move {
                processed.lock().unwrap().push(orphan.text.clone());
                Vec::new()
            }
        })
        .await;
        assert_eq!(*processed.lock().unwrap(), vec!["a", "b", "c"]);
    }

    // Regression: a re-processed orphan whose own loop produces NEW orphans
    // (the in-the-wild thread 9b5a05aa scenario where the user sent two
    // follow-ups in quick succession during recovery) used to lose those
    // child orphans because the spawned task discarded the ProcessResult.
    // The chain must keep draining until every appended orphan is processed.
    #[tokio::test]
    async fn drain_orphan_queue_processes_orphans_of_orphans() {
        // Orphan "a" produces orphan "b" when processed; orphan "b" produces
        // nothing. Both must be processed.
        let initial = vec![make_orphan("a")];
        let processed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let processed_clone = processed.clone();
        drain_orphan_queue(initial, move |orphan| {
            let processed = processed_clone.clone();
            async move {
                let text = orphan.text.clone();
                processed.lock().unwrap().push(text.clone());
                if text == "a" {
                    vec![make_orphan("b")]
                } else {
                    Vec::new()
                }
            }
        })
        .await;
        assert_eq!(*processed.lock().unwrap(), vec!["a", "b"]);
    }

    #[tokio::test]
    async fn drain_orphan_queue_handles_deep_chains() {
        // Each orphan produces one more orphan, four levels deep.
        let initial = vec![make_orphan("0")];
        let processed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let processed_clone = processed.clone();
        drain_orphan_queue(initial, move |orphan| {
            let processed = processed_clone.clone();
            async move {
                let n: i32 = orphan.text.parse().unwrap();
                processed.lock().unwrap().push(orphan.text);
                if n < 4 {
                    vec![make_orphan(&(n + 1).to_string())]
                } else {
                    Vec::new()
                }
            }
        })
        .await;
        assert_eq!(*processed.lock().unwrap(), vec!["0", "1", "2", "3", "4"]);
    }

    #[tokio::test]
    async fn drain_orphan_queue_no_op_for_empty_initial() {
        let processed = std::sync::Arc::new(std::sync::Mutex::new(0usize));
        let processed_clone = processed.clone();
        drain_orphan_queue(vec![], move |_orphan| {
            let processed = processed_clone.clone();
            async move {
                *processed.lock().unwrap() += 1;
                Vec::new()
            }
        })
        .await;
        assert_eq!(*processed.lock().unwrap(), 0);
    }
}
