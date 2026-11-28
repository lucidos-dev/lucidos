//! INVARIANT: EventBus consumer that settles open Lucidos Agent todo lists.
//!
//! On every persisted chat-thread terminator (`ResponseGenerated` /
//! `ResponseCanceled` / `ResponseAborted` / `ResponseFailed`, the full
//! `TERMINATOR_EVENT_TYPES` set in `thread_events/event_impl.rs`), calls
//! [`crate::engine::todo_state::settle_open_todos`] to enforce the agent's
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
use super::todo_state::{settle_open_todos, CHILD_UNPARK_EVENT_TYPES};
use super::LucidosEngine;
use uuid::Uuid;

pub async fn handle_event(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    emitted: &EmittedEvent,
    holds_background_work: bool,
) {
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
        // INVARIANT: Every chat terminator. Keep this list in sync with
        // `ThreadEvent::TERMINATOR_EVENT_TYPES`: `ResponseFailed` is in that
        // set (e.g. upstream LLM error, OOM-killed bash, empty assistant text
        // on a non-cancel turn) and must also settle open todos.
        ThreadEvent::ResponseGenerated { .. }
        | ThreadEvent::ResponseCanceled { .. }
        | ThreadEvent::ResponseAborted { .. }
        | ThreadEvent::ResponseFailed { .. } => {
            settle_open_todos(bus, pool, *thread_id, seq, holds_background_work).await;
        }
        // INVARIANT: The unpark that no turn follows. Deliberately the ONLY `EventWait*`
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
        // INVARIANT: The child-side mirror of the cancel: a card that resolves a child
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
            // INVARIANT: Same rule the settle already applies to its own queries: a probe
            // that could not run settles nothing, because `Abandoned` is
            // terminal and a wrong guess here cannot be walked back.
            crate::log_legacy!(
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

/// INVARIANT: Does a turn still own this thread's todo list?
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

pub fn settle_event_types() -> Vec<&'static str> {
    ThreadEvent::TERMINATOR_EVENT_TYPES
        .iter()
        .copied()
        .chain(["EventWaitCanceled"])
        .chain(CHILD_UNPARK_EVENT_TYPES)
        .collect()
}

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
        crate::log_legacy!(@Todo, "Todo consumer stream ended");
    })
}

/// INVARIANT: Settle on one read of the bus. Returns `false` once the bus closes.
///
/// `holds_background_work` reads the in-memory registry when the frame is
/// handled. So it cannot race the turn tail's arming, as an events query can.
pub async fn settle_after<F, Fut>(
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
    let Some(frames) = replay.read(received, pool).await else {
        return false;
    };
    for emitted in frames {
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
