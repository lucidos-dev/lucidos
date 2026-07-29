use super::*;
use super::msg_helpers::*;

// === Thread-aware tests ===

#[test]
fn messages_with_thread_id_preserved() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "msg1", "request_id": "r1", "thread_id": "t1"}),
            0,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "resp1", "request_id": "r1", "thread_id": "t1"}),
            1,
        ),
        make_event(
            "MessageReceived",
            json!({"text": "msg2", "request_id": "r2", "thread_id": "t1"}),
            2,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "resp2", "request_id": "r2", "thread_id": "t1"}),
            3,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 4); // 2 user + 2 assistant
}

#[test]
fn different_threads_build_independently() {
    // Events from two different threads interleaved — build_session_messages
    // processes them in order regardless of thread_id (filtering happens upstream)
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "thread1-msg", "request_id": "r1", "thread_id": "t1"}),
            0,
        ),
        make_event(
            "MessageReceived",
            json!({"text": "thread2-msg", "request_id": "r2", "thread_id": "t2"}),
            1,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "resp1", "request_id": "r1", "thread_id": "t1"}),
            2,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "resp2", "request_id": "r2", "thread_id": "t2"}),
            3,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].content, "thread1-msg");
    assert_eq!(msgs[1].content, "thread2-msg");
}

#[test]
fn follow_up_in_same_thread_different_request_ids() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "first question", "request_id": "r1", "thread_id": "t1"}),
            0,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "first answer", "request_id": "r1", "thread_id": "t1"}),
            1,
        ),
        make_event(
            "MessageReceived",
            json!({"text": "follow-up question", "request_id": "r2", "thread_id": "t1"}),
            2,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "follow-up answer", "request_id": "r2", "thread_id": "t1"}),
            3,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 4);
    // First exchange
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].content, "first question");
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].content, "first answer");
    // Follow-up exchange
    assert_eq!(msgs[2].role, "user");
    assert_eq!(msgs[2].content, "follow-up question");
    assert_eq!(msgs[3].role, "assistant");
    assert_eq!(msgs[3].content, "follow-up answer");
}

/// Regression: when trigger TextStreamed events (no channel) leak into a
/// CC thread's event stream, the trigger text must NOT contaminate the CC
/// exchange's response. With upstream thread_id filtering, these events should never
/// appear in the same stream — but even if they do, the CC response must be correct.
#[test]
fn trigger_text_does_not_contaminate_cc_response() {
    // Simulates the bug: CC exchange interleaved with trigger TextStreamed
    // events that leaked through because they lacked channel: "trigger".
    let events = vec![
        make_event(
            "MessageReceived",
            json!({
                "text": "explain changes",
                "channel": "claude_code",
                "thread_id": "cc-thread",
                "request_id": "r1"
            }),
            0,
        ),
        make_event("Thinking", json!({"thread_id": "cc-thread"}), 1),
        // Scheduled task text leaks in (different thread_id, no channel)
        make_event(
            "TextStreamed",
            json!({
                "text": "Kontrollsløyfen er kjørt. Alle tre pumpene ble justert.",
                "thread_id": "sched-thread"
            }),
            2,
        ),
        // CC response arrives
        make_event(
            "CodingAgentTextStreamed",
            json!({
                "text": "Here are the changes explained.",
                "channel": "claude_code",
                "thread_id": "cc-thread"
            }),
            3,
        ),
        make_event(
            "ResponseGenerated",
            json!({
                "text": "Here are the changes explained.",
                "channel": "claude_code",
                "thread_id": "cc-thread",
                "request_event_id": "cc-event-id"
            }),
            4,
        ),
    ];
    let msgs = build_session_messages(&events);

    // Find the CC response (channel=claude_code)
    let cc_response = msgs
        .iter()
        .find(|m| m.role == "assistant" && m.channel.as_deref() == Some("claude_code"));
    assert!(cc_response.is_some(), "CC response should exist");
    let cc = cc_response.unwrap();
    assert_eq!(cc.content, "Here are the changes explained.");
    assert!(
        !cc.content.contains("Kontrollsløyfen"),
        "CC response must NOT contain scheduled trigger text"
    );
}

/// Result text extends beyond streamed text — extra content must appear in events.
/// This mirrors the live-streaming fix: CC may bundle trailing text into the Result
/// without a preceding Message event. The DB reconstruction must capture it as an event
/// so the events-based renderer shows the full text.
#[test]
fn result_text_extra_content_appears_in_events() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "run tests"}), 0),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "Let me run the tests.", "channel": "claude_code"}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "Bash", "input": {"command": "cargo test"}}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "Bash", "result": "ok", "success": true}),
            3,
        ),
        // ResponseGenerated has extra text ("All tests passed!") not in the streamed text
        make_event(
            "ResponseGenerated",
            json!({
                "text": "Let me run the tests.\n\nAll tests passed!",
                "channel": "claude_code"
            }),
            4,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    let assistant = &msgs[1];
    assert_eq!(assistant.role, "assistant");

    // The events array must contain the extra "All tests passed!" text
    let text_events: Vec<_> = assistant
        .events
        .iter()
        .filter_map(|e| match e {
            ResponseEvent::Text { md } if !md.trim().is_empty() => Some(md.as_str()),
            _ => None,
        })
        .collect();
    let combined = text_events.join("");
    assert!(
        combined.contains("All tests passed!"),
        "Extra result text must appear in events. Got text events: {:?}",
        text_events,
    );
}

/// Claude Code session with tool calls but no text yet (e.g. reload during first tool call).
/// Step events must be preserved in a trailing assistant message so the frontend
/// can show More/Less and Steps toggles.
#[test]
fn cc_tool_calls_without_text_preserves_step_events() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "fix the bug", "channel": "claude_code"}),
            0,
        ),
        // CC starts a tool call before sending any text
        make_event(
            "ToolCalled",
            json!({"name": "Read", "args": {"file_path": "src/main.rs"}}),
            1,
        ),
        make_event(
            "ToolResult",
            json!({"name": "Read", "result": "fn main() {}", "success": true}),
            2,
        ),
        // No CodingAgentTextStreamed, no ResponseGenerated — reload mid-session
    ];
    let msgs = build_session_messages(&events);

    // Must have user + assistant (even though no text was streamed)
    assert_eq!(
        msgs.len(),
        2,
        "Expected user + assistant message, got {}",
        msgs.len()
    );
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].completed, None);

    // The assistant message must have step events
    let step_events: Vec<_> = msgs[1]
        .events
        .iter()
        .filter(|e| matches!(e, ResponseEvent::Step { .. }))
        .collect();
    assert!(
        !step_events.is_empty(),
        "Step events must be preserved when CC has tool calls but no text. Got events: {:?}",
        msgs[1].events,
    );
}

/// Claude Code session with tool calls and some text — step events interleaved with text.
/// Verifies that adding pending_events to the trailing flush doesn't break the
/// normal case where text exists alongside tool calls.
#[test]
fn cc_tool_calls_with_text_still_works() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "fix the bug", "channel": "claude_code"}),
            0,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "Let me read the file."}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "Read", "args": {"file_path": "src/main.rs"}}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "Read", "result": "fn main() {}", "success": true}),
            3,
        ),
        // No ResponseGenerated — reload mid-session
    ];
    let msgs = build_session_messages(&events);

    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].completed, None);

    // Must have both text and step events
    let has_text = msgs[1]
        .events
        .iter()
        .any(|e| matches!(e, ResponseEvent::Text { md } if !md.trim().is_empty()));
    let has_step = msgs[1]
        .events
        .iter()
        .any(|e| matches!(e, ResponseEvent::Step { .. }));
    assert!(has_text, "Text events expected");
    assert!(has_step, "Step events expected");
}

/// Regression: orchestrator turns whose entire output was tool calls had
/// `steps` on the SessionMessage but no surviving content for the LLM to
/// see on resume. End-to-end check: events → build_session_messages →
/// format_history_steps surfaces the tool names.
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
    // user (trigger), assistant (step 2 reply with 3 tool steps), user (callback)
    assert_eq!(msgs.len(), 3);
    let assistant = &msgs[1];
    assert_eq!(assistant.role, "assistant");
    assert_eq!(
        assistant.steps.len(),
        3,
        "all three tool calls should be attached as steps"
    );

    // The fix: format_history_steps must surface a non-empty summary for an
    // assistant turn whose entire output was tool calls.
    let summary = crate::engine::format_history_steps(&assistant.steps, &std::collections::HashSet::new())
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

#[test]
fn build_session_messages_marks_tool_step_success_from_payload() {
    let events = vec![
        make_event(
            "ToolCalled",
            json!({"name": "load_knowhow", "args": {"id": "x"}, "description": "Loading"}),
            0,
        ),
        make_event(
            "ToolResult",
            json!({"name": "load_knowhow", "result": "body", "success": true}),
            1,
        ),
        make_event("ResponseGenerated", json!({"text": "ok"}), 2),
    ];
    let msgs = build_session_messages(&events);
    let assistant = msgs.iter().find(|m| m.role == "assistant").expect("assistant");
    assert_eq!(assistant.steps.len(), 1);
    assert!(assistant.steps[0].success, "step.success should be true");
}

#[test]
fn build_session_messages_legacy_tool_result_without_success_defaults_to_true() {
    // Historical events in the event store lack the `success` key. The
    // projection must default to `true` to match the new event-schema default
    // and avoid the long-standing "everything looks failed" bug on resume.
    let events = vec![
        make_event(
            "ToolCalled",
            json!({"name": "x", "args": {}, "description": "d"}),
            0,
        ),
        make_event("ToolResult", json!({"name": "x", "result": "ok"}), 1),
        make_event("ResponseGenerated", json!({"text": "ok"}), 2),
    ];
    let msgs = build_session_messages(&events);
    let assistant = msgs.iter().find(|m| m.role == "assistant").expect("assistant");
    assert!(assistant.steps[0].success, "missing success defaults to true");
}
