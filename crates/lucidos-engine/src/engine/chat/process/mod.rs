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

use crate::engine::thread_events::{ActorMode, MessageOrigin, TriggerInvocation};
use crate::engine::types::*;
use crate::engine::LucidosEngine;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::process_helpers::TriggerContext;

mod context_build;
mod context_sections;
mod history;
mod run;
mod system_prompt;
mod titles;

// Re-exported only for the colocated `process_tests` module, which reaches
// these via `super::`. Non-test code imports them from the child modules
// directly (run.rs uses `context_build::*`; system_prompt.rs uses the const
// locally), so the re-exports are `cfg(test)`-gated to avoid unused warnings.
#[cfg(test)]
pub(crate) use context_build::{build_capture_sections, build_loaded_knowhow_block};
#[cfg(test)]
pub(crate) use system_prompt::ASK_USER_QUESTION_RULE;

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
                side_effect_grant,
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
            None,
            external_cancel,
        )
        .await
    }

    // Public chat entry — surface every per-request knob (model / context /
    // images / steps_tx / cancel) at the call boundary so triggers and HTTP
    // handlers can wire them through without re-creating a builder struct.
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
        coding_agent: Option<crate::runtime::CodingAgent>,
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
            coding_agent,
            pre_emitted_origin,
            title,
            origin,
            None,
        )
        .await
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
#[path = "../process_tests.rs"]
mod tests;
