//! EventBus consumer that settles open Lucidos Agent todo lists.
//!
//! On every persisted chat-thread terminator (`ResponseGenerated` /
//! `ResponseCanceled` / `ResponseAborted` / `ResponseFailed`, the full
//! `TERMINATOR_EVENT_TYPES` set in `thread_events/event_impl.rs`), calls
//! [`crate::engine::tools::todo::settle_open_todos`] to enforce the agent's
//! contract: either keep working the list until every item is `completed`, or
//! call `todo_write` with `[]` to drop it. If the agent left open items behind,
//! the helper emits a fresh `TodoListWritten` with those settled to `Waiting`
//! (the thread is parked on a live *event wait* or a live child) or to
//! `Abandoned` (it walked away), so the panel says which. All-completed lists are left alone, since
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
//! each write a `PromptInjected` re-entry anchor, so the re-entered turn's own
//! terminator settles the list; a cancel is "the one resolution that re-enters nothing
//! behind it" (`event_wait::emit_cancel`), so on an idle thread there is never
//! a next terminator and a `Waiting` list reads parked forever. That stranding
//! is what `docs/plans/2026-08-11-a-canceled-subscription-settles-the-todo-list.md`
//! closes, and it is the one hole in the "`Waiting` cannot strand" guarantee
//! the 2026-08-09 split claimed.
//!
//! **Child cards are the same shape.** A thread a live child still owes a
//! report is parked too. `ChildThreadCompleted` wakes the parent, so its turn
//! settles the list. `ChildReportFiled`, `ChildThreadStopped` and
//! `ChildThreadDetached` wake nothing, so each one is a trigger here. See
//! `docs/plans/2026-11-02-a-thread-parked-on-its-children-reads-as-waiting.md`.
//!
//! Coding-agent threads never emit `TodoListWritten` (CC has its own
//! `TodoWrite`), so the helper's `SELECT` short-circuits to `None` for them
//! and this consumer pays one indexed lookup per CC turn.

use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

use super::event_bus::{BusEvent, EmittedEvent, EventBus, LagReplay, ReplayScope};
use super::thread_events::ThreadEvent;
use super::thread_lifecycle::ThreadStatus;
use super::tools::todo::{settle_open_todos, CHILD_UNPARK_EVENT_TYPES};
use super::LucidosEngine;
use uuid::Uuid;

/// Process a single broadcast event: if it's a response-termination, a
/// canceled subscription or a child card that wakes nothing, run the open-todo
/// settle. Exposed so
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
        // arm: a delivery and an expiry each write a `PromptInjected` re-entry
        // anchor, so the re-entered turn's terminator settles the list, and handling
        // them here would race it and could write terminal `Abandoned`
        // over a list the agent is picking back up.
        //
        // `holds_background_work: false` is not a shortcut, it is the correct
        // answer on this path. That override covers the arm-after-terminator
        // race: the chat turn tail arms its background-task wait AFTER the loop
        // emitted the terminator, so the settle's anti-join cannot see it. A
        // cancel is later than the arming it resolves, so the anti-join is
        // authoritative here. And a background task nothing is subscribed to
        // never re-opens the thread. Reading the registry would strand the list
        // when the watched task was still running at **Stop waiting**.
        ThreadEvent::EventWaitCanceled { .. } => {
            settle_unless_a_turn_owns_the_list(bus, pool, *thread_id, seq, false).await;
        }
        // The child-side mirror of the cancel: a card that resolves a child
        // and starts no turn. `ChildThreadCompleted` is deliberately absent. It
        // wakes the parent, and the woken turn's terminator settles the list.
        //
        // These keep the registry read. The child path resolved no
        // background-task wait. So a running task still means a wait the tail
        // is about to arm, possibly after this card landed.
        ThreadEvent::ChildReportFiled { .. }
        | ThreadEvent::ChildThreadStopped { .. }
        | ThreadEvent::ChildThreadDetached { .. } => {
            settle_unless_a_turn_owns_the_list(bus, pool, *thread_id, seq, holds_background_work)
                .await;
        }
        _ => {}
    }
}

/// Settle the list at an unpark no turn follows, unless a turn still owns it.
async fn settle_unless_a_turn_owns_the_list(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    unpark_seq: i64,
    holds_background_work: bool,
) {
    match turn_in_flight(pool, thread_id).await {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            // Same rule the settle already applies to its own queries: a probe
            // that could not run settles nothing, because `Abandoned` is
            // terminal and a wrong guess here cannot be walked back.
            crate::log!(
                @Todo,
                "Could not resolve the turn state for thread {} at an unpark: {} (leaving the list untouched)",
                thread_id,
                e
            );
            return;
        }
    }
    settle_open_todos(bus, pool, thread_id, unpark_seq, holds_background_work).await;
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

/// The event types the settle runs on: every chat terminator, a canceled
/// subscription, and a child card no turn follows. `handle_event` decides
/// which of them settle.
fn settle_event_types() -> Vec<&'static str> {
    ThreadEvent::TERMINATOR_EVENT_TYPES
        .iter()
        .copied()
        .chain(["EventWaitCanceled"])
        .chain(CHILD_UNPARK_EVENT_TYPES)
        .collect()
}

/// Spawn the todo settle consumer as a background task.
/// Returns a `JoinHandle` so the caller can observe panics if needed.
pub fn spawn(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    let (mut rx, mut replay) = LagReplay::subscribe(
        &engine.event_bus,
        ReplayScope::Workspace,
        Some(settle_event_types()),
    );
    tokio::spawn(async move {
        let registry = &engine.bash_background;
        let holds_background_work = move |thread_id| registry.has_running_for_thread(thread_id);
        while settle_after(
            rx.recv().await,
            &mut replay,
            &engine.event_bus,
            engine.pool(),
            holds_background_work,
        )
        .await
        {}
        crate::log!(@Todo, "Todo consumer stream ended");
    })
}

/// Settle on one read of the bus. Returns `false` once the bus closes.
///
/// `holds_background_work` reads the in-memory registry when the frame is
/// handled. So it cannot race the turn tail's arming, as an events query can.
pub(crate) async fn settle_after<F, Fut>(
    received: Result<EmittedEvent, RecvError>,
    replay: &mut LagReplay,
    bus: &EventBus,
    pool: &sqlx::PgPool,
    holds_background_work: F,
) -> bool
where
    F: Fn(Uuid) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    // A lag hands back the terminators it dropped, so each still settles as
    // of its own sequence.
    let Some(frames) = replay.read(received, pool).await else {
        return false;
    };
    for emitted in frames {
        // Only a persisted thread frame can settle, so only that pays for the
        // registry read.
        let holds = match &emitted.typed {
            BusEvent::Thread { thread_id, .. } if emitted.seq.is_some() => {
                holds_background_work(*thread_id).await
            }
            _ => false,
        };
        handle_event(bus, pool, &emitted, holds).await;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::event_subscription::EventSubscription;
    use crate::engine::thread_events::{
        ChildCompletionStatus, EventMeta, EventWaitCancelCause, TodoStatus, WakeMode,
    };
    use crate::engine::tools::todo::todo_tool_impl;
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

    fn is_terminator(event: &ThreadEvent) -> bool {
        matches!(
            event,
            ThreadEvent::ResponseGenerated { .. }
                | ThreadEvent::ResponseCanceled { .. }
                | ThreadEvent::ResponseAborted { .. }
                | ThreadEvent::ResponseFailed { .. }
        )
    }

    async fn dispatch_next_terminator_with_background(
        bus: &EventBus,
        pool: &sqlx::PgPool,
        rx: &mut Receiver<EmittedEvent>,
        thread_id: Uuid,
        holds_background_work: bool,
    ) {
        let ev = next_matching(rx, thread_id, is_terminator).await;
        handle_event(bus, pool, &ev, holds_background_work).await;
    }

    /// The thread's next persisted event that `pick` accepts. Everything before
    /// it is skipped unhandled.
    async fn next_matching(
        rx: &mut Receiver<EmittedEvent>,
        thread_id: Uuid,
        pick: impl Fn(&ThreadEvent) -> bool,
    ) -> EmittedEvent {
        loop {
            let ev = rx.recv().await.expect("broadcast channel should not close");
            if let BusEvent::Thread {
                thread_id: tid,
                event,
                ..
            } = &ev.typed
            {
                if *tid == thread_id && pick(event) {
                    return ev;
                }
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

    async fn seed_in_progress_list(bus: &EventBus, pool: &sqlx::PgPool, thread_id: Uuid) {
        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_tool_impl(bus, pool, &args, thread_id)
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
    /// the cancel arm of `handle_event`: a background task nothing is
    /// subscribed to cannot re-open the thread, so it does not park it).
    async fn dispatch_next_cancel(
        bus: &EventBus,
        pool: &sqlx::PgPool,
        rx: &mut Receiver<EmittedEvent>,
        thread_id: Uuid,
    ) {
        let ev = next_matching(rx, thread_id, |e| {
            matches!(e, ThreadEvent::EventWaitCanceled { .. })
        })
        .await;
        handle_event(bus, pool, &ev, true).await;
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
        seed_in_progress_list(bus, pool, thread_id).await;
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

        seed_in_progress_list(&bus, &pool, thread_id).await;
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

        seed_in_progress_list(&bus, &pool, thread_id).await;
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

        seed_in_progress_list(&bus, &pool, thread_id).await;
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

        seed_in_progress_list(&bus, &pool, thread_id).await;
        drain(&mut rx);

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseFailed {
                error: "upstream 500".to_string(),
                spend_cap: None,
                agent_exit: None,
                provider_error: None,
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

        seed_in_progress_list(&bus, &pool, thread_id).await;
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

        seed_in_progress_list(&bus, &pool, thread_id).await;
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

        seed_in_progress_list(&bus, &pool, thread_id).await;
        drain(&mut rx);

        // Emit a MessageReceived — not a terminator, must not trigger cleanup.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "hi".to_string(),
                user_image_hashes: vec![],
                device_id: None,
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

    /// A delivery and an expiry each write a `PromptInjected` re-entry anchor,
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
        let cancel = next_matching(&mut rx, thread_id, |e| {
            matches!(e, ThreadEvent::EventWaitCanceled { .. })
        })
        .await;
        pool.close().await;

        handle_event(&bus, &pool, &cancel, false).await;
        assert_nothing_settles(&bus, &pool, &mut rx, thread_id).await;

        teardown_test_db(&db).await;
    }

    /// A terminator lost to a broadcast lag must still settle the list.
    /// Otherwise its open items read as in progress for good.
    #[tokio::test]
    async fn a_terminator_lost_to_a_lag_still_settles_the_list() {
        use crate::engine::event_bus::{LagReplay, ReplayScope};
        use crate::test_support::flood_past_bus_capacity;

        let (bus, _setup_rx, pool, db) = setup().await;
        let (mut rx, mut replay) =
            LagReplay::subscribe(&bus, ReplayScope::Workspace, Some(settle_event_types()));
        let thread_id = Uuid::new_v4();

        seed_in_progress_list(&bus, &pool, thread_id).await;
        emit_response_generated(&bus, thread_id).await;
        flood_past_bus_capacity(&bus);
        let mut watch = bus.subscribe();

        let open = settle_after(rx.recv().await, &mut replay, &bus, &pool, |_| async {
            false
        })
        .await;
        assert!(open, "a lag must not end the consumer");
        let settled = next_todo_items(&mut watch, thread_id).await;
        assert_eq!(
            settled[0].status,
            TodoStatus::Abandoned,
            "the dropped terminator settles the list on replay, got {:?}",
            settled[0].status,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// The replay list gates live frames too, so the canceled subscription
    /// `handle_event` settles on must be in it.
    #[test]
    fn the_settle_list_names_the_canceled_subscription() {
        let cancel = ThreadEvent::EventWaitCanceled {
            wait_id: Uuid::new_v4(),
            cause: EventWaitCancelCause::UserStop,
            on: watching(),
            reason: String::new(),
        };
        assert!(settle_event_types().contains(&cancel.event_type()));
    }

    /// Spawn `child` as a direct child of `parent`, the way the spawn path
    /// does: the child's first `MessageReceived` names the parent, and the
    /// projection opens its row as running.
    async fn spawn_child(bus: &EventBus, parent: Uuid, child: Uuid, wake_mode: Option<WakeMode>) {
        bus.emit(BusEvent::Thread {
            thread_id: child,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode,
                text: "child task".to_string(),
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                mode: crate::engine::thread_events::ActorMode::Human,
                model: None,
                reasoning_effort: None,
                parent_thread_id: Some(parent),
                spawning_event_id: None,
                origin: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit the child's MessageReceived");
    }

    /// The child's turn ended. Its card on the parent is emitted separately,
    /// so each test picks which card it is.
    async fn finish_child(pool: &sqlx::PgPool, child: Uuid) {
        seed_thread_status(pool, child, "idle").await;
    }

    fn child_completed(child_thread_id: Uuid) -> ThreadEvent {
        ThreadEvent::ChildThreadCompleted {
            child_thread_id,
            child_thread_title: None,
            status: ChildCompletionStatus::Success,
            summary: "done".to_string(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
            integrated_branch: None,
        }
    }

    fn child_report_filed(child_thread_id: Uuid) -> ThreadEvent {
        ThreadEvent::ChildReportFiled {
            child_thread_id,
            child_thread_title: None,
            status: ChildCompletionStatus::Success,
            summary: "done".to_string(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
            integrated_branch: None,
        }
    }

    fn child_stopped(child_thread_id: Uuid) -> ThreadEvent {
        ThreadEvent::ChildThreadStopped {
            child_thread_id,
            child_thread_title: None,
        }
    }

    fn child_detached(child_thread_id: Uuid) -> ThreadEvent {
        ThreadEvent::ChildThreadDetached {
            child_thread_id,
            child_thread_title: None,
        }
    }

    async fn emit_on(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit on the parent");
    }

    fn is_child_unpark(event: &ThreadEvent) -> bool {
        matches!(
            event,
            ThreadEvent::ChildReportFiled { .. }
                | ThreadEvent::ChildThreadStopped { .. }
                | ThreadEvent::ChildThreadDetached { .. }
        )
    }

    async fn dispatch_next_child_unpark(
        bus: &EventBus,
        pool: &sqlx::PgPool,
        rx: &mut Receiver<EmittedEvent>,
        parent: Uuid,
    ) {
        let ev = next_matching(rx, parent, is_child_unpark).await;
        handle_event(bus, pool, &ev, false).await;
    }

    /// Settle `parent`'s in-progress list at a terminator and return its status.
    async fn settle_at_terminator(
        bus: &EventBus,
        pool: &sqlx::PgPool,
        rx: &mut Receiver<EmittedEvent>,
        parent: Uuid,
    ) -> TodoStatus {
        emit_response_generated(bus, parent).await;
        dispatch_next_terminator(bus, pool, rx, parent).await;
        next_todo_items(rx, parent).await[0].status
    }

    /// The reported shape: an orchestrator spawned children, armed no wait on
    /// purpose, and ended its turn. Returns with the list parked.
    async fn park_on_children(
        bus: &EventBus,
        pool: &sqlx::PgPool,
        rx: &mut Receiver<EmittedEvent>,
        parent: Uuid,
        children: &[Uuid],
    ) {
        seed_in_progress_list(bus, pool, parent).await;
        for child in children {
            spawn_child(bus, parent, *child, None).await;
        }
        drain(rx);
        let status = settle_at_terminator(bus, pool, rx, parent).await;
        assert_eq!(
            status,
            TodoStatus::Waiting,
            "precondition: a live child parks the list"
        );
    }

    #[tokio::test]
    async fn a_live_child_at_the_terminator_parks_the_list() {
        let (bus, mut rx, pool, db) = setup().await;
        park_on_children(&bus, &pool, &mut rx, Uuid::new_v4(), &[Uuid::new_v4()]).await;
        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// A `no-wake` child still files its report for the parent's next turn, so
    /// the parent is still owed something.
    #[tokio::test]
    async fn a_live_no_wake_child_parks_the_list_too() {
        let (bus, mut rx, pool, db) = setup().await;
        let parent = Uuid::new_v4();

        seed_in_progress_list(&bus, &pool, parent).await;
        spawn_child(&bus, parent, Uuid::new_v4(), Some(WakeMode::NoWake)).await;
        drain(&mut rx);

        let status = settle_at_terminator(&bus, &pool, &mut rx, parent).await;
        assert_eq!(status, TodoStatus::Waiting);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// The Thread Queue holds a spawn with no `thread_summaries` row until it
    /// starts, and that spawn will report to the parent all the same.
    #[tokio::test]
    async fn a_queued_spawn_with_no_row_parks_the_list() {
        let (bus, mut rx, pool, db) = setup().await;
        let parent = Uuid::new_v4();

        seed_in_progress_list(&bus, &pool, parent).await;
        sqlx::query(
            "INSERT INTO thread_queue (id, kind, thread_id, request, status) \
             VALUES ($1, 'sub-thread', $2, jsonb_build_object('parent_thread_id', $3::text), \
                     'queued')",
        )
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .bind(parent.to_string())
        .execute(&pool)
        .await
        .expect("seed a queued spawn");
        drain(&mut rx);

        let status = settle_at_terminator(&bus, &pool, &mut rx, parent).await;
        assert_eq!(status, TodoStatus::Waiting);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// A detached child's queue entry still names the old parent in its
    /// request, but the child reports to nobody.
    #[tokio::test]
    async fn a_queued_child_moved_to_top_level_does_not_park_the_list() {
        let (bus, mut rx, pool, db) = setup().await;
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();

        seed_in_progress_list(&bus, &pool, parent).await;
        spawn_child(&bus, parent, child, None).await;
        finish_child(&pool, child).await;
        sqlx::query(
            "INSERT INTO thread_queue (id, kind, thread_id, request, status) \
             VALUES ($1, 'sub-thread', $2, jsonb_build_object('parent_thread_id', $3::text), \
                     'queued')",
        )
        .bind(Uuid::new_v4())
        .bind(child)
        .bind(parent.to_string())
        .execute(&pool)
        .await
        .expect("seed the child's queue entry");
        emit_on(&bus, parent, child_detached(child)).await;
        drain(&mut rx);

        let status = settle_at_terminator(&bus, &pool, &mut rx, parent).await;
        assert_eq!(status, TodoStatus::Abandoned);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// A child that reported before the terminator owes the parent nothing, so
    /// the parent walked away.
    #[tokio::test]
    async fn a_child_that_finished_before_the_terminator_does_not_park_the_list() {
        let (bus, mut rx, pool, db) = setup().await;
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();

        seed_in_progress_list(&bus, &pool, parent).await;
        spawn_child(&bus, parent, child, None).await;
        finish_child(&pool, child).await;
        emit_on(&bus, parent, child_completed(child)).await;
        drain(&mut rx);

        let status = settle_at_terminator(&bus, &pool, &mut rx, parent).await;
        assert_eq!(status, TodoStatus::Abandoned);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// The consumer is async. A child live at the terminator that reports
    /// before the consumer runs was still live when the response ended.
    #[tokio::test]
    async fn a_child_resolving_after_the_terminator_still_parks_the_list() {
        let (bus, mut rx, pool, db) = setup().await;
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();

        seed_in_progress_list(&bus, &pool, parent).await;
        spawn_child(&bus, parent, child, None).await;
        drain(&mut rx);

        emit_response_generated(&bus, parent).await;
        let terminator = next_matching(&mut rx, parent, is_terminator).await;
        finish_child(&pool, child).await;
        emit_on(&bus, parent, child_report_filed(child)).await;

        handle_event(&bus, &pool, &terminator, false).await;
        let settled = next_todo_items(&mut rx, parent).await;
        assert_eq!(settled[0].status, TodoStatus::Waiting);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// The last live child resolving with no turn behind it settles the
    /// parked list, since nothing is owed to the parent any more.
    async fn assert_the_last_child_unpark_abandons(
        resolve: impl FnOnce(Uuid) -> ThreadEvent,
        child_idles: bool,
    ) {
        let (bus, mut rx, pool, db) = setup().await;
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();

        park_on_children(&bus, &pool, &mut rx, parent, &[child]).await;
        if child_idles {
            finish_child(&pool, child).await;
        }
        emit_on(&bus, parent, resolve(child)).await;
        dispatch_next_child_unpark(&bus, &pool, &mut rx, parent).await;

        let settled = next_todo_items(&mut rx, parent).await;
        assert_eq!(settled[0].status, TodoStatus::Abandoned);
        assert_nothing_settles(&bus, &pool, &mut rx, parent).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_last_child_filing_its_report_abandons_the_list() {
        assert_the_last_child_unpark_abandons(child_report_filed, true).await;
    }

    #[tokio::test]
    async fn the_last_child_stopping_abandons_the_list() {
        assert_the_last_child_unpark_abandons(child_stopped, true).await;
    }

    /// A detached child keeps running, but its result never reaches the
    /// former parent.
    #[tokio::test]
    async fn the_last_child_detached_abandons_the_list() {
        assert_the_last_child_unpark_abandons(child_detached, false).await;
    }

    #[tokio::test]
    async fn one_of_two_children_filing_changes_nothing_and_the_last_settles_once() {
        let (bus, mut rx, pool, db) = setup().await;
        let parent = Uuid::new_v4();
        let first = Uuid::new_v4();
        let last = Uuid::new_v4();

        park_on_children(&bus, &pool, &mut rx, parent, &[first, last]).await;

        finish_child(&pool, first).await;
        emit_on(&bus, parent, child_report_filed(first)).await;
        dispatch_next_child_unpark(&bus, &pool, &mut rx, parent).await;
        assert_nothing_settles(&bus, &pool, &mut rx, parent).await;

        finish_child(&pool, last).await;
        emit_on(&bus, parent, child_report_filed(last)).await;
        dispatch_next_child_unpark(&bus, &pool, &mut rx, parent).await;
        let settled = next_todo_items(&mut rx, parent).await;
        assert_eq!(settled[0].status, TodoStatus::Abandoned);
        assert_nothing_settles(&bus, &pool, &mut rx, parent).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// A completion wakes the parent, and the woken turn's terminator settles
    /// the list. Settling here would race that turn.
    #[tokio::test]
    async fn a_completed_child_is_left_to_the_turn_it_wakes() {
        let (bus, mut rx, pool, db) = setup().await;
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();

        park_on_children(&bus, &pool, &mut rx, parent, &[child]).await;
        finish_child(&pool, child).await;
        emit_on(&bus, parent, child_completed(child)).await;
        assert_nothing_settles(&bus, &pool, &mut rx, parent).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    async fn assert_a_turn_owning_the_list_blocks_a_child_unpark(status: &str) {
        let (bus, mut rx, pool, db) = setup().await;
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();

        park_on_children(&bus, &pool, &mut rx, parent, &[child]).await;
        seed_thread_status(&pool, parent, status).await;
        finish_child(&pool, child).await;
        emit_on(&bus, parent, child_report_filed(child)).await;
        dispatch_next_child_unpark(&bus, &pool, &mut rx, parent).await;
        assert_nothing_settles(&bus, &pool, &mut rx, parent).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_child_unpark_settles_nothing_while_the_parent_is_running() {
        assert_a_turn_owning_the_list_blocks_a_child_unpark("running").await;
    }

    #[tokio::test]
    async fn a_child_unpark_settles_nothing_while_the_parent_awaits_an_answer() {
        assert_a_turn_owning_the_list_blocks_a_child_unpark("waiting_for_user_answer").await;
    }

    #[tokio::test]
    async fn a_child_unpark_settles_nothing_while_the_parent_is_paused() {
        assert_a_turn_owning_the_list_blocks_a_child_unpark("paused").await;
    }

    /// The replay list gates live frames too, so every child unpark must be in
    /// it, and the completion must not.
    #[test]
    fn the_settle_list_names_the_child_unparks_and_not_the_completion() {
        let child = Uuid::new_v4();
        let types = settle_event_types();
        for unpark in [
            child_report_filed(child),
            child_stopped(child),
            child_detached(child),
        ] {
            assert!(types.contains(&unpark.event_type()), "{unpark:?}");
        }
        assert!(!types.contains(&child_completed(child).event_type()));
    }
}
