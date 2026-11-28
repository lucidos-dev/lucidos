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
pub(crate) mod workspace_prompt_footprint;

pub(crate) use run::resolve_available_route_overrides;

#[cfg(test)]
pub(crate) use context_build::{
    assemble_turn_text, build_capture_sections, build_knowhow_retention_block,
    build_loaded_knowhow_block, TurnSections,
};
#[cfg(test)]
pub(crate) use system_prompt::{ASK_USER_QUESTION_RULE, PLUGIN_SETUP_RULE, REPEATED_ACTION_RULE};
#[cfg(test)]
pub(crate) use turn_tail::TurnTail;

pub(crate) enum OrphanLane {
    LucidosAgent(Option<crate::engine::PreAdmission>),
    CodingAgent(Vec<crate::engine::AgentUserInput>),
}

#[easy_ext::ext(ChatProcessExt)]
pub(crate) impl LucidosEngine {
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
        provider: Option<&str>,
        queue_entry_id: Uuid,
        retry_of: Option<crate::engine::thread_events::RetryOf>,
        target: FireTarget,
        actor_email: Option<String>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
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
        // INVARIANT: The trigger's own model when it pinned one, else the account's
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
                None,
                None,
                reasoning_effort,
                provider,
                None,
                None,
                None,
                None,
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
                None,
            )
            .await?;
        // INVARIANT: A continuing fire may coalesce into a live turn. Then the thread's
        // last `TriggerStarted` belongs to an earlier fire.
        match thread_id {
            None => trigger_process_result(&self.pool, result).await,
            Some(_) => Ok(result),
        }
    }

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
            None,
            None,
            config.reasoning_effort.as_deref(),
            config.provider.as_deref(),
            None,
            None,
            None,
            None,
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
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_message_with_steps(
        &self,
        user_message: &str,
        model_override: Option<&str>,
        app_context: Option<AppContext>,
        file_context: Option<String>,
        reasoning_effort: Option<&str>,
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
        hook_args: Option<&crate::core::event_hooks::HookArgs>,
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
            hook_args,
        )
        .await
    }

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
            None,
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
#[path = "hook_refused_tests.rs"]
mod hook_refused_tests;

#[cfg(test)]
#[path = "blocked_run_tests.rs"]
mod blocked_run_tests;
