use super::ToolOutcome;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent, TodoItem, TodoStatus};
pub(crate) use crate::engine::todo_state::latest_todo_list;
#[cfg(test)]
use crate::engine::todo_state::settle_open_todos;
use crate::engine::todo_state::{holds_live_child, TodoSnapshot};
use crate::engine::LucidosEngine;
use sqlx::PgPool;
use uuid::Uuid;

pub(crate) const MAX_TODO_ITEMS: usize = 50;

const ACTION_WRITE: &str = "write";
const ACTION_READ: &str = "read";

pub(crate) async fn todo_tool_impl(
    event_bus: &EventBus,
    pool: &PgPool,
    args: &serde_json::Value,
    thread_id: uuid::Uuid,
) -> ToolOutcome {
    let action = match args.get("action") {
        None | Some(serde_json::Value::Null) => ACTION_WRITE,
        Some(serde_json::Value::String(raw)) => raw.as_str(),
        Some(_) => {
            return Err(format!(
                "Error: `action` must be a string: `{ACTION_WRITE}` (the default) or \
                 `{ACTION_READ}`"
            ))
        }
    };
    match action {
        ACTION_WRITE => todo_write(event_bus, args, thread_id).await,
        ACTION_READ if args.get("todos").is_some() => Err(format!(
            "Error: `action: \"{ACTION_READ}\"` takes no `todos`. Omit them to read the list, \
             or drop `action` to write the ones you passed."
        )),
        ACTION_READ => todo_read(pool, thread_id).await,
        other => Err(format!(
            "Error: unknown `action` `{other}`; use `{ACTION_WRITE}` (the default) or \
             `{ACTION_READ}`"
        )),
    }
}

async fn todo_read(pool: &PgPool, thread_id: uuid::Uuid) -> ToolOutcome {
    let snapshot = latest_todo_list(pool, thread_id)
        .await
        .map_err(|e| format!("Error: could not read the todo list: {}", e))?;
    let Some(list) = snapshot else {
        return Ok("No todo list on this thread yet.".to_string());
    };
    if list.items.is_empty() {
        return Ok(match list.notes.as_deref() {
            Some(notes) => format!("Todo list is empty.\nNotes: {notes}"),
            None => "Todo list is empty.".to_string(),
        });
    }
    Ok(format!(
        "Todo list, {} items:\n{}",
        list.items.len(),
        render_todo_list(&list.items, list.notes.as_deref()),
    ))
}

fn render_todo_list(items: &[TodoItem], notes: Option<&str>) -> String {
    let mut out = String::new();
    for (idx, item) in items.iter().enumerate() {
        out.push_str(&format!(
            "{}. [{}] {} ({})\n",
            idx + 1,
            item.status.as_str(),
            item.content,
            item.active_form,
        ));
    }
    if let Some(notes) = notes {
        out.push_str(&format!("Notes: {notes}\n"));
    }
    if items
        .iter()
        .any(|item| matches!(item.status, TodoStatus::Waiting | TodoStatus::Abandoned))
    {
        out.push_str(
            "`waiting` and `abandoned` are the engine's: it settles whatever you left \
             unfinished when a response ended. Write the list again to correct anything you \
             have since finished or dropped.\n",
        );
    }
    out
}

pub(crate) fn turn_start_block(
    mode: crate::engine::chat::process::context_mode::ContextMode,
    snapshot: Option<&TodoSnapshot>,
) -> String {
    if mode.is_on() {
        return String::new();
    }
    let Some(list) = snapshot else {
        return String::new();
    };
    if list.items.is_empty() || list.all_completed() {
        return String::new();
    }
    format!(
        "[TODO LIST]\nYour list, as the user sees it in the prompt bar right now:\n{}\
         [END TODO LIST]",
        render_todo_list(&list.items, list.notes.as_deref()),
    )
}

async fn todo_write(
    event_bus: &EventBus,
    args: &serde_json::Value,
    thread_id: uuid::Uuid,
) -> ToolOutcome {
    let raw_items = match args.get("todos").and_then(|v| v.as_array()) {
        Some(items) => items,
        None if args.get("todos").is_none() => {
            return Err(format!(
                "Error: `todos` is required to {ACTION_WRITE} the list (pass `[]` to clear it). \
                 Only `action: \"{ACTION_READ}\"` may omit it."
            ))
        }
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

    let notes = match args.get("notes") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(raw)) => {
            let trimmed = raw.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Some(_) => return Err("Error: `notes` must be a string".to_string()),
    };

    let answer = if items.is_empty() {
        match notes.as_deref() {
            Some(notes) => format!("Todo list cleared.\nNotes: {notes}"),
            None => "Todo list cleared.".to_string(),
        }
    } else {
        format!(
            "Todo list updated, {} items:\n{}",
            items.len(),
            render_todo_list(&items, notes.as_deref()),
        )
    };

    event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::TodoListWritten { items, notes },
            meta: EventMeta::NONE,
        })
        .await
        .map_err(|e| format!("Error: failed to emit TodoListWritten: {}", e))?;

    Ok(answer)
}

pub(crate) async fn open_todo_count(pool: &PgPool, thread_id: Uuid) -> Result<usize, sqlx::Error> {
    Ok(latest_todo_list(pool, thread_id).await?.map_or(0, |list| {
        list.items.iter().filter(|i| i.status.is_open()).count()
    }))
}

#[easy_ext::ext(TodoExt)]
pub(crate) impl LucidosEngine {
    /// INVARIANT: The two facts the *wake check* decides on, as
    /// `(open items, something will re-open this thread)`.
    ///
    /// The open count is `None` when the read failed, which is UNKNOWN rather
    /// than zero. `Some(0)` means the list genuinely holds nothing open.
    /// `should_nudge_unwatched_turn` combines them with the per-turn bound.
    ///
    /// **The probe order is the cost control.** Both registry reads are
    /// in-memory, and either one true settles the answer. So they run FIRST and
    /// skip the queries, which keeps a parked thread free. The open count runs
    /// next, and the live-child query only when items are open, so an ordinary
    /// turn with no list pays one query.
    ///
    /// **Current state is read, not the settle's as-of answers.** This runs
    /// inside the turn, before any terminator exists. There is no sequence to
    /// be as-of, so the registries and the child rows are the current truth.
    /// The settle is an async consumer and must answer as of its terminator,
    /// which is why it cannot share these readers.
    async fn wake_check_facts(&self, thread_id: Uuid) -> (Option<usize>, bool) {
        let covered = !self.live_waits.for_thread(thread_id).await.is_empty()
            || self.bash_background.has_running_for_thread(thread_id).await;
        if covered {
            return (None, true);
        }

        let open = match open_todo_count(&self.pool, thread_id).await {
            Ok(0) => return (Some(0), false),
            Ok(open) => open,
            Err(e) => {
                log_legacy!(
                    "[Todo] Could not read the todo list for thread {}: {}. \
                     Not asking whether this turn leaves work unwatched.",
                    thread_id,
                    e
                );
                return (None, false);
            }
        };
        let child_probe = holds_live_child(&self.pool, thread_id).await;
        if let Err(e) = &child_probe {
            log_legacy!(
                "[Todo] Could not read the live children of thread {}: {}. \
                 Not asking whether this turn leaves work unwatched.",
                thread_id,
                e
            );
        }
        wake_facts_given_child_probe(open, child_probe)
    }
}

fn wake_facts_given_child_probe(
    open: usize,
    holds_live_child: Result<bool, sqlx::Error>,
) -> (Option<usize>, bool) {
    match holds_live_child {
        Ok(covered) => (Some(open), covered),
        Err(_) => (None, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::chat::process::context_mode::ContextMode;
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
                    event: ThreadEvent::TodoListWritten { items, .. },
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

        let out = todo_tool_impl(&bus, &pool, &args, thread_id).await;
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
    async fn a_write_answers_with_the_resulting_list() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "Read code",  "active_form": "Reading code",  "status": "completed" },
                { "content": "Write tests","active_form": "Writing tests", "status": "in_progress" },
            ]
        });
        let answer = todo_tool_impl(&bus, &pool, &args, thread_id)
            .await
            .expect("the write succeeds");

        for expected in [
            "Read code",
            "Reading code",
            "completed",
            "Write tests",
            "Writing tests",
            "in_progress",
        ] {
            assert!(
                answer.contains(expected),
                "the answer dropped `{expected}`: {answer}"
            );
        }

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_read_answers_with_the_list_and_writes_nothing() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_list(&bus, &pool, thread_id, &["in_progress", "pending"]).await;
        let before = latest_todo_list(&pool, thread_id)
            .await
            .expect("the list reads")
            .expect("a list was written");
        drain_todo_events_for(&mut rx, thread_id).await;

        let answer = todo_tool_impl(&bus, &pool, &json!({ "action": "read" }), thread_id)
            .await
            .expect("the read succeeds");
        assert!(answer.contains("item 0"), "got: {answer}");
        assert!(answer.contains("in_progress"), "got: {answer}");

        assert_no_todo_cleanup(&mut rx, thread_id).await;
        let after = latest_todo_list(&pool, thread_id)
            .await
            .expect("the list reads")
            .expect("a list is still there");
        assert_eq!(before, after, "the read moved the stored list");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_read_reports_the_statuses_the_engine_assigned() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_list(&bus, &pool, thread_id, &["completed", "in_progress"]).await;
        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
        drain_todo_events_for(&mut rx, thread_id).await;

        let answer = todo_tool_impl(&bus, &pool, &json!({ "action": "read" }), thread_id)
            .await
            .expect("the read succeeds");
        assert!(answer.contains("abandoned"), "got: {answer}");
        assert!(
            answer.contains("are the engine's"),
            "the read has to say whose status that is: {answer}"
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_read_of_no_list_says_so() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let never = todo_tool_impl(&bus, &pool, &json!({ "action": "read" }), thread_id)
            .await
            .expect("the read succeeds");
        assert!(never.contains("No todo list"), "got: {never}");

        todo_tool_impl(&bus, &pool, &json!({ "todos": [] }), thread_id)
            .await
            .expect("clear");
        let cleared = todo_tool_impl(&bus, &pool, &json!({ "action": "read" }), thread_id)
            .await
            .expect("the read succeeds");
        assert!(cleared.contains("empty"), "got: {cleared}");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn an_unknown_or_wrong_typed_action_is_refused() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_list(&bus, &pool, thread_id, &["in_progress"]).await;

        for bad in [json!("reed"), json!("delete"), json!(42), json!(["read"])] {
            let out = todo_tool_impl(&bus, &pool, &json!({ "action": bad }), thread_id).await;
            assert!(
                matches!(&out, Err(msg) if msg.contains("write") && msg.contains("read")),
                "got: {:?}",
                out,
            );
        }

        let list = latest_todo_list(&pool, thread_id)
            .await
            .expect("the list reads")
            .expect("a list is still there");
        assert_eq!(list.items.len(), 1, "a refused call wrote something");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_null_action_is_the_write_default() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "action": null,
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "pending" },
            ]
        });
        let out = todo_tool_impl(&bus, &pool, &args, thread_id).await;
        assert!(
            matches!(&out, Ok(s) if s.contains("Todo list updated")),
            "got: {:?}",
            out,
        );

        let list = latest_todo_list(&pool, thread_id)
            .await
            .expect("the list reads")
            .expect("the write landed");
        assert_eq!(list.items[0].content, "a");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_read_carrying_a_list_is_refused() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_list(&bus, &pool, thread_id, &["in_progress"]).await;
        let args = json!({
            "action": "read",
            "todos": [
                { "content": "b", "active_form": "doing b", "status": "completed" },
            ]
        });
        let out = todo_tool_impl(&bus, &pool, &args, thread_id).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("takes no `todos`")),
            "got: {:?}",
            out,
        );

        let list = latest_todo_list(&pool, thread_id)
            .await
            .expect("the list reads")
            .expect("a list is still there");
        assert_eq!(list.items[0].content, "item 0", "the refusal wrote nothing");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_read_sees_only_its_own_threads_list() {
        let (bus, _rx, pool, db) = setup().await;
        let mine = Uuid::new_v4();
        let theirs = Uuid::new_v4();

        todo_tool_impl(
            &bus,
            &pool,
            &json!({"todos": [
                { "content": "mine", "active_form": "doing mine", "status": "pending" },
            ]}),
            mine,
        )
        .await
        .expect("write mine");
        todo_tool_impl(
            &bus,
            &pool,
            &json!({"todos": [
                { "content": "theirs", "active_form": "doing theirs", "status": "pending" },
            ]}),
            theirs,
        )
        .await
        .expect("write theirs");

        let answer = todo_tool_impl(&bus, &pool, &json!({ "action": "read" }), mine)
            .await
            .expect("the read succeeds");
        assert!(answer.contains("mine"), "got: {answer}");
        assert!(!answer.contains("theirs"), "leaked another list: {answer}");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    fn snapshot(statuses: &[TodoStatus]) -> TodoSnapshot {
        TodoSnapshot {
            items: statuses
                .iter()
                .enumerate()
                .map(|(i, status)| TodoItem {
                    content: format!("item {i}"),
                    active_form: format!("doing item {i}"),
                    status: *status,
                })
                .collect(),
            notes: None,
            sequence: 1,
        }
    }

    #[test]
    fn the_wake_check_counts_a_live_child_as_covered() {
        assert_eq!(wake_facts_given_child_probe(2, Ok(true)), (Some(2), true));
        assert_eq!(wake_facts_given_child_probe(2, Ok(false)), (Some(2), false));
        assert_eq!(
            wake_facts_given_child_probe(2, Err(sqlx::Error::PoolClosed)),
            (None, false)
        );
    }

    #[test]
    fn the_turn_start_block_is_empty_when_there_is_nothing_to_reconcile() {
        assert_eq!(turn_start_block(ContextMode::Off, None), "");
        assert_eq!(turn_start_block(ContextMode::Off, Some(&snapshot(&[]))), "");
        assert_eq!(
            turn_start_block(
                ContextMode::Off,
                Some(&snapshot(&[TodoStatus::Completed, TodoStatus::Completed]))
            ),
            ""
        );
    }

    #[test]
    fn the_turn_start_block_is_empty_under_the_context_mode() {
        let unfinished = snapshot(&[TodoStatus::Abandoned]);
        assert!(!turn_start_block(ContextMode::Off, Some(&unfinished)).is_empty());
        assert_eq!(turn_start_block(ContextMode::On, Some(&unfinished)), "");
    }

    #[test]
    fn the_turn_start_block_carries_a_settled_list() {
        let block = turn_start_block(
            ContextMode::Off,
            Some(&snapshot(&[TodoStatus::Completed, TodoStatus::Abandoned])),
        );
        assert!(block.starts_with("[TODO LIST]"), "got: {block}");
        assert!(block.ends_with("[END TODO LIST]"), "got: {block}");
        assert!(block.contains("abandoned"), "got: {block}");
        assert!(block.contains("are the engine's"), "got: {block}");
        assert!(block.contains("doing item 1"), "got: {block}");
    }

    #[test]
    fn the_settle_note_appears_only_on_a_settled_list() {
        let untouched = render_todo_list(&snapshot(&[TodoStatus::Pending]).items, None);
        assert!(!untouched.contains("are the engine's"), "got: {untouched}");
        let parked = render_todo_list(&snapshot(&[TodoStatus::Waiting]).items, None);
        assert!(parked.contains("are the engine's"), "got: {parked}");
        let dropped = render_todo_list(&snapshot(&[TodoStatus::Abandoned]).items, None);
        assert!(dropped.contains("are the engine's"), "got: {dropped}");
    }

    async fn latest_notes(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
        latest_todo_list(pool, thread_id)
            .await
            .expect("the list reads")
            .and_then(|list| list.notes)
    }

    #[tokio::test]
    async fn todo_write_keeps_notes_beside_the_items_and_replaces_them_whole() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let with_notes = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "pending" },
            ],
            "notes": "collect.sh needs bash 5, /opt/homebrew/bin/bash works",
        });
        let out = todo_tool_impl(&bus, &pool, &with_notes, thread_id).await;
        assert!(
            matches!(&out, Ok(s) if s.contains("Notes: collect.sh needs bash 5")),
            "the answer carries the notes back, got: {:?}",
            out
        );
        assert_eq!(
            latest_notes(&pool, thread_id).await.as_deref(),
            Some("collect.sh needs bash 5, /opt/homebrew/bin/bash works")
        );

        let without = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "completed" },
            ]
        });
        todo_tool_impl(&bus, &pool, &without, thread_id)
            .await
            .expect("second write");
        assert_eq!(latest_notes(&pool, thread_id).await, None);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_treats_a_blank_note_as_no_note() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({ "todos": [], "notes": "   \n  " });
        let out = todo_tool_impl(&bus, &pool, &args, thread_id).await;
        assert!(
            matches!(&out, Ok(s) if !s.contains("Notes:")),
            "got: {:?}",
            out
        );
        assert_eq!(latest_notes(&pool, thread_id).await, None);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_refuses_a_non_string_note() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        todo_tool_impl(
            &bus,
            &pool,
            &json!({"todos": [], "notes": "kept"}),
            thread_id,
        )
        .await
        .expect("seed write");

        for bad in [json!(42), json!(["a"]), json!({"text": "a"}), json!(true)] {
            let out =
                todo_tool_impl(&bus, &pool, &json!({"todos": [], "notes": bad}), thread_id).await;
            assert!(
                matches!(&out, Err(msg) if msg.contains("`notes` must be a string")),
                "got: {:?}",
                out,
            );
        }
        assert_eq!(
            latest_notes(&pool, thread_id).await.as_deref(),
            Some("kept"),
            "a refused call must not have written anything"
        );

        todo_tool_impl(&bus, &pool, &json!({"todos": [], "notes": null}), thread_id)
            .await
            .expect("null clears");
        assert_eq!(latest_notes(&pool, thread_id).await, None);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settling_the_list_leaves_the_notes_alone() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ],
            "notes": "the report is at artifacts/reports/week.md",
        });
        todo_tool_impl(&bus, &pool, &args, thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
        let settled = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(settled[0].status, TodoStatus::Abandoned);
        assert_eq!(
            latest_notes(&pool, thread_id).await.as_deref(),
            Some("the report is at artifacts/reports/week.md"),
            "the settle must not erase the agent's notes"
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_accepts_empty_list_as_clear() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let out = todo_tool_impl(&bus, &pool, &json!({"todos": []}), thread_id).await;
        assert!(
            matches!(&out, Ok(s) if s.contains("cleared")),
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

        for args in [json!({}), json!({ "action": "write" })] {
            let out = todo_tool_impl(&bus, &pool, &args, thread_id).await;
            assert!(
                matches!(&out, Err(msg) if msg.contains("`todos` is required")
                    && msg.contains("read")),
                "got: {:?}",
                out,
            );
        }

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn todo_write_rejects_non_array_todos() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let out = todo_tool_impl(&bus, &pool, &json!({"todos": "nope"}), thread_id).await;
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
        let out = todo_tool_impl(&bus, &pool, &json!({"todos": items}), thread_id).await;
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
        let out = todo_tool_impl(&bus, &pool, &args, thread_id).await;
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
        let out = todo_tool_impl(&bus, &pool, &args, thread_id).await;
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
        let out = todo_tool_impl(&bus, &pool, &args, thread_id).await;
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
        let out = todo_tool_impl(&bus, &pool, &args, thread_id).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("invalid") || msg.contains("status")),
            "got: {:?}",
            out,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

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
                        event: ThreadEvent::TodoListWritten { items, .. },
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

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
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
        todo_tool_impl(&bus, &pool, &args, thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_emits_nothing_when_list_is_already_empty() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        todo_tool_impl(&bus, &pool, &json!({"todos": []}), thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
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
        todo_tool_impl(&bus, &pool, &args, thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;

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
        todo_tool_impl(&bus, &pool, &args, thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;

        let flipped = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(flipped.len(), 2);
        assert_eq!(flipped[0].status, TodoStatus::Completed);
        assert_eq!(flipped[1].status, TodoStatus::Abandoned);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_is_idempotent_once_already_marked() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "completed" },
                { "content": "b", "active_form": "doing b", "status": "in_progress" },
            ]
        });
        todo_tool_impl(&bus, &pool, &args, thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
        let _flipped = next_todo_event(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_uses_only_the_latest_list_state() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let stale = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_tool_impl(&bus, &pool, &stale, thread_id)
            .await
            .expect("seed stale");

        let fresh = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "completed" },
            ]
        });
        todo_tool_impl(&bus, &pool, &fresh, thread_id)
            .await
            .expect("seed fresh");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_skips_when_newer_list_written_after_terminator() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let stale = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_tool_impl(&bus, &pool, &stale, thread_id)
            .await
            .expect("seed stale");

        let terminator_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) FROM events WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("max seq");

        let fresh = json!({
            "todos": [
                { "content": "b", "active_form": "doing b", "status": "in_progress" },
                { "content": "c", "active_form": "doing c", "status": "pending" },
            ]
        });
        todo_tool_impl(&bus, &pool, &fresh, thread_id)
            .await
            .expect("seed fresh");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, terminator_seq, false).await;
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
        let out = todo_tool_impl(&bus, &pool, &args, thread_id).await;
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
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "waiting" },
            ]
        });
        let out = todo_tool_impl(&bus, &pool, &args, thread_id).await;
        assert!(
            matches!(&out, Err(msg) if msg.contains("waiting") && msg.contains("engine-only")),
            "got: {:?}",
            out,
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }

    async fn seed_event_wait(bus: &EventBus, thread_id: Uuid) -> Uuid {
        use crate::core::event_subscription::EventSubscription;
        let wait_id = Uuid::new_v4();
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::EventWaitStarted {
                background_timing: None,
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
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "completed" },
                { "content": "b", "active_form": "doing b", "status": "in_progress" },
                { "content": "c", "active_form": "doing c", "status": "pending" },
            ]
        });
        todo_tool_impl(&bus, &pool, &args, thread_id)
            .await
            .expect("seed write");
        seed_event_wait(&bus, thread_id).await;
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;

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
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_tool_impl(&bus, &pool, &args, thread_id)
            .await
            .expect("seed write");
        let wait_id = seed_event_wait(&bus, thread_id).await;
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
        let parked = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(parked[0].status, TodoStatus::Waiting);

        deliver_event_wait(&bus, thread_id, wait_id).await;
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
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
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_tool_impl(&bus, &pool, &args, thread_id)
            .await
            .expect("seed write");
        let wait_id = seed_event_wait(&bus, thread_id).await;

        let terminator_seq = max_seq(&pool, thread_id).await;

        deliver_event_wait(&bus, thread_id, wait_id).await;
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, terminator_seq, false).await;

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
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_tool_impl(&bus, &pool, &args, thread_id)
            .await
            .expect("seed write");
        seed_event_wait(&bus, thread_id).await;
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
        let _parked = next_todo_event(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn settle_open_todos_never_un_abandons_an_item_for_a_later_subscription() {
        let (bus, mut rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        let args = json!({
            "todos": [
                { "content": "a", "active_form": "doing a", "status": "in_progress" },
            ]
        });
        todo_tool_impl(&bus, &pool, &args, thread_id)
            .await
            .expect("seed write");
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
        let abandoned = next_todo_event(&mut rx, thread_id).await;
        assert_eq!(abandoned[0].status, TodoStatus::Abandoned);

        seed_event_wait(&bus, thread_id).await;
        drain_todo_events_for(&mut rx, thread_id).await;

        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;
        assert_no_todo_cleanup(&mut rx, thread_id).await;

        pool.close().await;
        teardown_test_db(&db).await;
    }

    async fn seed_list(bus: &EventBus, pool: &sqlx::PgPool, thread_id: Uuid, statuses: &[&str]) {
        let todos: Vec<_> = statuses
            .iter()
            .enumerate()
            .map(|(i, status)| {
                json!({
                    "content": format!("item {i}"),
                    "active_form": format!("doing item {i}"),
                    "status": status,
                })
            })
            .collect();
        todo_tool_impl(bus, pool, &json!({ "todos": todos }), thread_id)
            .await
            .expect("seed write");
    }

    async fn count_open(pool: &sqlx::PgPool, thread_id: Uuid) -> usize {
        open_todo_count(pool, thread_id)
            .await
            .expect("open_todo_count should succeed")
    }

    #[tokio::test]
    async fn open_todo_count_is_zero_for_every_shape_that_leaves_nothing_open() {
        let (bus, _rx, pool, db) = setup().await;

        let never_wrote = Uuid::new_v4();
        assert_eq!(count_open(&pool, never_wrote).await, 0, "no list ever");

        let cleared = Uuid::new_v4();
        seed_list(&bus, &pool, cleared, &[]).await;
        assert_eq!(count_open(&pool, cleared).await, 0, "list cleared");

        let finished = Uuid::new_v4();
        seed_list(&bus, &pool, finished, &["completed", "completed"]).await;
        assert_eq!(count_open(&pool, finished).await, 0, "all completed");

        let walked_away = Uuid::new_v4();
        seed_list(&bus, &pool, walked_away, &["completed", "in_progress"]).await;
        settle_open_todos(&bus, &pool, walked_away, i64::MAX, false).await;
        assert_eq!(count_open(&pool, walked_away).await, 0, "already abandoned");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn open_todo_count_counts_pending_and_in_progress() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_list(
            &bus,
            &pool,
            thread_id,
            &["completed", "in_progress", "pending", "pending"],
        )
        .await;

        assert_eq!(count_open(&pool, thread_id).await, 3);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn open_todo_count_treats_a_parked_item_as_open() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_list(&bus, &pool, thread_id, &["completed", "in_progress"]).await;
        seed_event_wait(&bus, thread_id).await;
        settle_open_todos(&bus, &pool, thread_id, i64::MAX, false).await;

        assert_eq!(count_open(&pool, thread_id).await, 1, "the parked item");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn open_todo_count_reads_only_the_newest_list() {
        let (bus, _rx, pool, db) = setup().await;
        let thread_id = Uuid::new_v4();

        seed_list(
            &bus,
            &pool,
            thread_id,
            &["in_progress", "pending", "pending"],
        )
        .await;
        seed_list(&bus, &pool, thread_id, &["completed"]).await;

        assert_eq!(count_open(&pool, thread_id).await, 0);

        pool.close().await;
        teardown_test_db(&db).await;
    }
}
