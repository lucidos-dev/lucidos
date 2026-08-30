//! EventBus consumer that settles open Lucidos Agent todo lists.
//!
//! On every persisted chat-thread terminator (`ResponseGenerated` /
//! `ResponseCanceled` / `ResponseAborted` / `ResponseFailed`, the full
//! `TERMINATOR_EVENT_TYPES` set in `thread_events.rs`), calls
//! [`crate::engine::tools::todo::settle_open_todos`] to enforce the agent's
//! contract: either keep working the list until every item is `completed`, or
//! call `todo_write` with `[]` to drop it. If the agent left open items behind,
//! the helper emits a fresh `TodoListWritten` with those settled to `Waiting`
//! (the thread is parked on a live *event wait*) or to `Abandoned` (it walked
//! away), so the panel says which. All-completed lists are left alone, since
//! finished lists persist by design.
//!
//! The helper takes the terminator's sequence number so both of its questions
//! are answered as of the terminator rather than as of whenever this async
//! consumer got round to it: which `TodoListWritten` is the current list, and
//! whether the thread was still subscribed. Without that gate a fresh
//! todo_write from the next turn could be picked up by a delayed consumer and
//! clobbered, and a wait delivered in the meantime would make a parked thread
//! read as abandoned.
//!
//! **A terminator is not the only moment a thread stops being parked**, which
//! is why `EventWaitCanceled` is the second trigger. A delivery and an expiry
//! each write a `UserPromptInjected` re-entry anchor, so the re-entered turn's own
//! terminator settles the list; a cancel is "the one resolution that re-enters nothing
//! behind it" (`event_wait::emit_cancel`), so on an idle thread there is never
//! a next terminator and a `Waiting` list reads parked forever. That stranding
//! is what `docs/plans/2026-08-11-a-canceled-subscription-settles-the-todo-list.md`
//! closes, and it is the one hole in the "`Waiting` cannot strand" guarantee
//! the 2026-08-09 split claimed.
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
use super::thread_lifecycle::ThreadStatus;
use super::tools::todo::settle_open_todos;
use super::LucidosEngine;
use uuid::Uuid;

/// Process a single broadcast event: if it's a response-termination or a
/// canceled subscription on a thread, run the open-todo settle. Exposed so
/// tests can drive the dispatch without spawning the background task.
///
/// `holds_background_work` is read by the caller rather than here, which is
/// what keeps this function free of the engine handle and lets a test drive
/// both sides of the branch. See the settle for why an unfinished background
/// task counts as parked.
pub async fn handle_event(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    emitted: &EmittedEvent,
    holds_background_work: bool,
) {
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
    match event {
        // Every chat terminator. Keep this list in sync with
        // `ThreadEvent::TERMINATOR_EVENT_TYPES`: `ResponseFailed` is in that
        // set (e.g. upstream LLM error, OOM-killed bash, empty assistant text
        // on a non-cancel turn) and must also settle open todos.
        ThreadEvent::ResponseGenerated { .. }
        | ThreadEvent::ResponseCanceled { .. }
        | ThreadEvent::ResponseAborted { .. }
        | ThreadEvent::ResponseFailed { .. } => {
            settle_open_todos(bus, pool, *thread_id, seq, holds_background_work).await;
        }
        // The unpark that no turn follows. Deliberately the ONLY `EventWait*`
        // arm: a delivery and an expiry each write a `UserPromptInjected` re-entry
        // anchor, so the re-entered turn's terminator settles the list, and handling
        // them here would race it and could write terminal `Abandoned`
        // over a list the agent is picking back up.
        ThreadEvent::EventWaitCanceled { .. } => {
            settle_after_cancel(bus, pool, *thread_id, seq).await;
        }
        _ => {}
    }
}

/// Settle the list after a subscription was canceled, unless a turn still owns
/// it.
///
/// `holds_background_work: false` is not a shortcut, it is the correct answer
/// on this path. That override exists purely to cover the arm-after-terminator
/// race: the chat turn tail arms its background-task wait AFTER the loop
/// emitted the terminator, so `thread_held_event_wait`'s anti-join cannot see
/// it. A cancel is by definition later than the arming it resolves, so the
/// anti-join is authoritative here. And a background task nothing is
/// subscribed to will never re-open the thread, so a thread holding one is not
/// parked. Reading the registry here would strand the list in exactly the
/// reported case, where the watched task was still running when the user
/// pressed **Stop waiting**.
async fn settle_after_cancel(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    cancel_seq: i64,
) {
    match turn_in_flight(pool, thread_id).await {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            // Same rule the settle already applies to its own two queries: a
            // probe that could not run settles nothing, because `Abandoned` is
            // terminal and a wrong guess here cannot be walked back.
            crate::log!(
                @Todo,
                "Could not resolve the turn state for thread {} after a canceled subscription: {} (leaving the list untouched)",
                thread_id,
                e
            );
            return;
        }
    }
    settle_open_todos(bus, pool, thread_id, cancel_seq, false).await;
}

/// Does a turn still own this thread's todo list?
///
/// True while a turn is live or promised, which is when the settle must keep
/// its hands off: the agent can still finish the list itself, and `Abandoned`
/// is terminal, so settling under a running agent writes a verdict its own
/// terminator can no longer correct (the terminator finds no open item left).
/// The `AgentStandDown` cancel cause is the live case, since the agent retires
/// a watch from inside a turn.
///
/// Matched exhaustively rather than through a set-membership test, so a
/// seventh `ThreadStatus` is a compile error here instead of silently taking
/// the "no turn" side. A missing row is a definite no, not a failure.
async fn turn_in_flight(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await?;
    let Some(status) = status else {
        return Ok(false);
    };
    Ok(match ThreadStatus::parse(&status) {
        ThreadStatus::Running | ThreadStatus::WaitingForUserAnswer | ThreadStatus::Paused => true,
        ThreadStatus::Idle | ThreadStatus::Waiting | ThreadStatus::Failed => false,
    })
}

/// Spawn the todo settle consumer as a background task.
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
                    // Read at TERMINATOR time, which is the instant the
                    // question is about, and from the in-memory registry, so
                    // it cannot race the turn tail's arming the way an events
                    // query does.
                    //
                    // `seq.is_some()` mirrors `handle_event`'s own first guard,
                    // and keeping the two in sync is the point: without it this
                    // takes the registry lock and scans every background task
                    // for each event of the per-token streaming firehose, whose
                    // broadcasts are transient and which `handle_event` drops on
                    // its first line without ever reading this value.
                    let holds_background_work = match &emitted.typed {
                        BusEvent::Thread { thread_id, .. } if emitted.seq.is_some() => {
                            engine
                                .bash_background
                                .has_running_for_thread(*thread_id)
                                .await
                        }
                        _ => false,
                    };
                    handle_event(
                        &engine.event_bus,
                        engine.pool(),
                        &emitted,
                        holds_background_work,
                    )
                    .await;
                }
                Err(BroadcastStreamRecvError::Lagged(n)) => {
                    crate::log!(
                        @Todo,
                        "Broadcast lagged by {} events: the open-todo settle may have been missed for those terminators; continuing",
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
    use crate::core::event_subscription::EventSubscription;
    use crate::engine::thread_events::{EventMeta, EventWaitCancelCause, TodoStatus};
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
        dispatch_next_terminator_with_background(bus, pool, rx, thread_id, false).await
    }

    async fn dispatch_next_terminator_with_background(
        bus: &EventBus,
        pool: &sqlx::PgPool,
        rx: &mut Receiver<EmittedEvent>,
        thread_id: Uuid,
        holds_background_work: bool,
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
                handle_event(bus, pool, &ev, holds_background_work).await;
                return;
            }
        }
    }

    /// Block until the thread's next settle arrives, and **panic rather than
    /// hang** if it never does. The whole point of every test here is that a
    /// settle either happens or does not, so "no settle" is the most likely
    /// failure, and an unbounded `recv` would turn it into a wedged suite that
    /// reports nothing.
    async fn next_todo_items(
        rx: &mut Receiver<EmittedEvent>,
        thread_id: Uuid,
    ) -> Vec<super::super::thread_events::TodoItem> {
        let found = tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
            loop {
                let ev = rx.recv().await.expect("broadcast channel should not close");
                if let BusEvent::Thread {
                    thread_id: tid,
                    event: ThreadEvent::TodoListWritten { items, .. },
                    ..
                } = ev.typed
                {
                    if tid == thread_id {
                        return items;
                    }
                }
            }
        })
        .await;
        found.expect("expected a TodoListWritten settle for the thread, none arrived")
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

    /// The one subscription shape every wait helper below uses. The wait's
    /// content is irrelevant to the settle; only whether it is live is.
    fn watching() -> Vec<EventSubscription> {
        vec![EventSubscription {
            event_type: "ChangeProposed".into(),
            condition: None,
        }]
    }

    async fn emit_wait_started(bus: &EventBus, thread_id: Uuid, wait_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::EventWaitStarted {
                wait_id,
                tool_use_id: format!("toolu_{wait_id}"),
                on: watching(),
                reason: "watching for the change to land".into(),
                armed_at: chrono::Utc::now(),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                watermark: 0,
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit EventWaitStarted");
    }

    async fn emit_wait_canceled(bus: &EventBus, thread_id: Uuid, wait_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::EventWaitCanceled {
                wait_id,
                cause: EventWaitCancelCause::UserStop,
                on: watching(),
                reason: "watching for the change to land".into(),
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit EventWaitCanceled");
    }

    /// Pull the next `EventWaitCanceled` for the thread out of the channel and
    /// run it through the handler.
    ///
    /// `holds_background_work: true` is deliberate on every call. It is the
    /// terminator path's input and must never reach the cancel path, so passing
    /// the value that WOULD strand the list is what pins that (see
    /// `settle_after_cancel`: a background task nothing is subscribed to cannot
    /// re-open the thread, so it does not park it).
    async fn dispatch_next_cancel(
        bus: &EventBus,
        pool: &sqlx::PgPool,
        rx: &mut Receiver<EmittedEvent>,
        thread_id: Uuid,
    ) {
        loop {
            let ev = rx.recv().await.expect("broadcast channel should not close");
            if let BusEvent::Thread {
                thread_id: tid,
                event: ThreadEvent::EventWaitCanceled { .. },
                ..
            } = &ev.typed
            {
                if *tid == thread_id {
                    handle_event(bus, pool, &ev, true).await;
                    return;
                }
            }
        }
    }

    /// Feed every event for a short window through the handler and fail if any
    /// of them settles the thread's list. Covers both "this event type is not a
    /// trigger" and "this trigger declined to write".
    async fn assert_nothing_settles(
        bus: &EventBus,
        pool: &sqlx::PgPool,
        rx: &mut Receiver<EmittedEvent>,
        thread_id: Uuid,
    ) {
        use tokio::time::{timeout, Duration};
        let deadline = tokio::time::Instant::now() + Duration::from_millis(150);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return;
            }
            match timeout(remaining, rx.recv()).await {
                Ok(Ok(ev)) => {
                    handle_event(bus, pool, &ev, false).await;
                    if let BusEvent::Thread {
                        thread_id: tid,
                        event: ThreadEvent::TodoListWritten { items, .. },
                        ..
                    } = &ev.typed
                    {
                        if *tid == thread_id {
                            panic!("unexpected settle, got {:?}", items);
                        }
                    }
                }
                _ => return,
            }
        }
    }

    /// Park the thread the way the reported bug did: an in-progress list, one
    /// live wait, and a terminator that settles the open item to `Waiting`.
    /// Returns with the channel drained past that settle.
    async fn park_on_a_wait(
        bus: &EventBus,
        pool: &sqlx::PgPool,
        rx: &mut Receiver<EmittedEvent>,
        thread_id: Uuid,
        wait_ids: &[Uuid],
    ) {
        seed_in_progress_list(bus, thread_id).await;
        for wait_id in wait_ids {
            emit_wait_started(bus, thread_id, *wait_id).await;
        }
        drain(rx);

        emit_response_generated(bus, thread_id).await;
        dispatch_next_terminator(bus, pool, rx, thread_id).await;
        let parked = next_todo_items(rx, thread_id).await;
        assert_eq!(
            parked[0].status,
            TodoStatus::Waiting,
            "precondition: the terminator parks the list, got {:?}",
            parked[0].status,
        );
    }

    async fn seed_thread_status(pool: &sqlx::PgPool, thread_id: Uuid, status: &str) {
        sqlx::query(
            "INSERT INTO thread_summaries (thread_id, status) VALUES ($1, $2) \
             ON CONFLICT (thread_id) DO UPDATE SET status = EXCLUDED.status",
        )
        .bind(thread_id)
        .bind(status)
        .execute(pool)
        .await
        .expect("seed thread_summaries row");
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

    /// The engine-armed background-task wait is INVISIBLE to the anti-join:
    /// the chat turn tail arms it after the loop has already emitted this
    /// terminator, so its `EventWaitStarted` always sequences above the
    /// terminator, and this consumer can run before the arming happens at all.
    /// Without the registry read the list would settle `Abandoned` while the
    /// waiting indicator says the thread is watching a build, and
    /// `Abandoned` is terminal. That is the reported bug arriving through the
    /// fix for it.
    #[tokio::test]
    async fn handle_event_settles_to_waiting_when_the_thread_holds_background_work() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_in_progress_list(&bus, thread_id).await;
        // No EventWaitStarted at all: the wait does not exist yet at terminator
        // time, which is precisely the window this covers.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseGenerated {
                text: "Phase A is building, I will report when it lands".into(),
                model: None,
                reasoning_effort: None,
                images: vec![],
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit failed");
        dispatch_next_terminator_with_background(&bus, &pool, &mut rx, thread_id, true).await;

        let flipped = next_todo_items(&mut rx, thread_id).await;
        assert_eq!(flipped.len(), 1, "list kept, got {:?}", flipped);
        assert_eq!(
            flipped[0].status,
            super::super::thread_events::TodoStatus::Waiting,
            "a thread owning a running background task is parked, not abandoned; got {:?}",
            flipped[0].status,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn handle_event_settles_to_waiting_when_the_thread_holds_a_live_event_wait() {
        // The end-to-end shape of the reported bug, driven through the consumer
        // rather than the helper: the agent registered a subscription, said so,
        // and ended its turn. The terminator that means "walked away" for every
        // other turn means "parked" for this one.
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_in_progress_list(&bus, thread_id).await;
        emit_wait_started(&bus, thread_id, Uuid::new_v4()).await;
        drain(&mut rx);

        emit_response_generated(&bus, thread_id).await;
        dispatch_next_terminator(&bus, &pool, &mut rx, thread_id).await;

        let settled = next_todo_items(&mut rx, thread_id).await;
        assert_eq!(settled.len(), 1, "list kept, got {:?}", settled);
        assert_eq!(
            settled[0].status,
            super::super::thread_events::TodoStatus::Waiting,
            "in_progress parked rather than abandoned, got {:?}",
            settled[0].status,
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
                voice_session_id: None,
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
        assert_nothing_settles(&bus, &pool, &mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// The reported bug, end to end. **Stop waiting** on the last subscription
    /// is the one unpark no turn follows, so if the consumer ignores it the
    /// list reads `waiting` for the rest of the thread's life: every later
    /// terminator would find a parked thread and re-settle to the same status,
    /// and here there are no later terminators at all.
    #[tokio::test]
    async fn a_canceled_last_subscription_settles_the_list_to_abandoned() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();
        let wait_id = Uuid::new_v4();

        park_on_a_wait(&bus, &pool, &mut rx, thread_id, &[wait_id]).await;

        emit_wait_canceled(&bus, thread_id, wait_id).await;
        dispatch_next_cancel(&bus, &pool, &mut rx, thread_id).await;

        let settled = next_todo_items(&mut rx, thread_id).await;
        assert_eq!(settled.len(), 1, "list kept, got {:?}", settled);
        assert_eq!(
            settled[0].status,
            TodoStatus::Abandoned,
            "nothing will re-open the thread now, so the item is abandoned; got {:?}",
            settled[0].status,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// Two invariants in one setup, because they are the two halves of the same
    /// cascade: a cancel that leaves ANOTHER live wait must change nothing (or
    /// the 2026-08-09 bug returns, a still-parked thread reading abandoned),
    /// and archiving a thread holding N subscriptions must not write N
    /// near-identical lists into the transcript.
    ///
    /// Both fall out of the sequence-scoped anti-join plus `settle_to`'s
    /// already-settled short-circuit, with no counting anywhere: at cancel k the
    /// waits above it are still unresolved, so the target is `Waiting` again.
    #[tokio::test]
    async fn a_cascade_of_cancels_settles_once_when_the_last_wait_goes() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();
        let first = Uuid::new_v4();
        let last = Uuid::new_v4();

        park_on_a_wait(&bus, &pool, &mut rx, thread_id, &[first, last]).await;

        emit_wait_canceled(&bus, thread_id, first).await;
        dispatch_next_cancel(&bus, &pool, &mut rx, thread_id).await;
        assert_nothing_settles(&bus, &pool, &mut rx, thread_id).await;

        emit_wait_canceled(&bus, thread_id, last).await;
        dispatch_next_cancel(&bus, &pool, &mut rx, thread_id).await;

        let settled = next_todo_items(&mut rx, thread_id).await;
        assert_eq!(
            settled[0].status,
            TodoStatus::Abandoned,
            "the last cancel settles, got {:?}",
            settled[0].status,
        );
        assert_nothing_settles(&bus, &pool, &mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// A turn that is live or promised still owns the list, and `Abandoned` is
    /// terminal: settling under it writes a verdict the turn's own terminator
    /// can no longer correct, because by then no item is open. The live case is
    /// `AgentStandDown`, where the agent retires its own watch mid-turn.
    async fn assert_a_turn_owning_the_list_blocks_the_settle(status: &str) {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();
        let wait_id = Uuid::new_v4();

        park_on_a_wait(&bus, &pool, &mut rx, thread_id, &[wait_id]).await;
        seed_thread_status(&pool, thread_id, status).await;

        emit_wait_canceled(&bus, thread_id, wait_id).await;
        dispatch_next_cancel(&bus, &pool, &mut rx, thread_id).await;
        assert_nothing_settles(&bus, &pool, &mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_cancel_settles_nothing_while_the_thread_is_running() {
        assert_a_turn_owning_the_list_blocks_the_settle("running").await;
    }

    #[tokio::test]
    async fn a_cancel_settles_nothing_while_the_thread_awaits_an_answer() {
        assert_a_turn_owning_the_list_blocks_the_settle("waiting_for_user_answer").await;
    }

    #[tokio::test]
    async fn a_cancel_settles_nothing_while_the_thread_is_paused() {
        assert_a_turn_owning_the_list_blocks_the_settle("paused").await;
    }

    /// A delivery and an expiry each write a `UserPromptInjected` re-entry anchor,
    /// so the re-entered turn's own terminator settles the list. Handling them
    /// here would race it and could stamp terminal `Abandoned` over a list the
    /// agent is in the middle of picking back up.
    #[tokio::test]
    async fn a_delivered_or_expired_wait_is_left_to_the_turn_it_wakes() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();
        let delivered = Uuid::new_v4();
        let expired = Uuid::new_v4();

        park_on_a_wait(&bus, &pool, &mut rx, thread_id, &[delivered, expired]).await;

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::EventWaitDelivered {
                wait_id: delivered,
                event_id: Uuid::new_v4(),
                event_type: "ChangeProposed".into(),
                payload: json!({}),
                matched_index: 0,
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit EventWaitDelivered");
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::EventWaitExpired { wait_id: expired },
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit EventWaitExpired");

        assert_nothing_settles(&bus, &pool, &mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// A probe that could not run settles nothing, the same rule
    /// `settle_open_todos` already applies to its own two queries. Guessing is
    /// not available here: `Abandoned` is terminal, so a wrong guess cannot be
    /// walked back, and the thread's next terminator asks again.
    #[tokio::test]
    async fn a_failed_turn_state_probe_settles_nothing() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();
        let wait_id = Uuid::new_v4();

        park_on_a_wait(&bus, &pool, &mut rx, thread_id, &[wait_id]).await;
        emit_wait_canceled(&bus, thread_id, wait_id).await;

        // Capture the cancel before the pool goes, so the handler runs against a
        // real event and a dead pool: the shape of a connection lost between the
        // broadcast and this async consumer's first query.
        let cancel = loop {
            let ev = rx.recv().await.expect("broadcast channel should not close");
            if matches!(
                &ev.typed,
                BusEvent::Thread {
                    event: ThreadEvent::EventWaitCanceled { .. },
                    ..
                }
            ) {
                break ev;
            }
        };
        pool.close().await;

        handle_event(&bus, &pool, &cancel, false).await;
        assert_nothing_settles(&bus, &pool, &mut rx, thread_id).await;

        teardown_test_db(&db).await;
    }
}
