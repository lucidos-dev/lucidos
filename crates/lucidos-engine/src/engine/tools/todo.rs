//! `todo_write` tool — the Lucidos Agent's runtime todo list. Replace-whole-list
//! semantics: each call's `items` become the new truth and any prior list is
//! implicitly dropped. The empty list clears the panel.
//!
//! Also hosts [`mark_abandoned_todos`] — the engine-side post-response hook
//! that runs at response termination. The agent's contract is "either keep
//! working the list until every item is `completed`, or call `todo_write` with
//! `[]` to drop it explicitly." If the agent terminates a response with any
//! `pending` or `in_progress` item still on the list, the engine emits a new
//! `TodoListWritten` with those items flipped to `Abandoned` — the list stays
//! visible (the user keeps the trail of what was completed) but the abandoned
//! items render with a distinct status so it's obvious the agent walked away
//! from them. All-completed lists are left alone — finished lists persist by
//! design.
//!
//! See `docs/plans/2026-05-18-todo-list-design.md` for the design.

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
        if matches!(item.status, TodoStatus::Abandoned) {
            return Err(format!(
                "Error: todo item at index {} has status `abandoned` — that status \
                 is engine-only and set automatically when you end a response with \
                 non-completed items. Use `pending`, `in_progress`, or `completed`.",
                idx,
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

/// Engine-side post-response hook: flips any non-completed items in the
/// thread's latest todo list to `Abandoned` so the panel makes it obvious
/// the agent walked away from them. The list stays visible — the user keeps
/// the trail of what was actually completed.
///
/// Called from `todo_consumer` on every chat-thread terminator
/// (`ResponseGenerated` / `ResponseCanceled` / `ResponseAborted` /
/// `ResponseFailed`). The agent's contract: either finish the list (every
/// item `completed`) or call `todo_write` with `[]` to drop it. If the agent
/// did neither, the latest `TodoListWritten` at the time of the terminator
/// still has `pending` / `in_progress` items — this helper emits a fresh
/// `TodoListWritten` with those flipped to `Abandoned`. Already-completed
/// items keep their status; already-abandoned items short-circuit (no
/// re-emit on a list we've already marked).
///
/// `terminator_seq` is the bigserial `sequence` of the terminator that
/// triggered this call. We only consider TodoListWritten rows with
/// `sequence <= terminator_seq`, AND skip the flip if a NEWER
/// TodoListWritten exists (a fresh turn started before our async consumer
/// caught up — that turn owns its list, the consumer will see its own
/// terminator and handle it then).
///
/// Coding-agent threads never emit `TodoListWritten` (CC has its own
/// `TodoWrite`), so the SELECT short-circuits to `None` for them — no extra
/// gate needed at the call site.
pub async fn mark_abandoned_todos(
    event_bus: &EventBus,
    pool: &PgPool,
    thread_id: Uuid,
    terminator_seq: i64,
) {
    // SELECT the latest TodoListWritten in the thread regardless of sequence,
    // then decide. If `sequence > terminator_seq` a fresh turn has already
    // written a new list — skip; that turn's own terminator will run the
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
                "[Todo] mark_abandoned_todos query failed for thread {}: {}",
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
        // landed but before we got to it. Leave the new list alone — its
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
                "[Todo] mark_abandoned_todos: malformed TodoListWritten payload for thread {}",
                thread_id
            );
            return;
        }
    };

    if items.is_empty() {
        return;
    }
    // Nothing to flip if every non-completed slot is already abandoned (or
    // every item is completed). `Pending` / `InProgress` are the only states
    // the helper rewrites.
    let needs_flip = items.iter().any(|item| {
        matches!(item.status, TodoStatus::Pending | TodoStatus::InProgress)
    });
    if !needs_flip {
        return;
    }

    let flipped: Vec<TodoItem> = items
        .into_iter()
        .map(|item| match item.status {
            TodoStatus::Pending | TodoStatus::InProgress => TodoItem {
                status: TodoStatus::Abandoned,
                ..item
            },
            TodoStatus::Completed | TodoStatus::Abandoned => item,
        })
        .collect();

    event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::TodoListWritten { items: flipped },
                meta: EventMeta::NONE,
            },
            "[Todo] TodoListWritten (auto-mark abandoned)",
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
            let ev = rx
                .recv()
                .await
                .expect("broadcast channel should not close");
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
        assert!(matches!(&out, Ok(s) if s.contains("3 items")), "got: {:?}", out);

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
        assert!(matches!(&out, Ok(s) if s.contains("0 items")), "got: {:?}", out);

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
    async fn mark_abandoned_todos_emits_nothing_when_no_list_was_ever_written() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        mark_abandoned_todos(&bus, &pool, thread_id, i64::MAX).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn mark_abandoned_todos_emits_nothing_when_all_items_completed() {
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

        mark_abandoned_todos(&bus, &pool, thread_id, i64::MAX).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn mark_abandoned_todos_emits_nothing_when_list_is_already_empty() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        todo_write_impl(&bus, &json!({"todos": []}), thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        mark_abandoned_todos(&bus, &pool, thread_id, i64::MAX).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn mark_abandoned_todos_flips_in_progress_and_pending_keeps_completed() {
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

        mark_abandoned_todos(&bus, &pool, thread_id, i64::MAX).await;

        let flipped = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(flipped.len(), 3, "list preserved, got {:?}", flipped);
        assert_eq!(flipped[0].status, TodoStatus::Completed, "completed kept");
        assert_eq!(flipped[1].status, TodoStatus::Abandoned, "in_progress flipped");
        assert_eq!(flipped[2].status, TodoStatus::Abandoned, "pending flipped");
        assert_eq!(flipped[0].content, "a");
        assert_eq!(flipped[1].content, "b");
        assert_eq!(flipped[2].content, "c");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn mark_abandoned_todos_flips_pending_only_list() {
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

        mark_abandoned_todos(&bus, &pool, thread_id, i64::MAX).await;

        let flipped = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(flipped.len(), 2);
        assert_eq!(flipped[0].status, TodoStatus::Completed);
        assert_eq!(flipped[1].status, TodoStatus::Abandoned);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn mark_abandoned_todos_is_idempotent_once_already_marked() {
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
        mark_abandoned_todos(&bus, &pool, thread_id, i64::MAX).await;
        let _flipped = next_todo_event(&mut rx, thread_id).await;

        // Second call is a no-op.
        mark_abandoned_todos(&bus, &pool, thread_id, i64::MAX).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn mark_abandoned_todos_uses_only_the_latest_list_state() {
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

        mark_abandoned_todos(&bus, &pool, thread_id, i64::MAX).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn mark_abandoned_todos_skips_when_newer_list_written_after_terminator() {
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
        let terminator_seq: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM events WHERE thread_id = $1")
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
        mark_abandoned_todos(&bus, &pool, thread_id, terminator_seq).await;
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
}
