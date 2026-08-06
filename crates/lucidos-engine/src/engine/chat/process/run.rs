//! The chat-turn orchestrator: `process_message_with_steps_internal`. Walks the
//! per-turn pipeline — routing fast-paths, thread registration, the
//! MessageReceived boundary, title emission, history + system-prompt + context
//! assembly, and the agentic loop. The big sub-phases live in sibling modules
//! (`titles`, `history`, `system_prompt`, `context_sections`); this file wires
//! them together exactly as the original single function did.

use crate::core::{PreferenceStore, PREF_MODEL_IMAGE_DESCRIPTION, PREF_MODEL_MEMORY};
use crate::engine::agentic_loop::{
    cancel_cause_for_turn, meta_with_cancel_actor, terminal_result, until_canceled,
};
use crate::engine::context::{
    agent_context_char_budget, tool_definitions_chars, trim_history_from_oldest,
};
use crate::engine::thread_events::{ActorMode, EventChannel, MessageOrigin};
use crate::engine::types::*;
use crate::engine::{InjectedPrompt, LucidosEngine, ThreadGuard};
use crate::llm::{
    get_default_tools, get_image_generation_tool, get_notification_tool,
    get_save_thread_image_tool, get_view_image_tool, Message,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::events::{describe_images, emit_routing_failure, make_message_received};
use super::super::images::build_user_content_with_images;
use super::super::process_helpers::{
    build_trigger_started_event, classify_or_fallback, TriggerContext,
};
use super::context_build::{build_capture_sections, build_loaded_knowhow_block};
use super::PreEmittedOrigin;

pub(super) async fn resolve_route_overrides(
    pool: &sqlx::PgPool,
    use_coding_agent: Option<bool>,
    thread_id: Option<Uuid>,
    exclude_event_id: Option<Uuid>,
    model_override: Option<&str>,
    reasoning_effort: Option<&str>,
) -> (Option<String>, Option<String>) {
    if use_coding_agent == Some(true) {
        return (None, reasoning_effort.map(str::to_string));
    }

    // Chat: honor per-thread memory — a follow-up with no explicit override
    // reuses the model/effort this thread last ran with. `exclude_event_id`
    // drops the current turn's own MessageReceived when it was pre-emitted
    // upstream, so the thread never reads itself as its "previous" value.
    PreferenceStore::resolve_chat_overrides_for_thread(
        pool,
        thread_id,
        exclude_event_id,
        model_override.map(str::to_string),
        reasoning_effort.map(str::to_string),
    )
    .await
}

/// Whether a typed-instead-of-clicked message is eligible to answer a pending
/// `UserQuestionAsked` via the FreeText fast-path.
///
/// Only a genuine **human** follow-up may be consumed as the user's answer.
/// Agent- and engine-driven re-entries on the same thread — most importantly a
/// child-completion wake (`ActorMode::Agent`, see
/// `notify_parent_of_child_completion`), but also trigger fires and engine
/// recovery notes — must NOT be routed as the answer. Without this guard the
/// child's `[CHILD THREAD COMPLETED] …` block gets persisted as a bogus
/// `UserQuestionAnswered { FreeText }` (actor = `thread_link`/`child`),
/// silently consuming the user's open question. Excluded here, the wake falls
/// through to the injection fast-path below, which queues it as `WakeFromChild`
/// so the question stays live for the user and the completion is processed
/// right after they answer it.
///
/// A message whose starter event is ALREADY persisted is likewise not eligible:
/// answering consumes the text as `UserQuestionAnswered` and emits no
/// `MessageReceived`, so a pre-emitted message routed here would leave both
/// events in the thread for one thing the user said. The API boundary skips its
/// pre-emit when a question is open, so this only catches the narrow race where
/// one opened in between, plus the orphan re-process path, which carries a
/// persisted `MessageReceived` for the same reason.
pub(super) fn message_can_answer_pending_question(
    is_new_thread: bool,
    user_message: &str,
    mode: ActorMode,
    pre_emitted_origin: Option<PreEmittedOrigin>,
) -> bool {
    !is_new_thread
        && !user_message.is_empty()
        && mode == ActorMode::Human
        && pre_emitted_origin.is_none()
}

/// The constant inputs the setup-cancel exit needs, built once before the
/// turn's first phase so each cancel checkpoint in the setup is a one-liner.
/// See `LucidosEngine::cancel_during_setup`.
struct SetupCancel {
    thread_id: Uuid,
    request_id: Uuid,
    /// The turn's response meta (`request_event_id` + channel), so the
    /// terminator binds to the same exchange the loop's own cancel would.
    meta: crate::engine::thread_events::EventMeta,
    model: Option<String>,
    effort: Option<String>,
}

impl LucidosEngine {
    /// Internal: Process a message with optional trigger context
    // Same wide signature as `process_message_with_steps` plus trigger context.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn process_message_with_steps_internal(
        &self,
        user_message: &str,
        model_override: Option<&str>,
        trigger: Option<TriggerContext>, // None for user-driven chat; Some when fired by the scheduler
        app_context: Option<AppContext>, // app context if chatting from within an app
        file_context: Option<String>,    // file path if user is viewing a file
        reasoning_effort: Option<&str>,  // unified reasoning level: none/low/medium/high/xhigh/max
        user_images: Option<&[crate::api::ChatImage]>, // base64-encoded images pasted by user
        device_id: Option<&str>,         // device that sent this message
        use_coding_agent: Option<bool>,  // bypass LLM and spawn a coding agent directly
        event_id: Option<&str>,          // client-generated UUID for reliable matching
        thread_id: Option<Uuid>,         // None = new thread, Some = follow-up
        conflict_change_id: Option<Uuid>, // change ID for merge conflict resolution
        repo_id: Option<&str>,           // external repository ID for CC worktree
        url_context: Option<crate::api::UrlContext>, // webpage content from Tauri panel webview
        parent_thread_id: Option<Uuid>,  // parent thread if spawned by run_thread
        spawning_event_id: Option<Uuid>, // event in parent thread that triggered the spawn (mode != Human only)
        mode: ActorMode,
        cc_model: Option<&str>, // CC-specific model override (from compose view pre-session selection)
        coding_agent: Option<crate::runtime::CodingAgent>, // backend for a NEW coding-agent thread; stored value wins on follow-ups
        pre_emitted_origin: Option<PreEmittedOrigin>, // skip MessageReceived: the caller already persisted it
        title: Option<&str>, // caller-provided title (skips async LLM title gen)
        origin: Option<MessageOrigin>,
        external_cancel: Option<CancellationToken>, // forwarded into the per-thread cancel_token (used by triggers)
        urgency: crate::engine::FollowUpUrgency, // child follow-up only: preempt the child's in-flight turn
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        let urgent = urgency.is_urgent();
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

        // For chat turns, `None` means "use the user's Lucidos chat defaults",
        // not `LlmProvider::default_model()` (which drops `[1m]` and carries no
        // effort). For coding-agent turns, `None` must stay unset so the agent
        // path falls through to live/thread/backend settings instead of
        // inheriting the unrelated Lucidos chat reasoning preference.
        // `pre_emitted_origin` here is the upstream value (the fast-path
        // mutation below happens after this). When set, it is the current
        // turn's already-persisted MessageReceived (`events.id`); exclude it so
        // the per-thread lookup reads the PRIOR message, not this one. In the
        // normal path it's `None` and the current turn isn't emitted until later.
        let (resolved_model, resolved_effort) = resolve_route_overrides(
            &self.pool,
            use_coding_agent,
            thread_id,
            pre_emitted_origin.map(PreEmittedOrigin::event_id),
            model_override,
            reasoning_effort,
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

        // Spawn Flash image description in background (don't block main LLM flow).
        // Returns `(description, model)` so the agentic loop can emit
        // `ImageDescribed { model, .. }` with the actual model that produced
        // the description (the user's `model_image_description` preference, or
        // the extractor's default when the preference is empty / "default").
        let mut description_handle = if let (Some(imgs), Some(ref extractor)) =
            (user_images, &self.extractor)
        {
            if !imgs.is_empty() {
                let img_desc_model = PreferenceStore::get(&self.pool, PREF_MODEL_IMAGE_DESCRIPTION)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                match extractor.provider_for_model(&img_desc_model) {
                    Ok(provider) => {
                        // Resolve the model name we'll record on the event. The
                        // pref string wins when set; otherwise fall back to the
                        // provider's default (which is the model the call actually
                        // hits via `provider_for_model`'s "" / "default" branch).
                        let recorded_model =
                            if img_desc_model.is_empty() || img_desc_model == "default" {
                                provider.default_model().to_string()
                            } else {
                                img_desc_model
                            };
                        let imgs: Vec<crate::api::ChatImage> = imgs.to_vec();
                        Some(tokio::spawn(async move {
                            match describe_images(provider.as_ref(), &imgs).await {
                                Ok(desc) => Some((desc, recorded_model)),
                                Err(e) => {
                                    log!("[Chat] Image description failed: {}", e);
                                    None
                                }
                            }
                        }))
                    }
                    Err(e) => {
                        log!("[Chat] Failed to build image-description provider: {}", e);
                        None
                    }
                }
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

        // AskUserQuestion free-form path: if the thread is currently waiting
        // on a `UserQuestionAsked` and the user typed instead of clicking an
        // option, route the message to the answer-question handler with
        // `FreeText` instead of creating a new exchange. Applies to BOTH chat
        // and CC threads — the chat agent's `ask_user_question` tool blocks
        // on the same wait registry as CC's hook, so without this routing the
        // typed text would spawn a brand-new turn while the prior turn's tool
        // stays blocked, leaving the thread stuck "Requesting" forever.
        //
        // Gated on `mode == Human` (see `message_can_answer_pending_question`):
        // only a real human follow-up answers the question. An agent-driven
        // child-completion wake (`ActorMode::Agent`) must fall through to the
        // injection fast-path below instead of being persisted as a bogus
        // FreeText answer.
        //
        // Uses `lookup_active_question_tool_use_id` (not `..pending..`) so a
        // question orphaned by a prior `ResponseAborted`/`Canceled`/`Failed`/
        // `CodingAgentIdled` doesn't intercept the follow-up — otherwise the
        // typed text would be silently consumed as the dead question's answer
        // and `MessageReceived` would never be emitted.
        if message_can_answer_pending_question(
            is_new_thread,
            user_message,
            mode,
            pre_emitted_origin,
        ) {
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

        // Permission supersede: if this CC thread is waiting on a permission
        // card and the user typed a message instead of clicking a button,
        // resolve the pending permission(s) as denied so the card's buttons
        // stop dangling — otherwise the thread reads as stuck on the prompt
        // while CC moves on, and clicking the (still-rendered) buttons 404s
        // once CC interrupts and the in-memory waiter is gc'd. Unlike the
        // question free-form path above, this does NOT consume the message —
        // the typed text still routes to CC below as a normal follow-up.
        if use_coding_agent == Some(true) && !is_new_thread && !user_message.is_empty() {
            crate::engine::cc_permission::resolve_pending_permissions_as_superseded(
                self.pool(),
                &self.event_bus,
                &self.pending_cc_permission,
                thread_id,
                origin.clone(),
            )
            .await;
        }

        // Same supersede for the command-guard permission lane (ADR 0002): a
        // chat thread parked on a `CommandPermissionRequested` whose user types
        // a new message instead of clicking resolves the card as denied (which
        // also unblocks the parked agentic loop). Chat-only — the command lane
        // never fires on a CC thread.
        if use_coding_agent != Some(true) && !is_new_thread && !user_message.is_empty() {
            crate::engine::command_permission::resolve_pending_command_permissions_as_superseded(
                self.pool(),
                &self.event_bus,
                &self.pending_command_permission,
                thread_id,
                origin.clone(),
            )
            .await;
            // Same supersede for the chat MCP permission lane: a thread parked on
            // an `McpPermissionRequested` whose user types a new message instead
            // of clicking resolves the card as denied (and unblocks the parked
            // agentic loop). Chat-only, same as the command lane.
            crate::engine::mcp_permission::resolve_pending_mcp_permissions_as_superseded(
                self.pool(),
                &self.event_bus,
                &self.pending_mcp_permission,
                thread_id,
                origin.clone(),
            )
            .await;
        }

        // Fast-path for CC follow-ups: route via msg_tx BEFORE register_thread_queued
        // so a follow-up arriving during an active Claude Code session is picked up by the
        // running event loop instead of spawning a new Claude Code session (or blocking on
        // the prior turn's still-held ThreadGuard).
        //
        // This handles both idle sessions (msg picked up immediately) and busy sessions
        // (msg queued in the channel, picked up after current work finishes).
        if use_coding_agent == Some(true) && !is_new_thread {
            // Check if there's a live Claude Code session to route to.
            let has_session = {
                let sessions = self.agent_sessions.lock().await;
                sessions
                    .get(&thread_id)
                    .map(|s| s.is_live())
                    .unwrap_or(false)
            };

            // Race-bridge: when CC's spawn is still in flight, agent_sessions
            // is empty but active_threads holds the thread. Wait for the
            // session to register so this follow-up routes via msg_tx as the
            // fast-path intends. See `wait_for_cc_session_alive`.
            let has_session = has_session
                || super::super::process_helpers::wait_for_cc_session_alive(
                    &self.agent_sessions,
                    &self.active_threads,
                    thread_id,
                    std::time::Duration::from_secs(30),
                    std::time::Duration::from_millis(100),
                )
                .await;

            if has_session {
                // Interrupt-and-redirect. Interrupt the live turn (it ends as a
                // resumable Canceled boundary: no spurious ResponseGenerated,
                // no change proposed for the redirected-away work), then fall
                // through to route the follow-up as the next turn on the same
                // thread, full context preserved. Never fires for a child-wake.
                //
                // Codex arms unconditionally because its app-server / exec
                // protocols accept input only at a turn boundary, so a mid-turn
                // follow-up would otherwise queue invisibly behind a long turn.
                // Claude Code arms only when the caller marked the follow-up
                // urgent: CC steers via stdin, so a plain follow-up is
                // forwarded as-is and reaches the child at its next turn
                // boundary. That boundary is the catch, and `urgent` is the
                // opt-out from waiting for it. See `should_redirect_followup`.
                let redirect_idle_notify = {
                    let mut sessions = self.agent_sessions.lock().await;
                    super::super::process_helpers::arm_followup_redirect(
                        &mut sessions,
                        thread_id,
                        // Genuine user follow-up, not an engine-internal
                        // child-wake. A pre-emitted USER message is still a
                        // user follow-up and must keep arming the redirect.
                        !pre_emitted_origin.is_some_and(PreEmittedOrigin::is_engine_reentry),
                        urgent,
                        &origin,
                    )
                };
                if let Some(idle_notify) = redirect_idle_notify {
                    // Block until the interrupted turn reaches a boundary (idle
                    // or process exit) BEFORE emitting the follow-up's
                    // MessageReceived, so the turn's Canceled terminal is
                    // sequenced first (coding-agent exchanges group by sequence
                    // order — see the MessageReceived-first note below). Unlike
                    // apply_now's wait_for_idle, the boundary may ALREADY have
                    // been reached by the time we get here (the graceful
                    // interrupt can idle the turn before we register a waiter),
                    // so re-check is_waiting each iteration instead of relying on
                    // a future idle_notify alone. The engine's 8s interrupt
                    // escalation hard-kills a non-idling turn (→ process_exited),
                    // and REDIRECT_INTERRUPT_MAX_WAIT is the backstop above that.
                    // If the session died, the msg_tx send below fails and the
                    // lost-race fall-through below routes the follow-up via the
                    // slow `--resume` path instead of dropping it.
                    let redirect_deadline = std::time::Instant::now()
                        + super::super::process_helpers::REDIRECT_INTERRUPT_MAX_WAIT;
                    loop {
                        // Construct the notify future before the state check.
                        // (tokio registers the waiter on first *poll* — inside
                        // the timeout await below, after the check — and the run
                        // loop signals via notify_waiters() with no stored
                        // permit, so a notification landing between the check and
                        // the await is not retained. That's why correctness here
                        // rests on the bounded 2s re-poll, not on registration
                        // ordering: each iteration re-reads is_waiting, so the
                        // notify is only a latency optimization, never the
                        // safety net. A boundary already reached is caught by the
                        // check on the very next iteration.)
                        let notified = idle_notify.notified();
                        let at_boundary = {
                            let sessions = self.agent_sessions.lock().await;
                            match sessions.get(&thread_id) {
                                Some(s) => s.is_waiting || !s.is_live(),
                                None => true,
                            }
                        };
                        if at_boundary {
                            log!(
                                "[Chat] Codex redirect: interrupted turn reached a boundary for thread {} — follow-up runs as the next turn",
                                thread_id
                            );
                            break;
                        }
                        if std::time::Instant::now() >= redirect_deadline {
                            log!(
                                "[Chat] Codex redirect: timed out waiting for the interrupt on thread {} — routing the follow-up anyway",
                                thread_id
                            );
                            break;
                        }
                        let _ =
                            tokio::time::timeout(std::time::Duration::from_secs(2), notified).await;
                    }
                }

                // Emit MessageReceived FIRST — guarantees it gets a lower sequence
                // number and earlier timestamp than CodingAgentPromptSent (emitted by
                // the CC event loop when it picks up the message). Without this
                // ordering, CodingAgentPromptSent can race ahead and end up in the
                // wrong exchange, causing a brief "interrupted" flash and incorrect
                // thread section placement.
                //
                // Skip the emit whenever the caller already persisted the
                // event. Only a CHILD WAKE also changes how the input routes
                // (see `AgentInputKind::WakeFromChild` docs); a pre-emitted
                // user message is an ordinary follow-up.
                let (origin_event_id, input_kind) = if let Some(pre) = pre_emitted_origin {
                    let kind = if pre.is_engine_reentry() {
                        AgentInputKind::WakeFromChild
                    } else {
                        AgentInputKind::User
                    };
                    (Some(pre.event_id()), kind)
                } else {
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
                                // Clone, not move: the `!send_ok` branch below now
                                // falls through to the slow path, which reuses
                                // `device_name` (and so does the non-CC fast-path).
                                device_name.clone(),
                                parent_thread_id,
                                spawning_event_id,
                                mode,
                                None,
                                None,
                                origin.clone(),
                            ),
                            meta: EventMeta {
                                event_id: event_id.and_then(|s| uuid::Uuid::parse_str(s).ok()),
                                channel: Some(EventChannel::ClaudeCode),
                                ..EventMeta::NONE
                            },
                        })
                        .await?;
                    // Stash the emitted id so a lost-race fall-through to the slow
                    // path (below) doesn't re-emit MessageReceived — mirrors the
                    // non-CC injection fast-path. On the success path we return
                    // before the slow path reads it, so this is harmless there.
                    if let Some(result) = emit_result {
                        pre_emitted_origin = Some(PreEmittedOrigin::Message(result.event_id));
                    }
                    (
                        pre_emitted_origin
                            .map(PreEmittedOrigin::event_id)
                            .or_else(|| event_id.and_then(|s| uuid::Uuid::parse_str(s).ok())),
                        AgentInputKind::User,
                    )
                };

                let images = user_images.map(|imgs| imgs.to_vec());
                let send_ok = {
                    let sessions = self.agent_sessions.lock().await;
                    if let Some(session) = sessions.get(&thread_id) {
                        if session.is_live() {
                            // Track the expected Result before sending. If the
                            // send fails (channel dropped between the lookup
                            // and the send), undo the increment so the
                            // existing session's idle-exit cancel doesn't get
                            // suppressed by a phantom pending follow-up.
                            session
                                .pending_followups
                                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                            let send_result = session.msg_tx.send(AgentUserInput {
                                text: user_message.to_string(),
                                images,
                                origin_event_id,
                                kind: input_kind,
                            });
                            if send_result.is_err() {
                                session
                                    .pending_followups
                                    .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
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

                if send_ok {
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

                // The live session went away or is terminating between the
                // has_session check and the send — most commonly the idle-
                // termination race: the session loop set `process_exited` and
                // cancelled the subprocess (see the ExitSubprocess arm in
                // run_session/run.rs) while this follow-up was in flight. Don't
                // emit a routing failure ("please try again") and don't drop the
                // message — fall through to the slow path so it spawns a fresh
                // `--resume` session and runs as the next turn. MessageReceived
                // was already emitted above (`pre_emitted_origin` is set), so the
                // slow path won't re-emit it. Mirrors the non-CC injection
                // fast-path's lost-race fallback below.
                log!(
                    "[Chat] CC follow-up lost the live-session race for thread {} — falling back to slow path (resume)",
                    thread_id
                );
            }
        }

        // Fast-path for non-CC follow-ups: if the thread already has an active
        // agentic loop, inject the message (with images) mid-flight instead of
        // blocking in register_thread_queued for up to 60s. This mirrors the CC
        // fast-path above but uses injection_tx instead of msg_tx.
        //
        // An event-wait delivery takes this path like any other follow-up, and
        // that is load-bearing rather than incidental. It used to be barred:
        // an attached wake's prompt was the empty string (the payload lived in
        // a `ToolResult` the running turn never rebuilt), so it was pushed into
        // the 60 s queue instead, where the stuck-turn backstop evicted the
        // running turn to let it in. That is the 2026-08-06 abort. A wake now
        // carries real prose, so it injects.
        if use_coding_agent != Some(true) && !is_new_thread {
            let has_active = {
                let threads = self.active_threads.lock().unwrap();
                threads.contains_key(&thread_id)
            };

            // The Lucidos Agent's half of interrupt-and-redirect. An injected
            // prompt is only READ between loop iterations, so a turn inside a
            // long tool call sits on it for as long as that call runs. One tool
            // yields early (`bash_output(wait_secs=…)` parks on
            // `injection_notify`), and nothing else does, so an urgent
            // follow-up cannot rely on the soft path.
            //
            // Cancel WITHOUT injecting, then fall through to the slow path.
            // `register_thread_queued` waits for this turn to release the
            // handle, so the follow-up runs as the next turn with the
            // interrupted turn's Canceled terminal sequenced first, matching
            // the coding-agent lane. Injecting first would race the loop's own
            // drain: a prompt consumed by the dying turn's final iteration is
            // neither answered nor left for the orphan chain.
            if urgent && has_active {
                let preempted = self.cancel_thread_for_followup(thread_id, origin.clone());
                log!(
                    "[Chat] Urgent follow-up preempting the live turn on thread {} (preempted={})",
                    thread_id,
                    preempted
                );
            } else if has_active {
                // Emit MessageReceived FIRST — same ordering guarantee as the CC
                // fast-path. The agentic loop emits UserPromptInjected when it
                // picks up the injection; without this ordering, UserPromptInjected
                // can race ahead and create a duplicate exchange boundary.
                //
                // Skip the emit whenever the caller already persisted the
                // event. Only an ENGINE WAKE also changes how the input is
                // projected (see `PreEmittedOrigin::inject_kind` and the
                // `InjectedPromptKind` docs). A pre-emitted user message still
                // injects as `UserText`, so the loop acknowledges it with a
                // `UserPromptInjected`.
                use crate::engine::thread_events::EventMeta;
                let inject_kind = if let Some(pre) = pre_emitted_origin {
                    pre.inject_kind()
                } else {
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
                    // Stash the emit for the race-recovery handoff below.
                    if let Some(result) = emit_result {
                        pre_emitted_origin = Some(PreEmittedOrigin::Message(result.event_id));
                    }
                    crate::engine::InjectedPromptKind::UserText
                };

                let origin_event_id = pre_emitted_origin
                    .map(PreEmittedOrigin::event_id)
                    .or_else(|| event_id.and_then(|s| Uuid::parse_str(s).ok()));
                let images = user_images.map(|imgs| imgs.to_vec());
                let injected = {
                    let threads = self.active_threads.lock().unwrap();
                    if let Some(handle) = threads.get(&thread_id) {
                        handle.inject(crate::engine::InjectedPrompt {
                            text: user_message.to_string(),
                            event_id: origin_event_id,
                            mode,
                            spawning_event_id,
                            images,
                            origin: origin.clone(),
                            kind: inject_kind,
                        })
                    } else {
                        false
                    }
                };

                if injected {
                    log!("[Chat] Follow-up injected into active thread {} (bypassed register_thread_queued)", thread_id);
                    // This path returns to the HTTP caller right here, so the
                    // injected message never reaches `run_agentic_loop`'s
                    // post-first-call emit — the handle would just be dropped.
                    // Detach the emit instead: without it an image that arrived
                    // mid-turn produced no `ImageDescribed` at all, so once its
                    // bytes aged out of context the thread held no record of
                    // what had been shown. Spawned rather than awaited to keep
                    // the injection response immediate.
                    if let (Some(handle), Some(origin_id)) =
                        (description_handle.take(), origin_event_id)
                    {
                        let event_store = self.event_store.clone();
                        let bus = self.event_bus.clone();
                        tokio::spawn(async move {
                            crate::engine::agentic_loop::emit_image_descriptions(
                                &event_store,
                                &bus,
                                thread_id,
                                origin_id,
                                Some(EventChannel::Chat),
                                Some(handle),
                            )
                            .await;
                        });
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

                // Injection failed — the agentic loop completed (and dropped its
                // injection receiver) between the has_active check and the send.
                // Fall through to the slow path so register_thread_queued can
                // start a fresh loop for this message. The slow path uses
                // `pre_emitted_origin` (set above whether MessageReceived was
                // freshly emitted OR the caller passed it for a child-wake) so
                // it doesn't double-emit a starter event.
                log!(
                    "[Chat] Follow-up injection lost the race for thread {} — falling back to slow path",
                    thread_id
                );
            }
        }

        // Wait for any in-progress request on this thread to finish, then register.
        // This queues follow-up messages instead of cancelling in-progress work.
        // `guard` ensures the thread is unregistered on every exit path: the
        // normal path drops it explicitly via `finalize_turn_and_drain_injections`
        // (remove-then-drain, see below), and the error/panic paths fall back to
        // its Drop impl.
        let (cancel_token, mut injection_rx, guard) = self.register_thread_queued(thread_id).await;

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
        let origin_id = if let Some(pre) = pre_emitted_origin {
            pre.event_id()
        } else {
            let (user_thread_event, user_meta) = if let Some(ref tc) = trigger {
                build_trigger_started_event(
                    &tc.trigger_id,
                    &tc.trigger_name,
                    &tc.invocation,
                    user_message,
                    tc.go_to_review,
                )
            } else {
                let is_cc = use_coding_agent == Some(true);
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
                            EventChannel::ClaudeCode
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

        // Publish this turn's anchor on the handle. Everything below stamps
        // `request_event_id: Some(origin_id)` on its events and on its own
        // terminator; an abort emitted from OUTSIDE the loop (restart teardown,
        // stuck-turn eviction, shutdown sweep) has no other way to learn it, and
        // guessing it from the newest originating-type event picks a queued
        // follow-up or a stale message instead. See
        // `engine::in_flight_request_event_id`.
        self.set_thread_request_event_id(thread_id, guard.generation(), origin_id);

        self.maybe_emit_titles(
            thread_id,
            &thread_id_str,
            title,
            is_trigger,
            is_new_thread,
            &trigger,
            user_message,
            user_images,
        )
        .await?;

        // `_guard` MUST stay alive across this await so cancel_thread lands
        // on the per-thread cancel_token (see process_cc.rs module doc).
        if use_coding_agent == Some(true) {
            // Agent-spawned threads via a parent's tool call (run_thread →
            // spawn_agent_thread) skip the 250ms CC_SPAWN_DEBOUNCE — no
            // user keyboard in the loop, no rapid-fire to coalesce.
            // Discriminator: mode=Agent + parent linkage (the spawn comes
            // from a tool call). Wake-from-child callbacks also pass
            // mode=Agent but have parent_thread_id=None, so they keep
            // coalescing as before.
            let skip_coalesce = mode == ActorMode::Agent
                && parent_thread_id.is_some()
                && spawning_event_id.is_some();
            return self
                .run_cc_chat_branch(
                    thread_id,
                    request_id,
                    user_message,
                    user_images,
                    repo_id,
                    cc_model,
                    reasoning_effort,
                    coding_agent,
                    conflict_change_id,
                    origin_id,
                    spawning_event_id,
                    &cancel_token,
                    chat_start,
                    skip_coalesce,
                )
                .await;
        }

        // ── Turn setup ──────────────────────────────────────────────────────
        //
        // Everything from here to `run_agentic_loop` assembles the turn's
        // context: history load, query classification (an LLM call), memory
        // retrieval, the system prompt, the context sections. On a big thread
        // that is tens of seconds, and none of it used to look at
        // `cancel_token`. The first check was the loop's own pre-iteration arm,
        // so a Stop pressed here did nothing until the whole phase finished
        // (observed: 32s of "Canceling…" with not one event emitted).
        //
        // So every long await below is raced against the token via
        // `until_canceled`, and the short ones get a plain `is_cancelled()`
        // checkpoint. Each of those is a pure READ: losing the race drops the
        // future, which is only safe because none of them emits an event or
        // writes anything. The exit routes through `cancel_during_setup`, which
        // emits the terminator AND runs the shared turn tail, so a follow-up
        // typed during the setup is still recovered and re-submitted.
        let response_channel: Option<EventChannel> = if is_trigger {
            Some(EventChannel::Trigger)
        } else {
            None
        };
        let cancel_exit = SetupCancel {
            thread_id,
            request_id,
            meta: crate::engine::thread_events::EventMeta {
                request_event_id: Some(origin_id),
                channel: response_channel,
                ..crate::engine::thread_events::EventMeta::NONE
            },
            // Mirrors the agentic loop's own cancel arm: the turn's resolved
            // model, or the provider default when the request carried none.
            model: {
                let provider = self.current_provider();
                let m = model_override.unwrap_or(provider.default_model());
                (!m.is_empty()).then(|| m.to_string())
            },
            effort: reasoning_effort.map(str::to_string),
        };
        if cancel_token.is_cancelled() {
            return Ok(self
                .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                .await);
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

        // Resume tool blocks + conversation history + per-thread loaded
        // knowhow, all derived from a single events fetch (see
        // `load_chat_history`).
        let Some(history_load) = until_canceled(
            &cancel_token,
            self.load_chat_history(
                is_trigger,
                is_new_thread,
                thread_id,
                &thread_id_str,
                user_message,
                &memory_model_pref,
            ),
        )
        .await
        else {
            return Ok(self
                .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                .await);
        };
        let super::history::ChatHistoryLoad {
            resume_tool_blocks,
            loaded_knowhow_docs,
            mut history_context,
            conversation_summary,
            history_image_hashes,
        } = history_load;

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
                // The single slowest step of the setup on a warm thread: a
                // full LLM round-trip before any of the turn's own events exist.
                let Some(classification) = until_canceled(
                    &cancel_token,
                    classify_or_fallback(extractor.classify_query(
                        user_message,
                        ctx,
                        Some(&memory_model_pref),
                    )),
                )
                .await
                else {
                    return Ok(self
                        .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                        .await);
                };
                classification
            } else {
                crate::memory::QueryClassification::default()
            }
        };

        // Retrieve relevant context from memory (skipped if classification says not needed)
        let response_meta = cancel_exit.meta.clone();
        let Some((mut memory_context, memory_results)) = until_canceled(
            &cancel_token,
            self.retrieve_context(user_message, &classification),
        )
        .await
        else {
            return Ok(self
                .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                .await);
        };

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

        // The turn's tool-call cap, read ONCE here and handed to both consumers
        // below: the system prompt states it to the model, and the agentic loop
        // enforces it. Two independent reads could disagree (and would hand the
        // model a number nothing enforces), and reading it here also freezes the
        // cap for the whole turn, so changing the setting mid-turn cannot move
        // the backstop under a running loop.
        let max_tool_calls = crate::core::PreferenceStore::max_tool_calls(&self.pool).await;

        let Some((system_prompt, missing_pref_keys, image_provider_available)) = until_canceled(
            &cancel_token,
            self.build_chat_system_prompt(
                &user_timezone,
                &user_language,
                device_id,
                is_trigger,
                &trigger,
                max_tool_calls,
            ),
        )
        .await
        else {
            return Ok(self
                .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                .await);
        };

        if !memory_context.is_empty() {
            log!(@Memory, "Retrieved {}KB of context", memory_context.len() / 1024);
        }

        let Some(context_sections) = until_canceled(
            &cancel_token,
            self.build_chat_context_sections(super::context_sections::ChatContextInputs {
                classification: &classification,
                user_profile: &user_profile,
                device_id,
                event_device: device_name.as_deref(),
                app_context: app_context.as_ref(),
                file_context: file_context.as_deref(),
                url_context: url_context.as_ref(),
                parent_thread_id,
            }),
        )
        .await
        else {
            return Ok(self
                .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                .await);
        };
        let super::context_sections::ChatContextSections {
            file_list_context,
            profile_context,
            device_preferences_context,
            credentials_context,
            email_accounts_context,
            oauth_context,
            app_context_section,
            file_context_section,
            url_context_section,
            thread_depth_context,
        } = context_sections;

        let mut tools = get_default_tools();
        tools.push(get_notification_tool());
        // Grouped notification-inbox tool (list / mark_read / mark_all_read) +
        // any other manifest-declared LLM tools — single source of truth.
        tools.extend(crate::capability_manifest::llm_tools());
        tools.push(crate::llm::get_navigate_ui_tool());
        // manage_models / manage_repositories are now manifest-built grouped tools
        // contributed by llm_tools() above (no longer spliced here).
        tools.push(get_save_thread_image_tool());
        // view_image re-loads an earlier thread image into vision — it needs no
        // image *generation* provider, so it's always available (unlike generate_image).
        tools.push(get_view_image_tool());
        if image_provider_available {
            tools.push(get_image_generation_tool());
        }
        // MCP server management is the grouped `mcp` manifest tool (spliced via
        // llm_tools() above). Discovered tools from running MCP servers are added
        // separately below.
        //
        // The one setup await that is checkpointed rather than raced: it can be
        // mid-RPC to a running MCP server, and dropping it there is not the pure
        // read the other phases are. So the checkpoint goes AFTER it, where it
        // catches a Stop that landed during the RPC. Before it the check would
        // be near-dead, since the preceding phase is raced and would already
        // have exited. The two remaining awaits (stopped-server summaries, the
        // capture preference) are in-memory / single-row and are followed
        // immediately by the loop's own pre-iteration check.
        tools.extend(self.mcp_manager.get_tool_definitions().await);
        if cancel_token.is_cancelled() {
            return Ok(self
                .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                .await);
        }

        // Budget for messages = model-derived total budget minus system prompt + tool
        // definitions overhead. The budget scales with the resolved model's context
        // window (e.g. ~1.49M chars for a 1M model vs ~288k chars for default
        // 200k-token Claude), so a big-window turn isn't trimmed back to the
        // smaller model's limit. The window comes from the model's registry row
        // when it declares one — the id-shape fallback has no rule for
        // OpenRouter / Gemini / local ids and would hand them all 200k.
        let provider = self.current_provider();
        let resolved_model = model_override.unwrap_or_else(|| provider.default_model());
        let total_budget = agent_context_char_budget(self.context_window_for(resolved_model));
        let tool_defs_chars = tool_definitions_chars(&tools);
        let prompt_overhead: usize = system_prompt.len() + tool_defs_chars;
        let message_budget = total_budget.saturating_sub(prompt_overhead);

        // `loaded_knowhow_docs` was already populated up in the follow-up
        // branch (or left empty for triggers / new threads). Build the block
        // here so its byte size counts against fixed_size below — knowhow
        // bodies can be large and were previously un-budgeted.
        let loaded_knowhow_block = build_loaded_knowhow_block(&loaded_knowhow_docs);

        // Trim expendable context sections if the initial message would exceed budget
        let fixed_size = profile_context.len()
            + device_preferences_context.len()
            + file_list_context.len()
            + credentials_context.len()
            + email_accounts_context.len()
            + oauth_context.len()
            + app_context_section.len()
            + file_context_section.len()
            + url_context_section.len()
            + loaded_knowhow_block.as_deref().map_or(0, str::len)
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
                // `len - excess` is a raw byte offset that can land mid-UTF-8
                // character (multilingual memory context) — `String::truncate`
                // panics off a char boundary, so snap down to the nearest one.
                let cut = memory_context.floor_char_boundary(memory_context.len() - excess);
                memory_context.truncate(cut);
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
        if !device_preferences_context.is_empty() {
            user_message_parts.push(&device_preferences_context);
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
        // Phase 3: per-thread loaded knowhow gets a dedicated section between
        // workspace inventory (oauth/credentials/etc.) and active context
        // (app/file/url) — the semantic tier between "what exists in this
        // workspace" and "what the user is currently working with". Phase 4
        // stubs the matching resume tool blocks so the body isn't billed
        // twice. The block lives in this turn's user message because the
        // LLM cannot see across turns without it (the resume blocks no
        // longer carry the body). Built earlier so its bytes count against
        // `fixed_size` for the trim-or-not decision.
        if let Some(block) = loaded_knowhow_block.as_deref() {
            user_message_parts.push(block);
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
        // Add stopped MCP servers context so the LLM knows they exist.
        // Initialised to empty `String` (not a conditional `let mcp_stopped_context;`)
        // so the same `&str` reference can be reused by `build_capture_sections`
        // below without violating control-flow init checks. Empty strings are
        // filtered out of both the user-message parts and the capture rows.
        let stopped_summaries = self.mcp_manager.get_stopped_server_summaries().await;
        let mcp_stopped_context = if !stopped_summaries.is_empty() {
            format!(
                "[STOPPED MCP SERVERS]\nThese MCP servers are configured but not running:\n{}\n[END STOPPED MCP SERVERS]",
                stopped_summaries.join("\n")
            )
        } else {
            String::new()
        };
        if !mcp_stopped_context.is_empty() {
            user_message_parts.push(&mcp_stopped_context);
        }
        let setup_reminder = if !is_trigger && !missing_pref_keys.is_empty() {
            let missing_list = missing_pref_keys.join(", ");
            format!("CRITICAL: The following preferences are not set: {}. Do NOT proceed with the user's request. Ask the user to configure these first.", missing_list)
        } else {
            String::new()
        };
        if !setup_reminder.is_empty() {
            user_message_parts.push(&setup_reminder);
        }
        let request_line = format!("Request: {}", user_message);
        user_message_parts.push(&request_line);

        // Attached images ride inline as base64 image blocks on the user message
        // (`build_user_content_with_images` below) and stay visible for the whole
        // turn. No on-disk path annotation: the chat `read_file` tool is rooted at
        // `data/artifacts/`, so a `.lucidos/tmp/images/…` hint only sent the model
        // chasing a path it can't reach ("the bot can't see my attached image").
        let user_message_text = user_message_parts.join("\n\n");

        // Section *shape* (name + char_count) is always built so the
        // modal can render the breakdown; only the body is gated by
        // `capture_context`. Body cap (8 KB) prevents a 100 KB system
        // prompt from bloating every events row. The actual assembly —
        // tagging each section with role + inner-tier group, plus the
        // per-loaded-knowhow and per-resume-tool-pair rows — lives in
        // `build_capture_sections` so it can be unit-tested without
        // standing up a full engine.
        let capture_body = PreferenceStore::capture_context(&self.pool).await?;
        let capture_sections = build_capture_sections(
            &system_prompt,
            &profile_context,
            &device_preferences_context,
            &file_list_context,
            &credentials_context,
            &email_accounts_context,
            &oauth_context,
            &memory_context,
            &history_context,
            &app_context_section,
            &file_context_section,
            &url_context_section,
            &mcp_stopped_context,
            &setup_reminder,
            &thread_depth_context,
            user_message,
            &loaded_knowhow_docs,
            &resume_tool_blocks,
            capture_body,
        );
        let capture_tools: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let capture_model = resolved_model.to_string();

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
        let mut terminator_settled = false;
        // The firing trigger's side-effect grant (ADR 0002, Phase 5); empty for
        // chat. The command guard consults it only on the trigger channel.
        let trigger_side_effect_grant: Vec<crate::engine::command_guard::SideEffectCategory> =
            trigger
                .as_ref()
                .map(|tc| tc.side_effect_grant.clone())
                .unwrap_or_default();
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
                guard.generation(),
                &mut terminator_settled,
                crate::engine::agentic_loop::ContextCaptureSeed {
                    sections: &capture_sections,
                    tools: &capture_tools,
                    tool_defs_chars,
                    model: &capture_model,
                    capture_body,
                },
                &trigger_side_effect_grant,
                max_tool_calls,
            )
            .await;

        // Defensive: if the loop returned without emitting a terminator for
        // this request, emit ResponseAborted so the UI's "running" state
        // clears. Skipped on the success path because the flag tracks every
        // emit site — the SQL existence check has no functional index on
        // `payload->>'request_event_id'` and would walk the whole thread
        // on every chat turn otherwise.
        if !terminator_settled {
            crate::engine::agentic_loop::ensure_terminator_emitted(
                &self.event_bus,
                &self.pool,
                thread_id,
                origin_id,
                response_channel,
            )
            .await;
        }

        let orphans = self
            .drain_turn_orphans(thread_id, guard, &mut injection_rx)
            .await;

        result.map(|mut r| {
            r.orphaned_injections = orphans;
            r
        })
    }

    /// The turn tail EVERY exit shares, normal or cancelled.
    ///
    /// Recovers messages that the fast-path send landed on `injection_tx` after
    /// the loop's last try_recv() but before the turn went idle. Without this
    /// they'd be silently lost when injection_rx is dropped on function return.
    /// `finalize_turn_and_drain_injections` drops the guard FIRST (removing the
    /// handle from active_threads under the same lock the fast-path send takes)
    /// and only THEN drains, so a follow-up racing this teardown is either
    /// buffered-and-recovered here or rejected → caller's slow path. It is never
    /// acknowledged into a channel nobody drains. The caller attaches the result
    /// to its `ProcessResult` so `chat_submit` can re-submit the orphans as
    /// regular follow-ups (= the next turn).
    ///
    /// A cancel during setup goes through here too: a Stop must end the current
    /// turn, never swallow the follow-up the user typed while it was unwinding.
    async fn drain_turn_orphans(
        &self,
        thread_id: Uuid,
        guard: ThreadGuard,
        injection_rx: &mut mpsc::UnboundedReceiver<InjectedPrompt>,
    ) -> Vec<InjectedPrompt> {
        let orphans = Self::finalize_turn_and_drain_injections(guard, injection_rx);
        let orphans =
            crate::engine::filter_removed_queued_prompts(&self.pool, thread_id, orphans).await;
        if !orphans.is_empty() {
            log!(
                "[Chat] {} orphaned injection(s) after turn exit for thread {}, re-submitting as follow-ups",
                orphans.len(),
                thread_id
            );
        }
        orphans
    }

    /// A user Stop observed during the turn's SETUP phase, before the agentic
    /// loop ever ran.
    ///
    /// Emits the same terminator the loop's pre-iteration cancel arm would have
    /// (`ResponseCanceled`, empty body, cause and actor both drained from the
    /// per-thread handle so the timeline records which device clicked Stop, and
    /// so an urgent child follow-up that landed during setup is still labelled
    /// a redirect rather than an abandonment), then runs the shared turn tail.
    /// Setup is where the wait actually is on a
    /// large thread: history load, query classification, memory retrieval and
    /// context assembly can run for tens of seconds, and until 2026-08-04
    /// nothing in there looked at the token, so "Canceling…" hung for the whole
    /// of it and the terminator landed BELOW the follow-up the user had typed
    /// meanwhile. See
    /// `docs/plans/2026-08-04-chat-stop-honored-during-turn-setup.md`.
    async fn cancel_during_setup(
        &self,
        cancel: &SetupCancel,
        guard: ThreadGuard,
        injection_rx: &mut mpsc::UnboundedReceiver<InjectedPrompt>,
    ) -> ProcessResult {
        crate::engine::thread_events::emit_response_canceled(
            &self.event_bus,
            &self.pool,
            cancel.thread_id,
            cancel_cause_for_turn(self, cancel.thread_id),
            String::new(),
            vec![],
            cancel.model.clone(),
            cancel.effort.clone(),
            meta_with_cancel_actor(self, cancel.thread_id, &cancel.meta),
            "[Chat] ResponseCanceled (cancel during turn setup)",
        )
        .await;
        let orphans = self
            .drain_turn_orphans(cancel.thread_id, guard, injection_rx)
            .await;
        let mut result = terminal_result(
            String::new(),
            vec![],
            cancel.request_id,
            cancel.thread_id,
            false,
        );
        result.orphaned_injections = orphans;
        result
    }
}
