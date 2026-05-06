use uuid::Uuid;

use super::title::emit_generated_title;
use crate::engine::thread_events::ActorMode;
use crate::engine::LucidosEngine;

impl LucidosEngine {
    /// Spawn a regular Lucidos thread as a child of the given parent.
    ///
    /// Non-async to avoid Send issues with `&self` across await points in
    /// nested spawn contexts.
    ///
    /// `caller_title` — Some(non-empty) is used as the title and skips LLM
    /// title generation; None falls back to a truncated-prompt placeholder
    /// followed by an LLM-generated replacement.
    /// `pre_emitted_origin` — Some skips re-emitting MessageReceived (the
    /// caller already emitted it and incremented active_children_count).
    /// `model` / `reasoning_effort` — chat-mode prefs to inherit; `None` falls
    /// through to the engine's `LUCIDOS_MODEL` env default.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_thread(
        &self,
        prompt: &str,
        parent_thread_id: Option<Uuid>,
        spawning_event_id: Option<Uuid>,
        child_thread_id: Uuid,
        pre_emitted_origin: Option<Uuid>,
        caller_title: Option<&str>,
        model: Option<String>,
        reasoning_effort: Option<String>,
    ) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let explicit_title = caller_title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);

        let bus = self.event_bus.clone();
        let title_provider = match &explicit_title {
            Some(_) => None,
            None => self
                .extractor
                .as_ref()
                .map(|ext| ext.provider_for_model("")),
        };
        let msg = prompt.to_string();
        let initial_title =
            explicit_title.unwrap_or_else(|| prompt.chars().take(60).collect::<String>());
        tokio::spawn(async move {
            if let Err(e) = bus
                .emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id: child_thread_id,
                    event: crate::engine::thread_events::ThreadEvent::ThreadTitleGenerated {
                        title: initial_title,
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                })
                .await
            {
                log!("[FanOut] Failed to emit title: {}", e);
            }

            if let Some(provider) = title_provider {
                emit_generated_title(&bus, &provider, child_thread_id, &msg, None, None).await;
            }
        });

        let engine = self.clone_arc();
        let prompt_owned = prompt.to_string();
        tokio::spawn(async move {
            if let Err(e) = engine
                .process_message_with_steps(
                    &prompt_owned,
                    model.as_deref(),
                    None,
                    None,
                    reasoning_effort.as_deref(),
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
                    ActorMode::Agent,
                    None,
                    pre_emitted_origin,
                    None,
                    None,
                )
                .await
            {
                log!("[FanOut] Child thread {} failed: {}", child_thread_id, e);
            }
        });

        Ok(child_thread_id)
    }
}
