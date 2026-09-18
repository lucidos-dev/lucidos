//! Title-emit orchestration for the chat flow: caller-provided title,
//! per-trigger "Run <name>" title, and async LLM title generation for the
//! first follow-up of a thread. Split out of
//! `process_message_with_steps_internal`; the actual title generation
//! primitives live in `super::super::title`.

use crate::engine::thread_events::EventMeta;
use crate::engine::LucidosEngine;
use uuid::Uuid;

use super::super::process_helpers::TriggerContext;
use super::super::title::{emit_generated_title, title_call};

impl LucidosEngine {
    /// Emit thread titles for this turn as needed: a caller-provided title
    /// short-circuits async generation; trigger threads get a "Run <name>"
    /// title; otherwise the first follow-up of a chat thread kicks off async
    /// LLM title generation.
    ///
    /// A thread somebody spoke on is the one exception, and it is named by
    /// `spawn_call_title_generation` instead. The two moments are the same, and
    /// only the input differs: one message here, the whole exchange there.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn maybe_emit_titles(
        &self,
        thread_id: Uuid,
        thread_id_str: &str,
        title: Option<&str>,
        is_trigger: bool,
        is_new_thread: bool,
        trigger: &Option<TriggerContext>,
        user_message: &str,
        user_images: Option<&[crate::api::ChatImage]>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
            if let Some(tc) = trigger {
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
            // A thread somebody SPOKE on is named from its whole exchange
            // rather than from this one message. A spoken sentence leans on
            // the one before it, so "Yeah, please check" is a whole request
            // and none of its subject. `spawn_call_title_generation` owns
            // that thread, and answers false for a thread nobody spoke on.
            if self.spawn_call_title_generation(thread_id).await {
                return Ok(());
            }
            // The same naming slot the call path takes. Two follow-ups a
            // second apart would otherwise both read "no title yet" and both
            // write one, because generating a title is a model call.
            let Some(slot) = self.claim_the_naming_of(thread_id) else {
                return Ok(());
            };
            // It's a follow-up — generate title if none exists yet
            let event_store = self.event_store.clone();
            if let Some(ref extractor) = self.extractor {
                match title_call(&self.pool, extractor).await {
                    Err(e) => {
                        log!("[Chat] Failed to build title provider for follow-up: {}", e);
                    }
                    Ok(call) => {
                        let msg = user_message.to_string();
                        let attached_images = user_images.map_or(0, |i| i.len());
                        let bus = self.event_bus.clone();
                        let tid_str = thread_id_str.to_string();
                        tokio::spawn(async move {
                            let _slot = slot;
                            match event_store.thread_has_title(&tid_str).await {
                                Ok(true) => {}
                                Ok(false) => {
                                    // Logged, like the sibling arm below. The
                                    // title still generates, just without the
                                    // image description.
                                    let image_desc = match event_store
                                        .get_thread_first_message(&tid_str)
                                        .await
                                    {
                                        Ok(found) => found.and_then(|(_, desc, _)| desc),
                                        Err(e) => {
                                            log!(
                                                "[Thread] First-message read failed for the title of {}: {}",
                                                tid_str,
                                                e
                                            );
                                            None
                                        }
                                    };
                                    emit_generated_title(
                                        &bus,
                                        &call,
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
            }
        }

        Ok(())
    }
}
