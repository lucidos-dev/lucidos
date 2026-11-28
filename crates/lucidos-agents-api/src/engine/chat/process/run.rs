use crate::core::{prefs, PreferenceStore};
use crate::engine::agent_session::spawn_gate::remote_start::{self, spend_is_held};
use crate::engine::agentic_loop::run::AgenticLoopRunExt;
use crate::engine::agentic_loop::{
    cancel_cause_for_turn, meta_with_cancel_actor, terminal_result, until_canceled,
};
use crate::engine::chat::held_messages::HeldMessageDeliveryExt;
use crate::engine::chat::process::context_sections::ContextSectionsExt;
use crate::engine::chat::process::history::HistoryExt;
use crate::engine::chat::process::memory_views::MemoryViewsExt;
use crate::engine::chat::process::system_prompt::SystemPromptExt;
use crate::engine::chat::process::titles::TitlesExt;
use crate::engine::chat::process::turn_clock::TurnClockExt;
use crate::engine::chat::process::turn_tail::TurnTailExt;
use crate::engine::chat::process_cc::ProcessCcExt;
use crate::engine::chat::rung::RungExt;
use crate::engine::context::trim_history_from_oldest;
use crate::engine::event_wait::OwnerTurn;
use crate::engine::thread_events::{ActorMode, EventChannel, MessageOrigin};
use crate::engine::tools::capabilities::CapabilitiesExt;
use crate::engine::types::*;
use crate::engine::{InjectedPrompt, LucidosEngine, ThreadGuard};
use crate::llm::Message;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::super::events::describe_images;
use super::super::images::{build_user_content_with_images, current_image_handles};
use super::super::process_helpers::{
    build_trigger_started_event, classify_or_fallback, pinned_classification, TriggerContext,
    TriggerTurn,
};
use super::context_build::{
    assemble_turn_text, build_capture_sections, build_knowhow_retention_block,
    build_loaded_knowhow_block, TurnSections,
};
use super::PreEmittedOrigin;
use crate::engine::message_events::{emit_routing_failure, make_message_received};

const ADMISSION_ATTEMPTS: u32 = 2;

pub(super) async fn trigger_model_override(
    pool: &sqlx::PgPool,
    registry: &crate::llm::ModelRegistry,
    is_configured: impl Fn(crate::llm::ProviderKind) -> bool,
    pinned: Option<&str>,
) -> Option<String> {
    if let Some(pinned) = pinned {
        return Some(pinned.to_string());
    }
    if let Some(stored) = prefs::TRIGGER_MODEL.stored_or_inherited(pool).await {
        return Some(stored);
    }
    let default = prefs::TRIGGER_MODEL.default_text();
    crate::llm::model_registry::reaches(registry, default, None, is_configured, |_, _| false)
        .then(|| default.to_string())
}

/// INVARIANT: Retain the batch until a claimed or live session owns its queued inputs.
struct CodingAgentFollowups<'a> {
    engine: &'a LucidosEngine,
    thread_id: Option<Uuid>,
    inputs: Vec<AgentUserInput>,
}

impl Drop for CodingAgentFollowups<'_> {
    fn drop(&mut self) {
        if self.inputs.is_empty() {
            return;
        }
        let thread_id = self
            .thread_id
            .expect("an orphan batch has an existing thread");
        self.engine.resubmit_orphans_of_failed_turn(
            thread_id,
            crate::engine::session_follow_ups::lost_followups_to_orphans(std::mem::take(
                &mut self.inputs,
            )),
        );
    }
}

pub(super) async fn prepend_claimed_followups(
    engine: &LucidosEngine,
    claim: &mut crate::engine::session_claim::SpawnClaim,
    followups: &mut Vec<AgentUserInput>,
) {
    if followups.is_empty() {
        return;
    }
    let mut sessions = engine.agent_sessions.lock().await;
    let session = sessions
        .get_mut(&claim.thread_id)
        .expect("the claim holds its entry");
    assert!(session.msg_tx.same_channel(&claim.entry.msg_tx));
    let newer: Vec<_> = std::iter::from_fn(|| claim.entry.msg_rx.try_recv().ok()).collect();
    for input in followups.drain(..).chain(newer) {
        let id = input.origin_event_id;
        claim
            .entry
            .msg_tx
            .send(input)
            .expect("the claim owns its receiver");
        if let Some(id) = id {
            session.unforwarded_inputs.insert(id);
        }
    }
}

pub(super) async fn resolve_coding_agent_lane(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    requested: Option<bool>,
) -> Result<Option<bool>, sqlx::Error> {
    match crate::engine::thread_agent(pool, thread_id).await? {
        Some(crate::engine::ThreadAgent::CodingAgent) => {
            if requested != Some(true) {
                log_legacy!("[Chat] Thread {thread_id} is a coding-agent thread, so its input runs the coding agent (the caller asked for {requested:?})");
            }
            Ok(Some(true))
        }
        Some(crate::engine::ThreadAgent::LucidosAgent) | None => Ok(requested),
    }
}

pub(super) async fn resolve_input_settings(
    origin: Option<&MessageOrigin>,
    use_coding_agent: Option<bool>,
    requested: crate::core::ResolvedModelSelection,
    ordinary: impl std::future::Future<Output = crate::core::ResolvedModelSelection>,
) -> Result<crate::core::ResolvedModelSelection, Box<dyn std::error::Error + Send + Sync>> {
    if matches!(origin, Some(MessageOrigin::SavedDraft { .. })) && use_coding_agent != Some(true) {
        if requested.model.as_deref().is_none_or(str::is_empty)
            || requested.provider.as_deref().is_none_or(str::is_empty)
        {
            return Err("saved_draft_chat_settings_unresolved".into());
        }
        return Ok(requested);
    }
    Ok(ordinary.await)
}

pub(crate) async fn resolve_route_overrides(
    pool: &sqlx::PgPool,
    defaults: crate::core::ModelDefaults<'_>,
    is_configured: impl Fn(crate::llm::ProviderKind) -> bool,
    use_coding_agent: Option<bool>,
    thread_id: Option<Uuid>,
    exclude_event_id: Option<Uuid>,
    overrides: crate::core::ResolvedModelSelection,
) -> crate::core::ResolvedModelSelection {
    if use_coding_agent == Some(true) {
        return crate::core::ResolvedModelSelection {
            reasoning_effort: overrides.reasoning_effort,
            ..crate::core::ResolvedModelSelection::default()
        };
    }

    let registry = defaults.registry;
    let fallback_model = defaults.fallback_model;
    let resolved = PreferenceStore::resolve_chat_overrides_for_thread(
        pool,
        defaults,
        thread_id,
        exclude_event_id,
        overrides,
    )
    .await;
    let crate::core::ResolvedModelSelection {
        model,
        reasoning_effort: effort,
        provider,
    } = resolved;
    let provider = provider.filter(|p| {
        crate::llm::ProviderKind::from_name(p).is_some_and(|kind| {
            crate::llm::model_registry::route_on(
                registry,
                model.as_deref().unwrap_or(fallback_model),
                kind,
            )
            .is_some()
        })
    });

    let effort = match (&model, effort) {
        (Some(m), Some(e)) => {
            let chosen = provider
                .as_deref()
                .and_then(crate::llm::ProviderKind::from_name);
            match crate::llm::model_registry::resolve_route(registry, m, chosen, &is_configured) {
                Ok(route) => {
                    crate::llm::reasoning::clamp_effort(&e, route.provider, &route.wire_id)
                        .map(str::to_string)
                }
                Err(_) => Some(e),
            }
        }
        (_, e) => e,
    };
    crate::core::ResolvedModelSelection {
        model,
        reasoning_effort: effort,
        provider,
    }
}

pub(crate) async fn resolve_available_route_overrides(
    pool: &sqlx::PgPool,
    routes: &crate::runtime::intelligence_ladder::ModelRoutes,
    use_coding_agent: Option<bool>,
    thread_id: Option<Uuid>,
    exclude_event_id: Option<Uuid>,
    overrides: crate::core::ResolvedModelSelection,
) -> crate::core::ResolvedModelSelection {
    use crate::runtime::intelligence_ladder::{default_chat_selection, ChatOffer};
    let offer = ChatOffer::load(pool, routes.clone()).await;
    let default = default_chat_selection(pool, &offer).await;
    let mut resolved = resolve_route_overrides(
        pool,
        crate::core::ModelDefaults {
            registry: &routes.registry,
            fallback_model: &default.model,
        },
        |kind| routes.configured.contains(&kind),
        use_coding_agent,
        thread_id,
        exclude_event_id,
        overrides,
    )
    .await;
    if use_coding_agent != Some(true) && resolved.model.is_none() {
        resolved.model = Some(default.model.clone());
        let effort = resolved.reasoning_effort.or(default.effort);
        resolved.reasoning_effort = effort.and_then(|effort| {
            crate::llm::model_registry::resolve_route(
                &routes.registry,
                &default.model,
                resolved
                    .provider
                    .as_deref()
                    .and_then(crate::llm::ProviderKind::from_name),
                |kind| routes.configured.contains(&kind),
            )
            .map_or_else(
                |_| Some(effort.clone()),
                |route| {
                    crate::llm::reasoning::clamp_effort(&effort, route.provider, &route.wire_id)
                        .map(str::to_string)
                },
            )
        });
    }
    resolved
}

#[easy_ext::ext(ProcessRunExt)]
pub(crate) impl LucidosEngine {
    async fn add_hook_argument_names(&self, tools: &mut [crate::llm::provider::ToolDefinition]) {
        use crate::core::event_hooks::{ArmState, HookListing, FIRST_MESSAGE_EVENT_TYPE};
        if !matches!(
            self.event_bus
                .hooks_armed_on(FIRST_MESSAGE_EVENT_TYPE)
                .await,
            Ok(true)
        ) {
            return;
        }
        let Some(catalog) = self.event_hook_catalog.get() else {
            return;
        };
        let Ok(armed) = crate::core::event_hooks::armed_hooks(&self.pool).await else {
            return;
        };
        let Ok(listings) = catalog.list_definitions(self.workspace_path()) else {
            return;
        };
        let mut names = Vec::new();
        for listing in &listings {
            let HookListing::Valid {
                slug, definition, ..
            } = listing
            else {
                continue;
            };
            let hook = armed
                .iter()
                .find(|hook| hook.slug == *slug && hook.event_type == FIRST_MESSAGE_EVENT_TYPE);
            if ArmState::of(hook, Some(listing)) == ArmState::Armed {
                names.extend(
                    definition
                        .arguments
                        .keys()
                        .map(|name| format!("{name} ({slug})")),
                );
            }
        }
        if names.is_empty() {
            return;
        }
        let names = names.join(", ");
        for tool in tools {
            if let Some(description) = tool
                .parameters
                .pointer_mut("/properties/hook_args/description")
            {
                *description = serde_json::json!(format!(
                    "Declared arguments in this workspace: {names}. Use lucidos hooks choices to find values. For a different workspace, use that target's declared arguments."
                ));
            }
        }
    }

    async fn chat_turn_tools(
        &self,
        thread_id: Uuid,
        gates: &crate::llm::ToolCapabilities,
        context_window: usize,
        model: &str,
    ) -> crate::engine::agentic_loop::TurnTools {
        let mut tools = super::system_prompt::chat_tools(gates);
        self.add_hook_argument_names(&mut tools).await;
        if crate::engine::turn_end::wake_decision::WakeDuty::owes_a_decision(&self.pool, thread_id)
            .await
        {
            tools.push(crate::llm::tools::wake_parent_tool());
        }
        let mcp_char_ceiling = crate::mcp::mcp_tool_char_ceiling_for(model, context_window);
        let mcp_surface = self
            .mcp_manager
            .tool_surface(thread_id, mcp_char_ceiling)
            .await;
        crate::engine::agentic_loop::TurnTools::new(tools, mcp_surface, mcp_char_ceiling)
    }
}

pub(super) fn message_can_answer_pending_question(
    is_new_thread: bool,
    user_message: &str,
    has_images: bool,
    mode: ActorMode,
    unattested: bool,
    pre_emitted_origin: Option<PreEmittedOrigin>,
) -> bool {
    !is_new_thread
        && (!user_message.is_empty() || has_images)
        && mode == ActorMode::Human
        && !unattested
        && pre_emitted_origin.is_none()
}

pub(super) fn cut_memory_tail(memory: &mut String, excess: usize) {
    let cut = memory.floor_char_boundary(memory.len() - excess);
    memory.truncate(cut);
    if let Some(pos) = memory.rfind('\n') {
        memory.truncate(pos);
    }
}

fn withdrawn_or(
    error: Box<dyn std::error::Error + Send + Sync>,
    request_id: Uuid,
    thread_id: Uuid,
) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
    if !crate::engine::consumption_claim::is_removed_refusal(&*error) {
        return Err(error);
    }
    log_legacy!("[Chat] Thread {thread_id}: the user took this message back, so it does not run");
    Ok(terminal_result(
        String::new(),
        vec![],
        request_id,
        thread_id,
        false,
    ))
}

async fn refused_start(
    bus: &crate::engine::event_bus::EventBus,
    refusal: crate::engine::event_bus::HookRefusal,
    is_trigger: bool,
    mode: ActorMode,
    request_id: Uuid,
    thread_id: Uuid,
    message_id: Option<&str>,
) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
    log_legacy!("[Chat] Thread {thread_id} does not start: {refusal}");
    if is_trigger {
        return Err(Box::new(refusal));
    }
    if mode == ActorMode::Human {
        bus.emit_or_log(
            crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::Toast {
                    message: refusal.to_string(),
                    level: "error".to_string(),
                },
            ),
            "[Chat] refused start Toast",
        )
        .await;
    }
    if let Some(id) = message_id {
        if crate::engine::consumption_claim::has_accepted_input(bus.pool(), thread_id, id).await? {
            return Err(Box::new(refusal));
        }
    }
    Ok(terminal_result(
        String::new(),
        vec![],
        request_id,
        thread_id,
        false,
    ))
}

pub(super) struct SetupCancel {
    thread_id: Uuid,
    request_id: Uuid,
    meta: crate::engine::thread_events::EventMeta,
    model: Option<String>,
    effort: Option<String>,
}

#[easy_ext::ext(ChatTurnRunExt)]
pub(super) impl LucidosEngine {
    async fn fail_turn(
        &self,
        thread_id: Uuid,
        anchor: Uuid,
        channel: Option<EventChannel>,
        error: Box<dyn std::error::Error + Send + Sync>,
        guard: ThreadGuard,
        injection_rx: &mut mpsc::UnboundedReceiver<InjectedPrompt>,
    ) -> Box<dyn std::error::Error + Send + Sync> {
        crate::engine::failure_terminator::ensure_run_failure_terminator_emitted(
            &self.event_bus,
            &self.pool,
            thread_id,
            anchor,
            channel,
            error.as_ref(),
        )
        .await;
        let orphans = self
            .drain_turn_orphans(thread_id, guard, injection_rx)
            .await;
        self.resubmit_orphans_of_failed_turn(thread_id, orphans);
        error
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_message_with_steps_internal(
        &self,
        user_message: &str,
        model_override: Option<&str>,
        mut trigger: Option<TriggerContext>,
        app_context: Option<AppContext>,
        file_context: Option<String>,
        reasoning_effort: Option<&str>,
        provider_override: Option<&str>,
        user_images: Option<&[crate::engine::types::ChatImage]>,
        device_id: Option<&str>,
        use_coding_agent: Option<bool>,
        event_id: Option<&str>,
        thread_id: Option<Uuid>,
        conflict_change_id: Option<Uuid>,
        repo_id: Option<&str>,
        url_context: Option<crate::engine::types::UrlContext>,
        parent_thread_id: Option<Uuid>,
        spawning_event_id: Option<Uuid>,
        wake_mode: Option<crate::engine::thread_events::WakeMode>,
        mode: ActorMode,
        cc_model: Option<&str>,
        coding_agent: Option<crate::runtime::CodingAgent>,
        pre_emitted_origin: Option<PreEmittedOrigin>,
        title: Option<&str>,
        origin: Option<MessageOrigin>,
        voice_session_id: Option<Uuid>,
        urgency: crate::engine::FollowUpUrgency,
        input_turn: Option<crate::engine::InputTurn>,
        pre_admission: Option<crate::engine::PreAdmission>,
        coding_agent_followups: Vec<AgentUserInput>,
        hook_args: Option<&crate::core::event_hooks::HookArgs>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        let mut coding_agent_followups = CodingAgentFollowups {
            engine: self,
            thread_id,
            inputs: coding_agent_followups,
        };
        let empty_args = crate::core::event_hooks::HookArgs::new();
        let hook_args = hook_args.unwrap_or(&empty_args);
        let mut input_turn = input_turn;
        if let Some(turn) = input_turn.as_mut() {
            turn.wait_for_predecessor().await;
        }
        if let Some(tid) = thread_id {
            if (mode == ActorMode::Engine
                || pre_emitted_origin.is_some_and(PreEmittedOrigin::is_engine_reentry))
                && crate::engine::thread_stop::thread_is_stopped(self.pool(), tid).await
            {
                return Ok(terminal_result(
                    String::new(),
                    vec![],
                    Uuid::new_v4(),
                    tid,
                    false,
                ));
            }
        }
        let use_coding_agent = match thread_id {
            None => use_coding_agent,
            Some(tid) => match resolve_coding_agent_lane(&self.pool, tid, use_coding_agent).await {
                Ok(lane) => lane,
                Err(e) => {
                    log_legacy!("[Chat] Could not read which agent runs thread {tid}: {e}");
                    emit_routing_failure(
                        &self.event_bus,
                        tid,
                        "Lucidos could not tell which agent runs this thread, so the message did not run. Send it again.",
                    )
                    .await?;
                    return Ok(terminal_result(
                        String::new(),
                        vec![],
                        Uuid::new_v4(),
                        tid,
                        false,
                    ));
                }
            },
        };
        let urgent = urgency.is_urgent();
        let chat_start = std::time::Instant::now();
        let mut proposed_change = false;

        // INVARIANT: Mutable so the non-CC fast-path can mark the event as already on
        // the wire when injection loses the race with a thread that completed
        // between the has_active check and the send — the slow path then
        // skips its own MessageReceived emit to avoid a duplicate.
        let mut pre_emitted_origin = pre_emitted_origin;

        let is_trigger = trigger.is_some();

        // INVARIANT: For chat turns, `None` means "use the user's Lucidos chat defaults",
        // not `LlmProvider::default_model()` (which drops `[1m]` and carries no
        // effort). For coding-agent turns, `None` must stay unset so the agent
        // path falls through to live/thread/backend settings instead of
        // inheriting the unrelated Lucidos chat reasoning preference.
        // `pre_emitted_origin` here is the upstream value (the fast-path
        // mutation below happens after this). When set, it is the current
        // turn's already-persisted MessageReceived (`events.id`); exclude it so
        // the per-thread lookup reads the PRIOR message, not this one. In the
        // normal path it's `None` and the current turn isn't emitted until later.
        // A Lucidos Agent turn holds its thread's gate from here until its
        // starter is recorded. A rung pick then lands before the pair resolves,
        // or lands afterward for the next turn (`chat::rung`).
        let keep_alive = pre_emitted_origin.is_some_and(PreEmittedOrigin::is_keep_alive);
        let mut turn_start = match thread_id {
            Some(tid) if use_coding_agent != Some(true) && !keep_alive => {
                Some(self.turn_start_gate.hold(tid).await)
            }
            _ => None,
        };
        let routes = self.model_routes();
        let requested = crate::core::ResolvedModelSelection {
            model: model_override.map(str::to_string),
            reasoning_effort: reasoning_effort.map(str::to_string),
            provider: provider_override.map(str::to_string),
        };
        let resolve_pair =
            |starter: Option<PreEmittedOrigin>, overrides: crate::core::ResolvedModelSelection| {
                resolve_available_route_overrides(
                    &self.pool,
                    &routes,
                    use_coding_agent,
                    thread_id,
                    starter.map(PreEmittedOrigin::event_id),
                    overrides,
                )
            };
        let mut resolved = resolve_input_settings(
            origin.as_ref(),
            use_coding_agent,
            requested.clone(),
            resolve_pair(pre_emitted_origin, requested.clone()),
        )
        .await?;

        let request_id = Uuid::new_v4();
        let is_new_thread = thread_id.is_none();
        let thread_id = thread_id.unwrap_or(request_id);
        let thread_id_str = thread_id.to_string();

        if let Some(TriggerTurn::Fire(fire)) = trigger.as_ref().map(|tc| &tc.turn) {
            self.thread_queue
                .record_entry_thread(fire.queue_entry_id, thread_id)
                .await;
        }

        // INVARIANT: AskUserQuestion free-form path: if the thread is currently waiting
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
            user_images.is_some_and(|images| !images.is_empty()),
            mode,
            matches!(origin, Some(MessageOrigin::UnattestedCaller { .. })),
            pre_emitted_origin,
        ) {
            if let Some(pending_tool_use_id) =
                crate::engine::agent_question::lookup_active_question_tool_use_id(
                    self.pool(),
                    thread_id,
                )
                .await
            {
                use crate::engine::agent_question::{
                    answer_pending_question_with_event_id, AnswerResult,
                };
                use crate::engine::thread_events::AnswerKind;
                let engine_arc: std::sync::Arc<Self> = self.clone_arc();
                let image_hashes = crate::engine::message_events::images_to_hashes(
                    self.workspace_path(),
                    user_images,
                );
                if image_hashes.len() != user_images.map_or(0, <[_]>::len) {
                    emit_routing_failure(
                        &self.event_bus,
                        thread_id,
                        "An image attached to this answer could not be stored, so the answer was not sent. Try attaching it again.",
                    )
                    .await?;
                    return Ok(terminal_result(
                        String::new(),
                        vec![],
                        request_id,
                        thread_id,
                        false,
                    ));
                }
                let answer = AnswerKind::FreeText {
                    text: user_message.to_string(),
                    image_hashes,
                };
                match answer_pending_question_with_event_id(
                    &engine_arc,
                    thread_id,
                    pending_tool_use_id,
                    answer,
                    origin.clone(),
                    event_id.and_then(|raw| Uuid::parse_str(raw).ok()),
                )
                .await
                {
                    AnswerResult::Resumed => {
                        log_legacy!(
                            "[Chat] Free-form answer routed to pending question for thread {}",
                            thread_id
                        );
                    }
                    AnswerResult::Conflict(msg) => {
                        log_legacy!(
                            "[Chat] Free-form answer conflict for thread {}: {}",
                            thread_id,
                            msg
                        );
                        let withdrawn = match event_id.and_then(|raw| Uuid::parse_str(raw).ok()) {
                            Some(id) => {
                                crate::engine::consumption_claim::was_removed(
                                    self.pool(),
                                    thread_id,
                                    id,
                                )
                                .await?
                            }
                            None => false,
                        };
                        if !withdrawn {
                            emit_routing_failure(&self.event_bus, thread_id, &msg).await?;
                        }
                    }
                }
                return Ok(terminal_result(
                    String::new(),
                    vec![],
                    request_id,
                    thread_id,
                    false,
                ));
            }
        }

        let mut description_handle = match user_images {
            Some(imgs) if !imgs.is_empty() && use_coding_agent != Some(true) => 'describe: {
                if spend_is_held(self.pool(), thread_id, origin.as_ref()).await {
                    break 'describe None;
                }
                let purpose = crate::engine::ContextPurpose::ImageDescribe;
                let call = self.aux_call(purpose).await;
                if let Some(refusal) = call.selection().refusal() {
                    log_legacy!("[Chat] Image description refused: {}", refusal);
                    break 'describe None;
                }
                let provider = call.provider();
                let effort = call.reasoning().map(str::to_string);
                let deadline = call.deadline();
                let imgs: Vec<crate::engine::types::ChatImage> = imgs.to_vec();
                let capture = crate::engine::AuxCapture::new(&self.event_bus, thread_id, purpose);
                Some(tokio::spawn(async move {
                    let described = tokio::time::timeout(
                        deadline,
                        describe_images(provider.as_ref(), &imgs, effort.as_deref(), &capture),
                    )
                    .await;
                    match described {
                        Ok(Ok(desc)) => Some((desc, provider.default_model().to_string())),
                        Ok(Err(e)) => {
                            log_legacy!("[Chat] Image description failed: {}", e);
                            None
                        }
                        Err(_) => {
                            log_legacy!("[Chat] Image description timed out ({:?})", deadline);
                            None
                        }
                    }
                }))
            }
            _ => None,
        };

        if use_coding_agent == Some(true)
            && !is_new_thread
            && !user_message.is_empty()
            && mode != ActorMode::Human
            && super::super::held_messages::message_is_held(
                self.pool(),
                thread_id,
                mode,
                pre_emitted_origin,
            )
            .await
        {
            self.event_bus
                .emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: super::super::held_messages::held_message_event(
                        &self.workspace_path,
                        user_message,
                        user_images,
                        mode,
                        origin.clone(),
                    ),
                    meta: crate::engine::thread_events::EventMeta {
                        channel: Some(EventChannel::ClaudeCode),
                        ..crate::engine::thread_events::EventMeta::NONE
                    },
                })
                .await?;
            log_legacy!("[Chat] Held an agent message on thread {}", thread_id);
            return Ok(terminal_result(
                String::new(),
                vec![],
                request_id,
                thread_id,
                false,
            ));
        }

        if use_coding_agent == Some(true)
            && !is_new_thread
            && !user_message.is_empty()
            && !keep_alive
        {
            if !super::super::process_helpers::reentry_reaches_live_session(
                &self.agent_sessions,
                thread_id,
                pre_emitted_origin,
            )
            .await
            {
                crate::engine::cc_permission::resolve_pending_permissions_as_superseded(
                    self.pool(),
                    &self.event_bus,
                    &self.pending_cc_permission,
                    thread_id,
                    origin.clone(),
                )
                .await;
            }

            // INVARIANT: Question supersede, the same move for the question lane. Getting
            // here means the fast-path above did NOT consume this message as
            // the answer, so any question still open is one this follow-up
            // cannot answer: an agent-driven message, or a question the agent
            // already overtook. Leaving it open deadlocks the thread, because
            // the agent is parked inside the call that asked and only an answer
            // releases it. See
            // `crate::engine::agent_question::resolve_pending_question_as_superseded`.
            //
            // The one exception is an engine re-entry on a question the user
            // can still answer, such as a child's completion: it cannot kill
            // the card, so superseding would only throw the question away.
            //
            // Coding-agent lane only. A chat thread queues a non-answering
            // follow-up as an injection and keeps the question live and
            // answerable, which is the behaviour we want there.
            if !super::super::process_helpers::follow_up_keeps_open_question(
                self.pool(),
                &self.agent_sessions,
                thread_id,
                pre_emitted_origin,
            )
            .await
            {
                crate::engine::agent_question::resolve_pending_question_as_superseded(
                    &self.clone_arc(),
                    thread_id,
                    origin.clone(),
                )
                .await;
            }

            if mode == ActorMode::Human {
                self.clone_arc().deliver_held_messages(thread_id).await;
            }
        }

        if use_coding_agent != Some(true) && !is_new_thread {
            let has_live_turn = crate::engine::has_other_live_turn(
                &self.active_threads,
                thread_id,
                pre_admission.as_ref(),
            );
            if crate::engine::held_deliveries::delivery_is_held(
                self.pool(),
                thread_id,
                pre_emitted_origin,
                has_live_turn,
            )
            .await
            {
                log_legacy!(
                    "[Chat] Held a delivery behind the open question on thread {}",
                    thread_id
                );
                return Ok(terminal_result(
                    String::new(),
                    vec![],
                    request_id,
                    thread_id,
                    false,
                ));
            }
        }

        let chat_follow_up =
            use_coding_agent != Some(true) && !is_new_thread && !user_message.is_empty();
        let has_live_turn = crate::engine::has_other_live_turn(
            &self.active_threads,
            thread_id,
            pre_admission.as_ref(),
        );
        if chat_follow_up
            && super::super::process_helpers::chat_follow_up_supersedes_cards(mode, has_live_turn)
        {
            crate::engine::command_permission::resolve_pending_command_permissions_as_superseded(
                self.pool(),
                &self.event_bus,
                &self.pending_command_permission,
                thread_id,
                origin.clone(),
            )
            .await;
            crate::engine::mcp_permission::resolve_pending_mcp_permissions_as_superseded(
                self.pool(),
                &self.event_bus,
                &self.pending_mcp_permission,
                thread_id,
                origin.clone(),
            )
            .await;
        }
        if chat_follow_up && mode == ActorMode::Human {
            crate::engine::form_requests::supersede_on_user_message(
                self.pool(),
                &self.event_bus,
                thread_id,
                origin.clone(),
            )
            .await;
        }

        // INVARIANT: Fast-path for CC follow-ups: route via msg_tx BEFORE register_thread_queued
        // so a follow-up arriving during an active Claude Code session is picked up by the
        // running event loop instead of spawning a new Claude Code session (or blocking on
        // the prior turn's still-held ThreadGuard).
        //
        // This handles both idle sessions (msg picked up immediately) and busy sessions
        // (msg queued in the channel, picked up after current work finishes).
        let mut spawn_claim = None;
        if use_coding_agent == Some(true) {
            loop {
                // INVARIANT: One `agent_sessions` lock hold either claims the thread for
                // this message's spawn or finds the run that holds it. A
                // follow-up during startup finds the starting run and queues.
                let claim = if keep_alive {
                    self.claim_thread_for_keep_alive(thread_id).await?
                } else {
                    self.claim_thread_for_spawn(thread_id, coding_agent).await?
                };
                if let Some(mut claim) = claim {
                    prepend_claimed_followups(self, &mut claim, &mut coding_agent_followups.inputs)
                        .await;
                    self.name_requested_settings(
                        &claim,
                        cc_model,
                        resolved.reasoning_effort.as_deref(),
                    )
                    .await;
                    spawn_claim = Some(claim);
                    break;
                }
                if keep_alive {
                    log_legacy!("[KeepAlive] Thread {thread_id} is busy, so this beat is skipped");
                    return Ok(terminal_result(
                        String::new(),
                        vec![],
                        request_id,
                        thread_id,
                        false,
                    ));
                }
                let redirect_idle_notify = {
                    let mut sessions = self.agent_sessions.lock().await;
                    super::super::process_helpers::arm_followup_redirect(
                        &mut sessions,
                        thread_id,
                        // INVARIANT: Genuine user follow-up, not an engine-internal
                        // child-wake. A pre-emitted USER message is still a
                        // user follow-up and must keep arming the redirect.
                        !pre_emitted_origin.is_some_and(PreEmittedOrigin::is_engine_reentry),
                        urgent,
                        &origin,
                    )
                };
                if let Some(idle_notify) = redirect_idle_notify {
                    // INVARIANT: Block until the interrupted turn reaches a boundary (idle
                    // or process exit) BEFORE emitting the follow-up's
                    // MessageReceived, so the turn's Canceled terminal is
                    // sequenced first (coding-agent exchanges group by sequence
                    // order — see the MessageReceived-first note below). The
                    // graceful interrupt can idle the turn before we register
                    // a waiter, so the boundary may already be behind us.
                    // Re-check is_waiting each iteration as well. The engine's 8s interrupt
                    // escalation hard-kills a non-idling turn (→ process_exited),
                    // and REDIRECT_INTERRUPT_MAX_WAIT is the backstop above that.
                    // If the session died, the msg_tx send below fails and the
                    // next pass of the claim loop spawns a `--resume` for the
                    // follow-up instead of dropping it.
                    let redirect_deadline = std::time::Instant::now()
                        + super::super::process_helpers::REDIRECT_INTERRUPT_MAX_WAIT;
                    loop {
                        // INVARIANT: Construct the notify future before the state check.
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
                            log_legacy!(
                                "[Chat] Codex redirect: interrupted turn reached a boundary for thread {} — follow-up runs as the next turn",
                                thread_id
                            );
                            break;
                        }
                        if std::time::Instant::now() >= redirect_deadline {
                            log_legacy!(
                                "[Chat] Codex redirect: timed out waiting for the interrupt on thread {} — routing the follow-up anyway",
                                thread_id
                            );
                            break;
                        }
                        let _ =
                            tokio::time::timeout(std::time::Duration::from_secs(2), notified).await;
                    }
                }

                // INVARIANT: Emit MessageReceived FIRST — guarantees it gets a lower sequence
                // number and earlier timestamp than CodingAgentPromptSent (emitted by
                // the CC event loop when it picks up the message). Without this
                // ordering, CodingAgentPromptSent can race ahead and end up in the
                // wrong exchange, causing a brief "interrupted" flash and incorrect
                // thread section placement.
                //
                // Skip the emit whenever the caller already persisted the
                // event. Only a CHILD WAKE also changes how the input routes
                // (see `AgentInputKind::ReentryFromEngine` docs); a pre-emitted
                // user message is an ordinary follow-up.
                let (origin_event_id, input_kind) = if let Some(pre) = pre_emitted_origin {
                    let kind = if pre.is_engine_reentry() {
                        AgentInputKind::ReentryFromEngine
                    } else {
                        AgentInputKind::User
                    };
                    (Some(pre.event_id()), kind)
                } else {
                    use crate::engine::thread_events::EventMeta;
                    let coding_agent = self.thread_coding_agent(thread_id).await;
                    let (rung_model, rung_effort) = self
                        .coding_agent_rung(thread_id, coding_agent, None, None)
                        .await;
                    let message = make_message_received(
                        &self.workspace_path,
                        user_message,
                        user_images,
                        device_id,
                        parent_thread_id,
                        spawning_event_id,
                        wake_mode,
                        mode,
                        rung_model.as_deref(),
                        rung_effort.as_deref(),
                        None,
                        origin.clone(),
                        voice_session_id,
                    );
                    let meta = EventMeta {
                        event_id: event_id.and_then(|s| uuid::Uuid::parse_str(s).ok()),
                        channel: Some(EventChannel::ClaudeCode),
                        ..EventMeta::NONE
                    };
                    let asked = match self
                        .event_bus
                        .ask_hooks_with_args(thread_id, &message, &meta, hook_args)
                        .await
                    {
                        Ok(asked) => asked,
                        Err(refusal) => {
                            return refused_start(
                                &self.event_bus,
                                refusal,
                                is_trigger,
                                mode,
                                request_id,
                                thread_id,
                                event_id,
                            )
                            .await;
                        }
                    };
                    let emit_result = self
                        .event_bus
                        .emit_asked(asked, thread_id, message, meta)
                        .await?;
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
                    let mut sessions = self.agent_sessions.lock().await;
                    if let Some(session) = sessions.get_mut(&thread_id) {
                        if session.is_live() {
                            // INVARIANT: The message is visible to the session's idle decision the
                            // moment it is in the channel (that decision reads
                            // `msg_rx` under this same lock), and the run loop records
                            // it as owed when it forwards it to the driver. Only a
                            // withdraw needs telling that it is still on its way.
                            let sent = session
                                .msg_tx
                                .send(AgentUserInput {
                                    text: user_message.to_string(),
                                    images,
                                    origin_event_id,
                                    kind: input_kind,
                                    completion: None,
                                })
                                .is_ok();
                            if let (true, Some(id)) = (sent, origin_event_id) {
                                session.unforwarded_inputs.insert(id);
                            }
                            if sent {
                                let mut followups =
                                    std::mem::take(&mut coding_agent_followups.inputs).into_iter();
                                while let Some(followup) = followups.next() {
                                    let id = followup.origin_event_id;
                                    if let Err(unsent) = session.msg_tx.send(followup) {
                                        coding_agent_followups.inputs.push(unsent.0);
                                        coding_agent_followups.inputs.extend(followups);
                                        break;
                                    }
                                    if let Some(id) = id {
                                        session.unforwarded_inputs.insert(id);
                                    }
                                }
                            }
                            sent
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };

                if send_ok {
                    log_legacy!("[Chat] CC follow-up routed via msg_tx for thread {} (bypassed register_thread_queued)", thread_id);
                    return Ok(terminal_result(
                        String::new(),
                        vec![],
                        request_id,
                        thread_id,
                        false,
                    ));
                }

                // INVARIANT: The live session went away or is terminating between the
                // claim's look and the send, most commonly the idle exit. Don't
                // emit a routing failure and don't drop the message. The next
                // pass waits for that run to let go, then claims the thread. The
                // message then spawns a fresh `--resume` session as the next turn.
                // MessageReceived was already emitted above (`pre_emitted_origin`
                // is set), so the slow path won't re-emit it.
                log_legacy!(
                    "[Chat] CC follow-up lost the live-session race for thread {} — claiming the thread for a resume",
                    thread_id
                );
            }
        }

        // INVARIANT: The Lucidos Agent's half of interrupt-and-redirect. An injected
        // prompt is only READ between loop iterations. A turn inside a long
        // tool call therefore sits on it for as long as that call runs. One
        // tool yields early (`bash_output(wait_secs=…)` parks on
        // `injection_notify`), and nothing else does, so an urgent follow-up
        // cannot rely on the soft path.
        //
        // Cancel WITHOUT injecting, then fall through to the queue.
        // `register_thread_queued` waits for this turn to release the handle,
        // so the follow-up runs as the next turn. The interrupted turn's
        // Canceled terminal is sequenced first, matching the coding-agent
        // lane. Injecting first would race the loop's own drain. A prompt
        // consumed by the dying turn's final iteration is neither answered nor
        // left for the orphan chain.
        if urgent && use_coding_agent != Some(true) && !is_new_thread {
            let preempted = self.cancel_thread_for_followup(thread_id, origin.clone());
            log_legacy!(
                "[Chat] Urgent follow-up preempting the live turn on thread {} (preempted={})",
                thread_id,
                preempted
            );
        }

        // INVARIANT: Single-flight admission for the Lucidos Agent lane.
        //
        // `try_register_thread` asks whether a turn is already live here, and
        // installs this turn's handle, under one lock. Two wakes for one
        // logical event therefore cannot both come back owning the thread. A
        // refusal means a turn IS live, so the message is injected into it
        // instead of queued behind it: one paid model call for one event.
        //
        // An event-wait delivery coalesces like any other follow-up, and that
        // is load-bearing rather than incidental. It used to be barred. An
        // attached wake's prompt was the empty string, its payload living in a
        // `ToolResult` the running turn never rebuilt. So it went into the
        // queue, where the stuck-turn backstop evicted the running turn to let
        // it in. A wake now carries real prose, so it injects.
        let mut admitted = pre_admission.map(crate::engine::PreAdmission::take);
        if admitted.is_none() && use_coding_agent != Some(true) && !is_new_thread && !urgent {
            for attempt in 1..=ADMISSION_ATTEMPTS {
                if let Some(registration) = self.try_register_thread(thread_id) {
                    admitted = Some(registration);
                    break;
                }
                if requested != crate::core::ResolvedModelSelection::default()
                    && self.live_chat_pair(thread_id).await? != resolved
                {
                    log_legacy!("[Chat] Thread {thread_id}: the live turn runs another pair, so this message queues");
                    break;
                }
                // INVARIANT: Emit MessageReceived FIRST — same ordering guarantee as the CC
                // fast-path. The agentic loop emits PromptInjected when it
                // picks up the injection; without this ordering, PromptInjected
                // can race ahead and create a duplicate exchange boundary.
                //
                // Skip the emit whenever the caller already persisted the
                // event, this loop's own earlier pass included. Only an ENGINE
                // WAKE also changes how the input is projected (see
                // `PreEmittedOrigin::inject_kind` and the `InjectedPromptKind`
                // docs). A pre-emitted user message still injects as
                // `UserText`, so the loop acknowledges it with a
                // `PromptInjected`.
                use crate::engine::thread_events::EventMeta;
                let inject_kind = if let Some(pre) = pre_emitted_origin {
                    pre.inject_kind()
                } else {
                    let live = self.live_chat_pair(thread_id).await?;
                    let live_anchor = self
                        .active_threads
                        .lock()
                        .unwrap()
                        .get(&thread_id)
                        .and_then(|h| *h.request_event_id.lock().unwrap());
                    let emit_result = self
                        .event_bus
                        .emit(crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: make_message_received(
                                &self.workspace_path,
                                user_message,
                                user_images,
                                device_id,
                                parent_thread_id,
                                spawning_event_id,
                                wake_mode,
                                mode,
                                live.model.as_deref(),
                                live.reasoning_effort.as_deref(),
                                live.provider.as_deref(),
                                origin.clone(),
                                voice_session_id,
                            ),
                            meta: EventMeta {
                                event_id: event_id.and_then(|s| uuid::Uuid::parse_str(s).ok()),
                                channel: Some(EventChannel::Chat),
                                request_event_id: live_anchor,
                                ..EventMeta::NONE
                            },
                        })
                        .await;
                    let emit_result = match emit_result {
                        Ok(result) => result,
                        Err(e) => return withdrawn_or(e, request_id, thread_id),
                    };
                    if let Some(result) = emit_result {
                        pre_emitted_origin = Some(PreEmittedOrigin::Message(result.event_id));
                    }
                    crate::engine::InjectedPromptKind::UserText
                };

                let origin_event_id = pre_emitted_origin
                    .map(PreEmittedOrigin::event_id)
                    .or_else(|| event_id.and_then(|s| Uuid::parse_str(s).ok()));
                let images = user_images.map(|imgs| imgs.to_vec());
                let resumed_text = crate::engine::thread_stop::resume_text(
                    self.pool(),
                    thread_id,
                    user_message,
                    origin_event_id,
                )
                .await?;
                let injected = self.inject_into_live_turn(
                    thread_id,
                    crate::engine::InjectedPrompt {
                        text: resumed_text,
                        event_id: origin_event_id,
                        mode,
                        spawning_event_id,
                        images,
                        origin: origin.clone(),
                        kind: inject_kind,
                    },
                );

                if injected {
                    log_legacy!(
                        "[Chat] Follow-up coalesced into the live turn on thread {}",
                        thread_id
                    );
                    // INVARIANT: This path returns to the HTTP caller right here, so the
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
                    return Ok(terminal_result(
                        String::new(),
                        vec![],
                        request_id,
                        thread_id,
                        false,
                    ));
                }

                log_legacy!(
                    "[Chat] Thread {} neither admitted nor injectable (attempt {} of {})",
                    thread_id,
                    attempt,
                    ADMISSION_ATTEMPTS
                );
            }
        }

        let (cancel_token, mut injection_rx, guard) = match admitted {
            Some(registration) => registration,
            None if keep_alive => match crate::engine::try_register_thread(
                &self.active_threads,
                &self.thread_completion,
                thread_id,
            ) {
                crate::engine::Admission::Admitted(registration) => registration,
                crate::engine::Admission::Busy { .. } => {
                    log_legacy!("[KeepAlive] Thread {thread_id} has a turn running, so this beat is skipped");
                    return Ok(terminal_result(
                        String::new(),
                        vec![],
                        request_id,
                        thread_id,
                        false,
                    ));
                }
            },
            None => match turn_start.take() {
                Some(gate) => {
                    drop(gate);
                    let registration = self.register_thread_queued(thread_id).await;
                    turn_start = Some(self.turn_start_gate.hold(thread_id).await);
                    resolved = resolve_input_settings(
                        origin.as_ref(),
                        use_coding_agent,
                        requested.clone(),
                        resolve_pair(pre_emitted_origin, requested.clone()),
                    )
                    .await?;
                    registration
                }
                None => self.register_thread_queued(thread_id).await,
            },
        };
        let model_override = resolved.model.as_deref();
        let reasoning_effort = resolved.reasoning_effort.as_deref();
        let provider_override = resolved.provider.as_deref();
        let chosen_provider = resolved.as_selection().provider;

        use crate::engine::thread_events::EventMeta;
        let origin_id = if let Some(pre) = pre_emitted_origin {
            pre.event_id()
        } else {
            let started = trigger
                .as_ref()
                .and_then(|tc| build_trigger_started_event(tc, user_message, &resolved));
            let (user_thread_event, user_meta) = if let Some(started) = started {
                started
            } else {
                let is_cc = use_coding_agent == Some(true);
                let (req_model, req_effort) = match spawn_claim.as_ref().filter(|_| is_cc) {
                    Some(claim) => {
                        self.coding_agent_rung(
                            thread_id,
                            claim.entry.coding_agent,
                            cc_model.map(String::from),
                            reasoning_effort.map(String::from),
                        )
                        .await
                    }
                    None => (
                        model_override.map(String::from),
                        reasoning_effort.map(String::from),
                    ),
                };
                (
                    make_message_received(
                        &self.workspace_path,
                        user_message,
                        user_images,
                        device_id,
                        parent_thread_id,
                        spawning_event_id,
                        wake_mode,
                        mode,
                        req_model.as_deref(),
                        req_effort.as_deref(),
                        provider_override,
                        origin.clone(),
                        voice_session_id,
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

            let asked = self
                .event_bus
                .ask_hooks_with_args(thread_id, &user_thread_event, &user_meta, hook_args)
                .await;
            let emitted = match asked {
                Ok(asked) => {
                    self.event_bus
                        .emit_asked(asked, thread_id, user_thread_event, user_meta)
                        .await
                }
                Err(refusal) => {
                    let orphans = self
                        .drain_turn_orphans(thread_id, guard, &mut injection_rx)
                        .await;
                    self.resubmit_orphans_of_failed_turn(thread_id, orphans);
                    return refused_start(
                        &self.event_bus,
                        refusal,
                        is_trigger,
                        mode,
                        request_id,
                        thread_id,
                        event_id,
                    )
                    .await;
                }
            };
            let emit_result = match emitted {
                Ok(result) => result.expect("persisted event must return EmitResult"),
                Err(e) => {
                    let orphans = self
                        .drain_turn_orphans(thread_id, guard, &mut injection_rx)
                        .await;
                    self.resubmit_orphans_of_failed_turn(thread_id, orphans);
                    return withdrawn_or(e, request_id, thread_id);
                }
            };
            if let Some(parent) = parent_thread_id {
                self.mcp_manager.inherit_enabled(parent, thread_id).await;
            }
            if let Some(tc) = trigger.as_ref() {
                self.copy_trigger_ceiling(thread_id, &tc.trigger_id).await;
            }
            emit_result.event_id
        };
        let response_channel = is_trigger.then_some(EventChannel::Trigger);
        self.set_thread_request_event_id(thread_id, guard.generation(), origin_id);
        if use_coding_agent != Some(true) && !keep_alive {
            if let Err(error) = self
                .event_bus
                .emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::ChatTurnSettingsResolved {
                        model: resolved.model.clone().unwrap_or_default(),
                        reasoning_effort: resolved.reasoning_effort.clone(),
                        provider: resolved.provider.clone(),
                    },
                    meta: EventMeta {
                        channel: Some(response_channel.unwrap_or(EventChannel::Chat)),
                        request_event_id: Some(origin_id),
                        ..EventMeta::NONE
                    },
                })
                .await
            {
                if crate::engine::consumption_claim::is_removed_refusal(&*error) {
                    let orphans = self
                        .drain_turn_orphans(thread_id, guard, &mut injection_rx)
                        .await;
                    self.resubmit_orphans_of_failed_turn(thread_id, orphans);
                    return withdrawn_or(error, request_id, thread_id);
                }
                return Err(self
                    .fail_turn(
                        thread_id,
                        origin_id,
                        response_channel,
                        error,
                        guard,
                        &mut injection_rx,
                    )
                    .await);
            }
        }
        // INVARIANT: Recorded, and the thread is registered, so a later input now queues
        // behind this turn rather than racing it. Release it, and the gate.
        // The thread now holds its continue key, so a racing fire finds it.
        drop(input_turn.take());
        drop(turn_start);
        if let Some(TriggerTurn::Fire(fire)) = trigger.as_mut().map(|tc| &mut tc.turn) {
            drop(fire.continue_key.take());
        }

        if let Err(e) = self
            .maybe_emit_titles(
                thread_id,
                &thread_id_str,
                title,
                is_trigger,
                is_new_thread,
                &trigger,
                user_message,
                user_images,
            )
            .await
        {
            return Err(self
                .fail_turn(
                    thread_id,
                    origin_id,
                    response_channel,
                    e,
                    guard,
                    &mut injection_rx,
                )
                .await);
        }

        let resumed_text = if use_coding_agent == Some(true) {
            user_message.to_string()
        } else {
            match crate::engine::thread_stop::resume_text(
                self.pool(),
                thread_id,
                user_message,
                Some(origin_id),
            )
            .await
            {
                Ok(text) => text,
                Err(error) => {
                    return Err(self
                        .fail_turn(
                            thread_id,
                            origin_id,
                            response_channel,
                            error.into(),
                            guard,
                            &mut injection_rx,
                        )
                        .await)
                }
            }
        };
        let user_message = resumed_text.as_str();

        if use_coding_agent == Some(true) {
            // INVARIANT: Returns straight out, so the post-loop net below never sees it.
            // The branch emits its own terminator on most failures; the gate
            // in `fail_turn` makes this a no-op whenever it did.
            // The keep-alive holds the thread now, so its turn is the only
            // one the flag can describe until it clears it again below.
            let _keep_alive_turn = match keep_alive {
                true => Some(self.event_bus.begin_keep_alive_turn(thread_id).await),
                false => None,
            };
            let cc_result = self
                .run_cc_chat_branch(
                    spawn_claim
                        .take()
                        .expect("the coding-agent lane claims the thread before it registers"),
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
            let outcome = match cc_result {
                Err(e) => Err(self
                    .fail_turn(
                        thread_id,
                        origin_id,
                        response_channel,
                        e,
                        guard,
                        &mut injection_rx,
                    )
                    .await),
                ok => ok,
            };
            return outcome;
        }

        if let Err(e) = remote_start::gate_remote_start(
            &self.event_bus,
            thread_id,
            response_channel.unwrap_or(EventChannel::Chat),
        )
        .await
        {
            return Err(self
                .fail_turn(
                    thread_id,
                    origin_id,
                    response_channel,
                    e,
                    guard,
                    &mut injection_rx,
                )
                .await);
        }

        // INVARIANT: ── Turn setup ──────────────────────────────────────────────────────
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
        let cancel_exit = SetupCancel {
            thread_id,
            request_id,
            meta: crate::engine::thread_events::EventMeta {
                request_event_id: Some(origin_id),
                channel: response_channel,
                ..crate::engine::thread_events::EventMeta::NONE
            },
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

        let user_timezone = self.user_timezone.read().await.clone();
        let user_language = self.user_language.read().await.clone();
        let user_profile = self.user_profile.snapshot().await;

        let classification_call = self
            .aux_call(crate::engine::ContextPurpose::QueryClassification)
            .await;

        // INVARIANT: This turn's clock. Every reading the model sees derives from it,
        // never from `Utc::now()`. The message array is rebuilt from events on
        // each request. A wall-clock reading inside it would differ on rebuild,
        // and cost the message tier the hit the system tier just gained.
        //
        // Keyed on the thread, NOT on `origin_id`: an answer-driven resume
        // re-uses the interrupted turn's anchor, which can be a day old. See
        // `super::turn_clock`.
        let turn_started_at = self.turn_started_at(thread_id).await;

        let Some(capabilities) = until_canceled(&cancel_token, self.read_turn_capabilities()).await
        else {
            return Ok(self
                .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                .await);
        };

        let context_mode = super::context_mode::ContextMode::from_capabilities(&capabilities.gates);
        let memory_tree = capabilities.gates.memory_tree;

        if !memory_tree {
            self.note_history_read_begun(thread_id, guard.generation());
        }

        let Some(history_load) = until_canceled(
            &cancel_token,
            self.load_chat_history(
                is_trigger || is_new_thread,
                matches!(pre_emitted_origin, Some(PreEmittedOrigin::Message(_)))
                    .then_some(origin_id),
                thread_id,
                user_message,
                turn_started_at,
                &capabilities.gates,
            ),
        )
        .await
        else {
            return Ok(self
                .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                .await);
        };
        let super::history::ChatHistoryLoad {
            mut resume_tool_blocks,
            loaded_knowhow_docs,
            mut history_context,
            conversation_summary,
            history_image_hashes,
        } = history_load;

        let base_ctx = self.extraction_context_base().await;
        let extraction_ctx =
            Self::extraction_context_with_conversation(&base_ctx, &conversation_summary);

        let classification = if let Some(pinned) = pinned_classification() {
            pinned
        } else {
            let ctx = if conversation_summary.is_empty() {
                None
            } else {
                Some(conversation_summary.as_str())
            };
            let capture = crate::engine::AuxCapture::new(
                &self.event_bus,
                thread_id,
                crate::engine::ContextPurpose::QueryClassification,
            );
            let Some(classification) = until_canceled(
                &cancel_token,
                classify_or_fallback(
                    crate::memory::classify_query(
                        &self.pool,
                        user_message,
                        ctx,
                        &classification_call,
                        !memory_tree,
                        &capture,
                    ),
                    classification_call.deadline(),
                ),
            )
            .await
            else {
                return Ok(self
                    .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                    .await);
            };
            classification
        };

        let response_meta = cancel_exit.meta.clone();
        let recall = async {
            if memory_tree {
                (String::new(), Vec::new())
            } else {
                self.retrieve_context(user_message, &classification).await
            }
        };
        let Some((mut memory_context, recalled_memories)) =
            until_canceled(&cancel_token, recall).await
        else {
            return Ok(self
                .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                .await);
        };

        if classification.needs_memory && !memory_tree {
            self.event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::MemoryRecalled {
                            results: recalled_memories.len(),
                            queries: classification.sub_queries.clone(),
                            memories: recalled_memories,
                        },
                        meta: response_meta,
                    },
                    "[Chat] MemoryRecalled",
                )
                .await;
        }

        let max_tool_calls = crate::core::PreferenceStore::max_tool_calls(&self.pool).await;

        if context_mode.is_on() {
            resume_tool_blocks.clear();
        }

        // INVARIANT: The thread's checklist, read once per turn, on BOTH arms. Under the
        // mode it rides inside the working understanding at the tail, so the
        // loop needs the snapshot it starts from. With the mode off it becomes
        // the `[TODO LIST]` block below. That block is the only thing putting
        // the settle's own rewrites back in front of the agent.
        //
        // One indexed lookup, the same one the settle and the wake check
        // already run per turn. A thread that never wrote a list finds no row
        // and adds no bytes.
        let todo_snapshot = match crate::engine::tools::todo::latest_todo_list(
            &self.pool, thread_id,
        )
        .await
        {
            Ok(found) => found,
            Err(e) => {
                log_legacy!(
                    "[Chat] todo list read failed for thread {}: {}. This turn carries no checklist",
                    thread_id,
                    e
                );
                None
            }
        };
        let todo_seed: Vec<crate::engine::thread_events::TodoItem> = todo_snapshot
            .as_ref()
            .filter(|_| context_mode.is_on())
            .map(|list| list.items.clone())
            .unwrap_or_default();
        let todo_notes_seed: Option<String> = todo_snapshot
            .as_ref()
            .filter(|_| context_mode.is_on())
            .and_then(|list| list.notes.clone());

        let todo_list_block =
            crate::engine::tools::todo::turn_start_block(context_mode, todo_snapshot.as_ref());

        let document = if context_mode.is_on() {
            match super::working_understanding::latest_document(&self.pool, thread_id).await {
                Ok(found) => found.unwrap_or_default(),
                Err(e) => {
                    log_legacy!(
                        "[Chat] working understanding read failed for thread {}: {}. This turn carries none",
                        thread_id,
                        e
                    );
                    Default::default()
                }
            }
        } else {
            Default::default()
        };

        let Some((system_prompt, missing_pref_keys)) = until_canceled(
            &cancel_token,
            self.build_chat_system_prompt(
                &user_timezone,
                &user_language,
                device_id,
                &trigger,
                max_tool_calls,
                &capabilities,
            ),
        )
        .await
        else {
            return Ok(self
                .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                .await);
        };

        if !memory_context.is_empty() {
            log_legacy!(@Memory, "Retrieved {}KB of context", memory_context.len() / 1024);
        }

        let Some(context_sections) = until_canceled(
            &cancel_token,
            self.build_chat_context_sections(super::context_sections::ChatContextInputs {
                classification: &classification,
                user_profile: &user_profile,
                thread_id,
                turn_anchor: origin_id,
                device_id,
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

        let provider = self.current_provider();
        let resolved_model = model_override.unwrap_or_else(|| provider.default_model());
        let context_window = self.context_window_for(resolved_model, chosen_provider);
        let budget_model = self.request_budget_model_for(resolved_model, chosen_provider);

        let mut mcp_start_failures = match &trigger {
            Some(tc) => {
                self.mcp_manager
                    .enable_listed(thread_id, &tc.mcp_servers)
                    .await
            }
            None => Vec::new(),
        };
        mcp_start_failures.extend(self.mcp_manager.start_enabled_servers(thread_id).await);
        let tools = self
            .chat_turn_tools(
                thread_id,
                &capabilities.gates,
                context_window,
                &budget_model,
            )
            .await;
        if cancel_token.is_cancelled() {
            return Ok(self
                .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                .await);
        }

        let total_budget =
            crate::engine::context::agent_context_char_budget_for(&budget_model, context_window);
        let prompt_overhead: usize = system_prompt.len() + tools.defs_chars();
        let message_budget = total_budget.saturating_sub(prompt_overhead);

        let memory_views = if memory_tree {
            let surface = self.chat_surface(thread_id, is_trigger).await;
            self.note_history_read_begun(thread_id, guard.generation());
            let Some(views) = until_canceled(
                &cancel_token,
                self.turn_memory_views(
                    thread_id,
                    surface,
                    resolved_model,
                    user_message,
                    message_budget / 2,
                ),
            )
            .await
            else {
                return Ok(self
                    .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                    .await);
            };
            views
        } else {
            super::memory_views::TurnMemoryViews::default()
        };

        let loaded_knowhow_block = build_loaded_knowhow_block(&loaded_knowhow_docs);
        let knowhow_retention_block = build_knowhow_retention_block(&loaded_knowhow_docs);

        let current_time_block =
            super::turn_clock::current_time_block(turn_started_at, &user_timezone);

        // INVARIANT: The other readings the cached system block must not hold. Each is
        // read ONCE here, so every round of this turn sends the same bytes and
        // the message prefix stays cacheable (`super::turn_tail`).
        //
        // Raced against the token like every other setup read, and this one
        // has to be: `version_status` forks git probes that take tens of
        // seconds on a saturated host, and a Stop pressed during one would
        // otherwise wait it out.
        let Some(engine_build_block) = until_canceled(&cancel_token, async {
            if crate::paths::has_lucidos_source() {
                super::turn_tail::engine_build_block(&self.version_status().await)
            } else {
                String::new()
            }
        })
        .await
        else {
            return Ok(self
                .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                .await);
        };
        let client_url_block = super::turn_tail::client_url_block(&self.client_url());
        let Some(child_status_block) = until_canceled(
            &cancel_token,
            super::child_status::child_status_block(&self.pool, thread_id),
        )
        .await
        else {
            return Ok(self
                .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                .await);
        };

        let trigger_knowhow_context = match trigger.as_ref() {
            None => String::new(),
            Some(t) => {
                let listing = super::super::process_helpers::build_trigger_knowhow_section(
                    &self.workspace_path.join(crate::core::TRIGGERS_DIR),
                    &t.slug,
                );
                let Some(removed) = until_canceled(
                    &cancel_token,
                    crate::triggers::affected::removed_knowhow(
                        &self.pool,
                        &self.workspace_path.join(crate::core::DATA_DIR),
                        &t.trigger_id,
                    ),
                )
                .await
                else {
                    return Ok(self
                        .cancel_during_setup(&cancel_exit, guard, &mut injection_rx)
                        .await);
                };
                let removed = removed.unwrap_or_else(|e| {
                    log_legacy!(
                        "[Chat] removed know-how lookup failed for trigger {}: {}",
                        t.trigger_id,
                        e
                    );
                    Vec::new()
                });
                let removed =
                    super::super::process_helpers::build_removed_knowhow_section(&removed);
                [listing, removed]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            }
        };

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
            + knowhow_retention_block.as_deref().map_or(0, str::len)
            + todo_list_block.len()
            + child_status_block.len()
            + engine_build_block.len()
            + client_url_block.len()
            + current_time_block.len()
            + tools.mcp_dropped_notice().map_or(0, str::len)
            + trigger_knowhow_context.len()
            + user_message.len()
            + memory_views.bytes()
            + 500;
        let expendable_budget = message_budget.saturating_sub(fixed_size);
        let expendable_size = memory_context.len() + history_context.len();
        if expendable_size > expendable_budget {
            log_legacy!("[Chat] Initial context ({}KB) exceeds message budget ({}KB, prompt overhead {}KB), trimming",
                (fixed_size + expendable_size) / 1024, message_budget / 1024, prompt_overhead / 1024);
            let mut excess = expendable_size - expendable_budget;
            if memory_context.len() >= excess {
                cut_memory_tail(&mut memory_context, excess);
                excess = 0;
            } else {
                excess -= memory_context.len();
                memory_context.clear();
            }
            if excess > 0 {
                trim_history_from_oldest(&mut history_context, excess);
            }
        }

        let mcp_servers_context = super::workspace_prompt_footprint::build_mcp_servers_section(
            &self
                .mcp_manager
                .server_lines(
                    &self.mcp_manager.enabled_servers(thread_id).await,
                    &mcp_start_failures,
                    trigger.as_ref().map(|tc| tc.mcp_servers.as_slice()),
                )
                .await,
        );
        let setup_reminder = if !missing_pref_keys.is_empty() {
            let missing_list = missing_pref_keys.join(", ");
            format!("CRITICAL: The following preferences are not set: {}. Do NOT proceed with the user's request. Ask the user to configure these first.", missing_list)
        } else {
            String::new()
        };

        let turn_text = assemble_turn_text(&TurnSections {
            recent: &memory_views.recent,
            profile: &profile_context,
            email_accounts: &email_accounts_context,
            oauth: &oauth_context,
            thread_depth: &thread_depth_context,
            loaded_knowhow: loaded_knowhow_block.as_deref().unwrap_or_default(),
            device_preferences: &device_preferences_context,
            memory: &memory_context,
            history: &history_context,
            file_list: &file_list_context,
            credentials: &credentials_context,
            knowhow_retention: knowhow_retention_block.as_deref().unwrap_or_default(),
            app_context: &app_context_section,
            file_context: &file_context_section,
            url_context: &url_context_section,
            todo_list: &todo_list_block,
            mcp_servers: &mcp_servers_context,
            mcp_dropped_notice: tools.mcp_dropped_notice().unwrap_or_default(),
            trigger_knowhow: &trigger_knowhow_context,
            setup_reminder: &setup_reminder,
            request: user_message,
            tail: super::turn_tail::TurnTail {
                child_threads: &child_status_block,
                engine_build: &engine_build_block,
                client_url: &client_url_block,
                current_time: &current_time_block,
            },
        });

        // INVARIANT: Attached images ride inline as base64 image blocks on the user message
        // (`build_user_content_with_images` below) and stay visible for the whole
        // turn. No on-disk path annotation: the chat `read_file` tool is rooted at
        // `data/artifacts/`, so a `.lucidos/tmp/images/…` hint only sent the model
        // chasing a path it can't reach ("the bot can't see my attached image").

        let workspace_view = memory_views.workspace();
        let (capture_memory, capture_history) = if memory_tree {
            (workspace_view.as_str(), memory_views.thread.text.as_str())
        } else {
            (memory_context.as_str(), history_context.as_str())
        };
        let capture_sections = build_capture_sections(
            &system_prompt,
            &profile_context,
            &device_preferences_context,
            &file_list_context,
            &credentials_context,
            &email_accounts_context,
            &oauth_context,
            capture_memory,
            capture_history,
            &app_context_section,
            &file_context_section,
            &url_context_section,
            &mcp_servers_context,
            tools.mcp_dropped_notice().unwrap_or_default(),
            &trigger_knowhow_context,
            &setup_reminder,
            &thread_depth_context,
            &todo_list_block,
            user_message,
            &super::turn_tail::TurnTail {
                child_threads: &child_status_block,
                engine_build: &engine_build_block,
                client_url: &client_url_block,
                current_time: &current_time_block,
            },
            &loaded_knowhow_docs,
            &resume_tool_blocks,
        );
        let capture_model = resolved_model.to_string();

        let user_content = memory_views.lead(
            turn_text.stable,
            build_user_content_with_images(
                turn_text.volatile,
                &self.workspace_path,
                &history_image_hashes,
                user_images,
                &user_images
                    .map(|imgs| current_image_handles(&self.workspace_path, imgs))
                    .unwrap_or_default(),
            ),
        );

        let mut messages = resume_tool_blocks;
        messages.push(Message {
            role: "user".to_string(),
            content: user_content,
        });

        let mut terminator_settled = false;
        let result = self
            .run_agentic_loop(
                &mut messages,
                &system_prompt,
                tools,
                request_id,
                thread_id,
                response_channel,
                !is_trigger,
                message_budget,
                &extraction_ctx,
                description_handle,
                origin_id,
                &mut proposed_change,
                user_images,
                device_id,
                resolved.as_selection(),
                &cancel_token,
                &mut injection_rx,
                guard.generation(),
                &mut terminator_settled,
                crate::engine::agentic_loop::ContextCaptureSeed {
                    sections: &capture_sections,
                    model: &capture_model,
                },
                max_tool_calls,
                super::context_mode::CuratedTurn {
                    mode: context_mode,
                    schedule: capabilities.schedule,
                    document,
                    todo: todo_seed,
                    todo_notes: todo_notes_seed,
                },
            )
            .await;

        // INVARIANT: Defensive: if the loop returned without emitting a terminator for
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

        // INVARIANT: The turn is over and this thread may still own running background
        // work. Only an event wait re-opens the thread when it completes. If the
        // model armed none, the finish lands with nobody watching and the
        // thread stops: the five-hour release stall behind this code. Arming
        // here is the engine noticing on the model's behalf; it no-ops when
        // there is no background work or when a wait already covers it.
        //
        // Gated on a CLEAN end, both halves load-bearing. A cancelled turn is
        // the user pressing Stop or a follow-up superseding this turn: in the
        // first case re-opening the thread later resurrects work they abandoned,
        // and in the second the next turn runs its own tail in a moment. An
        // errored turn is not a state to schedule a wake from either.
        if result.is_ok() && !cancel_token.is_cancelled() {
            self.arm_wait_for_running_background_tasks(thread_id, OwnerTurn::Ended)
                .await;
        }

        let orphans = self
            .drain_turn_orphans(thread_id, guard, &mut injection_rx)
            .await;

        match result {
            Ok(mut r) => {
                r.orphaned_injections = orphans;
                Ok(r)
            }
            Err(e) => {
                self.resubmit_orphans_of_failed_turn(thread_id, orphans);
                Err(e)
            }
        }
    }

    /// INVARIANT: The turn tail EVERY exit shares, normal or cancelled.
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
    ///
    /// **This drain is total while the process lives, so it reads no event
    /// store.** A `chat::queued_recovery` union here was tried and removed: the
    /// contract above leaves exactly two cases, and the query's whole yield is
    /// the second one. Those messages the refused caller already owns, so
    /// re-submitting them answers each twice. The recovery belongs where the
    /// channel is GONE, which is the resume (`chat/rerun.rs`).
    async fn drain_turn_orphans(
        &self,
        thread_id: Uuid,
        guard: ThreadGuard,
        injection_rx: &mut mpsc::UnboundedReceiver<InjectedPrompt>,
    ) -> Vec<InjectedPrompt> {
        let orphans = Self::finalize_turn_and_drain_injections(guard, injection_rx);
        let orphans = crate::engine::agentic_loop::filter_removed_queued_prompts(
            &self.pool, thread_id, orphans,
        )
        .await;
        let orphans: Vec<_> = orphans
            .into_iter()
            .filter(|p| p.kind.can_carry_a_turn())
            .collect();
        if !orphans.is_empty() {
            log_legacy!(
                "[Chat] {} orphaned injection(s) after turn exit for thread {}, re-submitting as follow-ups",
                orphans.len(),
                thread_id
            );
        }
        orphans
    }

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
