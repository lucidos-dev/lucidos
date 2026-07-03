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
    // Events with channel="trigger" should be skipped regardless of type
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

    // Regular chat messages should still be indexed
    let regular = EventRow::new("MessageReceived", serde_json::json!({"text": "Hello"}));
    assert!(LucidosEngine::memory_content_for_event(&regular).is_some());
}

#[test]
fn canonicalize_artifact_path_strips_prefix() {
    // python.rs emits data-relative paths — under artifacts/ they look like this.
    assert_eq!(
        LucidosEngine::canonicalize_artifact_path("artifacts/output.csv"),
        "output.csv"
    );
    // Nested under artifacts/ also strips just the leading "artifacts/".
    assert_eq!(
        LucidosEngine::canonicalize_artifact_path("artifacts/subfolder/notes.md"),
        "subfolder/notes.md"
    );
}

#[test]
fn canonicalize_artifact_path_passes_through_already_canonical() {
    // files.rs / import.rs / email.rs strip the prefix at the emit site, so
    // the consumer must accept already-canonical paths unchanged.
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
    // A python tool writing under data/apps/ emits the data-relative path
    // — the caller's read_artifact() will then look under data/artifacts/apps/
    // and fail, which is the desired behavior (apps aren't memory-indexed,
    // matching what walk_artifact_history reports).
    assert_eq!(
        LucidosEngine::canonicalize_artifact_path("apps/foo/bar.txt"),
        "apps/foo/bar.txt"
    );
}
