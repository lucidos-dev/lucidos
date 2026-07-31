use super::msg_helpers::*;
use super::*;

// ---------------------------------------------------------------
// Events (ResponseEvent) tests
// ---------------------------------------------------------------

/// Thinking event creates a ResponseEvent::Step with context metadata.
#[test]
fn events_include_thinking_step() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "hello", "request_id": "s1"}),
            0,
        ),
        make_event(
            "Thinking",
            json!({
                "request_id": "s1",
                "context_tokens": 32000,
                "context_messages": 12,
                "trimmed": true,
            }),
            1,
        ),
        make_event("TextStreamed", json!({"text": "Hi!"}), 2),
        make_event("ResponseGenerated", json!({"text": "Hi!"}), 3),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    assert!(!assistant.events.is_empty());
    // First event should be the Thinking step
    match &assistant.events[0] {
        ResponseEvent::Step {
            description,
            context_tokens,
            context_messages,
            trimmed,
            ..
        } => {
            assert_eq!(description, "Requesting");
            assert_eq!(*context_tokens, Some(32000));
            assert_eq!(*context_messages, Some(12));
            assert_eq!(*trimmed, Some(true));
        }
        _ => panic!("Expected Step event, got Text"),
    }
}

/// MemorySearched event creates a ResponseEvent::Step.
#[test]
fn events_include_memory_searched_step() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "what's my birthday?", "request_id": "s1"}),
            0,
        ),
        make_event(
            "MemorySearched",
            json!({"results": 7, "queries": ["birthday", "date of birth"], "request_id": "s1"}),
            1,
        ),
        make_event(
            "Thinking",
            json!({"text": "Context: 1000 tokens, 5 messages", "request_id": "s1", "context_tokens": 1000, "context_messages": 5}),
            2,
        ),
        make_event(
            "TextStreamed",
            json!({"text": "Your birthday is Jan 1."}),
            3,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "Your birthday is Jan 1."}),
            4,
        ),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    let steps = step_events(&assistant.events);
    assert_eq!(
        steps.len(),
        2,
        "Should have MemorySearched + Thinking steps"
    );
    assert_eq!(steps[0], ("Memory searched", true));
    assert_eq!(steps[1], ("Requesting", true));
}

/// ToolResult populates detail on the preceding Step event.
#[test]
fn events_step_has_detail_from_tool_result() {
    let text = pad("Let me read that file for you now", 100);
    let events = vec![
        make_event("MessageReceived", json!({"text": "show file"}), 0),
        make_event("TextStreamed", json!({"text": &text}), 1),
        make_event(
            "ToolCalled",
            json!({"name": "read_file", "args": {"path": "foo.rs"}}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "read_file", "success": true, "result": "fn main() {}"}),
            3,
        ),
        make_event("TextStreamed", json!({"text": " Done reading."}), 4),
        make_event(
            "ResponseGenerated",
            json!({"text": &format!("{} Done reading.", text)}),
            5,
        ),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    // Find the step event for read_file
    let step = assistant
        .events
        .iter()
        .find(|e| matches!(e, ResponseEvent::Step { tool_name: Some(n), .. } if n == "read_file"));
    assert!(step.is_some(), "Should have a step event for read_file");
    if let Some(ResponseEvent::Step {
        detail, success, ..
    }) = step
    {
        assert!(*success);
        assert_eq!(*detail, Some("12 chars".to_string())); // "fn main() {}" = 12 chars
    }
}

/// ToolResult sets success=false when tool fails.
#[test]
fn events_step_success_updated_by_tool_result() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "read file"}), 0),
        make_event(
            "ToolCalled",
            json!({"name": "read_file", "args": {"path": "missing.rs"}}),
            1,
        ),
        make_event(
            "ToolResult",
            json!({"name": "read_file", "success": false, "result": "File not found: missing.rs"}),
            2,
        ),
        make_event(
            "TextStreamed",
            json!({"text": "Sorry, the file doesn't exist."}),
            3,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "Sorry, the file doesn't exist."}),
            4,
        ),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    let step = assistant
        .events
        .iter()
        .find(|e| matches!(e, ResponseEvent::Step { tool_name: Some(n), .. } if n == "read_file"));
    assert!(step.is_some());
    if let Some(ResponseEvent::Step {
        success, detail, ..
    }) = step
    {
        assert!(!*success);
        assert_eq!(*detail, Some("File not found: missing.rs".to_string()));
    }
}

/// text -> tool -> text produces [Text, Step, Text] event ordering.
#[test]
fn events_ordering_text_step_text() {
    let c1 = pad("Analyzing the file structure", 100);
    let c2 = pad(" Here are the results of the analysis", 100);
    let full = format!("{}{}", c1, c2);

    let events = vec![
        make_event("MessageReceived", json!({"text": "analyze"}), 0),
        make_event("TextStreamed", json!({"text": &c1}), 1),
        make_event(
            "ToolCalled",
            json!({"name": "read_file", "args": {"path": "a.rs"}}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "read_file", "success": true, "result": "code"}),
            3,
        ),
        make_event("TextStreamed", json!({"text": &c2}), 4),
        make_event("ResponseGenerated", json!({"text": &full}), 5),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];

    // Should be: Text, Step, Text
    assert_eq!(assistant.events.len(), 3, "events: {:?}", assistant.events);
    assert!(matches!(&assistant.events[0], ResponseEvent::Text { md } if md == &c1));
    assert!(matches!(&assistant.events[1], ResponseEvent::Step { .. }));
    assert!(matches!(&assistant.events[2], ResponseEvent::Text { md } if md == &c2));
}

/// tool -> text produces [Step, Text] (no empty text event before step).
#[test]
fn events_ordering_step_before_text() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "do it"}), 0),
        make_event(
            "ToolCalled",
            json!({"name": "search_artifacts", "args": {}}),
            1,
        ),
        make_event(
            "ToolResult",
            json!({"name": "search_artifacts", "success": true, "result": "match1\nmatch2"}),
            2,
        ),
        make_event(
            "TextStreamed",
            json!({"text": "Found 2 matches in the search results."}),
            3,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "Found 2 matches in the search results."}),
            4,
        ),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];

    // Should be: Step, Text — no empty text before step
    let texts = text_events(&assistant.events);
    let steps = step_events(&assistant.events);
    assert_eq!(steps.len(), 1);
    assert_eq!(texts.len(), 1);
    assert_eq!(texts[0], "Found 2 matches in the search results.");
    // Step should come before text in the array
    let step_idx = assistant
        .events
        .iter()
        .position(|e| matches!(e, ResponseEvent::Step { .. }))
        .unwrap();
    let text_idx = assistant
        .events
        .iter()
        .position(|e| matches!(e, ResponseEvent::Text { .. }))
        .unwrap();
    assert!(step_idx < text_idx, "Step should come before Text");
}

/// text -> tool -> tool -> text (small deltas) produces properly interleaved
/// [Text, Step, Text, Step, Text] — no merging, interleaving is preserved.
#[test]
fn events_ordering_multiple_steps_between_text() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "help"}), 0),
        make_event("TextStreamed", json!({"text": "Ok."}), 1),
        make_event(
            "ToolCalled",
            json!({"name": "search_artifacts", "args": {}}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "search_artifacts", "success": true, "result": "r1"}),
            3,
        ),
        make_event("TextStreamed", json!({"text": " Checking."}), 4),
        make_event(
            "ToolCalled",
            json!({"name": "read_file", "args": {"path": "a.rs"}}),
            5,
        ),
        make_event(
            "ToolResult",
            json!({"name": "read_file", "success": true, "result": "code"}),
            6,
        ),
        make_event(
            "TextStreamed",
            json!({"text": " Here is a thorough and detailed answer to your question based on all the information I gathered."}),
            7,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "Ok. Checking. Here is a thorough and detailed answer to your question based on all the information I gathered."}),
            8,
        ),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];

    // All text deltas create events regardless of size — interleaving matters
    let steps = step_events(&assistant.events);
    let texts = text_events(&assistant.events);
    assert_eq!(steps.len(), 2, "should have 2 step events");
    assert_eq!(
        texts.len(),
        3,
        "should have 3 text events (interleaved with steps)"
    );
    assert_eq!(texts[0], "Ok.");
    assert_eq!(texts[1], " Checking.");
}

/// Claude Code exchanges use the CC text buffer for events.
#[test]
fn events_cc_exchange_uses_cc_buffer() {
    let c1 = pad("Analyzing the codebase structure and patterns", 100);
    let c2 = pad(" Applied all changes successfully to the files", 100);
    let full = format!("{}{}", c1, c2);

    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "fix it", "request_id": "s1"}),
            0,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": &c1, "request_id": "s1"}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "edit_file", "args": {"path": "a.rs"}, "request_id": "s1"}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "edit_file", "success": true, "result": "done", "request_id": "s1"}),
            3,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": &c2, "request_id": "s1"}),
            4,
        ),
        {
            let mut e = make_event(
                "ResponseGenerated",
                json!({"text": &full, "request_id": "s1"}),
                5,
            );
            e.payload["channel"] = json!("claude_code");
            e
        },
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];

    // Should have Text, Step, Text events
    assert_eq!(
        assistant.events.len(),
        3,
        "CC events: {:?}",
        assistant.events
    );
    assert!(matches!(&assistant.events[0], ResponseEvent::Text { md } if md == &c1));
    assert!(
        matches!(&assistant.events[1], ResponseEvent::Step { tool_name: Some(n), .. } if n == "edit_file")
    );
    assert!(matches!(&assistant.events[2], ResponseEvent::Text { md } if md == &c2));
}

/// Events on canceled exchange.
#[test]
fn events_on_canceled_exchange() {
    let c1 = pad("Working on the requested changes to the codebase", 100);
    let c2 = pad(" Partial result before cancellation occurred here", 100);
    let full = format!("{}{}", c1, c2);

    let events = vec![
        make_event("MessageReceived", json!({"text": "do it"}), 0),
        make_event("TextStreamed", json!({"text": &c1}), 1),
        make_event("ToolCalled", json!({"name": "edit_file", "args": {}}), 2),
        make_event(
            "ToolResult",
            json!({"name": "edit_file", "success": true, "result": "ok"}),
            3,
        ),
        make_event("TextStreamed", json!({"text": &c2}), 4),
        make_event("ResponseCanceled", json!({"text": &full}), 5),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    assert!(assistant.canceled);
    let texts = text_events(&assistant.events);
    assert_eq!(texts.len(), 2);
    assert_eq!(texts[0], c1);
    assert_eq!(texts[1], c2);
}

/// Events on interrupted exchange.
#[test]
fn events_on_interrupted_exchange() {
    let c1 = pad("Starting the analysis of the requested files now", 100);
    let c2 = pad(" More detailed text about the analysis results here", 100);

    let events = vec![
        make_event("MessageReceived", json!({"text": "do it"}), 0),
        make_event(
            "TextStreamed",
            json!({"text": &c1, "channel": "claude_code"}),
            1,
        ),
        make_event("ToolCalled", json!({"name": "read_file", "args": {}}), 2),
        make_event(
            "ToolResult",
            json!({"name": "read_file", "success": true, "result": "ok"}),
            3,
        ),
        make_event(
            "TextStreamed",
            json!({"text": &c2, "channel": "claude_code"}),
            4,
        ),
        make_event("MessageReceived", json!({"text": "stop"}), 5),
        make_event("ResponseGenerated", json!({"text": "OK stopped."}), 6),
    ];
    let msgs = build_session_messages(&events);
    let interrupted = &msgs[1];
    assert_eq!(interrupted.completed, Some(false));
    let texts = text_events(&interrupted.events);
    assert_eq!(texts.len(), 2);
    assert_eq!(texts[0], c1);
    assert_eq!(texts[1], c2);
}

/// Events idempotent on replay.
#[test]
fn events_replay_idempotent() {
    let c1 = &pad("First text event content", 80);
    let c2 = &pad("Second text event content", 80);
    let full = format!("{}{}", c1, c2);
    let events = vec![
        make_event("MessageReceived", json!({"text": "q"}), 0),
        make_event("TextStreamed", json!({"text": c1}), 1),
        make_event("ToolCalled", json!({"name": "t", "args": {}}), 2),
        make_event(
            "ToolResult",
            json!({"name": "t", "success": true, "result": "r"}),
            3,
        ),
        make_event("TextStreamed", json!({"text": c2}), 4),
        make_event("ResponseGenerated", json!({"text": &full}), 5),
    ];
    let msgs1 = build_session_messages(&events);
    let msgs2 = build_session_messages(&events);
    let e1 = text_events(&msgs1[1].events);
    let e2 = text_events(&msgs2[1].events);
    assert_eq!(e1, e2, "events replay must be idempotent");
}

/// No events for a simple response without tools or streaming.
#[test]
fn events_empty_for_no_streaming() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "hi"}), 0),
        make_event("ResponseGenerated", json!({"text": "Hello!"}), 1),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    assert!(assistant.events.is_empty(), "no streaming = no events");
}

/// Single text stream with no tools: events should have exactly 1 text event.
#[test]
fn events_single_text_no_tools() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "hello"}), 0),
        make_event("TextStreamed", json!({"text": "Hi there!"}), 1),
        make_event("ResponseGenerated", json!({"text": "Hi there!"}), 2),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    let texts = text_events(&assistant.events);
    assert_eq!(texts.len(), 1);
    assert_eq!(texts[0], "Hi there!");
}

/// When a Thinking event exists but no TextStreamed events occur,
/// the response text from ResponseGenerated must be added as a Text event.
/// Without this, the frontend uses the events-based rendering path
/// (because the Step event exists), but finds no Text events — so the
/// response content is invisible ("Done" but no text shown).
#[test]
fn events_text_event_when_thinking_but_no_streaming() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "where do I live"}), 0),
        make_event(
            "Thinking",
            json!({
                "context_tokens": 4000,
                "context_messages": 1,
            }),
            1,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "You live in New York."}),
            2,
        ),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    assert_eq!(assistant.content, "You live in New York.");
    // Must have a text event so the events-based rendering path can show it
    let texts = text_events(&assistant.events);
    assert_eq!(
        texts.len(),
        1,
        "response text must be in events when Thinking step exists, got: {:?}",
        assistant.events
    );
    assert_eq!(texts[0], "You live in New York.");
}

/// Same as above but for Claude Code channel.
#[test]
fn events_text_event_when_thinking_but_no_streaming_cc() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "fix the bug", "channel": "claude_code"}),
            0,
        ),
        make_event(
            "Thinking",
            json!({
                "context_tokens": 8000,
                "context_messages": 5,
            }),
            1,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "Fixed.", "channel": "claude_code"}),
            2,
        ),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    assert_eq!(assistant.content, "Fixed.");
    let texts = text_events(&assistant.events);
    assert_eq!(
        texts.len(),
        1,
        "CC response text must be in events, got: {:?}",
        assistant.events
    );
    assert_eq!(texts[0], "Fixed.");
}

/// CC crash scenario: TextStreamed events with claude_code channel followed
/// by ResponseAborted (emitted by safety net when CC exits without producing
/// a Result event). Must produce an aborted assistant message, NOT a trailing
/// buffer flush with completed=None that shows as "Working" on reload.
#[test]
fn cc_crash_with_response_aborted_shows_aborted() {
    let partial = "I'm looking at the files...";
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "fix it", "channel": "claude_code"}),
            0,
        ),
        make_event(
            "TextStreamed",
            json!({"text": partial, "channel": "claude_code"}),
            1,
        ),
        // CC crashed — safety net emits ResponseAborted
        make_event(
            "ResponseAborted",
            json!({"text": partial, "channel": "claude_code"}),
            2,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    let assistant = &msgs[1];
    assert!(
        assistant.aborted,
        "CC crash with ResponseAborted must be marked aborted"
    );
    assert!(!assistant.canceled, "CC crash must NOT be marked canceled");
    assert_eq!(assistant.completed, Some(true));
    assert_eq!(assistant.content, partial);
    assert_eq!(assistant.channel.as_deref(), Some("claude_code"));
}

/// CC cancel with empty buffer: user cancels before any text is produced.
/// ResponseCanceled with empty content must still produce a canceled message.
#[test]
fn cc_cancel_empty_buffer_shows_canceled() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "do it", "channel": "claude_code"}),
            0,
        ),
        make_event(
            "ResponseCanceled",
            json!({"text": "", "channel": "claude_code"}),
            1,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    let assistant = &msgs[1];
    assert!(assistant.canceled, "empty cancel must be marked canceled");
    assert_eq!(assistant.completed, Some(true));
    assert_eq!(assistant.channel.as_deref(), Some("claude_code"));
}

/// When ResponseGenerated content extends beyond TextStreamed content,
/// the extra text must appear as a final Text event so events-based
/// rendering can show it (collapsed view uses events, not content).
#[test]
fn events_capture_result_text_remainder() {
    let streamed = "Let me check the code.";
    let result =
        "Let me check the code.\n\nI've fixed the issue. The changes are in commit abc123.";
    let events = vec![
        make_event("MessageReceived", json!({"text": "fix bug"}), 0),
        make_event(
            "TextStreamed",
            json!({"text": streamed, "channel": "claude_code"}),
            1,
        ),
        make_event("ToolCalled", json!({"name": "edit_file", "input": {}}), 2),
        make_event(
            "ToolResult",
            json!({"name": "edit_file", "result": "ok", "success": true}),
            3,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": result, "channel": "claude_code"}),
            4,
        ),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    let texts = text_events(&assistant.events);
    // Should have: streamed text before tool, plus the remainder from result_text
    assert!(
        texts.len() >= 2,
        "expected at least 2 text events, got {}: {:?}",
        texts.len(),
        texts
    );
    assert_eq!(texts[0], streamed);
    assert!(
        texts.last().unwrap().contains("fixed the issue"),
        "last text event should contain result remainder, got: {:?}",
        texts
    );
}
