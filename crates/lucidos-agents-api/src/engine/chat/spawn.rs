use crate::engine::chat::process::ChatProcessExt;
use uuid::Uuid;

use super::PreEmittedOrigin;
use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::engine::LucidosEngine;

#[easy_ext::ext(ChatSpawnExt)]
pub(crate) impl LucidosEngine {
    #[allow(clippy::too_many_arguments)]
    fn spawn_thread(
        &self,
        prompt: &str,
        parent_thread_id: Option<Uuid>,
        spawning_event_id: Option<Uuid>,
        wake_mode: Option<crate::engine::thread_events::WakeMode>,
        child_thread_id: Uuid,
        pre_emitted_origin: Option<Uuid>,
        caller_title: Option<&str>,
        route: crate::core::ResolvedModelSelection,
        origin: Option<MessageOrigin>,
        hook_args: crate::core::event_hooks::HookArgs,
    ) -> Result<(Uuid, tokio::task::JoinHandle<()>), Box<dyn std::error::Error + Send + Sync>> {
        let caller_title = super::spawn_naming(caller_title, prompt).caller_title;

        let mode = MessageOrigin::mode_or(origin.as_ref(), ActorMode::Agent);
        let engine = self.clone_arc();
        let prompt_owned = prompt.to_string();
        let depth = crate::scheduler::user_tasks::current_event_trigger_depth();
        let handle = tokio::spawn(crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH.scope(
            depth,
            async move {
                if let Err(e) = engine
                    .process_message_with_steps(
                        &prompt_owned,
                        route.model.as_deref(),
                        None,
                        None,
                        route.reasoning_effort.as_deref(),
                        route.provider.as_deref(),
                        None,
                        None,
                        None,
                        None,
                        Some(child_thread_id),
                        None,
                        None,
                        None,
                        parent_thread_id,
                        spawning_event_id,
                        wake_mode,
                        mode,
                        None,
                        None,
                        pre_emitted_origin.map(PreEmittedOrigin::Message),
                        caller_title.as_deref(),
                        origin,
                        None,
                        crate::engine::FollowUpUrgency::Normal,
                        None,
                        Some(&hook_args),
                    )
                    .await
                {
                    log_legacy!("[FanOut] Child thread {} failed: {}", child_thread_id, e);
                }
            },
        ));

        Ok((child_thread_id, handle))
    }
}

#[cfg(test)]
mod tests {
    const SPAWN_SRC: &str = include_str!("spawn.rs");
    const EXECUTOR_SRC: &str =
        include_str!("../../../../lucidos-engine/src/engine/thread_queue/executor.rs");

    #[test]
    fn the_fallback_emit_stamps_the_mode_the_eager_emit_does() {
        let call = concat!(
            "MessageOrigin::mode_or(origin.as_ref(), ",
            "ActorMode::Agent)"
        );
        assert!(EXECUTOR_SRC.contains(call), "the eager emit's mode");
        assert!(SPAWN_SRC.contains(call), "the spawn fallback's mode");
    }

    #[test]
    fn the_spawned_turn_runs_under_the_callers_event_trigger_depth() {
        let read = concat!(
            "let depth = crate::scheduler::user_tasks::",
            "current_event_trigger_depth();"
        );
        let scope = concat!("EVENT_TRIGGER_DEPTH", ".scope(\n            depth,");
        assert!(
            SPAWN_SRC.contains(read),
            "the depth is read before the spawn"
        );
        assert!(
            SPAWN_SRC.contains(scope),
            "the spawned task is scoped to it"
        );
    }
}
