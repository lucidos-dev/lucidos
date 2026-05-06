//! EventBus consumer that indexes chat messages into vector memory.
//!
//! Subscribes to the EventBus and indexes `MessageReceived` and
//! `ResponseGenerated` events. Events from trigger threads
//! (channel == "trigger") are skipped — their repetitive
//! prompts/responses would pollute search results.

use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use super::event_bus::BusEvent;
use super::LucidosEngine;

/// Spawn the memory indexer as a background task.
/// Returns a `JoinHandle` so the caller can observe panics if needed.
pub fn spawn(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    let rx = engine.event_bus.subscribe();
    tokio::spawn(async move {
        let stream = BroadcastStream::new(rx);
        tokio::pin!(stream);

        while let Some(Ok(emitted)) = stream.next().await {
            // Only index persisted thread events (seq != None)
            let Some(_seq) = emitted.seq else { continue };

            let BusEvent::Thread {
                thread_id,
                event,
                meta,
            } = &emitted.typed
            else {
                continue;
            };

            // Skip trigger-driven threads entirely
            if meta.channel.as_ref() == Some(&crate::engine::thread_events::EventChannel::Trigger) {
                continue;
            }

            // Extract indexable text (single source of truth on ThreadEvent)
            let Some(text) = event.indexable_text() else {
                continue;
            };
            if text.trim().is_empty() {
                continue;
            }

            let ctx = engine
                .build_extraction_context_for_event(Some(*thread_id), Some(emitted.event_id))
                .await;
            engine.index_text(text, ctx.as_deref(), emitted.event_id).await;
        }

        log!(@Memory, "Memory indexer stream ended");
    })
}
