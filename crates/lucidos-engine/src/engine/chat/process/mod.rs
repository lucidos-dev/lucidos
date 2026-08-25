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

use crate::engine::thread_events::{ActorMode, MessageOrigin, TriggerInvocation};
use crate::engine::types::*;
use crate::engine::LucidosEngine;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::process_helpers::TriggerContext;

mod context_build;
pub(crate) mod context_mode;
pub(crate) mod context_panel;
mod context_sections;
mod history;
mod run;
mod system_prompt;
mod titles;
mod turn_clock;
mod turn_tail;
pub(crate) mod working_understanding;
// `pub(super)` so the per-trigger knowhow listing in `chat::process_helpers`
// can share this module's routing-description ceiling.
pub(super) mod workspace_payload;

// Re-exported only for the colocated `process_tests` module, which reaches
// these via `super::`. Non-test code imports them from the child modules
// directly (run.rs uses `context_build::*`; system_prompt.rs uses the const
// locally), so the re-exports are `cfg(test)`-gated to avoid unused warnings.
#[cfg(test)]
pub(crate) use context_build::{build_capture_sections, build_loaded_knowhow_block};
// `PLUGIN_SETUP_RULE` also travels to `tools::plugins_tests`, whose seed test
// pins the route against the two lines the engine actually sends.
#[cfg(test)]
pub(crate) use system_prompt::{ASK_USER_QUESTION_RULE, PLUGIN_SETUP_RULE, REPEATED_ACTION_RULE};
#[cfg(test)]
pub(crate) use turn_tail::TurnTail;

/// A turn's exchange-starter event that the CALLER already persisted, so
/// `process_message_with_steps_internal` must not emit its own
/// `MessageReceived`.
///
/// The variant is load-bearing, and it is a second fact rather than a
/// consequence of the first. "The event is already on the wire" says nothing
/// about whether the input is a message the person sent, yet a live thread
/// routes the two differently: a message is injected as `UserText` and
/// acknowledged with a `UserPromptInjected`, and it arms the Codex redirect
/// interrupt; an engine re-entry is injected as `ReentryFromEngine`, silently.
///
/// Modelling both as one nullable id made every reader infer the second from
/// the first (`pre_emitted_origin.is_none()` == "genuine user follow-up").
/// That held only because no pre-emitting caller could reach the follow-up
/// fast-paths: each one either starts a NEW thread or genuinely is an engine
/// re-entry. The moment a caller pre-emits a real message on a LIVE thread,
/// the inference misclassifies it as a re-entry, so the user's queued message is
/// ingested with no visible acknowledgment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreEmittedOrigin {
    /// A message from a person or an agent whose `MessageReceived` the caller
    /// persisted before dispatch: the chat API boundary's emit-before-ack, or
    /// the Thread Queue executor's eager child `MessageReceived`. Routes
    /// exactly like a message this function emitted itself.
    Message(Uuid),
    /// An engine-internal re-entry on an existing thread, anchored to an event
    /// the UI already renders: a child thread's completion re-opening its parent
    /// (`notify_parent_of_child_completion`), a continuation rerun's note
    /// (`chat::rerun`), or the re-processing of an orphaned injection. Not
    /// something the person just typed, so it must not surface as one.
    EngineReentry(Uuid),
    /// An **event-wait re-entry** (`engine::event_wait`). A re-entry like the one
    /// above, split out because a live thread has to inject it under its own
    /// name: the two are projected identically but come from different
    /// places, and folding a wait re-entry into `ReentryFromEngine` would put a
    /// child that does not exist into the log.
    ///
    /// Anchored to the `UserPromptInjected` `emit_resolution` wrote beside the
    /// resolution, which carries the payload as prose. One shape, always: a
    /// subscription does not hold its thread's turn, so there is never a
    /// dangling tool call for the delivery to land in instead.
    WaitReentry(Uuid),
}

impl PreEmittedOrigin {
    /// The `events.id` of the already-persisted event.
    pub(crate) fn event_id(self) -> Uuid {
        match self {
            PreEmittedOrigin::Message(id)
            | PreEmittedOrigin::EngineReentry(id)
            | PreEmittedOrigin::WaitReentry(id) => id,
        }
    }

    /// True for an engine-internal re-entry, false for a real message.
    pub(crate) fn is_engine_reentry(self) -> bool {
        matches!(
            self,
            PreEmittedOrigin::EngineReentry(_) | PreEmittedOrigin::WaitReentry(_)
        )
    }

    /// How a LIVE thread injects this re-entry. `Message` is not an engine
    /// re-entry at all, so it routes as ordinary user text.
    pub(crate) fn inject_kind(self) -> crate::engine::InjectedPromptKind {
        match self {
            PreEmittedOrigin::Message(_) => crate::engine::InjectedPromptKind::UserText,
            PreEmittedOrigin::EngineReentry(_) => {
                crate::engine::InjectedPromptKind::ReentryFromEngine
            }
            PreEmittedOrigin::WaitReentry(_) => crate::engine::InjectedPromptKind::ReentryFromWait,
        }
    }
}

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
        side_effect_grant: Vec<crate::engine::command_guard::SideEffectCategory>,
        model: Option<&str>,
        reasoning_effort: Option<&str>,
        external_cancel: Option<CancellationToken>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        // The trigger's own model / effort when it pinned one, else `None` so
        // the single canonical resolver in `process_message_with_steps_internal`
        // applies the account chat preferences. An explicit value here wins
        // there, which is what makes a pinned trigger a pin.
        self.process_message_with_steps_internal(
            prompt,
            model,
            Some(TriggerContext {
                trigger_id: trigger_id.to_string(),
                trigger_name: trigger_name.to_string(),
                slug: slug.to_string(),
                invocation,
                go_to_review,
                side_effect_grant,
            }),
            None, // app_context
            None, // file_context
            reasoning_effort,
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
            None,
            external_cancel,
            crate::engine::FollowUpUrgency::Normal,
        )
        .await
    }

    // Public chat entry — surface every per-request knob (model / context /
    // images / steps_tx / cancel) at the call boundary so triggers and HTTP
    // handlers can wire them through without re-creating a builder struct.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn process_message_with_steps(
        &self,
        user_message: &str,
        model_override: Option<&str>,
        app_context: Option<AppContext>,
        file_context: Option<String>,
        reasoning_effort: Option<&str>,
        images: Option<&[crate::api::ChatImage]>,
        device_id: Option<&str>,
        use_coding_agent: Option<bool>,
        event_id: Option<&str>,
        thread_id: Option<Uuid>,
        conflict_change_id: Option<Uuid>,
        repo_id: Option<&str>,
        url_context: Option<crate::api::UrlContext>,
        parent_thread_id: Option<Uuid>,
        spawning_event_id: Option<Uuid>,
        mode: ActorMode,
        cc_model: Option<&str>,
        coding_agent: Option<crate::runtime::CodingAgent>,
        pre_emitted_origin: Option<PreEmittedOrigin>,
        title: Option<&str>,
        origin: Option<MessageOrigin>,
        // `Normal` for every caller but a child follow-up the parent marked
        // urgent, which preempts the child's in-flight turn instead of queueing
        // behind it. Named rather than a bare `bool` so the call sites stay
        // readable in a column of `None`s.
        urgency: crate::engine::FollowUpUrgency,
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
            use_coding_agent,
            event_id,
            thread_id,
            conflict_change_id,
            repo_id,
            url_context,
            parent_thread_id,
            spawning_event_id,
            mode,
            cc_model,
            coding_agent,
            pre_emitted_origin,
            title,
            origin,
            None,
            urgency,
        )
        .await
    }

    pub(crate) fn clean_response(&self, content: &str) -> String {
        let content = content.trim();

        // Every arm is `starts_with`: this function REPLACES the whole assistant
        // message, and its output is what `agentic_loop/run.rs` persists as
        // `ResponseGenerated`. The `run_python({` arm used `contains`, so a turn
        // that merely QUOTED a `run_python({...})` call anywhere in its prose
        // (explaining the repeated-tool guard, say, which the chat system prompt
        // documents) had its entire real answer thrown away and replaced with the
        // canned line below. Only the raw-tool-echo shape this guard was written
        // for starts with the token.
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
#[path = "../process_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "history_tests.rs"]
mod history_tests;

#[cfg(test)]
#[path = "context_release_tests.rs"]
mod context_release_tests;
