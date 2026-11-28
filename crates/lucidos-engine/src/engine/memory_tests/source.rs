use super::LucidosEngine;
use crate::core::EventRow;

#[test]
fn trigger_completed_is_not_indexed() {
    let event = EventRow::new(
        "TriggerCompleted",
        serde_json::json!({
            "trigger_id": "t-1",
            "trigger_name": "Calendar sync",
            "result_summary": "Synced 47 events from Google Calendar",
        }),
    );
    let content = LucidosEngine::memory_content_for_event(&event);
    assert!(
        content.is_none(),
        "Trigger events should not be indexed into memory"
    );
}

#[test]
fn message_received_produces_indexable_content() {
    let msg = EventRow::new(
        "MessageReceived",
        serde_json::json!({"text": "Hello world"}),
    );
    let content = LucidosEngine::memory_content_for_event(&msg);
    assert_eq!(content.unwrap(), "Hello world");
}

#[test]
fn trigger_channel_events_not_indexed() {
    let msg = EventRow::new(
        "MessageReceived",
        serde_json::json!({
            "text": "Check weather",
            "channel": "trigger",
        }),
    );
    assert!(
        LucidosEngine::memory_content_for_event(&msg).is_none(),
        "MessageReceived in trigger channel should not be indexed"
    );

    let regular = EventRow::new("MessageReceived", serde_json::json!({"text": "Hello"}));
    assert!(LucidosEngine::memory_content_for_event(&regular).is_some());
}

#[test]
fn canonicalize_artifact_path_strips_prefix() {
    assert_eq!(
        LucidosEngine::canonicalize_artifact_path("artifacts/output.csv"),
        "output.csv"
    );
    assert_eq!(
        LucidosEngine::canonicalize_artifact_path("artifacts/subfolder/notes.md"),
        "subfolder/notes.md"
    );
}

#[test]
fn canonicalize_artifact_path_passes_through_already_canonical() {
    assert_eq!(
        LucidosEngine::canonicalize_artifact_path("notes.md"),
        "notes.md"
    );
    assert_eq!(
        LucidosEngine::canonicalize_artifact_path("email/2026-05/attachment.pdf"),
        "email/2026-05/attachment.pdf"
    );
}

#[test]
fn canonicalize_artifact_path_leaves_non_artifact_paths_alone() {
    assert_eq!(
        LucidosEngine::canonicalize_artifact_path("apps/foo/bar.txt"),
        "apps/foo/bar.txt"
    );
}
