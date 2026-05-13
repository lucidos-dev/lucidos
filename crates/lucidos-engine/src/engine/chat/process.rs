use crate::core::{
    CredentialStore, EmailStore, OAuthStore, PreferenceStore, PREF_MODEL_IMAGE_DESCRIPTION,
    PREF_MODEL_MEMORY, PREF_MODEL_TITLE,
};
use crate::llm::{
    get_default_tools, get_image_generation_tool, get_mcp_tools, get_notification_tool,
    get_save_thread_image_tool, Message,
};
use crate::runtime::BrowserLogins;
use chrono::Utc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::context::{
    format_history_content, format_history_steps, trim_history_from_oldest,
    truncate_head_tail, AGENT_CONTEXT_CHAR_BUDGET, HISTORY_COMPRESS_THRESHOLD,
    HISTORY_RECENT_MESSAGES, HISTORY_VERBATIM_TAIL,
};
use crate::engine::thread_events::{ActorMode, EventChannel, MessageOrigin, TriggerInvocation};
use crate::engine::types::*;
use crate::engine::LucidosEngine;

use super::events::{
    describe_images, emit_routing_failure, format_relative_age, make_message_received,
};
use super::images::{
    build_user_content_with_images, filter_recent_history_image_hashes, image_recency_cutoff,
    save_images_to_tmp, MAX_HISTORY_IMAGE_MESSAGES,
};
use super::process_helpers::{
    build_system_knowhow_section, build_trigger_knowhow_section, build_trigger_started_event,
    classify_or_fallback, summarize_or_fallback, TriggerContext, ENGINE_RESTART_RULE,
};
use super::recursion_guard::MAX_THREAD_DEPTH;
use super::title::emit_generated_title;

impl LucidosEngine {
    /// Process a trigger prompt (emits TriggerStarted instead of MessageReceived).
    /// `invocation` records which path fired this run (cron schedule or matched event).
    /// `external_cancel`, when set, is forwarded into the per-thread cancellation
    /// token so the scheduler can stop an in-flight trigger cleanly (UI delete,
    /// disable, update) without aborting the agentic loop mid-tool.
    #[allow(clippy::too_many_arguments)]
    pub async fn process_trigger(
        &self,
        trigger_id: &str,
        trigger_name: &str,
        slug: &str,
        prompt: &str,
        invocation: TriggerInvocation,
        go_to_review: bool,
        external_cancel: Option<CancellationToken>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        // Chat-pref resolution lives in `process_message_with_steps_internal`
        // — pass `None` here and let the single canonical resolver apply it.
        self.process_message_with_steps_internal(
            prompt,
            None,
            Some(TriggerContext {
                trigger_id: trigger_id.to_string(),
                trigger_name: trigger_name.to_string(),
                slug: slug.to_string(),
                invocation,
                go_to_review,
            }),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            None,
            None,
            external_cancel,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn process_message_with_steps(
        &self,
        user_message: &str,
        model_override: Option<&str>,
        app_context: Option<AppContext>,
        file_context: Option<String>,
        reasoning_effort: Option<&str>,
        images: Option<&[crate::api::ChatImage]>,
        device_id: Option<&str>,
        use_claude_code: Option<bool>,
        event_id: Option<&str>,
        thread_id: Option<Uuid>,
        conflict_change_id: Option<Uuid>,
        repo_id: Option<&str>,
        url_context: Option<crate::api::UrlContext>,
        parent_thread_id: Option<Uuid>,
        spawning_event_id: Option<Uuid>,
        mode: ActorMode,
        cc_model: Option<&str>,
        pre_emitted_origin: Option<Uuid>,
        title: Option<&str>,
        origin: Option<MessageOrigin>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        self.process_message_with_steps_internal(
            user_message,
            model_override,
            None,
            app_context,
            file_context,
            reasoning_effort,
            images,
            device_id,
            use_claude_code,
            event_id,
            thread_id,
            conflict_change_id,
            repo_id,
            url_context,
            parent_thread_id,
            spawning_event_id,
            mode,
            cc_model,
            pre_emitted_origin,
            title,
            origin,
            None,
        )
        .await
    }

    /// Internal: Process a message with optional trigger context
    #[allow(clippy::too_many_arguments)]
    async fn process_message_with_steps_internal(
        &self,
        user_message: &str,
        model_override: Option<&str>,
        trigger: Option<TriggerContext>, // None for user-driven chat; Some when fired by the scheduler
        app_context: Option<AppContext>, // app context if chatting from within an app
        file_context: Option<String>,    // file path if user is viewing a file
        reasoning_effort: Option<&str>,  // unified reasoning level: none/low/medium/high/xhigh/max
        user_images: Option<&[crate::api::ChatImage]>, // base64-encoded images pasted by user
        device_id: Option<&str>,         // device that sent this message
        use_claude_code: Option<bool>,   // bypass LLM and spawn Claude Code directly
        event_id: Option<&str>,          // client-generated UUID for reliable matching
        thread_id: Option<Uuid>,         // None = new thread, Some = follow-up
        conflict_change_id: Option<Uuid>, // change ID for merge conflict resolution
        repo_id: Option<&str>,           // external repository ID for CC worktree
        url_context: Option<crate::api::UrlContext>, // webpage content from Tauri panel webview
        parent_thread_id: Option<Uuid>,  // parent thread if spawned by run_thread
        spawning_event_id: Option<Uuid>, // event in parent thread that triggered the spawn (mode != Human only)
        mode: ActorMode,
        cc_model: Option<&str>, // CC-specific model override (from compose view pre-session selection)
        pre_emitted_origin: Option<Uuid>, // skip MessageReceived if already emitted by spawn_thread
        title: Option<&str>,    // caller-provided title (skips async LLM title gen)
        origin: Option<MessageOrigin>,
        external_cancel: Option<CancellationToken>, // forwarded into the per-thread cancel_token (used by triggers)
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        let chat_start = std::time::Instant::now();
        // Track whether a pending change was proposed during this request
        // (set to true when Claude Code proposes changes, used to trigger SSE broadcast)
        let mut proposed_change = false;

        // Mutable so the non-CC fast-path can mark the event as already on
        // the wire when injection loses the race with a thread that completed
        // between the has_active check and the send — the slow path then
        // skips its own MessageReceived emit to avoid a duplicate.
        let mut pre_emitted_origin = pre_emitted_origin;

        let is_trigger = trigger.is_some();

        // `None` here means "use the user's chat defaults", not "use
        // `LlmProvider::default_model()`" — which strips the `[1m]` suffix and
        // doesn't carry an effort, so without this resolve the stamp drifts
        // away from the user's selection on every internal re-entry path.
        let (resolved_model, resolved_effort) = PreferenceStore::resolve_chat_overrides(
            &self.pool,
            model_override.map(str::to_string),
            reasoning_effort.map(str::to_string),
        )
        .await;
        let model_override = resolved_model.as_deref();
        let reasoning_effort = resolved_effort.as_deref();

        // Resolve device tooltip info for the MessageReceived event
        let device_name = if let Some(did) = device_id {
            crate::core::DeviceStore::tooltip_info(&self.pool, did).await
        } else {
            None
        };

        // Spawn Flash image description in background (don't block main LLM flow)
        let description_handle = if let (Some(imgs), Some(ref extractor)) =
            (user_images, &self.extractor)
        {
            if !imgs.is_empty() {
                let img_desc_model = PreferenceStore::get(&self.pool, PREF_MODEL_IMAGE_DESCRIPTION)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let provider = extractor.provider_for_model(&img_desc_model);
                let imgs: Vec<crate::api::ChatImage> = imgs.to_vec();
                Some(tokio::spawn(async move {
                    match describe_images(&provider, &imgs).await {
                        Ok(desc) => Some(desc),
                        Err(e) => {
                            log!("[Chat] Image description failed: {}", e);
                            None
                        }
                    }
                }))
            } else {
                None
            }
        } else {
            None
        };

        // Generate a per-request ID for ProcessResult tracking
        let request_id = Uuid::new_v4();
        // A new thread is one where the caller didn't supply a thread_id
        // (backwards compat: if no thread_id, use request_id).
        // The frontend now always sends thread_id, but scheduled tasks and
        // tool-invoked CC may still omit it.
        let is_new_thread = thread_id.is_none();
        let thread_id = thread_id.unwrap_or(request_id);
        let thread_id_str = thread_id.to_string();

        // CC AskUserQuestion free-form path: if the thread is currently waiting
        // on a `UserQuestionAsked` and the user typed instead of clicking an
        // option, route the message to the answer-question handler with
        // `FreeText` instead of creating a new exchange. Without this, the user's
        // text would spawn a brand-new CC turn alongside an unresolved question.
        //
        // Uses `lookup_active_question_tool_use_id` (not `..pending..`) so a
        // question orphaned by a prior `ResponseAborted`/`Canceled`/`Failed`/
        // `CodingAgentIdled` doesn't intercept the follow-up — otherwise the
        // typed text would be silently consumed as the dead question's answer
        // and `MessageReceived` would never be emitted.
        if use_claude_code == Some(true) && !is_new_thread && !user_message.is_empty() {
            if let Some(pending_tool_use_id) =
                crate::engine::agent_question::lookup_active_question_tool_use_id(
                    self.pool(),
                    thread_id,
                )
                .await
            {
                use crate::engine::agent_question::{answer_pending_question, AnswerResult};
                use crate::engine::thread_events::AnswerKind;
                let engine_arc: std::sync::Arc<Self> = self.clone_arc();
                let answer = AnswerKind::FreeText {
                    text: user_message.to_string(),
                };
                match answer_pending_question(
                    &engine_arc,
                    thread_id,
                    pending_tool_use_id,
                    answer,
                    origin.clone(),
                )
                .await
                {
                    AnswerResult::Resumed => {
                        log!(
                            "[Chat] Free-form answer routed to pending question for thread {}",
                            thread_id
                        );
                    }
                    AnswerResult::Conflict(msg) => {
                        log!(
                            "[Chat] Free-form answer conflict for thread {}: {}",
                            thread_id,
                            msg
                        );
                        emit_routing_failure(&self.event_bus, thread_id, &msg).await?;
                    }
                }
                return Ok(ProcessResult {
                    response: String::new(),
                    steps: vec![],
                    images: vec![],
                    request_id,
                    thread_id,
                    proposed_change: false,
                    auto_apply: false,
                    orphaned_injections: vec![],
                });
            }
        }

        // Fast-path for CC follow-ups: route via msg_tx BEFORE register_thread_queued
        // so a follow-up arriving during an active CC session is picked up by the
        // running event loop instead of spawning a new CC session (or blocking on
        // the prior turn's still-held ThreadGuard).
        //
        // This handles both idle sessions (msg picked up immediately) and busy sessions
        // (msg queued in the channel, picked up after current work finishes).
        if use_claude_code == Some(true) && !is_new_thread {
            // Check if there's a live CC session to route to.
            let has_session = {
                let sessions = self.agent_sessions.lock().await;
                sessions
                    .get(&thread_id)
                    .map(|s| !s.process_exited)
                    .unwrap_or(false)
            };

            if has_session {
                // Emit MessageReceived FIRST — guarantees it gets a lower sequence
                // number and earlier timestamp than CodingAgentPromptSent (emitted by
                // the CC event loop when it picks up the message). Without this
                // ordering, CodingAgentPromptSent can race ahead and end up in the
                // wrong exchange, causing a brief "interrupted" flash and incorrect
                // thread section placement.
                use crate::engine::thread_events::EventMeta;
                self.event_bus
                    .emit(crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: make_message_received(
                            &self.workspace_path,
                            user_message,
                            user_images,
                            device_id,
                            device_name,
                            parent_thread_id,
                            spawning_event_id,
                            mode,
                            None,
                            None,
                            origin.clone(),
                        ),
                        meta: EventMeta {
                            event_id: event_id.and_then(|s| uuid::Uuid::parse_str(s).ok()),
                            channel: Some(EventChannel::CodingAgent),
                            ..EventMeta::NONE
                        },
                    })
                    .await?;

                let images = user_images.map(|imgs| imgs.to_vec());
                let send_ok = {
                    let sessions = self.agent_sessions.lock().await;
                    if let Some(session) = sessions.get(&thread_id) {
                        if !session.process_exited {
                            // Track the expected Result before sending. If the
                            // send fails (channel dropped between the lookup
                            // and the send), undo the increment so the
                            // existing session's idle-exit cancel doesn't get
                            // suppressed by a phantom pending follow-up.
                            session.pending_followups.fetch_add(
                                1,
                                std::sync::atomic::Ordering::AcqRel,
                            );
                            let send_result = session.msg_tx.send(AgentUserInput {
                                text: user_message.to_string(),
                                images,
                                origin_event_id: event_id
                                    .and_then(|s| uuid::Uuid::parse_str(s).ok()),
                            });
                            if send_result.is_err() {
                                session.pending_followups.fetch_sub(
                                    1,
                                    std::sync::atomic::Ordering::AcqRel,
                                );
                                false
                            } else {
                                true
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };

                if !send_ok {
                    emit_routing_failure(
                        &self.event_bus,
                        thread_id,
                        "Claude Code session ended while routing message. Please try again.",
                    )
                    .await?;
                }

                log!("[Chat] CC follow-up routed via msg_tx for thread {} (bypassed register_thread_queued)", thread_id);
                return Ok(ProcessResult {
                    response: String::new(),
                    steps: vec![],
                    images: vec![],
                    request_id,
                    thread_id,
                    proposed_change: false,
                    auto_apply: false,
                    orphaned_injections: vec![],
                });
            }
        }

        // Fast-path for non-CC follow-ups: if the thread already has an active
        // agentic loop, inject the message (with images) mid-flight instead of
        // blocking in register_thread_queued for up to 60s. This mirrors the CC
        // fast-path above but uses injection_tx instead of msg_tx.
        if use_claude_code != Some(true) && !is_new_thread {
            let has_active = {
                let threads = self.active_threads.lock().unwrap();
                threads.contains_key(&thread_id)
            };

            if has_active {
                // Emit MessageReceived FIRST — same ordering guarantee as the CC
                // fast-path. The agentic loop emits UserPromptInjected when it picks
                // up the injection; without this ordering, UserPromptInjected can
                // race ahead and create a duplicate exchange boundary.
                use crate::engine::thread_events::EventMeta;
                let emit_result = self
                    .event_bus
                    .emit(crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: make_message_received(
                            &self.workspace_path,
                            user_message,
                            user_images,
                            device_id,
                            device_name.clone(),
                            parent_thread_id,
                            spawning_event_id,
                            mode,
                            model_override,
                            reasoning_effort,
                            origin.clone(),
                        ),
                        meta: EventMeta {
                            event_id: event_id.and_then(|s| uuid::Uuid::parse_str(s).ok()),
                            channel: Some(EventChannel::Chat),
                            ..EventMeta::NONE
                        },
                    })
                    .await?;

                let images = user_images.map(|imgs| imgs.to_vec());
                let injected = {
                    let threads = self.active_threads.lock().unwrap();
                    if let Some(handle) = threads.get(&thread_id) {
                        handle
                            .injection_tx
                            .send(crate::engine::InjectedPrompt {
                                text: user_message.to_string(),
                                event_id: event_id.and_then(|s| Uuid::parse_str(s).ok()),
                                mode,
                                spawning_event_id,
                                images,
                                origin: origin.clone(),
                                kind: crate::engine::InjectedPromptKind::UserText,
                            })
                            .is_ok()
                    } else {
                        false
                    }
                };

                if injected {
                    log!("[Chat] Follow-up injected into active thread {} (bypassed register_thread_queued)", thread_id);
                    return Ok(ProcessResult {
                        response: String::new(),
                        steps: vec![],
                        images: vec![],
                        request_id,
                        thread_id,
                        proposed_change: false,
                        auto_apply: false,
                        orphaned_injections: vec![],
                    });
                }

                // Injection failed — the agentic loop completed (and dropped its
                // injection receiver) between the has_active check and the send.
                // Fall through to the slow path so register_thread_queued can
                // start a fresh loop for this message. The MessageReceived was
                // already persisted above, so flag the slow path to skip its
                // own emit (otherwise the user message double-renders).
                if let Some(result) = emit_result {
                    pre_emitted_origin = Some(result.event_id);
                }
                log!(
                    "[Chat] Follow-up injection lost the race for thread {} — falling back to slow path",
                    thread_id
                );
            }
        }

        // Wait for any in-progress request on this thread to finish, then register.
        // This queues follow-up messages instead of cancelling in-progress work.
        // The _guard ensures the thread is automatically unregistered on all exit
        // paths (normal return, error, or panic) via its Drop impl.
        let (cancel_token, mut injection_rx, _guard) = self.register_thread_queued(thread_id).await;

        // Trigger-driven runs hand the scheduler's per-trigger cancel down here.
        // Forward it onto the per-thread token so deleting/disabling/updating the
        // trigger mid-flight signals the agentic loop to exit cleanly (the loop
        // observes `cancel_token` between iterations) instead of being aborted
        // — which would tear it down mid-tool and leave the thread "running".
        // The DropGuard fires when this fn returns so the forwarder exits even
        // when the trigger completes without `ext` ever being cancelled —
        // otherwise one forwarder leaks per trigger run.
        let _forwarder_done_guard = external_cancel.map(|ext| {
            let thread_token = cancel_token.clone();
            let done = CancellationToken::new();
            let done_in_forwarder = done.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = ext.cancelled() => { thread_token.cancel(); }
                    _ = done_in_forwarder.cancelled() => {}
                }
            });
            done.drop_guard()
        });

        // RequestId was used by the forwarder to route events — no longer needed
        // since the bus emits directly with thread_id.

        // Persist + broadcast the exchange boundary event via EventBus.
        // When pre_emitted_origin is set, MessageReceived was already emitted by
        // spawn_thread — skip to avoid double-emitting (and double-incrementing
        // the parent's active_children_count).
        use crate::engine::thread_events::EventMeta;
        let origin_id = if let Some(id) = pre_emitted_origin {
            id
        } else {
            let (user_thread_event, user_meta) =
                if let Some(ref tc) = trigger {
                    build_trigger_started_event(
                        &tc.trigger_id,
                        &tc.trigger_name,
                        &tc.invocation,
                        user_message,
                        tc.go_to_review,
                    )
                } else {
                    let is_cc = use_claude_code == Some(true);
                    let (req_model, req_effort) = if is_cc {
                        (None, None)
                    } else {
                        (model_override, reasoning_effort)
                    };
                    (
                        make_message_received(
                            &self.workspace_path,
                            user_message,
                            user_images,
                            device_id,
                            device_name.clone(),
                            parent_thread_id,
                            spawning_event_id,
                            mode,
                            req_model,
                            req_effort,
                            origin.clone(),
                        ),
                        EventMeta {
                            event_id: event_id.and_then(|s| uuid::Uuid::parse_str(s).ok()),
                            channel: Some(if is_cc {
                                EventChannel::CodingAgent
                            } else {
                                EventChannel::Chat
                            }),
                            ..EventMeta::NONE
                        },
                    )
                };

            let emit_result = self
                .event_bus
                .emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: user_thread_event,
                    meta: user_meta,
                })
                .await?
                .expect("persisted event must return EmitResult");
            emit_result.event_id
        };

        // Auto-save images to .lucidos/tmp/images/ so the LLM can reference them by path
        let saved_image_paths = if let Some(imgs) = user_images {
            save_images_to_tmp(&self.workspace_path, imgs)
        } else {
            Vec::new()
        };

        // Caller-provided title — emit immediately, skip async LLM title generation
        let has_caller_title = if let Some(t) = title {
            let t = t.trim();
            if !t.is_empty() {
                self.event_bus
                    .emit(crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ThreadTitleGenerated {
                            title: t.to_string(),
                        },
                        meta: EventMeta::NONE,
                    })
                    .await?;
                true
            } else {
                false
            }
        } else {
            false
        };

        // Trigger threads are titled "Run <trigger name>" so users can spot trigger runs at a glance.
        if is_trigger && !has_caller_title {
            if let Some(ref tc) = trigger {
                self.event_bus
                    .emit(crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ThreadTitleGenerated {
                            title: format!("Run {}", tc.trigger_name),
                        },
                        meta: EventMeta::NONE,
                    })
                    .await?;
            }
        }

        // Generate title for follow-up threads (when a thread gets its second message)
        if !is_new_thread && !is_trigger && !has_caller_title {
            // It's a follow-up — generate title if none exists yet
            let event_store = self.event_store.clone();
            if let Some(ref extractor) = self.extractor {
                let title_model = PreferenceStore::get(&self.pool, PREF_MODEL_TITLE)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let provider = extractor.provider_for_model(&title_model);
                let msg = user_message.to_string();
                let attached_images = user_images.map_or(0, |i| i.len());
                let bus = self.event_bus.clone();
                let tid_str = thread_id_str.clone();
                tokio::spawn(async move {
                    match event_store.thread_has_title(&tid_str).await {
                        Ok(true) => {}
                        Ok(false) => {
                            let image_desc = event_store
                                .get_thread_first_message(&tid_str)
                                .await
                                .ok()
                                .flatten()
                                .and_then(|(_, desc, _)| desc);
                            emit_generated_title(
                                &bus,
                                &provider,
                                thread_id,
                                &msg,
                                image_desc.as_deref(),
                                None,
                                attached_images,
                            )
                            .await;
                        }
                        Err(e) => {
                            log!("[Thread] Failed to check title existence: {}", e);
                        }
                    }
                });
            }
        }

        // `_guard` MUST stay alive across this await so cancel_thread lands
        // on the per-thread cancel_token (see process_cc.rs module doc).
        if use_claude_code == Some(true) {
            return self
                .run_cc_chat_branch(
                    thread_id,
                    request_id,
                    user_message,
                    user_images,
                    repo_id,
                    cc_model,
                    reasoning_effort,
                    conflict_change_id,
                    origin_id,
                    spawning_event_id,
                    &cancel_token,
                    chat_start,
                )
                .await;
        }


        // Snapshot RwLock values once per request
        let user_timezone = self.user_timezone.read().await.clone();
        let user_language = self.user_language.read().await.clone();
        let user_profile = self.user_profile.read().await.clone();

        // Read memory model preference once for use in summarization and classification
        let memory_model_pref = PreferenceStore::get(&self.pool, PREF_MODEL_MEMORY)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();

        // Resume tool blocks: full ToolUse + ToolResult Message pairs for the
        // most recent N tool calls (Phase 3). Pinned `load_knowhow` results
        // survive regardless of N — see
        // `build_resume_tool_blocks_with_skip_ids`. Empty for triggers and
        // the no-history path.
        let mut resume_tool_blocks: Vec<Message> = Vec::new();
        let mut resume_skip_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // Load conversation history from DB.
        // For follow-ups, load only the thread's messages to avoid cross-thread leakage.
        // If >HISTORY_COMPRESS_THRESHOLD messages, older messages are summarized via Flash
        // and only the last HISTORY_RECENT_MESSAGES are included verbatim.
        let (mut history_context, conversation_summary, history_image_hashes) = if !is_trigger {
            // Follow-ups: scope to thread; new threads: load global recent messages.
            //
            // For follow-ups we fetch the thread's events ONCE and derive both
            // the SessionMessage history (for stringified `[CONVERSATION
            // HISTORY]` formatting) AND the verbatim resume tool blocks (most
            // recent N + pinned `load_knowhow` results) from that single
            // walk. The earlier shape did two separate DB calls
            // (`get_thread_messages` then `get_thread_events`) for the same
            // rows — same SQL, same thread_id, twice the round-trip — and
            // also silently swallowed the second call's error, losing
            // procedure context exactly when the bug Phase 3 fixes recurs.
            let messages_result = if !is_new_thread {
                match self.event_store.get_thread_events(&thread_id_str).await {
                    Ok(events) => {
                        let (blocks, skip_ids) =
                            crate::core::store::build_resume_tool_blocks_with_skip_ids(
                                &events,
                                crate::core::store::RESUME_VERBATIM_TOOL_TAIL,
                            );
                        resume_tool_blocks = blocks;
                        resume_skip_ids = skip_ids;
                        Ok(crate::core::store::build_session_messages(&events))
                    }
                    Err(e) => {
                        log!(
                            "[Chat] resume context load failed (DB error): {}; \
                             orchestrator will resume without verbatim tool history",
                            e
                        );
                        Err(e)
                    }
                }
            } else {
                self.event_store
                    .get_recent_messages((HISTORY_RECENT_MESSAGES * 2 + 2) as i64, None)
                    .await
            };

            match messages_result {
                Ok(messages) => {
                    // All messages except the one we just appended (last one)
                    let all_prior: Vec<_> = if messages
                        .last()
                        .map(|m| m.role == "user" && m.content == user_message)
                        .unwrap_or(false)
                    {
                        messages[..messages.len().saturating_sub(1)].to_vec()
                    } else {
                        messages
                    };

                    // New threads pull history from multiple recent threads — their
                    // images are irrelevant (and can consume hundreds of thousands of tokens).
                    let prior_image_hashes: Vec<Vec<String>> = if is_new_thread {
                        vec![]
                    } else {
                        filter_recent_history_image_hashes(&all_prior, MAX_HISTORY_IMAGE_MESSAGES)
                    };

                    // Per-message flag: is this message's image data included in the
                    // LLM context? Used by format_history_msg to annotate dropped
                    // images with "image not included, may be outdated".
                    let image_data_included: Vec<bool> = {
                        let cutoff = image_recency_cutoff(&all_prior, MAX_HISTORY_IMAGE_MESSAGES);
                        let mut user_idx = 0usize;
                        all_prior
                            .iter()
                            .map(|m| {
                                if m.role == "user" {
                                    let included = !is_new_thread
                                        && user_idx >= cutoff
                                        && !m.user_image_hashes.is_empty();
                                    user_idx += 1;
                                    included
                                } else {
                                    false
                                }
                            })
                            .collect()
                    };

                    // Pre-compute thread image indices per message so history annotations
                    // can include thread:N references (e.g. "[attached image (thread:3)]").
                    // This counts ALL images (user + generated) in sequential order to match
                    // the thread:N numbering used by walk_thread_images.
                    let msg_image_starts: Vec<usize> = {
                        let mut starts = Vec::with_capacity(all_prior.len());
                        let mut idx: usize = 0;
                        for m in all_prior.iter() {
                            starts.push(idx);
                            if m.role == "user" {
                                idx += m.user_image_hashes.len();
                            } else {
                                idx += m.images.len();
                            }
                        }
                        starts
                    };

                    // Format a message for history context with tiered truncation.
                    // - Last HISTORY_VERBATIM_TAIL messages: fully verbatim (only 15K safety net)
                    // - Earlier messages: user messages verbatim, assistant messages compacted to ~1500 chars
                    // `msg_idx` indexes into `all_prior` to look up `image_data_included`.
                    let now = Utc::now();
                    let format_history_msg = |m: &crate::core::store::SessionMessage,
                                              is_verbatim: bool,
                                              img_start: usize,
                                              msg_idx: usize|
                     -> String {
                        let role = if m.role == "user" {
                            "User"
                        } else {
                            "Assistant"
                        };
                        let content = format_history_content(&m.content, &m.role, is_verbatim);
                        // Determine image kind: user-attached (with staleness tracking) or generated
                        let (label, n, stale_note) = if !m.user_image_hashes.is_empty() {
                            let included =
                                image_data_included.get(msg_idx).copied().unwrap_or(false);
                            let stale = if !included {
                                " — image not included, may be outdated"
                            } else {
                                ""
                            };
                            ("attached", m.user_image_hashes.len(), stale)
                        } else if !m.images.is_empty() {
                            ("generated", m.images.len(), "")
                        } else {
                            ("", 0, "")
                        };
                        let image_note = if n == 0 {
                            // No image data, but if a description survived, show it as text context
                            m.image_description
                                .as_ref()
                                .map(|d| format!(" [image description: {}]", d))
                                .unwrap_or_default()
                        } else {
                            let age = format_relative_age(now - m.created_at);
                            let range = if n <= 1 {
                                format!("thread:{}", img_start + 1)
                            } else {
                                format!("thread:{}-thread:{}", img_start + 1, img_start + n)
                            };
                            let count_prefix = if n <= 1 {
                                format!("{} image", label)
                            } else {
                                format!("{} {} images", label, n)
                            };
                            let desc_suffix = m
                                .image_description
                                .as_ref()
                                .map(|d| format!(": {}", d))
                                .unwrap_or_default();
                            format!(
                                " [{} ({}, {}{}){}]",
                                count_prefix, range, age, stale_note, desc_suffix
                            )
                        };
                        // Assistant turns may have only tool calls; m.content covers prose only.
                        // Tools whose `tool_called_event_id` is in `resume_skip_ids`
                        // are already represented as full Message::Blocks(...) pairs
                        // prepended to the LLM messages vec — suppress the
                        // duplicate `[tools: ...]` summary for them.
                        let steps_summary = if m.role == "assistant" {
                            format_history_steps(&m.steps, &resume_skip_ids).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        format!("{}: {}{}{}", role, content, steps_summary, image_note)
                    };

                    // Format messages with tiered truncation based on position.
                    // `idx_offset` is the index into both msg_image_starts and image_data_included.
                    let format_tiered = |msgs: &[crate::core::store::SessionMessage],
                                         idx_offset: usize|
                     -> Vec<String> {
                        let tail_start = msgs.len().saturating_sub(HISTORY_VERBATIM_TAIL);
                        msgs.iter()
                            .enumerate()
                            .map(|(i, m)| {
                                format_history_msg(
                                    m,
                                    i >= tail_start,
                                    msg_image_starts[idx_offset + i],
                                    idx_offset + i,
                                )
                            })
                            .collect()
                    };

                    if all_prior.is_empty() {
                        (String::new(), user_message.to_string(), prior_image_hashes)
                    } else if all_prior.len() <= HISTORY_COMPRESS_THRESHOLD {
                        // Short conversation — include all messages with tiered truncation
                        let turns = format_tiered(&all_prior, 0);
                        let history = format!(
                            "[CONVERSATION HISTORY (recent)]\n{}\n[END HISTORY]",
                            turns.join("\n")
                        );

                        let user_topics: Vec<&str> = all_prior
                            .iter()
                            .filter(|m| m.role == "user")
                            .map(|m| m.content.as_str())
                            .collect();
                        let mut summary = user_topics.join(" | ");
                        summary.push_str(" | ");
                        summary.push_str(user_message);
                        let summary: String = summary.chars().take(500).collect();
                        (history, summary, prior_image_hashes)
                    } else {
                        // Long conversation — summarize older messages, keep recent with tiered truncation
                        let split_point = all_prior.len().saturating_sub(HISTORY_RECENT_MESSAGES);
                        let older = &all_prior[..split_point];
                        let recent = &all_prior[split_point..];

                        let older_summary = if let Some(ref extractor) = self.extractor {
                            let older_turns: Vec<String> = older
                                .iter()
                                .enumerate()
                                .map(|(i, m)| format_history_msg(m, false, msg_image_starts[i], i))
                                .collect();
                            let older_text = older_turns.join("\n");
                            summarize_or_fallback(
                                extractor.summarize_conversation(
                                    &older_text,
                                    Some(&memory_model_pref),
                                ),
                                older.len(),
                            )
                            .await
                        } else {
                            format!("({} earlier messages not shown)", older.len())
                        };

                        let recent_turns = format_tiered(recent, split_point);

                        let history = format!(
                            "[CONVERSATION HISTORY (recent)]\n[Earlier (resolved — do NOT re-attempt fixes described here): {}]\n\nRecent:\n{}\n[END HISTORY]",
                            older_summary,
                            recent_turns.join("\n")
                        );

                        // Build extraction context from recent messages only
                        let user_topics: Vec<&str> = recent
                            .iter()
                            .filter(|m| m.role == "user")
                            .map(|m| m.content.as_str())
                            .collect();
                        let mut summary = user_topics.join(" | ");
                        summary.push_str(" | ");
                        summary.push_str(user_message);
                        let summary: String = summary.chars().take(500).collect();
                        (history, summary, prior_image_hashes)
                    }
                }
                Err(_) => (String::new(), user_message.to_string(), vec![]),
            }
        } else {
            (String::new(), user_message.to_string(), vec![])
        };

        // Build extraction context for tool execution (artifact writes, imports)
        let base_ctx = self.extraction_context_base().await;
        let extraction_ctx =
            Self::extraction_context_with_conversation(&base_ctx, &conversation_summary);

        // Memory indexing is handled by the EventBus memory consumer —
        // it reacts to persisted MessageReceived/ResponseGenerated events.

        let classification = {
            if let Some(ref extractor) = self.extractor {
                let ctx = if conversation_summary.is_empty() {
                    None
                } else {
                    Some(conversation_summary.as_str())
                };
                classify_or_fallback(extractor.classify_query(
                    user_message,
                    ctx,
                    Some(&memory_model_pref),
                ))
                .await
            } else {
                crate::memory::QueryClassification::default()
            }
        };

        // Retrieve relevant context from memory (skipped if classification says not needed)
        let response_meta = crate::engine::thread_events::EventMeta {
            request_event_id: Some(origin_id),
            channel: if is_trigger {
                Some(EventChannel::Trigger)
            } else {
                None
            },
            ..crate::engine::thread_events::EventMeta::NONE
        };
        let (mut memory_context, memory_results) =
            self.retrieve_context(user_message, &classification).await;

        // Emit MemorySearched step so the frontend can show it
        if classification.needs_memory {
            self.event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::MemorySearched {
                            results: memory_results,
                            queries: classification.sub_queries.clone(),
                        },
                        meta: response_meta,
                    },
                    "[Chat] MemorySearched",
                )
                .await;
        }

        // System prompt with current date for time-aware responses
        let now = Utc::now();
        let current_date = now.format("%A, %B %d, %Y").to_string(); // e.g., "Thursday, January 30, 2026"
        let current_time_utc = now.format("%H:%M UTC").to_string();

        // Build timezone section based on whether timezone is set
        let timezone_section = if user_timezone.is_empty() {
            format!(r#"CURRENT TIME: {} at {}"#, current_date, current_time_utc)
        } else {
            // Calculate user's local time
            let tz: chrono_tz::Tz = user_timezone.parse().unwrap_or(chrono_tz::UTC);
            let local_now = now.with_timezone(&tz);
            let current_time_local = local_now.format("%H:%M").to_string();

            // Get UTC offset in hours (e.g., +1 for CET, +2 for CEST)
            use chrono::Offset;
            let offset_seconds = local_now.offset().fix().local_minus_utc();
            let offset_hours = offset_seconds / 3600;
            let utc_offset_str = if offset_hours >= 0 {
                format!("+{}", offset_hours)
            } else {
                format!("{}", offset_hours)
            };

            format!(
                r#"CURRENT TIME: {} at {} (user's local time: {} {})
USER TIMEZONE: {} (UTC{})

TIMEZONE HANDLING:
- The user speaks in their LOCAL time ({}).
- All timestamps are stored as UTC in the database.
- ALWAYS display times to the user in their local timezone (not UTC).
- Cron uses 6 fields: second minute hour day-of-month month day-of-week
- When user says "at 8am", use "0 0 8 * * *" (second=0, minute=0, hour=8).
- Example: "daily at 8am" → cron "0 0 8 * * *", "at 9:30" → "0 30 9 * * *"
- The system automatically handles daylight saving time adjustments."#,
                current_date,
                current_time_utc,
                current_time_local,
                user_timezone,
                user_timezone,
                utc_offset_str,
                user_timezone
            )
        };

        // Mandatory setup preferences — to add a new one, add a (key, instruction) tuple.
        // Each is checked against PreferenceStore; any missing key triggers setup mode.
        // Global prefs (timezone, language) use PreferenceStore::get.
        // Per-device prefs (push_notifications) use PreferenceStore::get_for_device.
        let mandatory_prefs: &[(&str, &str, bool)] = &[
            ("timezone", "- TIMEZONE: Ask what timezone they are in and call set_timezone (e.g., \"America/New_York\", \"Europe/London\", \"Asia/Tokyo\").", false),
            ("language", "- LANGUAGE: Ask what language they prefer and call set_language to save it.", false),
            ("push_notifications", "- PUSH NOTIFICATIONS: Ask if they want to enable browser push notifications for scheduled task alerts. If yes, call enable_push_notifications(enabled=true). If no, call enable_push_notifications(enabled=false) so you don't ask again.", true),
        ];

        let mut missing_instructions = Vec::new();
        let mut missing_pref_keys = Vec::new();
        for (key, instruction, per_device) in mandatory_prefs {
            let value = if *per_device {
                if let Some(did) = device_id {
                    PreferenceStore::get_for_device(&self.pool, key, did)
                        .await
                        .ok()
                        .flatten()
                } else {
                    // No device context (child thread, scheduled task) — per-device
                    // prefs are irrelevant, treat as already configured
                    continue;
                }
            } else {
                PreferenceStore::get(&self.pool, key).await.ok().flatten()
            };
            if value.is_none() {
                missing_instructions.push(*instruction);
                missing_pref_keys.push(*key);
            }
        }

        let language_section = if !missing_instructions.is_empty() {
            log!(
                "[Chat] Setup required — missing preferences: {}",
                missing_instructions.join(", ")
            );
            format!("SETUP REQUIRED — DO NOT PROCEED UNTIL COMPLETE:\nThe following settings are not configured. You MUST ask the user for these BEFORE doing anything else. Do NOT answer questions, create tasks, or perform any work until setup is complete.\n{}", missing_instructions.join("\n"))
        } else {
            format!(
                "USER LANGUAGE: {}\nAlways respond in {}.",
                user_language, user_language
            )
        };

        let workspace_name = self.workspace_name();

        let system_prompt = format!(
            r#"You are managing Lucidos, a personal assistant running in the "{workspace_name}" workspace. You help users organize their life and work through natural conversation.

WORKSPACE: {workspace_name} ({workspace_path})
All threads, events, artifacts, and data you access belong to this workspace. When the user refers to "my threads", "events", or other data, it means data in this workspace.

{}

{}

PERSONAL DATA ACCESS:
This is the user's PRIVATE workspace containing THEIR OWN personal documents, files, and data.
The user has FULL rights to access, view, and discuss ANY information in their workspace.
This includes personal identifiers (SSN, ID numbers, addresses, phone numbers, etc.) from their own documents.
When the user asks about content in their files, provide it - this is their data, not a privacy violation.
Do NOT refuse to discuss the user's own personal information from their own files."#,
            timezone_section,
            language_section,
            workspace_name = workspace_name,
            workspace_path = self.workspace_path.display()
        );

        let system_prompt_base = r#"
PERSONALITY:
- Warm but concise - acknowledge what users say, ask relevant follow-ups
- Proactive - offer to create files, track things, set reminders when appropriate
- Contextual - remember past conversations and reference them naturally

MEMORY:
- You have access to LONG-TERM MEMORY organized by topic with dated entries
- Recent entries within a topic represent the CURRENT state — they supersede older entries
- When user asks broad questions, draw from all memory topics
- Timestamps show when facts were recorded

SELF-AWARENESS (answer these naturally when asked):
- "Who are you?" → Brief intro + mention what you're currently tracking for them
- "What am I working on?" → Summarize active projects from files and recent conversations
- "What do you know about me?" → Summarize learned context (name, projects, preferences)

USER PROFILE:
- user_profile.md - Store learned info about the user (name, preferences, context)
- NEVER read user_profile.md - it's already included in your context below!
- ONLY write CONFIRMED facts - things the user explicitly stated, never guesses or inferences
- If unsure what the user means, ASK to confirm before writing to profile
- If user_profile.md doesn't exist when user asks personal questions, suggest they tell you about themselves first

WORKSPACE LAYOUT:
The workspace root has two top-level areas:

  .lucidos/                    ← Ephemeral, NOT under data/, NOT git-tracked
    tmp/                      ← Temp files from http_request (e.g., .lucidos/tmp/oura_data.json)
    exhaust/                  ← Internal runtime temp (do not reference)
  data/
    artifacts/              ← User files (notes, imported data, projects)
      user_profile.md       ← Learned facts about the user
      imported/             ← Files imported from APIs or local filesystem
        <service>/          ← e.g., oura/, weather/
      projects/             ← Major project folders
        <name>/notes.md
      screenshots/          ← Captured screenshots
    apps/<name>/            ← App UIs (index.html, styles.css, manifest.json)
      manifest.json         ← User-facing metadata (name, description) — NOT in your context
      knowhow/              ← Reference — technical details, "how to do it well" (evolves as you learn)
      intents/              ← User intent — what the user wants, in their terms (stable)
      scripts/              ← Helper scripts invoked by intents or knowhow
      triggers/             ← App-specific scheduled triggers
    knowhow/                ← General domain reference docs (API specs, data formats)
    triggers/               ← Standalone scheduled triggers (not app-specific)
      <name>/               ← Trigger directory
        <name>.md           ← Trigger prompt definition
        scripts/            ← Trigger-specific scripts

CONTENT TAXONOMY:
Three content types — scoped inside apps, knowhow domains, or triggers:
- Intent = what the user wants, described in their terms. A high-level workflow: goals, conditions, desired outcomes. Written by the user, changes only when the user's needs change. Think of it as what you'd tell a competent assistant — not technical how-to, but the desired outcome and order.
  MUST include YAML frontmatter with:
    - `name`: Human-readable name for the intent
    - `knowhow`: List of knowhow IDs to load when executing this intent (optional). An ID is the file's path under data/knowhow/ WITHOUT the .md suffix INCLUDING any subdirectory: data/knowhow/weather/api.md → 'weather/api', NOT 'api'. Engine-shipped reference docs use the 'system-knowhow/' prefix.
  Example:
    ---
    name: Daily Weather Check
    knowhow:
      - weather/api
    ---
    Check the forecast for the upcoming day...

- Knowhow = how to achieve it, described in technical terms. API details, data formats, quirks, workarounds. This is YOUR memory of how to do things well. You maintain it.
  MUST include YAML frontmatter with:
    - `name`: Human-readable name for the knowhow document
    - `description`: Short description for semantic discovery — the system matches user messages against this to automatically load relevant knowhow (optional; derived from body if absent)
  Example:
    ---
    name: Panasonic Comfort Cloud
    description: Controls and monitors Panasonic heatpumps via Comfort Cloud API
    ---
    ## API details...

- Script = code invoked by intents or knowhow.
- Trigger = scheduled/cron task. App-specific triggers live in apps/<name>/triggers/. Standalone triggers live in triggers/<name>/.

CONTINUOUS LEARNING:
When you discover something new during execution (a quirk, a better approach, a failure mode), update the relevant knowhow file. Knowhow is your living memory of how to do things well. Only change intents when the user's goal itself changes — never put technical details in intents.

- Tool paths for artifacts are relative to data/ — use "artifacts/notes.md", not "data/artifacts/notes.md"
- .lucidos/ paths are relative to workspace root — use ".lucidos/tmp/file.json", NEVER "data/.lucidos/tmp/file.json"
- In Python scripts (run_python), cwd is the workspace root. Use open('.lucidos/tmp/file.json') for temp files, open('data/artifacts/file.md') for artifacts.
- Everything under data/ (except postgres/) is git-tracked — files persist and have version history
- Never nest artifacts inside artifacts (e.g., DON'T write to artifacts/artifacts/x)

SCRIPT FILES (under apps/, triggers/, knowhow/scripts/):
- Every script the engine spawns gets the `lucidos` CLI on PATH and `LUCIDOS_WORKSPACE` set automatically.
- For data writes use `lucidos data write artifacts/<name>.json --from /tmp/x` (or `--from -` for stdin), NOT raw HTTP requests to the engine.
- For domain events use `lucidos events emit <Type> --summary "..." --payload '{...}'` and `lucidos events query --type <Type> --limit N`.
- See knowhow/lucidos-cli.md for the full reference.

FILE FORMATTING:
Always use clean, structured markdown:
- Use ## headings for sections
- Use bullet points for lists
- Use **bold** for key values
- Example project notes structure:
  ## Goals
  - **Target**: 75kg, 15% body fat

  ## Plan
  - Zone 2 cardio: 3x per week

  ## Progress
  - [tracked here]

THINKING vs RESPONSE:
- Your thinking block is for ALL internal reasoning, analysis, data inspection, and deliberation.
- Your response text is what the user sees. It contains ONLY the final, user-facing message — no analysis, no English summaries of what you found, no reasoning.
- NEVER repeat yourself between tool calls. If you already explained your analysis before a tool call, do NOT restate it after the tool returns. Just proceed to the next action or give your final answer. The user already read your earlier text — repeating it wastes their time.

CONVERSATION STYLE:
- Vary your responses - don't start every message the same way
- NEVER start with "Okay" or "Sure" - just answer directly
- When user shares what they're working on, acknowledge and ask ONE relevant follow-up
- Don't interrogate - let conversation flow naturally
- Create artifacts as conversation progresses, not all at once

COPYABLE TEXT:
- When outputting text the user will likely want to copy (commands, URLs, API keys, IDs, instructions for another session, etc.), wrap it in <copy>...</copy> tags
- The UI renders these as inline or block elements with a one-click copy button
- Use for: shell commands, file paths, config snippets, generated text, anything meant to be pasted elsewhere
- Do NOT use for: conversational text, explanations, or headings
- Code blocks (triple backticks) already have their own copy button — don't wrap those in <copy>

UNCERTAINTY:
- If you're unsure about factual information, USE web_search to look it up first
- Don't make up facts - if you don't know and can't search, admit it
- For riddles or puzzles, ask for hints rather than guessing wrong repeatedly
- ALWAYS use web_search when: answering trivia, identifying something, or verifying facts you're uncertain about
- NEVER write guesses to user_profile.md or other memory files - only write confirmed facts

CREATING WORKSPACE ASSETS — LOAD KNOWHOW FIRST:
Before creating a trigger, app, knowhow file, or plugin, you MUST first call
load_knowhow on the matching system-knowhow file:
- create_trigger / update_trigger → load `system-knowhow/building-a-trigger`
- create_app → load `system-knowhow/building-an-app`
- writing a new file under knowhow/ → load `system-knowhow/building-knowhow`
- packaging a plugin → load `system-knowhow/building-a-plugin`
Each loaded knowhow has a "Questions to settle with the user before creating"
section — that is the source of truth for what to ask before creating. The
ACTION FIRST rule below does NOT apply to creating workspace assets: load
the knowhow, follow its guidance (ask whatever it says to ask), then create.
Skip the load_knowhow call if you already loaded the same knowhow earlier
in this thread — its content is still in your context.

ACTION FIRST - NO CLARIFICATION LOOPS:
- JUST DO IT. If the user asks for something, DO IT immediately. Don't ask clarifying questions.
- "This week" = since Monday of the current week. "Last 7 days" = last 7 days. Figure it out.
- If a request is 80% clear, act on it. Only ask if you genuinely cannot proceed.
- NEVER ask "do you mean X?" or "just to clarify" - make a reasonable assumption and execute.
- If you're wrong, the user will tell you. That's faster than a clarification loop.
- Examples of BAD behavior (never do this):
  - User: "Show me this week's tasks" → Bad: "Do you mean since Monday or last 7 days?"
  - User: "What did I do today?" → Bad: "Do you want a summary or detailed list?"
- Examples of GOOD behavior:
  - User: "Show me this week's tasks" → Good: [immediately search and show tasks]
  - User: "What did I do today?" → Good: [immediately search and show today's activity]
- Exception: see CREATING WORKSPACE ASSETS above — that overrides this rule.

TOOLS: Use efficiently — don't loop. Call once per file, don't re-read files you just wrote. Prefer edit_file over write_file for existing files. For JSON files (.json, .slides), use edit_file with json_path + new_value instead of old_string + new_string — it handles parsing and escaping automatically. Example: edit_file(path=\"artifacts/deck.slides\", json_path=\"sections[1].slides[0].title\", new_value=\"Updated Title\"). All paths are relative to data/.

MEMORY CORRECTIONS:
- If the user says a memory is wrong (e.g., "I don't work at Acme Corp"), use correct_memory with a broad search_query (e.g., "Acme") and a specific wrong_fact (e.g., "User works at Acme Corp")
- The tool finds keyword matches, then only deletes entries semantically similar to wrong_fact — other entries mentioning the keyword are preserved
- Optionally provide a corrected fact to replace them
- Corrections persist across memory rebuilds
- After correcting memories, check if user_profile.md or other artifacts still reference the stale facts. If so, ASK the user whether they'd like you to update those files too — never edit artifacts automatically during memory correction

BROWSER TOOLS:
- browser_open is ONLY for external websites the user asks you to visit (e.g., news sites, APIs, research)
- NEVER use browser_open on your own App UIs, artifacts, or any Lucidos internal files
- App UIs are edited via read_file/write_file/edit_file — the user opens them in the frontend, not you
- Use visible=true when the user says "show me", "let me log in", "I want to watch", or when a site blocks headless browsers
- Browser uses a persistent profile — logins, cookies, and localStorage carry over between sessions
- If a site redirects to a login page during headless browsing, suggest the user log in with visible=true
- Use browser_clear_data to wipe all browser data (cookies, logins, cache) and start fresh

TRIGGERS:
- Cron: 6 fields (sec min hour dom month dow) in USER'S LOCAL timezone. DST is handled automatically.
- When running a triggered task, use send_notification only if there's something noteworthy to report. If nothing changed, just finish without notifying. Errors are auto-reported — you don't need to handle error notifications.

IMPORTING DATA & CREDENTIALS:
- API data: check credentials → use request_credential if missing → http_request → write_file to imported/<service>/
- NEVER accept tokens/keys pasted in chat — always use request_credential (secure popup, out of event log)
- If a user pastes a token in chat, redirect them to the secure input dialog
- Local files: use import_file with the full path

EMAIL SETUP:
- When user wants to set up email, guide them step by step:
  1. Ask which email provider/address they use
  2. Use web_search to find the provider's current IMAP/SMTP host, port, and auth requirements
  3. If the provider requires OAuth (most do now — Outlook, Gmail): guide through OAuth setup first (see OAUTH SETUP below)
  4. Call configure_email with the looked-up settings. Use use_oauth if OAuth is connected, otherwise fall back to app password
  5. Test by sending a test email or reading inbox
- Use read_emails to check inbox (returns summaries), read_email for full message content
- Use send_email to compose and send. If confirmation is required, the user sees a preview before sending.
- NEVER include email passwords in chat — configure_email handles secure input via popup

OAUTH SETUP:
- When a service needs OAuth (email, API access, etc.), guide the user step by step:
  1. Use web_search to find how to register an OAuth app with the provider (e.g., "Azure AD app registration for SMTP OAuth", "Google Cloud Console OAuth client setup")
  2. Walk the user through the registration steps — tell them what to set for redirect URI (http://localhost:*/oauth/callback), what scopes to enable, where to find client ID and secret
  3. Use request_credential to securely collect client_id and client_secret (service: "oauth:<provider>", auth_type: "oauth_client", format: {"client_id":"...","client_secret":"..."})
  4. Call connect_oauth_account with the provider name and required scopes (use web_search to find the correct scope URIs if unsure)
  5. Browser opens for user authorization → tokens are stored automatically
- Supported well-known providers (auth/token URLs built in): google, microsoft, github
- Custom providers: include auth_url, token_url in the client credentials JSON
- Always explain each step clearly — assume the user has never done OAuth setup before

LOOKING UP SPECIFIC DATA:
When asked about specific information (SSNs, phone numbers, dates, amounts, addresses, etc.):
1. Use list_files to find relevant files (PDFs often have .txt sidecar files with extracted text). For larger workspaces, use glob_files (e.g. 'artifacts/**/*.md') to narrow down by pattern, or grep_files to search file contents directly.
2. Use read_file to read the actual content - don't rely on memory summaries for exact values
3. Memory summaries describe WHAT files contain, not the exact data inside them
4. For PDFs: read the .txt sidecar file (e.g., "document.pdf.txt") which contains the extracted text

SEARCHING FILES:
- glob_files(pattern, limit?): find files by path pattern (e.g. 'apps/**/index.html', '**/*.csv'). Patterns are relative to data/.
- grep_files(pattern, path_glob?, case_insensitive?, max_matches?, context_lines?): regex-search file contents. Use path_glob to scope (e.g. 'artifacts/**/*.md').
- Prefer these over run_bash with rg/grep/find — they're structured, faster, and respect workspace boundaries.

LONG-RUNNING SHELL COMMANDS:
- run_bash is synchronous with a 300s ceiling — it WILL kill anything longer mid-stream.
- For HTTP polling, builds, scrapers, npm/cargo installs, large repo scans: use run_bash_background(command, timeout_secs?) to spawn and get a task_id immediately.
- Drain output with bash_output(task_id) — returns only what's new since the last call. Cancel with bash_kill(task_id).
- NEVER hand-roll `for i in range(...): time.sleep(...)` polling loops in run_python — that's exactly what this trio replaces.

REFRESHING OPEN WINDOWS:
When you modify a file that the user has open (shown in FOCUSED WINDOW context):
- After writing/editing: call refresh_file(path)
- App UIs are refreshed automatically when their files are modified — do NOT tell the user to refresh
- If the user doesn't have the file open, just mention the path — it will appear as a clickable link

FILE REFERENCES:
- Always use the full path when mentioning files (e.g., "artifacts/notes.md", not just "notes.md")
- Full paths become clickable links in the UI — bare filenames do not
- For apps, mention the app name — it becomes a clickable link that opens the app window

PLUGINS:
A plugin is a coherent bundle of workspace content (apps, knowhow, triggers, scripts) shipped by another author. Once installed, its files live under data/ and are indistinguishable from anything the user authored themselves.
- install_plugin(source, overwrite=false): install from a GitHub tree URL (e.g. 'https://github.com/lucidos-dev/plugins/tree/main/browser-learning'), a plain git URL, or a local '.lucidos-plugin' archive. If existing files would be overwritten the call returns a conflict list — relay it to the user and re-run with overwrite=true only after confirmation.
- check_plugin_updates(id?): survey installed plugins for newer versions at their source URL. Returns JSON; per-plugin fetch failures show as `error` entries and don't abort the rest.
- update_plugin(id): re-fetch the manifest and re-install if newer. Returns 'Already at latest (vX)' as a no-op when versions match.
- uninstall_plugin(id): GUIDE-ONLY — emits PluginUninstalled and returns the file list. Does NOT delete files (some may have been edited or shared with another plugin). Offer to delete via delete_file after the user confirms.

DOMAIN EVENTS:
Use emit_event and query_events to track and retrieve structured facts about what happened.
- emit_event: Record an outcome (e.g., HabitLogged, WorkoutCompleted, DataImported). Event types are PascalCase past tense. Payload must include a "summary" field.
- query_events: Look up past events by type and/or time range. Use this when apps or the user need historical data (e.g., "how many workouts this week?", "when did I last log X?"). Use limit=1 to get the most recent event of a type.
- Events are immutable, append-only — they represent facts, not intentions.
- App UIs access the platform via the Lucidos SDK (<script src="/api/v1/sdk.js">). Key SDK methods: lucidos.data.read/write/delete/list (file CRUD), lucidos.events.emit/query (domain events), lucidos.preferences.get/set, lucidos.ui.applyPreferences() (theme/font), lucidos.ui.navigate(target, params), lucidos.sse.on(type, cb) (real-time event stream).
- Prefer emit_event and query_events over run_python/SQL for event access. Use Python only for complex reporting or analysis that query_events can't handle.

PARALLEL WORK (FAN-OUT):
You have two tools for spawning Lucidos threads:
- run_claude: Start a Claude Code session for code tasks (creates worktree, proposes changes)
- run_thread: Start a Lucidos thread for non-code tasks (research, analysis, drafting)
Both accept an optional `relation` argument (default `"sub"`):
- `relation: "sub"` — sub-thread. Runs independently; when it completes a callback resumes this thread with its result. Use for delegated subtasks whose outcome you need yourself.
- `relation: "top"` — top-thread. Independent top-level thread; this thread does NOT resume when it finishes. Use when the spawn is for the user to follow themselves (e.g. "do this in a separate thread", report-style work the user reads later).
The callback only works for SAME-workspace sub-threads spawned via these tools. POSTs to another workspace's /api/chat/stream are always fire-and-forget — see the cross-workspace knowhow.
For pipelines where step N depends on step N-1's outcome, spawn one sub-thread per response and wait for the callback before spawning the next — do not batch sequential spawns in one response.

__ENGINE_RESTART_RULE__

ENGINE INTERNALS YOU CANNOT OBSERVE:
You cannot count your own tool calls, detect a per-turn cap, or measure any internal engine budget. The only real per-turn cap is at 100 tool calls; when it fires the engine prepends "[ENGINE-LIMIT]" to its message — that prefix is the only signal the cap was hit. Never claim you "hit a tool-call cap", "tool-call limit", "tool-call budget", "per-turn limit", or any similar made-up engine internal. If you stop mid-task, give the real reason or just keep going. Do NOT cite specific numbers (e.g. "~25 calls", "agentic_loop.rs", "MAX_ITERATIONS") about the agent loop — those numbers are not visible to you and inventing them poisons long-term memory for future turns.

IMPORTANT — spawn threads sparingly:
- DEFAULT: Do the work yourself. Use your own tools (web_search, read_file, run_python, etc.) directly.
- ONLY spawn a child thread when the task has multiple TRULY INDEPENDENT subtasks that benefit from parallel execution (e.g., researching 3 unrelated topics simultaneously).
- NEVER spawn a thread for something you could do with a single tool call or a few sequential steps.
- NEVER spawn one thread per item in a list — batch related work together.
- Maximum 3 child threads per parent, maximum depth 3. Budget them wisely.
- Each child thread costs tokens and time. Fewer, well-scoped threads beat many small ones.

CRITICAL RULES:
1. NEVER say "I've updated/created X" unless write_file/edit_file returned a success response
2. NEVER describe what you "would do" — actually DO IT by calling the tool
3. For SPECIFIC DATA lookups (numbers, IDs, dates): ALWAYS read the file, don't guess from summaries
4. NEVER show code in responses unless the user explicitly asks for code
5. MULTIPLE FILES: When asked to create N files, call write_file N times IN THE SAME RESPONSE

VERIFICATION: Before saying "done", check that write_file returned "Created:" or "Updated:" — if not, you didn't actually do it!"#;

        let system_prompt_base =
            system_prompt_base.replace("__ENGINE_RESTART_RULE__", ENGINE_RESTART_RULE);

        let system_prompt = format!("{}{}", system_prompt, system_prompt_base);

        // Tell the LLM where Lucidos is running
        let api_port = std::env::var("LUCIDOS_API_PORT").unwrap_or_else(|_| "3000".to_string());
        let frontend_url = if let Some(origin) = self.frontend_origin.lock().unwrap().as_ref() {
            origin.clone()
        } else if let Ok(vite_port) = std::env::var("VITE_PORT") {
            format!("http://localhost:{}", vite_port)
        } else {
            format!("http://localhost:{}", api_port)
        };
        let system_prompt = format!("{}\n\nThe Lucidos client the user is talking to you from is at {}. To see App UIs, use capture_app_ui — never browser_open.",
            system_prompt, frontend_url);

        // Add browser login list to system prompt
        let system_prompt = if let Ok(logins) = BrowserLogins::list(&self.pool).await {
            if !logins.is_empty() {
                let domains: Vec<&str> = logins.iter().map(|(d, _)| d.as_str()).collect();
                format!("{}\n\nSites with probable saved browser sessions (auto-detected, may include false positives): {}",
                    system_prompt, domains.join(", "))
            } else {
                system_prompt
            }
        } else {
            system_prompt
        };

        // Add available apps to system prompt
        let apps_section = if let Ok(apps) = self.app_manager.list_apps() {
            if !apps.is_empty() {
                let mut section = String::from("\n\n## Available Apps\n\n");
                section.push_str("Apps are interactive UIs. Use navigate_ui to open them. Some apps have intents — use execute_intent(intent_id) to fulfill a stored intent.\n\n");
                for app in &apps {
                    section.push_str(&format!(
                        "- **{}** (id: `{}`): {}\n",
                        app.name, app.id, app.description
                    ));
                }
                section
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        // Add available prompts to system prompt (app-scoped + standalone triggers)
        let data_dir = self.workspace_path.join(crate::core::DATA_DIR);
        let all_intents = crate::core::IntentStore::load_all(&data_dir);
        let intents_section = if !all_intents.is_empty() {
            let mut section = String::from("\n\n## Available Intents\n\n");
            section.push_str("Stored descriptions of what the user wants. Use execute_intent(intent_id) to fulfill one. Each intent is paired with knowhow that tells you how to achieve it.\n\n");
            for p in &all_intents {
                section.push_str(&format!("- **{}** (id: `{}`)\n", p.name, p.id));
            }
            section
        } else {
            String::new()
        };

        // Add know-how summaries (general + app-specific)
        let kh_dirs = self.knowhow_dirs();
        let knowhow_summaries = crate::core::KnowhowStore::load_merged_summaries(&kh_dirs);
        let apps_dir = self.workspace_path.join(crate::core::APPS_DIR);
        let app_knowhow_summaries = crate::core::KnowhowStore::load_app_summaries(&apps_dir);
        let knowhow_section = if !knowhow_summaries.is_empty() || !app_knowhow_summaries.is_empty()
        {
            let mut section = String::from("\n\n## Know-how\n\n\
                Know-how files contain domain knowledge, procedures, and reference material. \
                When a user's request relates to a topic below, use `load_knowhow` to load the full content before responding.\n\n");
            for kh in &knowhow_summaries {
                section.push_str(&format!(
                    "- **{}** (id: `{}`): {}\n",
                    kh.name, kh.id, kh.description
                ));
            }
            for (app_id, kh) in &app_knowhow_summaries {
                section.push_str(&format!(
                    "- **{}** (id: `{}/{}`, app: {}): {}\n",
                    kh.name, app_id, kh.id, app_id, kh.description
                ));
            }
            section
        } else {
            String::new()
        };

        // Add system knowhow summaries (engine-shipped reference, never overrideable).
        let system_knowhow_summaries = self
            .system_knowhow_dir()
            .map(crate::core::SystemKnowhowStore::load_summaries)
            .unwrap_or_default();
        let system_knowhow_section = build_system_knowhow_section(&system_knowhow_summaries);

        let system_prompt = format!(
            "{}{}{}{}{}",
            system_prompt, apps_section, intents_section, knowhow_section, system_knowhow_section
        );

        let image_provider_available = self.current_image_provider().await.is_some();

        let system_prompt = {
            let mut section = format!("{}\n\n## Images\n\n\
                Images in the conversation are numbered sequentially (1-based) across all messages — user-pasted and generated. \
                The conversation history notes which messages had images with their thread:N index \
                (e.g. \"[attached image (thread:2)]\"). When images are included in the message content, \
                they are labeled as \"from earlier in the conversation\" or \"attached to current message\" \
                so you can tell which are new. \
                You can save any conversation image to an artifact file with the save_thread_image tool \
                (e.g., image: 'thread:1', path: 'artifacts/photos/reaction.jpg').", system_prompt);
            if image_provider_available {
                section.push_str(" You can also generate or edit images with the generate_image tool. \
                    To edit an existing image, reference it as 'thread:N' where N is its position in the thread, \
                    or use an artifact path like 'artifacts/photo.png'. \
                    When the user says \"edit the second image\", use input_images: [\"thread:2\"].");
            }
            section
        };

        // Trigger-fire framing rules (static — see TRIGGER_SYSTEM_ADDENDUM
        // docs). Appended last so the unconditional sections above stay
        // byte-identical between trigger fires and regular chats — that
        // shared prefix is what the LLM provider's prompt cache keys on.
        // The per-trigger knowhow listing is also trigger-only, so it lives
        // here next to the addendum (NOT in the unconditional knowhow_section
        // above, which would invalidate the cache prefix for every other
        // trigger thread).
        let system_prompt = if is_trigger {
            let trigger_knowhow_section = trigger
                .as_ref()
                .map(|t| {
                    let triggers_dir = self.workspace_path.join(crate::core::TRIGGERS_DIR);
                    build_trigger_knowhow_section(&triggers_dir, &t.slug)
                })
                .unwrap_or_default();
            format!(
                "{}{}{}",
                system_prompt,
                trigger_knowhow_section,
                crate::scheduler::user_tasks::TRIGGER_SYSTEM_ADDENDUM
            )
        } else {
            system_prompt
        };

        if !memory_context.is_empty() {
            log!(@Memory, "Retrieved {}KB of context", memory_context.len() / 1024);
        }

        // Include current file list so LLM doesn't need to call list_files
        let file_list_context = if !classification.needs_file_list {
            log!("[Chat] Skipping file list context (not needed for this query)");
            String::new()
        } else {
            let all_files = self.artifact_manager.list_artifacts().unwrap_or_default();

            if !all_files.is_empty() {
                let file_list = all_files
                    .iter()
                    .take(100)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n  ");
                let suffix = if all_files.len() > 100 {
                    format!("\n  ... and {} more", all_files.len() - 100)
                } else {
                    String::new()
                };
                format!("[CURRENT FILES]\n  {}{}\n[END FILES]", file_list, suffix)
            } else {
                String::new()
            }
        }; // end file_list_context if/else

        // Include user profile content so LLM doesn't need to read it
        let profile_context = if user_profile.is_empty() {
            String::new()
        } else {
            format!("[USER PROFILE - ALREADY IN CONTEXT - NEVER call read_file on user_profile.md]\n{}\n[END PROFILE]", user_profile)
        };

        // Include available API credentials so LLM knows which services are configured
        let credentials_context = if !classification.needs_credentials {
            log!("[Chat] Skipping credentials context (not needed for this query)");
            String::new()
        } else {
            match CredentialStore::list(&self.pool).await {
                Ok(creds) if !creds.is_empty() => {
                    let cred_list: Vec<String> = creds
                        .iter()
                        .map(|c| format!("  - {} ({})", c.service_name, c.base_url))
                        .collect();
                    format!("[CONFIGURED API CREDENTIALS - auth headers are auto-injected for these services]\n{}\nYou can use http_request directly with these APIs - credentials are automatically added.\n[END CREDENTIALS]", cred_list.join("\n"))
                }
                _ => String::new(),
            }
        };

        // Add configured email accounts to context
        let email_accounts_context = match EmailStore::list(&self.pool).await {
            Ok(accounts) if !accounts.is_empty() => {
                let mut section = String::from("[CONFIGURED EMAIL ACCOUNTS]\n");
                for acc in &accounts {
                    let auth = if acc.oauth_account_id.is_some() {
                        " [OAuth]"
                    } else {
                        ""
                    };
                    section.push_str(&format!("- {} ({}){}\n", acc.name, acc.email_address, auth));
                }
                section.push_str("[END EMAIL ACCOUNTS]");
                section
            }
            _ => String::new(),
        };

        // Add connected OAuth accounts to context
        let oauth_context = match OAuthStore::list(&self.pool).await {
            Ok(accounts) if !accounts.is_empty() => {
                let account_list: Vec<String> = accounts
                    .iter()
                    .map(|a| {
                        let email = a.email.as_deref().unwrap_or("unknown");
                        format!("  - {} ({}) — scopes: {}", a.provider, email, a.scopes)
                    })
                    .collect();
                format!("\n[CONNECTED OAUTH ACCOUNTS - auth tokens are auto-injected and refreshed for these providers]\n{}\nYou can use http_request with these providers' APIs — OAuth tokens are automatically added.\n[END OAUTH ACCOUNTS]", account_list.join("\n"))
            }
            _ => String::new(),
        };

        // Active app context — tell the LLM which app UI is open so it can read files as needed
        let app_context_section = if let Some(ref ctx) = app_context {
            let app_name = self
                .app_manager
                .get_app(&ctx.app_id)
                .ok()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| ctx.app_id.clone());

            // Load app-specific knowhow from data/apps/{id}/knowhow/*.md
            let app_knowhow_dir = self
                .workspace_path
                .join(crate::core::APPS_DIR)
                .join(&ctx.app_id)
                .join("knowhow");
            let app_knowhow = crate::core::knowhow::load_app_knowhow(&app_knowhow_dir);

            format!(
                "[ACTIVE APP UI]\n\
                The user has the '{name}' app UI open. \
                App files are at data/apps/{id}/. \
                Use list_files and read_file to inspect the app's files as needed.\n\
                {knowhow}\
                [END ACTIVE APP UI]",
                name = app_name,
                id = ctx.app_id,
                knowhow = app_knowhow
            )
        } else {
            String::new()
        };

        // Handle file context when the user is viewing a specific file
        let file_context_section = if let Some(ref path) = file_context {
            format!(
                "[ACTIVE FILE CONTEXT]\n\
The user currently has the file '{}' open in a preview window. \
They may be asking about or wanting to modify this file. \
Keep this in mind when interpreting their request.\n\
[END FILE CONTEXT]",
                path
            )
        } else {
            String::new()
        };

        // Handle URL context when the user has a webpage open in the panel webview
        let url_context_section = if let Some(ref ctx) = url_context {
            let title_line = ctx
                .title
                .as_deref()
                .map(|t| format!("Title: {}\n", t))
                .unwrap_or_default();
            if ctx.content.trim().is_empty() {
                // URL is open but content couldn't be extracted (cross-origin, CSP, etc.)
                format!(
                    "[ACTIVE URL CONTEXT]\n\
The user currently has a webpage open in the browser panel.\n\
URL: {}\n\
{}\
Page content could not be extracted. If the user asks about this page, use browse_url or web_search to retrieve its content.\n\
[END URL CONTEXT]",
                    ctx.url, title_line
                )
            } else {
                // Truncate content to 100K chars for safety (frontend already caps, but belt-and-suspenders)
                let content = if ctx.content.len() > 100_000 {
                    &ctx.content[..ctx.content.floor_char_boundary(100_000)]
                } else {
                    &ctx.content
                };
                format!(
                    "[ACTIVE URL CONTEXT]\n\
The user currently has a webpage open in a side panel and may be asking about its content.\n\
URL: {}\n\
{}\
--- Page Content ---\n\
{}\n\
--- End Page Content ---\n\
[END URL CONTEXT]",
                    ctx.url, title_line, content
                )
            }
        } else {
            String::new()
        };

        // Thread depth context — tell child threads their position so they make
        // better decisions about sub-threading
        let thread_depth_context = if let Some(pid) = parent_thread_id {
            let parent_depth: Result<Option<i32>, _> =
                sqlx::query_scalar("SELECT depth FROM thread_summaries WHERE thread_id = $1")
                    .bind(pid)
                    .fetch_optional(&self.pool)
                    .await;
            let depth: i32 = match parent_depth {
                Ok(Some(d)) => d + 1,
                Ok(None) => 1, // parent not yet in projection — default to depth 1
                Err(e) => {
                    log!(
                        "[Chat] Failed to query parent thread depth for context: {}",
                        e
                    );
                    1
                }
            };

            let remaining = MAX_THREAD_DEPTH - depth;
            let guidance = if remaining <= 0 {
                "You are at maximum thread depth. Do ALL work directly — no sub-threading available.".to_string()
            } else if remaining == 1 {
                "You have 1 level of sub-threading remaining. Use it only if absolutely necessary — strongly prefer doing work directly.".to_string()
            } else {
                format!("You have {} levels of sub-threading remaining. Prefer doing work directly; only delegate truly independent parallel tasks.", remaining)
            };
            format!(
                "[THREAD CONTEXT]\n\
                 You are a child thread at depth {} (max {}).\n\
                 {}\n\
                 [END THREAD CONTEXT]",
                depth, MAX_THREAD_DEPTH, guidance
            )
        } else {
            String::new()
        };

        let mut tools = get_default_tools();
        tools.push(get_notification_tool());
        tools.push(crate::llm::get_read_notifications_tool());
        tools.push(crate::llm::get_navigate_ui_tool());
        tools.push(crate::llm::get_manage_repositories_tool());
        tools.push(get_save_thread_image_tool());
        if image_provider_available {
            tools.push(get_image_generation_tool());
        }
        tools.extend(get_mcp_tools());
        // Add discovered tools from running MCP servers
        tools.extend(self.mcp_manager.get_tool_definitions().await);

        let response_channel: Option<EventChannel> = if is_trigger {
            Some(EventChannel::Trigger)
        } else {
            None
        };

        // Budget for messages = total budget minus system prompt + tool definitions overhead.
        // Same AGENT_CONTEXT_CHAR_BUDGET used by trim_context_if_needed in the agent loop.
        let prompt_overhead: usize = system_prompt.len()
            + tools
                .iter()
                .map(|t| t.name.len() + t.description.len() + t.parameters.to_string().len() + 100)
                .sum::<usize>();
        let message_budget = AGENT_CONTEXT_CHAR_BUDGET.saturating_sub(prompt_overhead);

        // Trim expendable context sections if the initial message would exceed budget
        let fixed_size = profile_context.len()
            + file_list_context.len()
            + credentials_context.len()
            + email_accounts_context.len()
            + oauth_context.len()
            + app_context_section.len()
            + file_context_section.len()
            + url_context_section.len()
            + user_message.len()
            + 500; // 500 for formatting
        let expendable_budget = message_budget.saturating_sub(fixed_size);
        let expendable_size = memory_context.len() + history_context.len();
        if expendable_size > expendable_budget {
            log!("[Chat] Initial context ({}KB) exceeds message budget ({}KB, prompt overhead {}KB), trimming",
                (fixed_size + expendable_size) / 1024, message_budget / 1024, prompt_overhead / 1024);
            let excess = expendable_size - expendable_budget;
            // Trim memory first (most expendable), then history from oldest (start)
            if memory_context.len() >= excess {
                memory_context.truncate(memory_context.len() - excess);
                if let Some(pos) = memory_context.rfind('\n') {
                    memory_context.truncate(pos);
                }
            } else {
                let remaining = excess - memory_context.len();
                memory_context.clear();
                // Trim oldest history first — preserves recent messages
                trim_history_from_oldest(&mut history_context, remaining);
            }
        }

        // Build user message from contextual sections
        let mut user_message_parts: Vec<&str> = Vec::new();
        if !profile_context.is_empty() {
            user_message_parts.push(&profile_context);
        }
        if !memory_context.is_empty() {
            user_message_parts.push(&memory_context);
        }
        if !history_context.is_empty() {
            user_message_parts.push(&history_context);
        }
        if !file_list_context.is_empty() {
            user_message_parts.push(&file_list_context);
        }
        if !credentials_context.is_empty() {
            user_message_parts.push(&credentials_context);
        }
        if !email_accounts_context.is_empty() {
            user_message_parts.push(&email_accounts_context);
        }
        if !oauth_context.is_empty() {
            user_message_parts.push(&oauth_context);
        }
        if !app_context_section.is_empty() {
            user_message_parts.push(&app_context_section);
        }
        if !file_context_section.is_empty() {
            user_message_parts.push(&file_context_section);
        }
        if !url_context_section.is_empty() {
            user_message_parts.push(&url_context_section);
        }
        if !thread_depth_context.is_empty() {
            user_message_parts.push(&thread_depth_context);
        }
        // Add stopped MCP servers context so the LLM knows they exist
        let mcp_stopped_context;
        let stopped_summaries = self.mcp_manager.get_stopped_server_summaries().await;
        if !stopped_summaries.is_empty() {
            mcp_stopped_context = format!(
                "[STOPPED MCP SERVERS]\nThese MCP servers are configured but not running:\n{}\n[END STOPPED MCP SERVERS]",
                stopped_summaries.join("\n")
            );
            user_message_parts.push(&mcp_stopped_context);
        }
        let setup_reminder;
        if !is_trigger && !missing_pref_keys.is_empty() {
            let missing_list = missing_pref_keys.join(", ");
            setup_reminder = format!("CRITICAL: The following preferences are not set: {}. Do NOT proceed with the user's request. Ask the user to configure these first.", missing_list);
            user_message_parts.push(&setup_reminder);
        }
        let request_line = format!("Request: {}", user_message);
        user_message_parts.push(&request_line);

        let user_message_text = {
            let base = user_message_parts.join("\n\n");
            if saved_image_paths.is_empty() {
                base
            } else {
                let path_annotations: Vec<String> = saved_image_paths
                    .iter()
                    .map(|p| format!("[Image saved to {}]", p))
                    .collect();
                format!("{}\n\n{}", base, path_annotations.join("\n"))
            }
        };

        // Section *shape* (name + char_count) is always built so the
        // modal can render the breakdown; only the body is gated by
        // `capture_context`. Body cap (8 KB) prevents a 100 KB system
        // prompt from bloating every events row.
        let capture_body = PreferenceStore::capture_context(&self.pool).await;
        const SECTION_PERSIST_MAX: usize = 8_000;
        let labeled: [(&str, &str); 12] = [
            ("System Instructions", &system_prompt),
            ("User Profile", &profile_context),
            ("Long-term Memory", &memory_context),
            ("Conversation History", &history_context),
            ("File List", &file_list_context),
            ("Credentials", &credentials_context),
            ("Email Accounts", &email_accounts_context),
            ("OAuth", &oauth_context),
            ("App Context", &app_context_section),
            ("File Context", &file_context_section),
            ("URL Context", &url_context_section),
            ("User Message", user_message),
        ];
        let capture_sections: Vec<ContextSection> = labeled
            .into_iter()
            .filter(|(_, content)| !content.is_empty())
            .map(|(name, content)| {
                let body = capture_body.then(|| {
                    if content.len() > SECTION_PERSIST_MAX {
                        truncate_head_tail(content, SECTION_PERSIST_MAX)
                    } else {
                        content.to_string()
                    }
                });
                ContextSection {
                    name: name.to_string(),
                    content: body,
                    char_count: content.chars().count(),
                }
            })
            .collect();
        let capture_tools: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let capture_model = model_override
            .unwrap_or_else(|| self.llm.default_model())
            .to_string();

        let user_content = build_user_content_with_images(
            user_message_text,
            &self.workspace_path,
            &history_image_hashes,
            user_images,
        );

        // Prepend reconstructed (ToolUse, ToolResult) Message pairs for the
        // most recent N tool calls (Phase 3). This keeps tool result bodies —
        // notably `load_knowhow` recipe contents — alive across resumes so
        // multi-step trigger pipelines don't collapse to the stringified
        // `[tools: ...]` summary on the next turn. See
        // `core::store::build_resume_tool_blocks_with_skip_ids`.
        let mut messages = resume_tool_blocks;
        messages.push(Message {
            role: "user".to_string(),
            content: user_content,
        });

        // Run the agentic loop (LLM call → parse response → execute tools → repeat)
        let mut terminator_emitted = false;
        let result = self
            .run_agentic_loop(
                &mut messages,
                &system_prompt,
                &tools,
                request_id,
                thread_id,
                response_channel,
                message_budget,
                &extraction_ctx,
                description_handle,
                origin_id,
                &mut proposed_change,
                user_images,
                device_id,
                model_override,
                reasoning_effort,
                &cancel_token,
                &mut injection_rx,
                &mut terminator_emitted,
                crate::engine::agentic_loop::ContextCaptureSeed {
                    sections: &capture_sections,
                    tools: &capture_tools,
                    model: &capture_model,
                    capture_body,
                },
            )
            .await;

        // Defensive: if the loop returned without emitting a terminator for
        // this request, emit ResponseAborted so the UI's "running" state
        // clears. Skipped on the success path because the flag tracks every
        // emit site — the SQL existence check has no functional index on
        // `payload->>'request_event_id'` and would walk the whole thread
        // on every chat turn otherwise.
        if !terminator_emitted {
            crate::engine::agentic_loop::ensure_terminator_emitted(
                &self.event_bus,
                &self.pool,
                thread_id,
                origin_id,
                response_channel,
            )
            .await;
        }

        // Drain orphaned injections — messages that arrived via inject_prompt()
        // after the agentic loop's last try_recv() but before the ThreadGuard
        // drops. Without this, the messages are silently lost when injection_rx
        // is dropped on function return.
        //
        // Race condition: the frontend sends follow-ups via inject_prompt when
        // effectiveThreadStatus is 'running'. But between ResponseGenerated
        // (emitted inside the loop) and the SSE arriving at the frontend, the
        // agentic loop has already exited — inject_prompt succeeds (thread is
        // still in active_threads) but nobody reads the channel.
        //
        // Fix: drain and attach to the result so chat_submit can re-submit them
        // as regular follow-up messages after the guard drops.
        let orphans = Self::drain_orphaned_injections(&mut injection_rx);
        if !orphans.is_empty() {
            log!("[Chat] {} orphaned injection(s) after agentic loop exit for thread {} — will re-submit as follow-ups",
                orphans.len(), thread_id);
        }

        result.map(|mut r| {
            r.orphaned_injections = orphans;
            r
        })
    }

    pub(crate) fn clean_response(&self, content: &str) -> String {
        let content = content.trim();

        if content.starts_with("Tool results:")
            || content.starts_with("[list_files]")
            || content.starts_with("[read_file]")
            || content.starts_with("[run_python]")
            || content.contains("run_python({")
        {
            return "Task completed. Check the workspace for any created files.".to_string();
        }

        content.to_string()
    }

    pub(crate) fn describe_tool(&self, name: &str, args: &serde_json::Value) -> String {
        if let Some(app_id) = args.get("app_id").and_then(|v| v.as_str()) {
            if matches!(name, "refresh_app" | "capture_app" | "navigate_ui") {
                if let Ok(app) = self.app_manager.get_app(app_id) {
                    let mut enriched = args.clone();
                    enriched["app_name"] = serde_json::Value::String(app.name);
                    return crate::core::describe_tool(name, &enriched);
                }
            }
        }
        crate::core::describe_tool(name, args)
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
