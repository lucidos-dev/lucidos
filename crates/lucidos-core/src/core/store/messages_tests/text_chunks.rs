use super::msg_helpers::*;
use super::*;

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
