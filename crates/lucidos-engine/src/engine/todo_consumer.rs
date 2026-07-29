//! EventBus consumer that garbage-collects abandoned Lucidos Agent todo lists.
//!
//! On every persisted chat-thread terminator (`ResponseGenerated` /
//! `ResponseCanceled` / `ResponseAborted` / `ResponseFailed` — the full
//! `TERMINATOR_EVENT_TYPES` set in `thread_events.rs`), calls
//! [`crate::engine::tools::todo::mark_abandoned_todos`] to enforce the agent's
//! contract: either keep working the list until every item is `completed`, or
//! call `todo_write` with `[]` to drop it. If the agent left non-completed
//! items behind, the helper emits a fresh `TodoListWritten` with those flipped
//! to `Abandoned` so the panel makes it obvious the work was dropped.
//! All-completed lists are left alone — finished lists persist by design.
//!
//! The helper takes the terminator's sequence number so the SELECT only
//! considers TodoListWritten rows that were persisted at-or-before the
//! terminator. Without that gate, a fresh todo_write from the next turn
//! could be picked up by a delayed consumer and clobbered by the abandon-flip.
//!
//! Coding-agent threads never emit `TodoListWritten` (CC has its own
//! `TodoWrite`), so the helper's `SELECT` short-circuits to `None` for them
//! and this consumer pays one indexed lookup per CC turn.

use std::sync::Arc;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use super::event_bus::{BusEvent, EmittedEvent, EventBus};
use super::thread_events::ThreadEvent;
use super::tools::todo::mark_abandoned_todos;
use super::LucidosEngine;

/// Process a single broadcast event: if it's a response-termination for a
/// thread, run the abandoned-todo cleanup. Exposed so tests can drive the
/// dispatch without spawning the background task.
pub async fn handle_event(bus: &EventBus, pool: &sqlx::PgPool, emitted: &EmittedEvent) {
    // Only react to persisted thread events; in-memory broadcasts (seq == None)
    // include transient streaming chunks we ignore.
    let Some(seq) = emitted.seq else {
        return;
    };
    let BusEvent::Thread {
        thread_id, event, ..
    } = &emitted.typed
    else {
        return;
    };
    // Match every chat terminator. Keep this list in sync with
    // `ThreadEvent::TERMINATOR_EVENT_TYPES` — `ResponseFailed` is in that set
    // (e.g. upstream LLM error, OOM-killed bash, empty assistant text on a
    // non-cancel turn) and must also clean up abandoned todos.
    if !matches!(
        event,
        ThreadEvent::ResponseGenerated { .. }
            | ThreadEvent::ResponseCanceled { .. }
            | ThreadEvent::ResponseAborted { .. }
            | ThreadEvent::ResponseFailed { .. }
    ) {
        return;
    }
    mark_abandoned_todos(bus, pool, *thread_id, seq).await;
}

/// Spawn the todo garbage collector as a background task.
/// Returns a `JoinHandle` so the caller can observe panics if needed.
pub fn spawn(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    let rx = engine.event_bus.subscribe();
    tokio::spawn(async move {
        let stream = BroadcastStream::new(rx);
        tokio::pin!(stream);

        // Loop body must survive both lag and unrelated stream hiccups.
        // `while let Some(Ok(_))` would exit the loop on `Some(Err(Lagged))`,
        // silently killing the consumer for the rest of the engine's lifetime.
        while let Some(result) = stream.next().await {
            match result {
                Ok(emitted) => {
                    handle_event(&engine.event_bus, engine.pool(), &emitted).await;
                }
                Err(BroadcastStreamRecvError::Lagged(n)) => {
                    crate::log!(
                        @Todo,
                        "Broadcast lagged by {} events — abandoned-todo cleanup may have been missed for those terminators; continuing",
                        n
                    );
                }
            }
        }

        crate::log!(@Todo, "Todo consumer stream ended");
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::thread_events::EventMeta;
    use crate::engine::tools::todo::todo_write_impl;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use serde_json::json;
    use tokio::sync::broadcast::Receiver;
    use uuid::Uuid;

    async fn setup() -> (EventBus, Receiver<EmittedEvent>, sqlx::PgPool, String) {
        let (pool, db) = setup_test_db().await;
        let (bus, _parent_rx) = EventBus::new(pool.clone());
        let rx = bus.subscribe();
        (bus, rx, pool, db)
    }

    /// Drain everything currently buffered in `rx` so a subsequent
    /// assertion sees a clean channel.
    fn drain(rx: &mut Receiver<EmittedEvent>) {
        while rx.try_recv().is_ok() {}
    }

    /// Pull the next persisted terminator broadcast out of the channel and
    /// run it through the handler. Keep the match list in sync with
    /// `handle_event` (all four terminators).
    async fn dispatch_next_terminator(
        bus: &EventBus,
        pool: &sqlx::PgPool,
        rx: &mut Receiver<EmittedEvent>,
        thread_id: Uuid,
    ) {
        loop {
            let ev = rx.recv().await.expect("broadcast channel should not close");
            let dispatch = if let BusEvent::Thread {
                thread_id: tid,
                event,
                ..
            } = &ev.typed
            {
                *tid == thread_id
                    && matches!(
                        event,
                        ThreadEvent::ResponseGenerated { .. }
                            | ThreadEvent::ResponseCanceled { .. }
                            | ThreadEvent::ResponseAborted { .. }
                            | ThreadEvent::ResponseFailed { .. }
                    )
            } else {
                false
            };
            if dispatch {
                handle_event(bus, pool, &ev).await;
                return;
            }
        }
    }

    async fn next_todo_items(rx: &mut Receiver<EmittedEvent>, thread_id: Uuid) -> Vec<super::super::thread_events::TodoItem> {
        loop {
            let ev = rx.recv().await.expect("broadcast channel should not close");
            if let BusEvent::Thread {
                thread_id: tid,
                event: ThreadEvent::TodoListWritten { items },
                ..
            } = ev.typed
            {
                if tid == thread_id {
                    return items;
                }
            }
        }
    }

    async fn seed_in_progress_list(bus: &EventBus, thread_id: Uuid) {
        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_write_impl(bus, &args, thread_id)
            .await
            .expect("seed write should succeed");
    }

    async fn emit_response_generated(bus: &EventBus, thread_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseGenerated {
                text: "done".to_string(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit response");
    }

    async fn emit_response_canceled(bus: &EventBus, pool: &sqlx::PgPool, thread_id: Uuid) {
        use crate::engine::thread_events::{emit_response_canceled, CancelCause};
        emit_response_canceled(
            bus,
            pool,
            thread_id,
            CancelCause::UserStop,
            String::new(),
            vec![],
            None,
            None,
            EventMeta::NONE,
            "[TodoConsumerTest] ResponseCanceled",
        )
        .await;
    }

    async fn emit_response_aborted(bus: &EventBus, thread_id: Uuid) {
        use crate::engine::thread_events::{emit_response_aborted, AbortCause};
        emit_response_aborted(
            bus,
            thread_id,
            AbortCause::EngineShutdown,
            String::new(),
            vec![],
            None,
            None,
            EventMeta::NONE,
            "[TodoConsumerTest] ResponseAborted",
        )
        .await;
    }

    #[tokio::test]
    async fn handle_event_marks_abandoned_on_response_generated() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_in_progress_list(&bus, thread_id).await;
        drain(&mut rx);

        emit_response_generated(&bus, thread_id).await;
        dispatch_next_terminator(&bus, &pool, &mut rx, thread_id).await;

        let flipped = next_todo_items(&mut rx, thread_id).await;
        assert_eq!(flipped.len(), 1, "list kept, got {:?}", flipped);
        assert_eq!(
            flipped[0].status,
            super::super::thread_events::TodoStatus::Abandoned,
            "in_progress flipped to abandoned, got {:?}",
            flipped[0].status,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn handle_event_marks_abandoned_on_response_canceled() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_in_progress_list(&bus, thread_id).await;
        drain(&mut rx);

        emit_response_canceled(&bus, &pool, thread_id).await;
        dispatch_next_terminator(&bus, &pool, &mut rx, thread_id).await;

        let flipped = next_todo_items(&mut rx, thread_id).await;
        assert_eq!(flipped.len(), 1, "list kept, got {:?}", flipped);
        assert_eq!(
            flipped[0].status,
            super::super::thread_events::TodoStatus::Abandoned,
            "in_progress flipped to abandoned, got {:?}",
            flipped[0].status,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn handle_event_marks_abandoned_on_response_aborted() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_in_progress_list(&bus, thread_id).await;
        drain(&mut rx);

        emit_response_aborted(&bus, thread_id).await;
        dispatch_next_terminator(&bus, &pool, &mut rx, thread_id).await;

        let flipped = next_todo_items(&mut rx, thread_id).await;
        assert_eq!(flipped.len(), 1, "list kept, got {:?}", flipped);
        assert_eq!(
            flipped[0].status,
            super::super::thread_events::TodoStatus::Abandoned,
            "in_progress flipped to abandoned, got {:?}",
            flipped[0].status,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn handle_event_marks_abandoned_on_response_failed() {
        // ResponseFailed is in TERMINATOR_EVENT_TYPES — chat turns that die
        // from upstream LLM errors, OOM-killed bash, or empty assistant text
        // must also trigger the cleanup. Without this, the most common
        // failure path leaves stale in_progress items forever.
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_in_progress_list(&bus, thread_id).await;
        drain(&mut rx);

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseFailed {
                error: "upstream 500".to_string(),
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit failed");
        dispatch_next_terminator(&bus, &pool, &mut rx, thread_id).await;

        let flipped = next_todo_items(&mut rx, thread_id).await;
        assert_eq!(flipped.len(), 1, "list kept, got {:?}", flipped);
        assert_eq!(
            flipped[0].status,
            super::super::thread_events::TodoStatus::Abandoned,
            "in_progress flipped to abandoned, got {:?}",
            flipped[0].status,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn handle_event_ignores_unrelated_events() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_in_progress_list(&bus, thread_id).await;
        drain(&mut rx);

        // Emit a MessageReceived — not a terminator, must not trigger cleanup.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                text: "hi".to_string(),
                user_image_hashes: vec![],
                device_id: None,
                device: None,
                image_description: None,
                mode: crate::engine::thread_events::ActorMode::Human,
                model: None,
                reasoning_effort: None,
                parent_thread_id: None,
                spawning_event_id: None,
                origin: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit message");

        // Drain just the MessageReceived; if a TodoListWritten cleanup
        // followed, we'd see it here and panic.
        use tokio::time::{timeout, Duration};
        let deadline = tokio::time::Instant::now() + Duration::from_millis(150);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match timeout(remaining, rx.recv()).await {
                Ok(Ok(ev)) => {
                    // Feed every event through handle_event — none should trigger cleanup.
                    handle_event(&bus, &pool, &ev).await;
                    if let BusEvent::Thread {
                        event: ThreadEvent::TodoListWritten { items },
                        thread_id: tid,
                        ..
                    } = ev.typed
                    {
                        if tid == thread_id {
                            panic!("unexpected cleanup, got {:?}", items);
                        }
                    }
                }
                _ => break,
            }
        }

        pool.close().await;
        teardown_test_db(&db).await;
    }
}
