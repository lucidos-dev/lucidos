//! EventBus consumer that indexes chat messages and artifacts into vector memory.
//!
//! Subscribes to:
//! - `BusEvent::Thread` events (MessageReceived, ResponseGenerated, etc.) —
//!   excluding trigger-channel events whose repetitive prompts/responses would
//!   pollute search results.
//! - `BusEvent::System(ArtifactCreated|Updated|Imported)` — every artifact
//!   write path emits one of these. Indexing here (one subscriber) instead of
//!   at each call site (multiple tools) closes the historical gap where
//!   `tools/python.rs` and `tools/email.rs` emitted artifact events without
//!   ever indexing them, so rebuild and live indexing diverged.

use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use super::event_bus::{BusEvent, SystemEvent};
use super::LucidosEngine;

/// Spawn the memory indexer as a background task.
/// Returns a `JoinHandle` so the caller can observe panics if needed.
pub fn spawn(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    let rx = engine.event_bus.subscribe();
    tokio::spawn(async move {
        let stream = BroadcastStream::new(rx);
        tokio::pin!(stream);

        while let Some(Ok(emitted)) = stream.next().await {
            match &emitted.typed {
                BusEvent::Thread {
                    thread_id,
                    event,
                    meta,
                } => {
                    // Only index persisted thread events (seq != None)
                    if emitted.seq.is_none() {
                        continue;
                    }
                    // Skip trigger-driven threads entirely
                    if meta.channel.as_ref()
                        == Some(&crate::engine::thread_events::EventChannel::Trigger)
                    {
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
                        .build_extraction_context_for_event(
                            Some(*thread_id),
                            Some(emitted.event_id),
                        )
                        .await;
                    engine.index_text(text, ctx.as_deref(), emitted.event_id).await;
                }
                BusEvent::System(SystemEvent::ArtifactCreated {
                    artifact_path,
                    commit,
                    ..
                })
                | BusEvent::System(SystemEvent::ArtifactUpdated {
                    artifact_path,
                    commit,
                    ..
                }) => {
                    index_artifact_event(&engine, artifact_path, commit).await;
                }
                BusEvent::System(SystemEvent::ArtifactImported {
                    artifact_path,
                    commit_hash,
                    ..
                }) => {
                    index_artifact_event(&engine, artifact_path, commit_hash).await;
                }
                _ => {}
            }
        }

        log!(@Memory, "Memory indexer stream ended");
    })
}

/// Read the artifact at the given commit and index it.
/// Silently skips when the read fails — that covers missing files (event for a
/// path under a non-`artifacts/` data subdir, e.g. python tool writing to apps/)
/// and PDFs whose binary content can't be UTF-8 decoded (`post_import_index`
/// indexes those out-of-band via the .txt sidecar).
///
/// The extension/path filter runs BEFORE the git read so binary writes
/// (images, archives, .so/.dll/.exe) don't pay for a full blob fetch under
/// `ArtifactManager`'s global `Mutex<Repository>` just to be discarded.
async fn index_artifact_event(engine: &Arc<LucidosEngine>, artifact_path: &str, commit: &str) {
    let canonical = LucidosEngine::canonicalize_artifact_path(artifact_path);
    if LucidosEngine::should_skip_artifact_for_memory(canonical) {
        return;
    }
    let Ok(content) = engine
        .artifact_manager
        .read_artifact_at_commit_string(canonical, commit)
    else {
        return;
    };
    let ctx = engine.extraction_context_base().await;
    engine
        .index_artifact_memory(canonical, &content, commit, chrono::Utc::now(), Some(&ctx))
        .await;
}
