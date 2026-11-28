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
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use super::consumption_claim::{
    held_message_at_receipt, held_message_receipt_target, HELD_MESSAGE_RECEIPT_TYPES,
};
use super::event_bus::{BusEvent, EmittedEvent, LagReplay, ReplayScope, SystemEvent};
use super::thread_events::{EventChannel, ThreadEvent};
use super::LucidosEngine;

/// One piece of indexing the consumer asks for.
#[derive(Debug, PartialEq)]
pub(crate) enum MemoryJob {
    /// A thread event's text, keyed by the event so "View source" finds it.
    Text {
        thread_id: Uuid,
        event_id: Uuid,
        text: String,
    },
    /// An artifact as it stood at one commit.
    Artifact { path: String, commit: String },
}

/// The event types that can carry a job.
fn memory_event_types() -> Vec<&'static str> {
    ThreadEvent::INDEXABLE_EVENT_TYPES
        .iter()
        .chain(
            HELD_MESSAGE_RECEIPT_TYPES
                .iter()
                .filter(|kind| !ThreadEvent::INDEXABLE_EVENT_TYPES.contains(kind)),
        )
        .copied()
        .chain(["ArtifactCreated", "ArtifactUpdated", "ArtifactImported"])
        .collect()
}

/// Spawn the memory indexer as a background task.
/// Returns a `JoinHandle` so the caller can observe panics if needed.
pub fn spawn(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    let (mut rx, mut replay) = LagReplay::subscribe(
        &engine.event_bus,
        ReplayScope::Workspace,
        Some(memory_event_types()),
    );
    tokio::spawn(async move {
        while let Some(jobs) = jobs_after(rx.recv().await, &mut replay, engine.pool()).await {
            for job in jobs {
                run(&engine, job).await;
            }
        }
        log_legacy!(@Memory, "Memory indexer stream ended");
    })
}

/// The jobs one read of the bus hands the indexer. `None` once the bus closes.
pub(crate) async fn jobs_after(
    received: Result<EmittedEvent, RecvError>,
    replay: &mut LagReplay,
    pool: &sqlx::PgPool,
) -> Option<Vec<MemoryJob>> {
    let frames = replay.read(received, pool).await?;
    let mut jobs = Vec::new();
    for frame in &frames {
        if let Some(job) = job(frame) {
            jobs.push(job);
        } else if let Some(job) = held_delivery_job(frame, pool).await {
            jobs.push(job);
        }
    }
    Some(jobs)
}

async fn held_delivery_job(emitted: &EmittedEvent, pool: &sqlx::PgPool) -> Option<MemoryJob> {
    let BusEvent::Thread {
        thread_id,
        event,
        meta,
    } = &emitted.typed
    else {
        return None;
    };
    let message = held_message_receipt_target(event, meta)?;
    let row = match held_message_at_receipt(pool, *thread_id, message, emitted.seq?).await {
        Ok(row) => row?,
        Err(error) => {
            log_legacy!(@Memory, "Could not load the source for a held message receipt: {error}");
            return None;
        }
    };
    let text = LucidosEngine::memory_content_for_event(&row)?;
    (!text.trim().is_empty()).then_some(MemoryJob::Text {
        thread_id: *thread_id,
        event_id: row.id,
        text,
    })
}

/// The job a frame carries, if any. Only a stored thread event is indexed,
/// and never one from a trigger thread, whose repetitive prompts would pollute
/// search.
fn job(emitted: &EmittedEvent) -> Option<MemoryJob> {
    match &emitted.typed {
        BusEvent::Thread {
            thread_id,
            event,
            meta,
        } => {
            emitted.seq?;
            if meta.channel == Some(EventChannel::Trigger) || meta.queued_for_permission {
                return None;
            }
            let text = event.indexable_text()?;
            if text.trim().is_empty() {
                return None;
            }
            Some(MemoryJob::Text {
                thread_id: *thread_id,
                event_id: emitted.event_id,
                text: text.to_string(),
            })
        }
        BusEvent::System(
            SystemEvent::ArtifactCreated {
                artifact_path,
                commit,
                ..
            }
            | SystemEvent::ArtifactUpdated {
                artifact_path,
                commit,
                ..
            }
            | SystemEvent::ArtifactImported {
                artifact_path,
                commit_hash: commit,
                ..
            },
        ) => Some(MemoryJob::Artifact {
            path: artifact_path.clone(),
            commit: commit.clone(),
        }),
        BusEvent::System(_) => None,
    }
}

async fn run(engine: &Arc<LucidosEngine>, job: MemoryJob) {
    match job {
        MemoryJob::Text {
            thread_id,
            event_id,
            text,
        } => {
            let ctx = engine
                .build_extraction_context_for_event(Some(thread_id), Some(event_id))
                .await;
            engine.index_text(&text, ctx.as_deref(), event_id).await;
        }
        MemoryJob::Artifact { path, commit } => index_artifact_event(engine, &path, &commit).await,
    }
}

/// Read the artifact at the given commit and index it.
/// Silently skips when the read fails — that covers missing files (event for a
/// path under a non-`artifacts/` data subdir, e.g. python tool writing to apps/)
/// and any binary content that can't be UTF-8 decoded (PDFs, images,
/// archives, …). Binaries get no memory index; their text is no longer
/// extracted server-side.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::EventBus;
    use crate::engine::thread_events::EventMeta;
    use crate::test_support::{
        flood_past_bus_capacity, message_received, seed_thread_event, setup_test_db,
        teardown_test_db,
    };

    /// A burst that lags the indexer must still be indexed, and an event it
    /// already indexed live must not be extracted a second time: each one is a
    /// billed model call.
    #[tokio::test]
    async fn a_lagged_burst_is_indexed_once() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let (mut rx, mut replay) =
            LagReplay::subscribe(&bus, ReplayScope::Workspace, Some(memory_event_types()));
        let thread_id = Uuid::new_v4();

        let live_id =
            seed_thread_event(&bus, thread_id, message_received("I live in Bergen")).await;
        let live = loop {
            let jobs = jobs_after(rx.recv().await, &mut replay, &pool)
                .await
                .unwrap();
            if !jobs.is_empty() {
                break jobs;
            }
        };
        assert_eq!(
            live,
            vec![MemoryJob::Text {
                thread_id,
                event_id: live_id,
                text: "I live in Bergen".into(),
            }]
        );

        let dropped_id =
            seed_thread_event(&bus, thread_id, message_received("I moved to Oslo")).await;
        bus.emit(BusEvent::System(SystemEvent::ArtifactCreated {
            artifact_path: "notes.md".into(),
            commit: "abc123".into(),
            source: None,
            writer_thread_id: None,
            writer: None, // Legacy event fixture.
        }))
        .await
        .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id: Uuid::new_v4(),
            event: message_received("a trigger's prompt"),
            meta: EventMeta {
                channel: Some(EventChannel::Trigger),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        flood_past_bus_capacity(&bus);

        let replayed = jobs_after(rx.recv().await, &mut replay, &pool)
            .await
            .expect("a lag must not end the indexer");
        assert_eq!(
            replayed,
            vec![
                MemoryJob::Text {
                    thread_id,
                    event_id: dropped_id,
                    text: "I moved to Oslo".into(),
                },
                MemoryJob::Artifact {
                    path: "notes.md".into(),
                    commit: "abc123".into(),
                },
            ]
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn held_delivery_vectors_use_the_original_event_only_after_the_receipt() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        for (lagged, coding_agent) in [(false, false), (true, false), (false, true), (true, true)] {
            let thread_id = Uuid::new_v4();
            let card = Uuid::new_v4().to_string();
            let request = if coding_agent {
                sqlx::query("INSERT INTO thread_summaries (thread_id, source) VALUES ($1, $2)")
                    .bind(thread_id)
                    .bind(crate::engine::thread_lifecycle::CODING_AGENT_SOURCE)
                    .execute(&pool)
                    .await
                    .unwrap();
                ThreadEvent::CodingAgentPermissionRequest {
                    request_id: card.clone(),
                    tool_use_id: "permission-tool".into(),
                    tool_name: "bash".into(),
                    input: serde_json::json!({"command": "pwd"}),
                    summary: "Read the working directory".into(),
                    grant_offer: None,
                }
            } else {
                ThreadEvent::CommandPermissionRequested {
                    request_id: card.clone(),
                    tool_use_id: "permission-tool".into(),
                    tool_name: "bash".into(),
                    command: "pwd".into(),
                    summary: "Read the working directory".into(),
                    decision_trace: None,
                    grant_head: None,
                    grant_offer: None,
                }
            };
            seed_thread_event(&bus, thread_id, request).await;
            let (mut rx, mut replay) =
                LagReplay::subscribe(&bus, ReplayScope::Workspace, Some(memory_event_types()));
            let held = bus
                .emit(BusEvent::Thread {
                    thread_id,
                    event: message_received("held text"),
                    meta: EventMeta {
                        queued_for_permission: true,
                        ..EventMeta::NONE
                    },
                })
                .await
                .unwrap()
                .unwrap()
                .event_id;

            let pending = jobs_after(rx.recv().await, &mut replay, &pool)
                .await
                .unwrap();
            assert!(pending.is_empty(), "{pending:?}");

            let answer = if coding_agent {
                ThreadEvent::CodingAgentPermissionResolved {
                    request_id: card,
                    allowed: false,
                    reason: None,
                    persist_scope: None,
                    standing_deny: None,
                }
            } else {
                ThreadEvent::CommandPermissionResolved {
                    request_id: card,
                    allowed: false,
                    reason: None,
                    persist_scope: None,
                    standing_deny: None,
                }
            };
            seed_thread_event(&bus, thread_id, answer).await;
            let closing = jobs_after(rx.recv().await, &mut replay, &pool)
                .await
                .unwrap();
            assert!(closing.is_empty(), "{closing:?}");

            let (event, meta) = if coding_agent {
                seed_thread_event(
                    &bus,
                    thread_id,
                    ThreadEvent::CodingAgentPromptSent {
                        text: "held text".into(),
                        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                        origin: None,
                        input_event_id: Some(held),
                    },
                )
                .await;
                let forwarded = jobs_after(rx.recv().await, &mut replay, &pool)
                    .await
                    .unwrap();
                assert!(forwarded.is_empty(), "{forwarded:?}");
                (
                    ThreadEvent::CodingAgentInputRead {
                        input_event_id: held,
                        started_turn: false,
                        verbatim: false,
                    },
                    EventMeta::NONE,
                )
            } else {
                (
                    ThreadEvent::ChatTurnSettingsResolved {
                        model: "test".into(),
                        reasoning_effort: None,
                        provider: None,
                    },
                    EventMeta {
                        request_event_id: Some(held),
                        ..EventMeta::NONE
                    },
                )
            };
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta,
            })
            .await
            .unwrap();
            if lagged {
                flood_past_bus_capacity(&bus);
            }
            let delivered = jobs_after(rx.recv().await, &mut replay, &pool)
                .await
                .unwrap();
            assert_eq!(
                delivered,
                [MemoryJob::Text {
                    thread_id,
                    event_id: held,
                    text: "held text".into(),
                }]
            );
        }
        teardown_test_db(&db).await;
    }

    /// The replay list gates live frames too, so an artifact event the
    /// indexer acts on but the list omits would never be indexed.
    #[test]
    fn every_artifact_event_the_indexer_acts_on_is_in_its_list() {
        let writes = [
            SystemEvent::ArtifactCreated {
                artifact_path: "notes.md".into(),
                commit: "abc123".into(),
                source: None,
                writer_thread_id: None,
                writer: None, // Legacy event fixture.
            },
            SystemEvent::ArtifactUpdated {
                artifact_path: "notes.md".into(),
                commit: "abc123".into(),
                source: None,
                writer_thread_id: None,
                writer: None, // Legacy event fixture.
            },
            SystemEvent::ArtifactImported {
                artifact_path: "notes.md".into(),
                source_type: "upload".into(),
                source_detail: String::new(),
                commit_hash: "abc123".into(),
                summary: None,
                writer_thread_id: None,
                writer: None, // Legacy event fixture.
            },
        ];
        for write in writes {
            let name = write.stored_event_type().to_string();
            let frame = EmittedEvent {
                event_id: Uuid::new_v4(),
                seq: Some(1),
                created: chrono::Utc::now(),
                typed: BusEvent::System(write),
                aggregate: None,
                depth: 0,
                emitting_trigger_id: None,
            };
            assert!(job(&frame).is_some(), "{name}");
            assert!(memory_event_types().contains(&name.as_str()), "{name}");
        }
    }
}
