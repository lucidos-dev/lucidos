//! `todo_write` tool — the Lucidos Agent's runtime todo list. Replace-whole-list
//! semantics: each call's `items` become the new truth and any prior list is
//! implicitly dropped. The empty list clears the panel.
//!
//! Also hosts [`settle_open_todos`], the engine-side post-response hook that
//! runs at response termination. The agent's contract is "either keep working
//! the list until every item is `completed`, or call `todo_write` with `[]` to
//! drop it explicitly." If the agent terminates a response with any OPEN item
//! still on the list, the engine emits a new `TodoListWritten` with those items
//! settled to an engine-only status. The list stays visible (the user keeps the
//! trail of what was completed) but the settled items render with a distinct
//! status, so it is obvious the agent did not see them through. All-completed
//! lists are left alone, since finished lists persist by design.
//!
//! **Which status they settle to is the point of the hook.** A thread still
//! holding a live *event wait* did not walk away, it parked: per ADR 0049
//! `await_event` does not hold the turn, so a subscribed thread ends its
//! response like any other and then sleeps until the event arrives. Its open
//! items settle to `Waiting`, which is the same reading the thread's own status
//! dot already gives that fact. Everything else settles to `Abandoned`.
//!
//! See `docs/plans/2026-05-18-todo-list-design.md` for the original design and
//! `docs/plans/2026-08-09-a-subscribed-threads-todo-list-reads-as-waiting.md`
//! for the waiting split.

use super::ToolOutcome;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent, TodoItem, TodoStatus};
use sqlx::PgPool;
use uuid::Uuid;

/// CC's `TodoWrite` uses the same order-of-magnitude limit — runaway lists
/// are a bug indicator, not a feature.
pub(crate) const MAX_TODO_ITEMS: usize = 50;

/// Standalone handler so tests can drive validation branches without a full
/// engine boot — `LucidosEngine::execute_todo_write` is the thin wrapper.
/// Pattern mirrors `dismiss_from_context_impl` in `tools/mod.rs`.
pub(crate) async fn todo_write_impl(
    event_bus: &EventBus,
    args: &serde_json::Value,
    thread_id: uuid::Uuid,
) -> ToolOutcome {
    let raw_items = match args.get("todos").and_then(|v| v.as_array()) {
        Some(items) => items,
        None if args.get("todos").is_none() => return Err("Error: `todos` is required".to_string()),
        None => return Err("Error: `todos` must be an array".to_string()),
    };

    if raw_items.len() > MAX_TODO_ITEMS {
        return Err(format!(
            "Error: too many todo items ({}); max is {}",
            raw_items.len(),
            MAX_TODO_ITEMS,
        ));
    }

    let mut items: Vec<TodoItem> = Vec::with_capacity(raw_items.len());
    let mut in_progress_seen = false;

    for (idx, raw) in raw_items.iter().enumerate() {
        let item: TodoItem = serde_json::from_value(raw.clone()).map_err(|e| {
            format!(
                "Error: todo item at index {} is invalid: {} \
                 (each item needs content, active_form, status)",
                idx, e,
            )
        })?;

        if item.content.trim().is_empty() {
            return Err(format!(
                "Error: todo item at index {} has empty `content`",
                idx,
            ));
        }
        if item.active_form.trim().is_empty() {
            return Err(format!(
                "Error: todo item at index {} has empty `active_form`",
                idx,
            ));
        }
        if matches!(item.status, TodoStatus::InProgress) {
            if in_progress_seen {
                return Err(format!(
                    "Error: at most one todo item may be `in_progress` at a time \
                     (second one found at index {})",
                    idx,
                ));
            }
            in_progress_seen = true;
        }
        // Both engine-only statuses are refused here, and with one message:
        // they are two answers to a question the engine settles at response
        // termination (did this thread park on an event wait, or walk away?),
        // so a model writing either would be asserting something it cannot know
        // yet. See `settle_open_todos`.
        if matches!(item.status, TodoStatus::Abandoned | TodoStatus::Waiting) {
            return Err(format!(
                "Error: todo item at index {} has status `{}`, which is engine-only \
                 and set automatically when you end a response with non-completed \
                 items. Use `pending`, `in_progress`, or `completed`.",
                idx,
                if matches!(item.status, TodoStatus::Waiting) {
                    "waiting"
                } else {
                    "abandoned"
                },
            ));
        }

        items.push(item);
    }

    let count = items.len();
    event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::TodoListWritten { items },
            meta: EventMeta::NONE,
        })
        .await
        .map_err(|e| format!("Error: failed to emit TodoListWritten: {}", e))?;

    Ok(format!("Todo list updated ({} items)", count))
}

/// Was this thread holding an unresolved *event wait* as of `as_of_seq`?
///
/// Asked of the event store rather than of `thread_summaries
/// .live_event_wait_count`, and the difference is the whole reason the helper
/// exists. That column is the CURRENT count, while this consumer is async: a
/// wait delivered between the terminator landing and the SELECT below would
/// make a thread that was demonstrably parked read as one that never waited,
/// which is exactly the bug this settles. The derivation is the same anti-join
/// the column's own migration backfills with, `sequence`-scoped on both sides so
/// the answer is the one that was true when the response ended.
///
/// `Err` means the question could not be answered, never "no". A settle is a
/// rewrite of what the user sees, and both guesses are wrong in their own
/// direction (guessing waiting strands a list reading as parked forever,
/// guessing abandoned is the reported bug), so the caller declines to write
/// anything and the next terminator asks again.
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

/// Engine-side post-response hook: settles every OPEN item in the thread's
/// latest todo list, so the panel says what actually became of the work. The
/// list stays visible either way and the user keeps the trail of what was
/// completed.
///
/// Open items settle to **`Waiting`** when the thread still holds a live event
/// wait at the terminator, and to **`Abandoned`** otherwise. A subscribed
/// thread parked on purpose (ADR 0049: `await_event` does not hold the turn, so
/// the response terminates normally and the thread sleeps), and calling that
/// abandoned within milliseconds of the agent saying it would keep watching is
/// the bug this split fixes. `TodoStatus::is_open` owns which statuses are
/// rewritable: `Waiting` is open, so a wait that resolves without the agent
/// touching the list settles it to `Abandoned` at the next terminator rather
/// than stranding; `Abandoned` is terminal, so a later subscription cannot
/// un-abandon an item the agent already walked away from.
///
/// Called from `todo_consumer` on every chat-thread terminator
/// (`ResponseGenerated` / `ResponseCanceled` / `ResponseAborted` /
/// `ResponseFailed`). The agent's contract: either finish the list (every
/// item `completed`) or call `todo_write` with `[]` to drop it. Already-settled
/// lists short-circuit, so a second terminator with the same answer re-emits
/// nothing.
///
/// `terminator_seq` is the bigserial `sequence` of the terminator that
/// triggered this call. We only consider TodoListWritten rows with
/// `sequence <= terminator_seq`, AND skip the settle if a NEWER
/// TodoListWritten exists (a fresh turn started before our async consumer
/// caught up, and that turn owns its list: the consumer will see its own
/// terminator and handle it then). The wait question is scoped to the same
/// sequence for the same reason.
///
/// Coding-agent threads never emit `TodoListWritten` (CC has its own
/// `TodoWrite`), so the SELECT short-circuits to `None` for them and no extra
/// gate is needed at the call site.
pub async fn settle_open_todos(
    event_bus: &EventBus,
    pool: &PgPool,
    thread_id: Uuid,
    terminator_seq: i64,
) {
    // SELECT the latest TodoListWritten in the thread regardless of sequence,
    // then decide. If `sequence > terminator_seq` a fresh turn has already
    // written a new list, so skip: that turn's own terminator will run the
    // cleanup. Otherwise the row at-or-before the terminator IS the list we
    // need to evaluate.
    let row: Option<(serde_json::Value, i64)> = match sqlx::query_as(
        "SELECT payload, sequence FROM events \
         WHERE thread_id = $1 AND event_type = 'TodoListWritten' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            log!(
                "[Todo] settle_open_todos query failed for thread {}: {}",
                thread_id,
                e
            );
            return;
        }
    };

    let Some((payload, latest_seq)) = row else {
        return;
    };

    if latest_seq > terminator_seq {
        // Race: a fresh turn already wrote a new list after this terminator
        // landed but before we got to it. Leave the new list alone, since its
        // own terminator will trigger a fresh cleanup pass.
        return;
    }

    let items: Vec<TodoItem> = match payload
        .get("items")
        .and_then(|v| serde_json::from_value::<Vec<TodoItem>>(v.clone()).ok())
    {
        Some(items) => items,
        None => {
            log!(
                "[Todo] settle_open_todos: malformed TodoListWritten payload for thread {}",
                thread_id
            );
            return;
        }
    };

    if items.is_empty() {
        return;
    }
    // Nothing to do when no item is still open: everything is completed, or
    // already settled by an earlier terminator.
    if !items.iter().any(|item| item.status.is_open()) {
        return;
    }

    // Only asked once there is something to settle, so an all-completed list
    // (the common case) never pays for it.
    let settled_status = match thread_held_event_wait(pool, thread_id, terminator_seq).await {
        Ok(true) => TodoStatus::Waiting,
        Ok(false) => TodoStatus::Abandoned,
        Err(e) => {
            log!(
                "[Todo] settle_open_todos could not resolve the event-wait state for thread {}: {} \
                 (leaving the list untouched; the next terminator retries)",
                thread_id,
                e
            );
            return;
        }
    };

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
                event: ThreadEvent::TodoListWritten { items: settled },
                meta: EventMeta::NONE,
            },
            "[Todo] TodoListWritten (auto-settle open items)",
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::EmittedEvent;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use serde_json::json;
    use tokio::sync::broadcast::Receiver;
    use uuid::Uuid;

    async fn setup() -> (EventBus, Receiver<EmittedEvent>, sqlx::PgPool, String) {
        let (pool, db) = setup_test_db().await;
        let (bus, _parent_rx) = EventBus::new(pool.clone());
        let events_rx = bus.subscribe();
        (bus, events_rx, pool, db)
    }

    async fn next_todo_event(rx: &mut Receiver<EmittedEvent>, thread_id: Uuid) -> Vec<TodoItem> {
        loop {
            let ev = rx.recv().await.expect("broadcast channel should not close");
            match ev.typed {
                BusEvent::Thread {
                    thread_id: tid,
                    event: ThreadEvent::TodoListWritten { items },
                    ..
                } if tid == thread_id => return items,
                _ => continue,
            }
        }
    }

    #[tokio::test]
    async fn todo_write_accepts_three_items_with_all_statuses_and_emits_event() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "Read code",  "active_form": "Reading code",  "status": "completed" },
                { "content": "Write tests","active_form": "Writing tests", "status": "in_progress" },
                { "content": "Update docs","active_form": "Updating docs", "status": "pending" },
            ]
        });

        let out = todo_write_impl(&bus, &args, thread_id).await;
        assert!(
            matches!(&out, Ok(s) if s.contains("3 items")),
            "got: {:?}",
            out
        );

        let items = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].status, TodoStatus::Completed);
        assert_eq!(items[1].status, TodoStatus::InProgress);
        assert_eq!(items[2].status, TodoStatus::Pending);
        assert_eq!(items[1].content, "Write tests");
        assert_eq!(items[1].active_form, "Writing tests");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_accepts_empty_list_as_clear() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let out = todo_write_impl(&bus, &json!({"todos": []}), thread_id).await;
        assert!(
            matches!(&out, Ok(s) if s.contains("0 items")),
            "got: {:?}",
            out
        );

        let items = next_todo_event(&mut rx, thread_id).await;
        assert!(items.is_empty());

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_rejects_missing_todos_field() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let out = todo_write_impl(&bus, &json!({}), thread_id).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("`todos` is required")),
            "got: {:?}",
            out,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_rejects_non_array_todos() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let out = todo_write_impl(&bus, &json!({"todos": "nope"}), thread_id).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("must be an array")),
            "got: {:?}",
            out,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_rejects_more_than_fifty_items() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let mut items = Vec::with_capacity(MAX_TODO_ITEMS + 1);
        for i in 0..=MAX_TODO_ITEMS {
            items.push(json!({
                "content": format!("item {}", i),
                "active_form": format!("doing item {}", i),
                "status": "pending",
            }));
        }
        let out = todo_write_impl(&bus, &json!({"todos": items}), thread_id).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("too many") && msg.contains("max is 50")),
            "got: {:?}",
            out,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_rejects_two_in_progress_items() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
                { "content": "b", "active_form": "doing b", "status": "in_progress" },
            ]
        });
        let out = todo_write_impl(&bus, &args, thread_id).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("at most one") && msg.contains("in_progress")),
            "got: {:?}",
            out,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_rejects_empty_content() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "   ", "active_form": "doing a", "status": "pending" },
            ]
        });
        let out = todo_write_impl(&bus, &args, thread_id).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("empty `content`")),
            "got: {:?}",
            out,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_rejects_empty_active_form() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "", "status": "pending" },
            ]
        });
        let out = todo_write_impl(&bus, &args, thread_id).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("empty `active_form`")),
            "got: {:?}",
            out,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_rejects_invalid_status() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "blocked" },
            ]
        });
        let out = todo_write_impl(&bus, &args, thread_id).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("invalid") || msg.contains("status")),
            "got: {:?}",
            out,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// Drain any TodoListWritten broadcasts already in flight for `thread_id`
    /// so a follow-up assertion sees a clean channel. Returns once the channel
    /// has no more buffered events for the thread.
    async fn drain_todo_events_for(rx: &mut Receiver<EmittedEvent>, thread_id: Uuid) {
        use tokio::time::{timeout, Duration};
        loop {
            match timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Ok(ev)) => match ev.typed {
                    BusEvent::Thread {
                        thread_id: tid,
                        event: ThreadEvent::TodoListWritten { .. },
                        ..
                    } if tid == thread_id => continue,
                    _ => continue,
                },
                _ => return,
            }
        }
    }

    /// Assert no TodoListWritten broadcast for `thread_id` arrives within a
    /// short window. Other broadcasts (e.g. thread_summaries projection
    /// side effects) are ignored — we only care about the cleanup-or-not.
    async fn assert_no_todo_cleanup(rx: &mut Receiver<EmittedEvent>, thread_id: Uuid) {
        use tokio::time::{timeout, Duration};
        let deadline = tokio::time::Instant::now() + Duration::from_millis(150);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return;
            }
            match timeout(remaining, rx.recv()).await {
                Ok(Ok(ev)) => {
                    if let BusEvent::Thread {
                        thread_id: tid,
                        event: ThreadEvent::TodoListWritten { items },
                        ..
                    } = ev.typed
                    {
                        if tid == thread_id {
                            panic!(
                                "expected no TodoListWritten cleanup, but got items: {:?}",
                                items
                            );
                        }
                    }
                }
                _ => return,
            }
        }
    }

    #[tokio::test]
    async fn settle_open_todos_emits_nothing_when_no_list_was_ever_written() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_emits_nothing_when_all_items_completed() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "completed" },
                { "content": "b", "active_form": "doing b", "status": "completed" },
            ]
        });
        todo_write_impl(&bus, &args, thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_emits_nothing_when_list_is_already_empty() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        todo_write_impl(&bus, &json!({"todos": []}), thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_flips_in_progress_and_pending_keeps_completed() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "completed" },
                { "content": "b", "active_form": "doing b", "status": "in_progress" },
                { "content": "c", "active_form": "doing c", "status": "pending" },
            ]
        });
        todo_write_impl(&bus, &args, thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;

        let flipped = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(flipped.len(), 3, "list preserved, got {:?}", flipped);
        assert_eq!(flipped[0].status, TodoStatus::Completed, "completed kept");
        assert_eq!(
            flipped[1].status,
            TodoStatus::Abandoned,
            "in_progress flipped"
        );
        assert_eq!(flipped[2].status, TodoStatus::Abandoned, "pending flipped");
        assert_eq!(flipped[0].content, "a");
        assert_eq!(flipped[1].content, "b");
        assert_eq!(flipped[2].content, "c");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_flips_pending_only_list() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "completed" },
                { "content": "b", "active_form": "doing b", "status": "pending" },
            ]
        });
        todo_write_impl(&bus, &args, thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;

        let flipped = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(flipped.len(), 2);
        assert_eq!(flipped[0].status, TodoStatus::Completed);
        assert_eq!(flipped[1].status, TodoStatus::Abandoned);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_is_idempotent_once_already_marked() {
        // Two terminators in a row (e.g. ResponseAborted then ResponseCanceled
        // via the safety-net path) must not re-emit a TodoListWritten when
        // the latest already shows abandoned + completed only.
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "completed" },
                { "content": "b", "active_form": "doing b", "status": "in_progress" },
            ]
        });
        todo_write_impl(&bus, &args, thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        // First call flips.
        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;
        let _flipped = next_todo_event(&mut rx, thread_id).await;

        // Second call is a no-op.
        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_uses_only_the_latest_list_state() {
        // Earlier write was mid-task; later write completed everything.
        // The hook sees the latest (all-completed) and stays quiet.
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let stale = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_write_impl(&bus, &stale, thread_id)
            .await
            .expect("seed stale");

        let fresh = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "completed" },
            ]
        });
        todo_write_impl(&bus, &fresh, thread_id)
            .await
            .expect("seed fresh");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_skips_when_newer_list_written_after_terminator() {
        // Simulates: turn A's terminator landed but the consumer was slow.
        // Before the consumer's SELECT, turn B started and wrote a fresh
        // in-progress list. The consumer must NOT flip turn B's items — it
        // should defer to turn B's own terminator.
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        // Seed turn A's stale in-progress list.
        let stale = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_write_impl(&bus, &stale, thread_id)
            .await
            .expect("seed stale");

        // Pretend turn A's terminator was assigned a sequence value — capture
        // the current max-sequence as our terminator marker. Anything written
        // AFTER that should be treated as turn B's fresh state.
        let terminator_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) FROM events WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("max seq");

        // Now turn B writes a fresh list AFTER the terminator.
        let fresh = json!({
            "todos": [
                { "content": "b", "active_form": "doing b", "status": "in_progress" },
                { "content": "c", "active_form": "doing c", "status": "pending" },
            ]
        });
        todo_write_impl(&bus, &fresh, thread_id)
            .await
            .expect("seed fresh");
        drain_todo_events_for(&mut rx, thread_id).await;

        // Consumer finally processes turn A's terminator. It should see the
        // newer turn-B list and skip — turn B owns its list.
        settle_open_todos(&bus, &pool, thread_id, terminator_seq).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_rejects_abandoned_status_from_llm() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "abandoned" },
            ]
        });
        let out = todo_write_impl(&bus, &args, thread_id).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("abandoned") && msg.contains("engine-only")),
            "got: {:?}",
            out,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_rejects_waiting_status_from_llm() {
        // The sibling of the test above: `waiting` is the engine's answer to a
        // question about the thread's subscriptions, so the model may no more
        // write it than it may write `abandoned`.
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "waiting" },
            ]
        });
        let out = todo_write_impl(&bus, &args, thread_id).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("waiting") && msg.contains("engine-only")),
            "got: {:?}",
            out,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    // ---- The waiting split: a subscribed thread parked, it did not walk away.

    async fn seed_event_wait(bus: &EventBus, thread_id: Uuid) -> Uuid {
        use crate::core::event_subscription::EventSubscription;
        let wait_id = Uuid::new_v4();
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::EventWaitStarted {
                wait_id,
                tool_use_id: format!("toolu_{wait_id}"),
                on: vec![EventSubscription {
                    event_type: "ChangeProposed".into(),
                    condition: None,
                }],
                reason: "watching for the change to land".into(),
                armed_at: chrono::Utc::now(),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                watermark: 0,
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit EventWaitStarted");
        wait_id
    }

    async fn deliver_event_wait(bus: &EventBus, thread_id: Uuid, wait_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::EventWaitDelivered {
                wait_id,
                event_id: Uuid::new_v4(),
                event_type: "ChangeProposed".into(),
                payload: json!({ "change_id": "abc" }),
                matched_index: 0,
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit EventWaitDelivered");
    }

    async fn max_seq(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM events WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(pool)
            .await
            .expect("max seq")
    }

    #[tokio::test]
    async fn settle_open_todos_settles_to_waiting_while_a_subscription_is_live() {
        // The reported bug. `await_event` does not hold the turn (ADR 0049), so
        // a thread that parks on a wait terminates its response like any other
        // and the hook ran on it within milliseconds of the agent saying it
        // would keep watching. Calling that "abandoned" is the lie.
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "completed" },
                { "content": "b", "active_form": "doing b", "status": "in_progress" },
                { "content": "c", "active_form": "doing c", "status": "pending" },
            ]
        });
        todo_write_impl(&bus, &args, thread_id)
            .await
            .expect("seed write");
        seed_event_wait(&bus, thread_id).await;
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;

        let settled = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(settled.len(), 3, "list preserved, got {:?}", settled);
        assert_eq!(settled[0].status, TodoStatus::Completed, "completed kept");
        assert_eq!(
            settled[1].status,
            TodoStatus::Waiting,
            "in_progress parked, got {:?}",
            settled[1].status,
        );
        assert_eq!(
            settled[2].status,
            TodoStatus::Waiting,
            "pending parked, got {:?}",
            settled[2].status,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_settles_waiting_items_to_abandoned_once_the_wait_resolves() {
        // `Waiting` is open, not terminal, and this is why: the wake arrived,
        // the agent still did not finish the list, so the next terminator has
        // to be able to call it what it now is. Otherwise a parked list reads
        // as parked forever.
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_write_impl(&bus, &args, thread_id)
            .await
            .expect("seed write");
        let wait_id = seed_event_wait(&bus, thread_id).await;
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;
        let parked = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(parked[0].status, TodoStatus::Waiting);

        deliver_event_wait(&bus, thread_id, wait_id).await;
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;
        let settled = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(
            settled[0].status,
            TodoStatus::Abandoned,
            "waiting item settles once nothing will wake the thread, got {:?}",
            settled[0].status,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_reads_the_wait_state_as_of_the_terminator() {
        // Why the liveness question is asked of the event store and not of
        // `thread_summaries.live_event_wait_count`: this consumer is async, so
        // a wait delivered between the terminator and the SELECT would make a
        // thread that was demonstrably parked read as one that never waited.
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_write_impl(&bus, &args, thread_id)
            .await
            .expect("seed write");
        let wait_id = seed_event_wait(&bus, thread_id).await;

        // The terminator landed here, with the wait still live.
        let terminator_seq = max_seq(&pool, thread_id).await;

        // ...and the delivery beat the consumer to it.
        deliver_event_wait(&bus, thread_id, wait_id).await;
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, terminator_seq).await;

        let settled = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(
            settled[0].status,
            TodoStatus::Waiting,
            "the answer is the one that was true at the terminator, got {:?}",
            settled[0].status,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_is_idempotent_while_the_subscription_stays_live() {
        // The sibling of `settle_open_todos_is_idempotent_once_already_marked`,
        // for the other settled status: the two terminators of one turn must
        // not each re-emit an identical parked list.
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_write_impl(&bus, &args, thread_id)
            .await
            .expect("seed write");
        seed_event_wait(&bus, thread_id).await;
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;
        let _parked = next_todo_event(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_never_un_abandons_an_item_for_a_later_subscription() {
        // `Abandoned` is terminal in the other direction. The agent walked away
        // from this item in an earlier turn; subscribing to something in a
        // later one does not make that item parked work.
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_write_impl(&bus, &args, thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;
        let abandoned = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(abandoned[0].status, TodoStatus::Abandoned);

        seed_event_wait(&bus, thread_id).await;
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }
}
