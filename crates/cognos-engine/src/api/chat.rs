use super::actor::build_message_origin;
use super::*;
use crate::engine::http::workspace_client::HEADER_WORKSPACE;
use crate::engine::thread_events::ActorMode;

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

/// Validate `(mode, parent_thread_id, spawning_event_id)` from a `ChatRequest`.
///
/// `mode` is mandatory on the API: callers must explicitly state who originated
/// the message. The mapping is enforced so the source of a thread is never
/// silently inferred:
///
/// - `mode = Human` — must NOT supply `parent_thread_id` or `spawning_event_id`.
///   Human-originated threads have no spawning context.
/// - `mode = Agent | Engine` — `parent_thread_id` is REQUIRED (the spawning thread).
///   `spawning_event_id` is REQUIRED for new external spawns; legacy callers
///   that don't yet know the originating event may omit it, in which case the
///   new thread will be linked to the parent without a specific event pointer.
///
/// Returns `Err(StatusCode::BAD_REQUEST)` if the constraint is violated or any
/// UUID is malformed — failing fast beats silently dropping the parent link.
fn validate_mode_and_spawn(
    request: &ChatRequest,
) -> Result<(Option<Uuid>, Option<Uuid>), StatusCode> {
    let parent_thread_id = match request.parent_thread_id.as_deref() {
        None => None,
        Some(s) => Some(Uuid::parse_str(s).map_err(|_| StatusCode::BAD_REQUEST)?),
    };
    let spawning_event_id = match request.spawning_event_id.as_deref() {
        None => None,
        Some(s) => Some(Uuid::parse_str(s).map_err(|_| StatusCode::BAD_REQUEST)?),
    };
    match request.mode {
        ActorMode::Human => {
            if parent_thread_id.is_some() || spawning_event_id.is_some() {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
        ActorMode::Agent | ActorMode::Engine => {
            if parent_thread_id.is_none() {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }
    Ok((parent_thread_id, spawning_event_id))
}

pub(super) async fn chat(
    State(state): State<AppState>,
    Json(request): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, StatusCode> {
    if let Some(ref model) = request.model {
        log!("Using model: {}", model);
    }

    let (parent_thread_id, spawning_event_id) = validate_mode_and_spawn(&request)?;
    let mode = request.mode;

    // Convert API contexts to engine contexts
    let app_ctx = request
        .app_context
        .map(|ctx| crate::engine::AppContext { app_id: ctx.app_id });
    let file_ctx = resolve_file_ctx(
        request.file_context.as_ref(),
        request.repo_file_context.as_ref(),
    );
    let url_ctx = request.url_context;

    let thread_id = request
        .thread_id
        .as_ref()
        .and_then(|s| Uuid::parse_str(s).ok());
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
    Json(request): Json<ChatRequest>,
) -> Result<Json<ChatSubmitResponse>, StatusCode> {
    // Capture frontend origin from the browser Origin header so the LLM
    // system prompt can show the user-facing CognOS URL. Skip cross-workspace
    // calls (X-Cognos-Workspace present) so a server-to-server caller can't
    // poison the cache with an unrelated host.
    if headers.get(HEADER_WORKSPACE).is_none() {
        if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
            let mut stored = state.engine.frontend_origin.lock().unwrap();
            if stored.is_none() {
                *stored = Some(origin.to_string());
            }
        }
    }
    let (parent_thread_id, spawning_event_id) = validate_mode_and_spawn(&request)?;
    let mode = request.mode;
    let engine_clone = state.engine.clone();
    let message = request.message.clone();
    let model = request.model.clone();
    let reasoning_effort = request.reasoning_effort.clone();
    let device_id = request.device_id.clone();

    // Workspace header takes precedence over device_id in build_message_origin
    // — skip the device-name lookup when present to save a roundtrip on the
    // cross-workspace path. Lookups are otherwise independent, so run them
    // concurrently when both apply.
    let workspace_header_present = headers.get(HEADER_WORKSPACE).is_some();
    let device_lookup = async {
        match device_id.as_deref() {
            Some(did) if !workspace_header_present => {
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
    let origin = build_message_origin(
        &headers,
        mode,
        device_id.as_deref(),
        device_label,
        parent_thread_id,
        parent_thread_title,
        spawning_event_id,
    );

    // Convert API contexts to engine contexts
    let app_ctx = request
        .app_context
        .map(|ctx| crate::engine::AppContext { app_id: ctx.app_id });
    let file_ctx = resolve_file_ctx(
        request.file_context.as_ref(),
        request.repo_file_context.as_ref(),
    );
    let url_ctx = request.url_context;
    let chat_images = request.images.map(super::compress_images);
    let use_claude_code = request.use_claude_code;
    let cc_model = request.cc_model;
    let event_id = request.event_id;
    let thread_id = request
        .thread_id
        .as_ref()
        .and_then(|s| Uuid::parse_str(s).ok());
    let conflict_change_id = request
        .conflict_change_id
        .as_ref()
        .and_then(|s| Uuid::parse_str(s).ok());
    let repo_id = request.repo_id;
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
                        let pending = crate::core::changes::list_pending(engine_clone.pool())
                            .await
                            .unwrap_or_else(|e| {
                                log!(
                                    "[Chat] Failed to list pending changes for auto-apply: {}",
                                    e
                                );
                                Vec::new()
                            });
                        if let Some(change) =
                            pending.iter().find(|c| c.request_id == res.request_id)
                        {
                            match engine_clone
                                .apply_change(change.id, actor_for_apply.clone())
                                .await
                            {
                                Ok(result) => {
                                    log!("Auto-applied change: {}", result.message);
                                }
                                Err(e) => {
                                    log!("Failed to auto-apply change: {}", e);
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
                    let pending = crate::core::changes::list_pending(engine_clone.pool())
                        .await
                        .unwrap_or_else(|e| {
                            log!(
                                "[Chat] Failed to list pending changes for ChangesUpdated: {}",
                                e
                            );
                            Vec::new()
                        });
                    let applied =
                        crate::core::changes::list_recently_applied(engine_clone.pool(), 15, None)
                            .await
                            .unwrap_or_else(|e| {
                                log!(
                                    "[Chat] Failed to list applied changes for ChangesUpdated: {}",
                                    e
                                );
                                Vec::new()
                            });
                    let restart = crate::core::changes::requires_restart_since(
                        engine_clone.pool(),
                        result_started_at,
                    )
                    .await;
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

                // Re-submit orphaned injections as regular follow-up messages.
                // These are messages that arrived after the processing loop exited
                // but before cleanup finished — either agentic loop orphans (from
                // inject_prompt after the loop) or CC lost follow-ups (from msg_tx
                // after CC process exit). MessageReceived was already emitted for
                // each, so pass pre_emitted_origin to skip duplicate emission.
                for orphan in &res.orphaned_injections {
                    let engine = engine_clone.clone();
                    let text = orphan.text.clone();
                    let pre_emitted = orphan.event_id;
                    let orphan_mode = orphan.mode;
                    let orphan_spawning_event_id = orphan.spawning_event_id;
                    let images = orphan.images.clone();
                    let tid = res.thread_id;
                    tokio::spawn(async move {
                        if let Err(e) = engine
                            .process_message_with_steps(
                                &text,
                                None,
                                None,
                                None,
                                None,
                                images.as_deref(),
                                None,
                                None,
                                None,
                                Some(tid),
                                None,
                                None,
                                None,
                                None,
                                orphan_spawning_event_id,
                                orphan_mode,
                                None,
                                pre_emitted,
                                None,
                                None,
                            )
                            .await
                        {
                            log!(
                                "[Chat] Failed to re-process orphaned injection for thread {}: {}",
                                tid,
                                e
                            );
                        }
                    });
                }
            }
            Err(ref e) => {
                log!("Chat error: {}", e);
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
                        log!("Failed to emit ResponseFailed: {}", emit_err);
                    }
                }
            }
        }
    });

    // Monitor the spawned task — if it panics, emit ResponseFailed + SessionEnded
    // and clean up the CC session so the thread doesn't get stuck in "running" state.
    if let Some(tid) = thread_id_for_panic {
        CognosEngine::monitor_cc_task(engine_for_panic, tid, handle);
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
) -> StatusCode {
    if let Some(ref tid) = query.thread_id {
        if let Ok(uuid) = Uuid::parse_str(tid) {
            state.engine.cancel_thread(uuid);
        }
    } else {
        state.engine.cancel_all_threads();
    }
    StatusCode::OK
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
    Json(request): Json<InjectRequest>,
) -> Result<StatusCode, StatusCode> {
    let thread_id = Uuid::parse_str(&request.thread_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    if request.message.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let event_id = request
        .event_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok());

    if state.engine.inject_prompt(
        thread_id,
        request.message,
        event_id,
        crate::engine::thread_events::ActorMode::Human,
        None,
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
        }
    }

    #[test]
    fn human_mode_with_no_spawn_context_is_valid() {
        let req = base_req(ActorMode::Human);
        let (parent, spawning) = validate_mode_and_spawn(&req).unwrap();
        assert_eq!(parent, None);
        assert_eq!(spawning, None);
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
        let (parent, spawning) = validate_mode_and_spawn(&req).unwrap();
        assert_eq!(parent, Some(parent_uuid));
        assert_eq!(spawning, Some(event_uuid));
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
    fn chat_request_legacy_sender_system_deserializes_to_agent_mode() {
        let parent_uuid = Uuid::new_v4();
        let event_uuid = Uuid::new_v4();
        let body = serde_json::json!({
            "message": "spawned task",
            "sender": "system",
            "parent_thread_id": parent_uuid.to_string(),
            "spawning_event_id": event_uuid.to_string(),
            "use_claude_code": true,
        });
        let req: ChatRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.mode, ActorMode::Agent);
        assert_eq!(
            req.parent_thread_id.as_deref(),
            Some(parent_uuid.to_string().as_str())
        );
        assert_eq!(
            req.spawning_event_id.as_deref(),
            Some(event_uuid.to_string().as_str())
        );
    }

    #[test]
    fn chat_request_legacy_sender_user_deserializes_to_human_mode() {
        let body = serde_json::json!({
            "message": "hi",
            "sender": "user",
        });
        let req: ChatRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.mode, ActorMode::Human);
    }

    #[test]
    fn chat_request_new_mode_field_deserializes() {
        let body = serde_json::json!({
            "message": "hi",
            "mode": "human",
        });
        let req: ChatRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.mode, ActorMode::Human);
    }
}
