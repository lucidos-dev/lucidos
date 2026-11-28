use crate::engine::agent_session::spawn_gate::remote_start::spend_is_held;
use crate::engine::thread_events::EventMeta;
use crate::engine::LucidosEngine;
use uuid::Uuid;

use super::super::process_helpers::TriggerContext;
use super::super::title::{name_untitled_thread, title_call, TITLE_RETRY_DELAYS};

#[easy_ext::ext(TitlesExt)]
pub(crate) impl LucidosEngine {
    #[allow(clippy::too_many_arguments)]
    async fn maybe_emit_titles(
        &self,
        thread_id: Uuid,
        thread_id_str: &str,
        title: Option<&str>,
        is_trigger: bool,
        is_new_thread: bool,
        trigger: &Option<TriggerContext>,
        user_message: &str,
        user_images: Option<&[crate::engine::types::ChatImage]>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if crate::engine::home_thread::is_home_thread(&self.pool, thread_id).await? {
            return Ok(());
        }
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

        if !is_new_thread
            && !is_trigger
            && !has_caller_title
            && !spend_is_held(&self.pool, thread_id, None).await
        {
            let Some(slot) = self.claim_the_naming_of(thread_id) else {
                return Ok(());
            };
            let event_store = self.event_store.clone();
            let call = title_call(self).await;
            let msg = user_message.to_string();
            let attached_images = user_images.map_or(0, |i| i.len());
            let pool = self.pool.clone();
            let bus = self.event_bus.clone();
            let tid_str = thread_id_str.to_string();
            tokio::spawn(async move {
                // INVARIANT: Held through every retry, so a later follow-up cannot start
                // a second namer while this one waits.
                let _slot = slot;
                let image_desc = match event_store.get_thread_first_message(&tid_str).await {
                    Ok(found) => found.and_then(|(_, desc, _)| desc),
                    Err(e) => {
                        log_legacy!(
                            "[Thread] First-message read failed for the title of {}: {}",
                            tid_str,
                            e
                        );
                        None
                    }
                };
                name_untitled_thread(
                    &pool,
                    &bus,
                    &call,
                    thread_id,
                    &msg,
                    image_desc.as_deref(),
                    attached_images,
                    &TITLE_RETRY_DELAYS,
                )
                .await;
            });
        }

        Ok(())
    }
}
