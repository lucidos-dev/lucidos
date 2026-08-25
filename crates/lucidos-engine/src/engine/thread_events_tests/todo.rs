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
    // The two engine-only statuses. `waiting` is the wire word the panel
    // branches on for a thread parked on an *event wait*, so it is pinned here
    // beside the rest rather than only being asserted through the settle path.
    assert_eq!(
        serde_json::to_value(TodoStatus::Waiting).unwrap(),
        json!("waiting")
    );
    assert_eq!(
        serde_json::to_value(TodoStatus::Abandoned).unwrap(),
        json!("abandoned")
    );
}

/// `Completed` and `Abandoned` are terminal; everything else a response
/// terminator may still rewrite. `Waiting` being OPEN is what stops a parked
/// list reading as parked forever once its wait resolves.
#[test]
fn only_completed_and_abandoned_are_terminal() {
    use crate::engine::thread_events::TodoStatus;
    assert!(TodoStatus::Pending.is_open());
    assert!(TodoStatus::InProgress.is_open());
    assert!(TodoStatus::Waiting.is_open());
    assert!(!TodoStatus::Completed.is_open());
    assert!(!TodoStatus::Abandoned.is_open());
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
        notes: None,
    };
    let serialized = serde_json::to_value(&event).unwrap();
    assert_eq!(serialized["type"], "TodoListWritten");
    assert_eq!(serialized["items"][0]["content"], "Run tests");
    assert_eq!(serialized["items"][0]["active_form"], "Running tests");
    assert_eq!(serialized["items"][0]["status"], "in_progress");
    assert_eq!(serialized["items"][1]["status"], "pending");
}

/// ADR 0085's *todo notes*, and the two shapes they must serialize into.
///
/// Present, the field is on the wire. Absent, the key is not there at all. A
/// payload written before the field existed and one written with the mode off
/// are then byte-identical. Every stored row replays through this enum, so a
/// required field would break every list ever written.
#[test]
fn todo_list_written_carries_notes_only_when_there_are_notes() {
    let with = ThreadEvent::TodoListWritten {
        items: Vec::new(),
        notes: Some("collect.sh needs bash 5".into()),
    };
    let json = serde_json::to_value(&with).unwrap();
    assert_eq!(json["notes"], "collect.sh needs bash 5");
    let parsed: ThreadEvent = serde_json::from_value(json).unwrap();
    assert!(
        matches!(parsed, ThreadEvent::TodoListWritten { notes: Some(n), .. } if n.contains("bash 5"))
    );

    let without = ThreadEvent::TodoListWritten {
        items: Vec::new(),
        notes: None,
    };
    let json = serde_json::to_value(&without).unwrap();
    assert!(
        json.get("notes").is_none(),
        "an unnoted list must not carry the key at all: {json}"
    );

    // A row from before the field existed reads back as unnoted.
    let legacy: ThreadEvent =
        serde_json::from_value(serde_json::json!({"type": "TodoListWritten", "items": []}))
            .expect("a pre-notes payload still parses");
    assert!(matches!(
        legacy,
        ThreadEvent::TodoListWritten { notes: None, .. }
    ));
}

#[test]
fn todo_list_written_round_trips_through_serde_with_empty_items() {
    let event = ThreadEvent::TodoListWritten {
        items: Vec::new(),
        notes: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    let parsed: ThreadEvent = serde_json::from_value(json.clone()).unwrap();
    let reserialized = serde_json::to_value(&parsed).unwrap();
    assert_eq!(json, reserialized);
    assert_eq!(parsed.event_type(), "TodoListWritten");
}

#[test]
fn todo_list_written_round_trips_through_serde_with_every_status() {
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
            TodoItem {
                content: "d".into(),
                active_form: "doing d".into(),
                status: TodoStatus::Waiting,
            },
            TodoItem {
                content: "e".into(),
                active_form: "doing e".into(),
                status: TodoStatus::Abandoned,
            },
        ],
        notes: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    let parsed: ThreadEvent = serde_json::from_value(json.clone()).unwrap();
    let reserialized = serde_json::to_value(&parsed).unwrap();
    assert_eq!(json, reserialized);
}

#[test]
fn todo_list_written_event_type_is_todo_list_written() {
    let event = ThreadEvent::TodoListWritten {
        items: Vec::new(),
        notes: None,
    };
    assert_eq!(event.event_type(), "TodoListWritten");
}

/// Persisted so the sticky panel restores after engine restart by replaying
/// the events table — without persistence the panel would be empty until
/// the next `todo_write` call.
#[test]
fn todo_list_written_is_persisted() {
    let event = ThreadEvent::TodoListWritten {
        items: Vec::new(),
        notes: None,
    };
    assert!(event.is_persisted());
}

/// NOT per-token streaming — a single full-list payload per call, not a
/// firehose. Triggers may legitimately subscribe via `on_event:`.
#[test]
fn todo_list_written_is_not_per_token_streaming() {
    let event = ThreadEvent::TodoListWritten {
        items: Vec::new(),
        notes: None,
    };
    assert!(!event.is_per_token_streaming());
}
