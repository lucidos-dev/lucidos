//! Read and settle the persisted checklist of a thread.

use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::child_limits::OWED_CHILDREN_SQL;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent, TodoItem, TodoStatus};

/// Was this thread holding an unresolved *event wait* as of `as_of_seq`?
///
/// Read live waits from events at the terminator sequence. A wait can resolve
/// before the async consumer runs, so the current projected count would lose
/// the parked state. The anti-join matches the projection's backfill with both
/// sides bounded by sequence.
///
/// On a read error, leave the list alone. Guessing could abandon live work or
/// strand the list in Waiting. A later terminator can try again.
async fn thread_held_event_wait(
    pool: &PgPool,
    thread_id: Uuid,
    as_of_seq: i64,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS ( \
           SELECT 1 FROM events s \
           WHERE s.thread_id = $1 \
             AND s.event_type = 'EventWaitStarted' \
             AND s.sequence <= $2 \
             AND s.payload->>'wait_id' IS NOT NULL \
             AND NOT EXISTS ( \
               SELECT 1 FROM events r \
               WHERE r.thread_id = $1 \
                 AND r.event_type IN ('EventWaitDelivered', 'EventWaitExpired', \
                                      'EventWaitCanceled') \
                 AND r.sequence <= $2 \
                 AND r.payload->>'wait_id' = s.payload->>'wait_id' \
             ) \
         )",
    )
    .bind(thread_id)
    .bind(as_of_seq)
    .fetch_one(pool)
    .await
}

/// The cards on a parent that resolve a child and start no turn behind them.
/// `todo_consumer` settles on each one.
pub(crate) const CHILD_UNPARK_EVENT_TYPES: [&str; 3] = [
    "ChildReportFiled",
    "ChildThreadStopped",
    "ChildThreadDetached",
];

/// The card on a parent that resolves a child AND wakes the parent. The woken
/// turn's terminator settles the list, so the consumer leaves this one alone.
const CHILD_WAKE_EVENT_TYPE: &str = "ChildThreadCompleted";

/// Was this thread owed a result by a live direct child as of `as_of_seq`?
///
/// No event records when a child became owed: a queued spawn has no events at
/// all, and a child's start lives on the child's own thread. So the answer is
/// a current read plus a correction. A child live now was live then, and so
/// was any child whose card landed on this thread after `as_of_seq`.
///
/// Every error this makes points at `Waiting`, never at terminal `Abandoned`.
/// A child spawned or revived after `as_of_seq` reads as live, and each of
/// those brings its own later settle. See
/// `docs/plans/2026-11-02-a-thread-parked-on-its-children-reads-as-waiting.md`.
///
/// `Err` means the question could not be answered, never "no", exactly as for
/// [`thread_held_event_wait`].
async fn thread_held_live_child(
    pool: &PgPool,
    thread_id: Uuid,
    as_of_seq: i64,
) -> Result<bool, sqlx::Error> {
    let resolutions: Vec<&str> = CHILD_UNPARK_EVENT_TYPES
        .into_iter()
        .chain([CHILD_WAKE_EVENT_TYPE])
        .collect();
    sqlx::query_scalar(&format!(
        "SELECT EXISTS ({owed}) \
             OR EXISTS ( \
               SELECT 1 FROM events \
               WHERE thread_id = $1 AND sequence > $2 AND event_type = ANY($3))",
        owed = *OWED_CHILDREN_SQL,
    ))
    .bind(thread_id)
    .bind(as_of_seq)
    .bind(resolutions)
    .fetch_one(pool)
    .await
}

/// Is this thread owed a result by a live direct child right now?
///
/// The *wake check*'s reader. It runs inside the turn, where no terminator
/// exists to be as-of, so the current state is the answer.
pub async fn holds_live_child(pool: &PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(&format!("SELECT EXISTS ({})", *OWED_CHILDREN_SQL))
        .bind(thread_id)
        .fetch_one(pool)
        .await
}

/// The thread's newest todo list: its items, its *todo notes*, and the sequence
/// it was written at.
///
/// An EMPTY list is a real snapshot here and is returned as one, because the
/// notes can outlive the items: an agent that finished its plan and still holds
/// a pointer worth keeping writes `todos: []` with notes. Callers that only
/// care about work in flight check `items` themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoSnapshot {
    pub items: Vec<TodoItem>,
    pub notes: Option<String>,
    pub sequence: i64,
}

impl TodoSnapshot {
    /// Is every item `completed`, so there is nothing left to reconcile?
    ///
    /// The gate on `turn_start_block`, and deliberately NOT
    /// `TodoStatus::is_open`. That predicate answers a different question, one
    /// the settle asks: may a terminator still rewrite this row? `Abandoned` is
    /// terminal to the settle, which is exactly why it is the status most worth
    /// showing the agent. Nothing but the agent writing the list again can
    /// correct it.
    ///
    /// An empty list is vacuously all-completed, and the caller drops it
    /// either way.
    pub fn all_completed(&self) -> bool {
        self.items
            .iter()
            .all(|item| item.status == TodoStatus::Completed)
    }
}

/// The thread's newest todo list, or `None` when there is nothing to read.
///
/// `Ok(None)` folds two cases every caller treats alike: no list was ever
/// written, and the row's payload is malformed. A coding-agent thread is always
/// the first, since Claude Code has its own `TodoWrite` and never emits this.
///
/// `Err` is neither. It is the question going unanswered, which callers must
/// not read as "nothing open" (`.claude/rules/rust.md`).
///
/// Shared by the settle, the *wake check* and the mode's turn setup, which
/// seeds the `[TODO]` heading of the *working understanding*. None of the three
/// can then disagree about which list is current. They differ in
/// what they do next: the settle compares the sequence against its terminator,
/// while the other two run inside the turn, where the newest list is current by
/// definition.
pub async fn latest_todo_list(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<TodoSnapshot>, sqlx::Error> {
    let row: Option<(serde_json::Value, i64)> = sqlx::query_as(
        "SELECT payload, sequence FROM events \
         WHERE thread_id = $1 AND event_type = 'TodoListWritten' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;

    let Some((payload, sequence)) = row else {
        return Ok(None);
    };

    let Some(items) = payload
        .get("items")
        .and_then(|v| serde_json::from_value::<Vec<TodoItem>>(v.clone()).ok())
    else {
        log_legacy!(
            "[Todo] malformed TodoListWritten payload for thread {}",
            thread_id
        );
        return Ok(None);
    };

    let notes = payload
        .get("notes")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    Ok(Some(TodoSnapshot {
        items,
        notes,
        sequence,
    }))
}

/// Settle open items in the thread's latest todo list. Keep completed items
/// visible.
///
/// A live wait or direct child makes open items `Waiting`. Otherwise they
/// become `Abandoned`. Waiting items can settle again when the thread unparks.
/// Abandoned items are terminal.
///
/// The consumer calls this on chat terminators and unparks that wake no turn. A
/// repeated outcome emits nothing. When an archive cancels several waits, only
/// the last cancel can abandon the list.
///
/// `terminator_seq` bounds the list, wait and child reads. Skip settlement if a
/// newer list already exists because it belongs to a later turn.
///
/// Coding-agent threads use their own todo tool and produce no
/// `TodoListWritten` rows.
pub async fn settle_open_todos(
    event_bus: &EventBus,
    pool: &PgPool,
    thread_id: Uuid,
    terminator_seq: i64,
    holds_background_work: bool,
) {
    // The list is read at-or-before nothing: `latest_todo_list` takes the
    // newest row whatever its sequence, and the race check below is what scopes
    // it to this terminator.
    let TodoSnapshot {
        items,
        notes,
        sequence: latest_seq,
    } = match latest_todo_list(pool, thread_id).await {
        Ok(Some(found)) => found,
        Ok(None) => return,
        Err(e) => {
            log_legacy!(
                "[Todo] settle_open_todos query failed for thread {}: {}",
                thread_id,
                e
            );
            return;
        }
    };

    if latest_seq > terminator_seq {
        // Race: a fresh turn already wrote a new list after this terminator
        // landed but before we got to it. Leave the new list alone, since its
        // own terminator will trigger a fresh cleanup pass.
        return;
    }

    // Nothing to do when no item is still open: everything is completed, or
    // already settled by an earlier terminator.
    if !items.iter().any(|item| item.status.is_open()) {
        return;
    }

    // Check in-memory background work first. The turn tail can arm its event
    // wait after the terminator, outside the sequence window. The registry
    // still recognizes the thread as parked while the build runs.
    if holds_background_work {
        settle_to(event_bus, thread_id, items, notes, TodoStatus::Waiting).await;
        return;
    }

    // Only asked once there is something to settle, so an all-completed list
    // (the common case) never pays for it.
    let settled_status = match thread_was_parked(pool, thread_id, terminator_seq).await {
        Ok(true) => TodoStatus::Waiting,
        Ok(false) => TodoStatus::Abandoned,
        Err(e) => {
            log_legacy!(
                "[Todo] settle_open_todos could not resolve whether thread {} was parked: {} \
                 (leaving the list untouched; the next terminator retries)",
                thread_id,
                e
            );
            return;
        }
    };

    settle_to(event_bus, thread_id, items, notes, settled_status).await;
}

/// Will something re-open this thread, as of `as_of_seq`: a live event wait,
/// or a live direct child that still owes it a result?
///
/// The wait is asked first because it is the cheaper query and the commoner
/// park.
async fn thread_was_parked(
    pool: &PgPool,
    thread_id: Uuid,
    as_of_seq: i64,
) -> Result<bool, sqlx::Error> {
    if thread_held_event_wait(pool, thread_id, as_of_seq).await? {
        return Ok(true);
    }
    thread_held_live_child(pool, thread_id, as_of_seq).await
}

/// Rewrite every OPEN item to `settled_status` and re-emit the list.
///
/// Extracted so every way a thread can be parked reaches one writer: an
/// unresolved *event wait* or live child at the terminator, and unfinished
/// background work the tail is about to subscribe to.
///
/// `notes` rides through untouched. Replace-whole-list means this re-emit IS
/// the list from here on. Dropping them would have the engine erase what the
/// agent wrote, every time a turn ended with an item still open.
async fn settle_to(
    event_bus: &EventBus,
    thread_id: Uuid,
    items: Vec<TodoItem>,
    notes: Option<String>,
    settled_status: TodoStatus,
) {
    // A list already settled to this exact status is left alone. Without this
    // the two terminators of one turn (e.g. ResponseAborted then the safety
    // net's ResponseCanceled) would each re-emit an identical list.
    if items
        .iter()
        .all(|item| !item.status.is_open() || item.status == settled_status)
    {
        return;
    }

    let settled: Vec<TodoItem> = items
        .into_iter()
        .map(|item| {
            if item.status.is_open() {
                TodoItem {
                    status: settled_status,
                    ..item
                }
            } else {
                item
            }
        })
        .collect();

    event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::TodoListWritten {
                    items: settled,
                    notes,
                },
                meta: EventMeta::NONE,
            },
            "[Todo] TodoListWritten (auto-settle open items)",
        )
        .await;
}
