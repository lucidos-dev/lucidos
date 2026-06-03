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
        let has_explicit_title = explicit_title.is_some();
        let msg = prompt.to_string();
        let initial_title =
            explicit_title.unwrap_or_else(|| prompt.chars().take(60).collect::<String>());
        let title_engine = self.clone_arc();
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

            // Build the title provider here (in the async task) so it can
            // honor the `model_title` preference, matching the chat and CC
            // paths; an unset/empty preference falls back to the extractor's
            // default model.
            if !has_explicit_title {
                if let Some(ref extractor) = title_engine.extractor {
                    let title_model = crate::core::PreferenceStore::get(
                        &title_engine.pool,
                        crate::core::PREF_MODEL_TITLE,
                    )
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                    match extractor.provider_for_model(&title_model) {
                        Ok(provider) => {
                            // Spawned child threads carry no images on their first prompt.
                            emit_generated_title(
                                &bus,
                                &provider,
                                child_thread_id,
                                &msg,
                                None,
                                None,
                                0,
                            )
                            .await;
                        }
                        Err(e) => {
                            log!("[FanOut] Failed to build title provider: {}", e)
                        }
                    }
                }
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
