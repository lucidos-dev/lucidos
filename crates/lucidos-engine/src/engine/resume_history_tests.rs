use crate::core::store::messages::build_session_messages;
use crate::core::EventRow;
use chrono::{TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

fn make_event(event_type: &str, payload: serde_json::Value, secs: i64) -> EventRow {
    EventRow {
        id: Uuid::new_v4(),
        event_type: event_type.to_string(),
        payload,
        created: Utc.timestamp_opt(1700000000 + secs, 0).unwrap(),
        thread_id: None,
        sequence: None,
    }
}

#[test]
fn orchestrator_resume_history_preserves_tool_calls() {
    let events = vec![
        make_event(
            "TriggerStarted",
            json!({"prompt": "Run nightly CI pipeline"}),
            0,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "load_knowhow", "args": {"id": "ops/nightly-pipeline"}}),
            1,
        ),
        make_event(
            "ToolResult",
            json!({"name": "load_knowhow", "success": true, "result": "<knowhow body>"}),
            2,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "emit_event", "args": {"event_type": "BuildClean", "payload": {}}}),
            3,
        ),
        make_event(
            "ToolResult",
            json!({"name": "emit_event", "success": true, "result": "Event BuildClean emitted"}),
            4,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "run_coding_agent", "args": {"prompt": "/harden-project"}}),
            5,
        ),
        make_event(
            "ToolResult",
            json!({"name": "run_coding_agent", "success": true, "result": "Claude Code session started"}),
            6,
        ),
        make_event("TextStreamed", json!({"text": "Step 2 started."}), 7),
        make_event("ResponseGenerated", json!({"text": "Step 2 started."}), 8),
        make_event(
            "MessageReceived",
            json!({"text": "[Child completed] Hardening complete. Session can finish."}),
            9,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 3);
    let assistant = &msgs[1];
    assert_eq!(assistant.role, "assistant");
    assert_eq!(
        assistant.steps.len(),
        3,
        "all three tool calls should be attached as steps"
    );

    let summary =
        crate::engine::format_history_steps(&assistant.steps, &std::collections::HashSet::new())
            .expect("orchestrator turn with tool calls must produce a history summary");
    assert!(
        summary.contains("load_knowhow") || summary.contains("know-how"),
        "summary should mention load_knowhow, got: {}",
        summary
    );
    assert!(
        summary.contains("BuildClean"),
        "summary should mention the emitted event type, got: {}",
        summary
    );
    assert!(
        summary.contains("run_coding_agent") || summary.contains("Claude Code"),
        "summary should mention run_coding_agent, got: {}",
        summary
    );
}
