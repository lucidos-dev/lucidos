use super::*;
use crate::core::EventRow;
use chrono::TimeZone;
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

/// Normal flow: user message -> streaming -> ResponseGenerated.
/// The response should have completed = Some(true), not interrupted.
#[test]
fn completed_response_not_interrupted() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "hello", "request_id": "s1"}),
            0,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "working...", "request_id": "s1"}),
            1,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "working...", "channel": "claude_code", "request_id": "s1"}),
            2,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].completed, Some(true));
}

/// Interruption: user sends follow-up while CC is still streaming.
/// The interrupted response should have completed = Some(false).
#[test]
fn followup_mid_stream_marks_interrupted() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "do something", "request_id": "s1"}),
            0,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "partial response", "request_id": "s1"}),
            1,
        ),
        // User sends follow-up before ResponseGenerated
        make_event(
            "MessageReceived",
            json!({"text": "actually do this instead", "request_id": "s1"}),
            2,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "new response", "request_id": "s1"}),
            3,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "new response", "channel": "claude_code", "request_id": "s1"}),
            4,
        ),
    ];
    let msgs = build_session_messages(&events);

    // Expected: user, assistant(interrupted), user, assistant(completed)
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].content, "do something");

    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].content, "partial response");
    assert_eq!(
        msgs[1].completed,
        Some(false),
        "interrupted response must have completed=false"
    );

    assert_eq!(msgs[2].role, "user");
    assert_eq!(msgs[2].content, "actually do this instead");

    assert_eq!(msgs[3].role, "assistant");
    assert_eq!(msgs[3].completed, Some(true));
}

/// Still-in-progress: streaming text exists but no ResponseGenerated yet
/// (session still active or engine crashed). Should be completed = None, NOT false.
#[test]
fn trailing_buffer_not_marked_interrupted() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "hello", "request_id": "s1"}),
            0,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "still working...", "request_id": "s1"}),
            1,
        ),
        // No ResponseGenerated — session still active
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].content, "still working...");
    assert_eq!(
        msgs[1].completed, None,
        "trailing buffer must have completed=None, not false"
    );
}

/// Two exchanges: first interrupted, second still in progress (trailing buffer).
/// Only the first should be marked as interrupted.
#[test]
fn interrupted_then_trailing_buffer() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "msg1", "request_id": "s1"}),
            0,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "response1", "request_id": "s1"}),
            1,
        ),
        // Follow-up interrupts
        make_event(
            "MessageReceived",
            json!({"text": "msg2", "request_id": "s1"}),
            2,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "response2", "request_id": "s1"}),
            3,
        ),
        // No ResponseGenerated — second exchange still in progress
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 4);

    // First response: interrupted (flushed by MessageReceived)
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(
        msgs[1].completed,
        Some(false),
        "first response was interrupted"
    );

    // Second response: still in progress (trailing buffer)
    assert_eq!(msgs[3].role, "assistant");
    assert_eq!(
        msgs[3].completed, None,
        "second response is trailing buffer, not interrupted"
    );
}

/// Follow-up after idle: user sends a second message after the first response
/// completed normally. Neither response should be marked as interrupted.
#[test]
fn followup_after_completed_not_interrupted() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "msg1", "request_id": "s1"}),
            0,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "resp1", "request_id": "s1"}),
            1,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "resp1", "channel": "claude_code", "request_id": "s1"}),
            2,
        ),
        // Second message after completion
        make_event(
            "MessageReceived",
            json!({"text": "msg2", "request_id": "s1"}),
            3,
        ),
        make_event(
            "CodingAgentTextStreamed",
            json!({"text": "resp2", "request_id": "s1"}),
            4,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "resp2", "channel": "claude_code", "request_id": "s1"}),
            5,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 4);
    assert_eq!(
        msgs[1].completed,
        Some(true),
        "first response completed normally"
    );
    assert_eq!(
        msgs[3].completed,
        Some(true),
        "second response completed normally"
    );
}

/// TextStreamed (non-CC) interruption: same semantics apply to regular chat.
#[test]
fn text_streamed_followup_mid_stream_marks_interrupted() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "q1", "request_id": "s1"}),
            0,
        ),
        make_event(
            "TextStreamed",
            json!({"text": "partial", "request_id": "s1"}),
            1,
        ),
        // Follow-up interrupts
        make_event(
            "MessageReceived",
            json!({"text": "q2", "request_id": "s1"}),
            2,
        ),
        make_event(
            "TextStreamed",
            json!({"text": "answer", "request_id": "s1"}),
            3,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "answer", "request_id": "s1"}),
            4,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 4);
    assert_eq!(
        msgs[1].completed,
        Some(false),
        "text-streamed response was interrupted"
    );
    assert_eq!(msgs[3].completed, Some(true));
}

/// ResponseFailed should produce an assistant message with [ERROR] prefix
/// so the frontend can detect it as an error and show a retry button.
#[test]
fn response_failed_produces_error_message() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "hello"}), 0),
        make_event("ResponseFailed", json!({"error": "Model not found"}), 1),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[1].role, "assistant");
    assert!(
        msgs[1].content.starts_with("[ERROR]"),
        "ResponseFailed content should start with [ERROR] prefix, got: {}",
        msgs[1].content
    );
    assert!(msgs[1].content.contains("Model not found"));
    assert_eq!(msgs[1].completed, Some(true));
}

/// ResponseFailed should prefer request_event_id from payload over positional tracking,
/// just like ResponseGenerated does. This matters when events from concurrent
/// requests are interleaved (e.g., scheduled trigger + user message).
#[test]
fn response_failed_prefers_payload_request_event_id() {
    let user1_id = Uuid::new_v4();
    let user2_id = Uuid::new_v4();
    let events = vec![
        // First user message
        EventRow {
            id: user1_id,
            event_type: "MessageReceived".to_string(),
            payload: json!({"text": "first message"}),
            created: Utc.timestamp_opt(1700000000, 0).unwrap(),
            thread_id: None,
            sequence: None,
        },
        // Second user message (positional tracker now points here)
        EventRow {
            id: user2_id,
            event_type: "MessageReceived".to_string(),
            payload: json!({"text": "second message"}),
            created: Utc.timestamp_opt(1700000001, 0).unwrap(),
            thread_id: None,
            sequence: None,
        },
        // ResponseFailed for the FIRST message (explicit request_event_id)
        EventRow {
            id: Uuid::new_v4(),
            event_type: "ResponseFailed".to_string(),
            payload: json!({"error": "API error", "request_event_id": user1_id.to_string()}),
            created: Utc.timestamp_opt(1700000002, 0).unwrap(),
            thread_id: None,
            sequence: None,
        },
    ];
    let msgs = build_session_messages(&events);
    // The ResponseFailed should link to user1, not user2
    let failed = msgs
        .iter()
        .find(|m| m.role == "assistant" && m.content.contains("API error"))
        .unwrap();
    assert_eq!(failed.request_event_id, Some(user1_id.to_string()),
            "ResponseFailed should use request_event_id from payload, not positional tracking (which would give user2)");
}

/// text_chunks should contain deltas (not overlapping full buffers)
/// snapshotted at each ToolCalled boundary, plus the final text segment.
/// Each chunk must be >= 80 chars to be snapshotted separately.
#[test]
fn text_chunks_contain_deltas_at_tool_boundaries() {
    // Use long-enough text segments (>= 80 chars) so they produce separate chunks
    let chunk1 = "Analyzing the codebase structure and identifying the key patterns used throughout the project files...";
    let chunk2 = " Now editing the identified files to apply the requested changes across all matching locations in code...";
    let chunk3 = " Done. All requested changes have been applied successfully and the tests are passing without errors.";
    let full = format!("{}{}{}", chunk1, chunk2, chunk3);
    assert!(chunk1.len() >= 80 && chunk2.len() >= 80 && chunk3.len() >= 80);

    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "do work", "request_id": "s1"}),
            0,
        ),
        make_event(
            "TextStreamed",
            json!({"text": chunk1, "request_id": "s1", "channel": "claude_code"}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "read_file", "args": {"path": "foo.rs"}, "request_id": "s1"}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "read_file", "success": true, "result": "contents", "request_id": "s1"}),
            3,
        ),
        make_event(
            "TextStreamed",
            json!({"text": chunk2, "request_id": "s1", "channel": "claude_code"}),
            4,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "write_file", "args": {"path": "foo.rs"}, "request_id": "s1"}),
            5,
        ),
        make_event(
            "ToolResult",
            json!({"name": "write_file", "success": true, "result": "ok", "request_id": "s1"}),
            6,
        ),
        make_event(
            "TextStreamed",
            json!({"text": chunk3, "request_id": "s1", "channel": "claude_code"}),
            7,
        ),
        {
            let mut e = make_event(
                "ResponseGenerated",
                json!({"text": full, "request_id": "s1"}),
                8,
            );
            e.payload["channel"] = json!("claude_code");
            e
        },
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    let assistant = &msgs[1];
    assert_eq!(assistant.role, "assistant");
    assert_eq!(
        assistant.text_chunks.len(),
        3,
        "expected 3 text chunks, got {:?}",
        assistant.text_chunks
    );
    assert_eq!(assistant.text_chunks[0], chunk1);
    assert_eq!(assistant.text_chunks[1], chunk2);
    assert_eq!(assistant.text_chunks[2], chunk3);
}

/// Small text before a tool call merges into the next chunk (no meaningless toggle).
#[test]
fn small_text_before_tool_merges_into_next_chunk() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "help", "request_id": "s1"}),
            0,
        ),
        // Tiny text before tool call — should NOT create a separate chunk
        make_event(
            "TextStreamed",
            json!({"text": "Ok.", "request_id": "s1"}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "search", "args": {}, "request_id": "s1"}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "search", "success": true, "result": "found", "request_id": "s1"}),
            3,
        ),
        make_event(
            "TextStreamed",
            json!({"text": " Here's the detailed answer to your question with all the relevant information.", "request_id": "s1"}),
            4,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "Ok. Here's the detailed answer to your question with all the relevant information.", "request_id": "s1"}),
            5,
        ),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    // "Ok." is only 3 chars — merged into a single chunk with the rest
    assert_eq!(
        assistant.text_chunks.len(),
        1,
        "small pre-tool text should merge, got {:?}",
        assistant.text_chunks
    );
}

/// text_chunks work for regular (non-CC) text streaming too.
#[test]
fn text_chunks_regular_chat_deltas() {
    // Use text segments >= 80 chars so they produce separate chunks
    let chunk1 = "Let me think about this carefully and analyze all the relevant information before responding...";
    let chunk2 = " Here's the detailed answer based on my thorough analysis of the question you asked about the topic.";
    let full = format!("{}{}", chunk1, chunk2);
    assert!(chunk1.len() >= 80 && chunk2.len() >= 80);

    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "help", "request_id": "s1"}),
            0,
        ),
        make_event(
            "TextStreamed",
            json!({"text": chunk1, "request_id": "s1"}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "search", "args": {}, "request_id": "s1"}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "search", "success": true, "result": "found", "request_id": "s1"}),
            3,
        ),
        make_event(
            "TextStreamed",
            json!({"text": chunk2, "request_id": "s1"}),
            4,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": full, "request_id": "s1"}),
            5,
        ),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    let assistant = &msgs[1];
    assert_eq!(
        assistant.text_chunks.len(),
        2,
        "expected 2 text chunks, got {:?}",
        assistant.text_chunks
    );
    assert_eq!(assistant.text_chunks[0], chunk1);
    assert_eq!(assistant.text_chunks[1], chunk2);
}

/// TextStreamed with channel=claude_code (unified format) should be handled
/// identically to the legacy CodingAgentTextStreamed event type.
#[test]
fn text_streamed_with_claude_code_channel_works() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "hello", "request_id": "s1"}),
            0,
        ),
        // New format: TextStreamed with channel=claude_code
        {
            let mut e = make_event(
                "TextStreamed",
                json!({"text": "working...", "request_id": "s1"}),
                1,
            );
            e.payload["channel"] = json!("claude_code");
            e
        },
        {
            let mut e = make_event(
                "ResponseGenerated",
                json!({"text": "working...", "request_id": "s1"}),
                2,
            );
            e.payload["channel"] = json!("claude_code");
            e
        },
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].content, "working...");
    assert_eq!(msgs[1].channel, Some("claude_code".to_string()));
    assert_eq!(msgs[1].completed, Some(true));
}

// ---------------------------------------------------------------
// Chunk toggle rules (More/Less):
//   - 0 chunks or 1 chunk -> no toggle
//   - 2+ chunks -> toggle (collapsed = latest chunk, expanded = all)
//   - Same behavior for: Done, Canceled, Continued Above, any state
// ---------------------------------------------------------------

/// No tool calls -> single text segment -> 1 chunk -> no More/Less toggle.
#[test]
fn single_chunk_no_toggle_done() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "hello"}), 0),
        make_event("TextStreamed", json!({"text": "Hi there!"}), 1),
        make_event("ResponseGenerated", json!({"text": "Hi there!"}), 2),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    assert_eq!(assistant.completed, Some(true));
    // Only 1 chunk -> no toggle
    assert!(
        assistant.text_chunks.len() <= 1,
        "single response without tools should have 0 or 1 chunks, got {:?}",
        assistant.text_chunks
    );
}

/// No streaming at all, just a direct ResponseGenerated -> 0 chunks -> no toggle.
#[test]
fn no_streaming_no_chunks() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "hi"}), 0),
        make_event("ResponseGenerated", json!({"text": "Hello!"}), 1),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    assert_eq!(assistant.text_chunks.len(), 0);
}

// Helper: generate a text segment of exactly `n` chars padded with dots
fn pad(prefix: &str, n: usize) -> String {
    let mut s = prefix.to_string();
    while s.len() < n {
        s.push('.');
    }
    s.truncate(n);
    s
}

/// Two chunks (one tool call) -> More/Less toggle should appear.
#[test]
fn two_chunks_toggle_done() {
    let c1 = pad("Searching for results across the codebase", 100);
    let c2 = pad(" Here are the detailed results I found", 100);
    let full = format!("{}{}", c1, c2);
    let events = vec![
        make_event("MessageReceived", json!({"text": "search something"}), 0),
        make_event("TextStreamed", json!({"text": c1}), 1),
        make_event("ToolCalled", json!({"name": "search", "args": {}}), 2),
        make_event(
            "ToolResult",
            json!({"name": "search", "success": true, "result": "found"}),
            3,
        ),
        make_event("TextStreamed", json!({"text": c2}), 4),
        make_event("ResponseGenerated", json!({"text": full}), 5),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    assert_eq!(assistant.text_chunks.len(), 2);
    assert_eq!(assistant.text_chunks[0], c1);
    assert_eq!(assistant.text_chunks[1], c2);
}

/// Three chunks (two tool calls) -> collapsed shows last chunk.
#[test]
fn three_chunks_collapsed_shows_last() {
    let c1 = pad("Step 1: Reading the file contents", 100);
    let c2 = pad(" Step 2: Applying the requested edits", 100);
    let c3 = pad(" All done with the changes successfully", 100);
    let full = format!("{}{}{}", c1, c2, c3);
    let events = vec![
        make_event("MessageReceived", json!({"text": "do it"}), 0),
        make_event("TextStreamed", json!({"text": c1}), 1),
        make_event("ToolCalled", json!({"name": "read", "args": {}}), 2),
        make_event(
            "ToolResult",
            json!({"name": "read", "success": true, "result": "ok"}),
            3,
        ),
        make_event("TextStreamed", json!({"text": c2}), 4),
        make_event("ToolCalled", json!({"name": "write", "args": {}}), 5),
        make_event(
            "ToolResult",
            json!({"name": "write", "success": true, "result": "ok"}),
            6,
        ),
        make_event("TextStreamed", json!({"text": c3}), 7),
        make_event("ResponseGenerated", json!({"text": full}), 8),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    assert_eq!(assistant.text_chunks.len(), 3);
    assert_eq!(*assistant.text_chunks.last().unwrap(), c3);
    let joined: String = assistant.text_chunks.iter().cloned().collect();
    assert_eq!(joined, assistant.content);
}

/// Canceled exchange still has chunks -> toggle should work the same.
#[test]
fn chunks_on_canceled_exchange() {
    let c1 = pad("Working on the requested changes to the codebase", 100);
    let c2 = pad(" Partial result before cancellation occurred", 100);
    let full = format!("{}{}", c1, c2);
    let events = vec![
        make_event("MessageReceived", json!({"text": "do it"}), 0),
        make_event("TextStreamed", json!({"text": c1}), 1),
        make_event("ToolCalled", json!({"name": "edit", "args": {}}), 2),
        make_event(
            "ToolResult",
            json!({"name": "edit", "success": true, "result": "ok"}),
            3,
        ),
        make_event("TextStreamed", json!({"text": c2}), 4),
        make_event("ResponseCanceled", json!({"text": full}), 5),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    assert!(assistant.canceled);
    assert_eq!(assistant.text_chunks.len(), 2);
    assert_eq!(assistant.text_chunks[0], c1);
    assert_eq!(assistant.text_chunks[1], c2);
}

/// Interrupted exchange (continued above) still has chunks.
#[test]
fn chunks_on_interrupted_exchange() {
    let c1 = pad(
        "Starting the analysis of the requested files in the repository",
        100,
    );
    let c2 = pad(" More detailed text about the analysis results found", 100);
    let events = vec![
        make_event("MessageReceived", json!({"text": "do it"}), 0),
        make_event(
            "TextStreamed",
            json!({"text": c1, "channel": "claude_code"}),
            1,
        ),
        make_event("ToolCalled", json!({"name": "read", "args": {}}), 2),
        make_event(
            "ToolResult",
            json!({"name": "read", "success": true, "result": "ok"}),
            3,
        ),
        make_event(
            "TextStreamed",
            json!({"text": c2, "channel": "claude_code"}),
            4,
        ),
        make_event("MessageReceived", json!({"text": "stop"}), 5),
        make_event("ResponseGenerated", json!({"text": "OK stopped."}), 6),
    ];
    let msgs = build_session_messages(&events);
    let interrupted = &msgs[1];
    assert_eq!(interrupted.completed, Some(false));
    assert_eq!(interrupted.text_chunks.len(), 2);
    assert_eq!(interrupted.text_chunks[0], c1);
    assert_eq!(interrupted.text_chunks[1], c2);
}

/// Canceled with empty content — no "Canceled by user" text stored.
/// The canceled flag should still be set, and content should be empty.
#[test]
fn canceled_with_empty_content() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "do something"}), 0),
        make_event("ResponseCanceled", json!({"text": ""}), 1),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 2);
    let assistant = &msgs[1];
    assert!(assistant.canceled);
    assert_eq!(assistant.content, "");
    assert_eq!(assistant.completed, Some(true));
}

/// Canceled with partial content — only the streamed text, no appended label.
#[test]
fn canceled_with_partial_content_no_label() {
    let partial = "I started working on the analysis";
    let events = vec![
        make_event("MessageReceived", json!({"text": "analyze this"}), 0),
        make_event("TextStreamed", json!({"text": partial}), 1),
        make_event("ResponseCanceled", json!({"text": partial}), 2),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    assert!(assistant.canceled);
    assert_eq!(assistant.content, partial);
    // No "Canceled by user" appended
    assert!(!assistant.content.contains("Canceled"));
}

/// Multiple exchanges — each has independent chunks.
#[test]
fn chunks_independent_per_exchange() {
    let c1 = pad("First chunk of text for exchange one with details", 100);
    let c2 = pad("Second chunk of text for exchange one with more", 100);
    let full1 = format!("{}{}", c1, c2);
    let events = vec![
        make_event("MessageReceived", json!({"text": "first"}), 0),
        make_event("TextStreamed", json!({"text": c1}), 1),
        make_event("ToolCalled", json!({"name": "t1", "args": {}}), 2),
        make_event(
            "ToolResult",
            json!({"name": "t1", "success": true, "result": "r"}),
            3,
        ),
        make_event("TextStreamed", json!({"text": c2}), 4),
        make_event("ResponseGenerated", json!({"text": full1}), 5),
        // Exchange 2: 1 chunk (no toggle)
        make_event("MessageReceived", json!({"text": "second"}), 6),
        make_event("TextStreamed", json!({"text": "Simple answer"}), 7),
        make_event("ResponseGenerated", json!({"text": "Simple answer"}), 8),
    ];
    let msgs = build_session_messages(&events);
    assert_eq!(msgs.len(), 4); // 2 user + 2 assistant
    let a1 = &msgs[1]; // first assistant
    assert_eq!(a1.text_chunks.len(), 2);
    let a2 = &msgs[3]; // second assistant
    assert!(
        a2.text_chunks.len() <= 1,
        "second exchange has no tools, should have 0 or 1 chunks"
    );
}

/// Tool call with no text before it -> no empty chunk created.
#[test]
fn no_empty_chunk_before_first_tool() {
    let events = vec![
        make_event("MessageReceived", json!({"text": "do it"}), 0),
        // Tool called immediately, no text streamed first
        make_event("ToolCalled", json!({"name": "search", "args": {}}), 1),
        make_event(
            "ToolResult",
            json!({"name": "search", "success": true, "result": "found"}),
            2,
        ),
        make_event("TextStreamed", json!({"text": "Here are the results."}), 3),
        make_event(
            "ResponseGenerated",
            json!({"text": "Here are the results."}),
            4,
        ),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    // Should have 1 chunk (text after tool), not an empty chunk before tool
    assert_eq!(assistant.text_chunks.len(), 1);
    assert_eq!(assistant.text_chunks[0], "Here are the results.");
}

/// Multiple consecutive small texts before multiple tools all merge into one chunk.
#[test]
fn multiple_small_texts_before_multiple_tools_merge() {
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "help", "request_id": "s1"}),
            0,
        ),
        make_event(
            "TextStreamed",
            json!({"text": "Ok.", "request_id": "s1"}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "search", "args": {}, "request_id": "s1"}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "search", "success": true, "result": "r1", "request_id": "s1"}),
            3,
        ),
        make_event(
            "TextStreamed",
            json!({"text": " Let me check.", "request_id": "s1"}),
            4,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "read_file", "args": {}, "request_id": "s1"}),
            5,
        ),
        make_event(
            "ToolResult",
            json!({"name": "read_file", "success": true, "result": "r2", "request_id": "s1"}),
            6,
        ),
        make_event(
            "TextStreamed",
            json!({"text": " Here is a thorough and detailed answer to your question based on all the information I gathered.", "request_id": "s1"}),
            7,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": "Ok. Let me check. Here is a thorough and detailed answer to your question based on all the information I gathered.", "request_id": "s1"}),
            8,
        ),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    // "Ok." (3) + " Let me check." (14) are both under 80 — everything merges into 1 chunk
    assert_eq!(
        assistant.text_chunks.len(),
        1,
        "all small pre-tool texts should merge, got {:?}",
        assistant.text_chunks
    );
    assert_eq!(assistant.text_chunks[0], "Ok. Let me check. Here is a thorough and detailed answer to your question based on all the information I gathered.");
}

/// Small text before first tool, but large text before second tool -> 2 chunks.
#[test]
fn small_then_large_text_before_tools() {
    let small = "Ok.";
    let large = " Here is a very detailed explanation of how I will approach this problem and find the answer.";
    let tail = " And here is the final comprehensive answer to your question with all the relevant details included.";
    assert!(large.len() >= 80);
    let full = format!("{}{}{}", small, large, tail);

    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "help", "request_id": "s1"}),
            0,
        ),
        make_event(
            "TextStreamed",
            json!({"text": small, "request_id": "s1"}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "t1", "args": {}, "request_id": "s1"}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "t1", "success": true, "result": "r1", "request_id": "s1"}),
            3,
        ),
        make_event(
            "TextStreamed",
            json!({"text": large, "request_id": "s1"}),
            4,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "t2", "args": {}, "request_id": "s1"}),
            5,
        ),
        make_event(
            "ToolResult",
            json!({"name": "t2", "success": true, "result": "r2", "request_id": "s1"}),
            6,
        ),
        make_event("TextStreamed", json!({"text": tail, "request_id": "s1"}), 7),
        make_event(
            "ResponseGenerated",
            json!({"text": &full, "request_id": "s1"}),
            8,
        ),
    ];
    let msgs = build_session_messages(&events);
    let assistant = &msgs[1];
    // "Ok." merges forward (too small), then "Ok." + large = 95 chars -> chunk at tool 2.
    // Tail becomes second chunk.
    assert_eq!(
        assistant.text_chunks.len(),
        2,
        "expected 2 chunks, got {:?}",
        assistant.text_chunks
    );
    assert_eq!(assistant.text_chunks[0], format!("{}{}", small, large));
    assert_eq!(assistant.text_chunks[1], tail);
}

/// Exact boundary: 80 chars creates a chunk, 79 does not.
#[test]
fn exact_80_char_boundary() {
    let exactly_80 = "A".repeat(80);
    let exactly_79 = "B".repeat(79);
    let tail = " Done with the full and complete response text here.";

    // 80 chars -> should create a chunk
    let full80 = format!("{}{}", exactly_80, tail);
    let events80 = vec![
        make_event(
            "MessageReceived",
            json!({"text": "q", "request_id": "s1"}),
            0,
        ),
        make_event(
            "TextStreamed",
            json!({"text": &exactly_80, "request_id": "s1"}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "t", "args": {}, "request_id": "s1"}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "t", "success": true, "result": "r", "request_id": "s1"}),
            3,
        ),
        make_event(
            "TextStreamed",
            json!({"text": &tail, "request_id": "s1"}),
            4,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": &full80, "request_id": "s1"}),
            5,
        ),
    ];
    let msgs80 = build_session_messages(&events80);
    assert_eq!(
        msgs80[1].text_chunks.len(),
        2,
        "80 chars should produce 2 chunks"
    );

    // 79 chars -> should NOT create a chunk
    let full79 = format!("{}{}", exactly_79, tail);
    let events79 = vec![
        make_event(
            "MessageReceived",
            json!({"text": "q", "request_id": "s1"}),
            0,
        ),
        make_event(
            "TextStreamed",
            json!({"text": &exactly_79, "request_id": "s1"}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "t", "args": {}, "request_id": "s1"}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "t", "success": true, "result": "r", "request_id": "s1"}),
            3,
        ),
        make_event(
            "TextStreamed",
            json!({"text": &tail, "request_id": "s1"}),
            4,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": &full79, "request_id": "s1"}),
            5,
        ),
    ];
    let msgs79 = build_session_messages(&events79);
    assert_eq!(
        msgs79[1].text_chunks.len(),
        1,
        "79 chars should merge into 1 chunk"
    );
}

/// Replay idempotency: calling build_session_messages on the same events always gives the same chunks.
#[test]
fn replay_idempotent() {
    let chunk1 = &pad("First chunk text", 80);
    let chunk2 = &pad("Second chunk text", 80);
    let full = format!("{}{}", chunk1, chunk2);
    let events = vec![
        make_event(
            "MessageReceived",
            json!({"text": "q", "request_id": "s1"}),
            0,
        ),
        make_event(
            "TextStreamed",
            json!({"text": chunk1, "request_id": "s1"}),
            1,
        ),
        make_event(
            "ToolCalled",
            json!({"name": "t", "args": {}, "request_id": "s1"}),
            2,
        ),
        make_event(
            "ToolResult",
            json!({"name": "t", "success": true, "result": "r", "request_id": "s1"}),
            3,
        ),
        make_event(
            "TextStreamed",
            json!({"text": chunk2, "request_id": "s1"}),
            4,
        ),
        make_event(
            "ResponseGenerated",
            json!({"text": &full, "request_id": "s1"}),
            5,
        ),
    ];
    let msgs1 = build_session_messages(&events);
    let msgs2 = build_session_messages(&events);
    assert_eq!(
        msgs1[1].text_chunks, msgs2[1].text_chunks,
        "replay must be idempotent"
    );
}

// ---------------------------------------------------------------
// Events (ResponseEvent) tests
// ---------------------------------------------------------------

/// Helper: count text events in a ResponseEvent vec
fn text_events(events: &[ResponseEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            ResponseEvent::Text { md } => Some(md.as_str()),
            _ => None,
        })
        .collect()
}

/// Helper: count step events in a ResponseEvent vec
fn step_events(events: &[ResponseEvent]) -> Vec<(&str, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            ResponseEvent::Step {
                description,
                success,
                ..
            } => Some((description.as_str(), *success)),
            _ => None,
        })
        .collect()
}

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
    assert_eq!(steps[0], ("Memory: 7 results", true));
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

/// CC session with tool calls but no text yet (e.g. reload during first tool call).
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

/// CC session with tool calls and some text — step events interleaved with text.
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
