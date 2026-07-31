use super::*;
use serde_json::json;

#[test]
fn todo_status_serializes_as_snake_case() {
    use crate::engine::thread_events::TodoStatus;
    assert_eq!(
        serde_json::to_value(TodoStatus::Pending).unwrap(),
        json!("pending")
    );
    assert_eq!(
        serde_json::to_value(TodoStatus::InProgress).unwrap(),
        json!("in_progress")
    );
    assert_eq!(
        serde_json::to_value(TodoStatus::Completed).unwrap(),
        json!("completed")
    );
}

#[test]
fn todo_list_written_serializes_with_type_tag_and_items() {
    use crate::engine::thread_events::{TodoItem, TodoStatus};
    let event = ThreadEvent::TodoListWritten {
        items: vec![
            TodoItem {
                content: "Run tests".into(),
                active_form: "Running tests".into(),
                status: TodoStatus::InProgress,
            },
            TodoItem {
                content: "Write docs".into(),
                active_form: "Writing docs".into(),
                status: TodoStatus::Pending,
            },
        ],
    };
    let serialized = serde_json::to_value(&event).unwrap();
    assert_eq!(serialized["type"], "TodoListWritten");
    assert_eq!(serialized["items"][0]["content"], "Run tests");
    assert_eq!(serialized["items"][0]["active_form"], "Running tests");
    assert_eq!(serialized["items"][0]["status"], "in_progress");
    assert_eq!(serialized["items"][1]["status"], "pending");
}

#[test]
fn todo_list_written_round_trips_through_serde_with_empty_items() {
    let event = ThreadEvent::TodoListWritten { items: Vec::new() };
    let json = serde_json::to_value(&event).unwrap();
    let parsed: ThreadEvent = serde_json::from_value(json.clone()).unwrap();
    let reserialized = serde_json::to_value(&parsed).unwrap();
    assert_eq!(json, reserialized);
    assert_eq!(parsed.event_type(), "TodoListWritten");
}

#[test]
fn todo_list_written_round_trips_through_serde_with_all_three_statuses() {
    use crate::engine::thread_events::{TodoItem, TodoStatus};
    let event = ThreadEvent::TodoListWritten {
        items: vec![
            TodoItem {
                content: "a".into(),
                active_form: "doing a".into(),
                status: TodoStatus::Pending,
            },
            TodoItem {
                content: "b".into(),
                active_form: "doing b".into(),
                status: TodoStatus::InProgress,
            },
            TodoItem {
                content: "c".into(),
                active_form: "doing c".into(),
                status: TodoStatus::Completed,
            },
        ],
    };
    let json = serde_json::to_value(&event).unwrap();
    let parsed: ThreadEvent = serde_json::from_value(json.clone()).unwrap();
    let reserialized = serde_json::to_value(&parsed).unwrap();
    assert_eq!(json, reserialized);
}

#[test]
fn todo_list_written_event_type_is_todo_list_written() {
    let event = ThreadEvent::TodoListWritten { items: Vec::new() };
    assert_eq!(event.event_type(), "TodoListWritten");
}

/// Persisted so the sticky panel restores after engine restart by replaying
/// the events table — without persistence the panel would be empty until
/// the next `todo_write` call.
#[test]
fn todo_list_written_is_persisted() {
    let event = ThreadEvent::TodoListWritten { items: Vec::new() };
    assert!(event.is_persisted());
}

/// NOT per-token streaming — a single full-list payload per call, not a
/// firehose. Triggers may legitimately subscribe via `on_event:`.
#[test]
fn todo_list_written_is_not_per_token_streaming() {
    let event = ThreadEvent::TodoListWritten { items: Vec::new() };
    assert!(!event.is_per_token_streaming());
}
