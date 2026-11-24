//! Chat message processing. The public entry points (`process_trigger`,
//! `process_message_with_steps`) and the small `clean_response` /
//! `describe_tool` helpers live here; the heavy internal pipeline is split by
//! phase into child modules:
//! - [`run`] — the `process_message_with_steps_internal` orchestrator.
//! - [`system_prompt`] — system-prompt assembly + `ASK_USER_QUESTION_RULE`.
//! - [`history`] — conversation history + resume context loading.
//! - [`context_sections`] — per-turn context-section builders.
//! - [`titles`] — title-emit orchestration.
//! - [`context_build`] — pure capture-section / loaded-knowhow helpers.
//! - [`context_mode`]: the self-curated context mode, in one place.
//! - [`working_understanding`]: the model's own document, parsed out of its
//!   reply and rendered back at the tail of every round.
//! - [`turn_clock`]: the turn's time context, split across the two prompt tiers.
//! - [`turn_tail`]: the engine build state and the client URL, split the same
//!   way, plus the struct carrying all three tail blocks.

use crate::engine::chat::process::run::ChatTurnRunExt;
use crate::engine::thread_events::{ActorMode, EngineReason, MessageOrigin, TriggerInvocation};
use crate::engine::types::*;
use crate::engine::LucidosEngine;
use crate::triggers::continue_key::FireTarget;
use uuid::Uuid;

use super::process_helpers::{TriggerContext, TriggerFire, TriggerTurn};

mod child_status;
mod context_build;
pub(crate) mod context_mode;
pub(crate) mod context_panel;
mod context_sections;
pub(crate) mod footprint_sections;
mod history;
mod memory_views;
mod run;
pub(crate) mod side_question;
mod system_prompt;
mod titles;
mod turn_clock;
mod turn_tail;
pub(crate) mod working_understanding;
// Crate-visible so the per-trigger knowhow listing in `chat::process_helpers`
// shares its routing-description ceiling, and `widget_guidance` its app lines.
pub(crate) mod workspace_prompt_footprint;

// A `run_thread` child resolves its inherited route the way a turn does.
pub(crate) use run::resolve_available_route_overrides;

// Re-exported only for the colocated `process_tests` module, which reaches
// these via `super::`. Non-test code imports them from the child modules
// directly (run.rs uses `context_build::*`; system_prompt.rs uses the const
// locally), so the re-exports are `cfg(test)`-gated to avoid unused warnings.
#[cfg(test)]
pub(crate) use context_build::{
    assemble_turn_text, build_capture_sections, build_knowhow_retention_block,
    build_loaded_knowhow_block, TurnSections,
};
// `PLUGIN_SETUP_RULE` also travels to `tools::plugins_tests`, whose seed test
// pins the route against the two lines the engine actually sends.
#[cfg(test)]
pub(crate) use system_prompt::{ASK_USER_QUESTION_RULE, PLUGIN_SETUP_RULE, REPEATED_ACTION_RULE};
#[cfg(test)]
pub(crate) use turn_tail::TurnTail;

/// The lane an orphaned input re-runs on.
pub(crate) enum OrphanLane {
    /// Keeps the admission the orphan chain already won, if any.
    LucidosAgent(Option<crate::engine::PreAdmission>),
    CodingAgent(Vec<crate::engine::AgentUserInput>),
}

#[easy_ext::ext(ChatProcessExt)]
pub(crate) impl LucidosEngine {
    /// Process a trigger prompt (emits TriggerStarted instead of MessageReceived).
    /// `invocation` records which path fired this run (cron schedule or matched event).
    /// `target` says whether the fire starts a thread or continues the open
    /// thread holding its continue key (ADR 0623).
    #[allow(clippy::too_many_arguments)]
    async fn process_trigger(
        &self,
        trigger_id: &str,
        trigger_name: &str,
        slug: &str,
        prompt: &str,
        invocation: TriggerInvocation,
        go_to_review: bool,
        mcp_servers: Vec<String>,
        model: Option<&str>,
        reasoning_effort: Option<&str>,
        // The backend this trigger pinned, or `None` to let the model's own
        // preferred provider decide. A pin naming an unconfigured backend
        // refuses the fire rather than running it somewhere else.
        provider: Option<&str>,
        // See `TriggerContext::queue_entry_id`.
        queue_entry_id: Uuid,
        retry_of: Option<crate::engine::thread_events::RetryOf>,
        target: FireTarget,
        actor_email: Option<String>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        // A continuing fire is engine input on someone else's thread, so it
        // names its trigger as the origin. A new thread's `TriggerStarted`
        // stamps that origin itself.
        let (thread_id, continue_key, origin, mode) = match target {
            FireTarget::NewThread(claim) => (None, claim, None, ActorMode::Agent),
            FireTarget::Continue(thread_id) => {
                let origin = MessageOrigin::engine(EngineReason::Scheduler {
                    trigger_id: trigger_id.to_string(),
                    trigger_name: Some(trigger_name.to_string()),
                    actor_email: actor_email.clone(),
                });
                (Some(thread_id), None, Some(origin), ActorMode::Engine)
            }
        };
        // The trigger's own model when it pinned one, else the account's
        // trigger model, so a fire never starts on the chat default. The
        // resolver in `process_message_with_steps_internal` takes an explicit
        // value as the override, which is what makes a pinned trigger a pin.
        // The effort stays `None` unless pinned, and resolves for that model.
        let configured = self.current_provider().configured_providers();
        let model = run::trigger_model_override(
            &self.pool,
            &self.model_registry,
            |kind| configured.as_ref().is_none_or(|set| set.contains(&kind)),
            model,
        )
        .await;
        let result = self
            .process_message_with_steps_internal(
                prompt,
                model.as_deref(),
                Some(TriggerContext {
                    trigger_id: trigger_id.to_string(),
                    trigger_name: trigger_name.to_string(),
                    slug: slug.to_string(),
                    go_to_review,
                    mcp_servers,
                    turn: TriggerTurn::Fire(TriggerFire {
                        retry_of,
                        invocation,
                        queue_entry_id,
                        continue_key,
                        actor_email,
                    }),
                }),
                None, // app_context
                None, // file_context
                reasoning_effort,
                provider,
                None, // user_images
                None, // device_id
                None, // use_coding_agent
                None, // event_id
                thread_id,
                None,
                None,
                None,
                None,
                None,
                None,
                mode,
                None,
                None,
                None,
                None,
                origin,
                None,
                crate::engine::FollowUpUrgency::Normal,
                None,
                None,
                Vec::new(),
            )
            .await?;
        // A continuing fire may coalesce into a live turn. Then the thread's
        // last `TriggerStarted` belongs to an earlier fire.
        match thread_id {
            None => trigger_process_result(&self.pool, result).await,
            Some(_) => Ok(result),
        }
    }

    /// Carry on a trigger run that parked on a grant card, as a turn of the
    /// same trigger: its model, its MCP servers and the trigger channel's
    /// guard. `anchor` is the parked turn the resumed events join.
    async fn resume_trigger_turn(
        &self,
        config: &crate::triggers::TriggerConfig,
        thread_id: Uuid,
        note: &str,
        anchor: Uuid,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        let configured = self.current_provider().configured_providers();
        let model = run::trigger_model_override(
            &self.pool,
            &self.model_registry,
            |kind| configured.as_ref().is_none_or(|set| set.contains(&kind)),
            config.model.as_deref(),
        )
        .await;
        self.process_message_with_steps_internal(
            note,
            model.as_deref(),
            Some(TriggerContext {
                trigger_id: config.id.clone(),
                trigger_name: config.name.clone(),
                slug: config.slug.clone(),
                go_to_review: config.go_to_review,
                mcp_servers: config.mcp_servers.clone(),
                turn: TriggerTurn::Resume,
            }),
            None, // app_context
            None, // file_context
            config.reasoning_effort.as_deref(),
            config.provider.as_deref(),
            None, // user_images
            None, // device_id
            None, // use_coding_agent
            None, // event_id
            Some(thread_id),
            None,
            None,
            None,
            None,
            None,
            None,
            ActorMode::Engine,
            None,
            None,
            Some(super::PreEmittedOrigin::EngineReentry(anchor)),
            None,
            Some(MessageOrigin::engine(EngineReason::ContinuationStarted)),
            None,
            crate::engine::FollowUpUrgency::Normal,
            None,
            None,
            Vec::new(),
        )
        .await
    }

    // Public chat entry — surface every per-request knob (model / context /
    // images / steps_tx / cancel) at the call boundary so triggers and HTTP
    // handlers can wire them through without re-creating a builder struct.
    #[allow(clippy::too_many_arguments)]
    async fn process_message_with_steps(
        &self,
        user_message: &str,
        model_override: Option<&str>,
        app_context: Option<AppContext>,
        file_context: Option<String>,
        reasoning_effort: Option<&str>,
        // Backend to serve the model. See the internal orchestrator's own
        // parameter: honoured or refused, never substituted.
        provider_override: Option<&str>,
        images: Option<&[crate::engine::types::ChatImage]>,
        device_id: Option<&str>,
        use_coding_agent: Option<bool>,
        event_id: Option<&str>,
        thread_id: Option<Uuid>,
        conflict_change_id: Option<Uuid>,
        repo_id: Option<&str>,
        url_context: Option<crate::engine::types::UrlContext>,
        parent_thread_id: Option<Uuid>,
        spawning_event_id: Option<Uuid>,
        // The child's wake mode, beside its parent link. `None` for every
        // message but a child's spawning one.
        wake_mode: Option<crate::engine::thread_events::WakeMode>,
        mode: ActorMode,
        cc_model: Option<&str>,
        coding_agent: Option<crate::runtime::CodingAgent>,
        pre_emitted_origin: Option<PreEmittedOrigin>,
        title: Option<&str>,
        origin: Option<MessageOrigin>,
        // The *voice session* this message was spoken on. `None` is typed,
        // which is every caller but the voice one. It rides beside `origin`
        // because it answers the same question: how this message got here.
        //
        // Threaded even though the voice path normally pre-emits, so a
        // pre-emit that failed still writes a message that says it was spoken.
        voice_session_id: Option<Uuid>,
        // `Normal` for every caller but a child follow-up the parent marked
        // urgent, which preempts the child's in-flight turn instead of queueing
        // behind it. Named rather than a bare `bool` so the call sites stay
        // readable in a column of `None`s.
        urgency: crate::engine::FollowUpUrgency,
        // This input's place in its thread's input order, taken by the chat
        // handler's admission. Other callers promise no order and pass `None`.
        input_turn: Option<crate::engine::InputTurn>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        self.process_message_with_steps_internal(
            user_message,
            model_override,
            None,
            app_context,
            file_context,
            reasoning_effort,
            provider_override,
            images,
            device_id,
            use_coding_agent,
            event_id,
            thread_id,
            conflict_change_id,
            repo_id,
            url_context,
            parent_thread_id,
            spawning_event_id,
            wake_mode,
            mode,
            cc_model,
            coding_agent,
            pre_emitted_origin,
            title,
            origin,
            voice_session_id,
            urgency,
            input_turn,
            None,
            Vec::new(),
        )
        .await
    }

    /// Orphan-only entry, on the lane the thread's agent decides (ADR 0678).
    #[allow(clippy::too_many_arguments)]
    async fn process_orphan(
        &self,
        text: &str,
        images: Option<&[crate::engine::types::ChatImage]>,
        thread_id: Uuid,
        spawning_event_id: Option<Uuid>,
        mode: ActorMode,
        origin: Option<PreEmittedOrigin>,
        lane: OrphanLane,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        let (use_coding_agent, admission, followups) = match lane {
            OrphanLane::LucidosAgent(admission) => (None, admission, Vec::new()),
            OrphanLane::CodingAgent(followups) => (Some(true), None, followups),
        };
        self.process_message_with_steps_internal(
            text,
            None,
            None,
            None,
            None,
            None,
            None,
            images,
            None,
            use_coding_agent,
            None,
            Some(thread_id),
            None,
            None,
            None,
            None,
            spawning_event_id,
            None,
            mode,
            None,
            None,
            origin,
            None,
            None,
            None,
            crate::engine::FollowUpUrgency::Normal,
            None,
            admission,
            followups,
        )
        .await
    }

    fn clean_response(&self, content: &str) -> String {
        clean_response_text(content)
    }

    fn describe_tool(&self, name: &str, args: &serde_json::Value) -> String {
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

fn clean_response_text(content: &str) -> String {
    let content = content.trim();

    if content.starts_with("Tool results:")
        || content.starts_with("[list_files]")
        || content.starts_with("[read_file]")
        || content.starts_with("[run_python]")
        || content.starts_with("run_python({")
    {
        return "Task completed. Check the workspace for any created files.".to_string();
    }

    content.to_string()
}

pub(in crate::engine) async fn trigger_process_result(
    pool: &sqlx::PgPool,
    result: ProcessResult,
) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
    let terminal: Option<String> = sqlx::query_scalar(
        "SELECT event_type FROM events \
         WHERE aggregate = 'thread' AND aggregate_id = $1 \
           AND event_type = ANY($2::text[]) \
           AND sequence > (SELECT MAX(sequence) FROM events \
                           WHERE aggregate = 'thread' AND aggregate_id = $1 \
                             AND event_type = 'TriggerStarted') \
         ORDER BY sequence ASC LIMIT 1",
    )
    .bind(result.thread_id.to_string())
    .bind(crate::engine::thread_events::ThreadEvent::TERMINATOR_EVENT_TYPES)
    .fetch_optional(pool)
    .await?;
    if terminal.as_deref() == Some("ResponseAborted") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "The trigger run was interrupted before it completed",
        )
        .into());
    }
    Ok(result)
}

#[cfg(test)]
#[path = "../process_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "history_tests.rs"]
mod history_tests;

#[cfg(test)]
#[path = "context_release_tests.rs"]
mod context_release_tests;

#[cfg(test)]
#[path = "withdrawn_tests.rs"]
mod withdrawn_tests;

#[cfg(test)]
#[path = "blocked_run_tests.rs"]
mod blocked_run_tests;
