use super::msg_helpers::*;

// ----------------------------------------------------------------------------
// Orphan-tool-call repair (thread b101c3d7 reproduction).
//
// `build_resume_tool_blocks_with_skip_ids` historically dropped any
// `ToolCalled` event whose matching `ToolResult` was missing — a silent
// failure that left the orphan to surface later as a Claude API 400. The
// builder now must emit a synthetic `ToolResult` stub for every orphan so
// every assistant `ToolUse` block has a matching `ToolResult` block on the
// wire. The skip set must include the orphan's `ToolCalled` event id so the
// stringified `[tools: ...]` summary doesn't double-render the call.
// ----------------------------------------------------------------------------

#[test]
fn build_resume_tool_blocks_emits_synthetic_stub_for_orphan_tool_called() {
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let orphan_id = uuid::Uuid::new_v4();
    let events = vec![
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "MessageReceived".into(),
            payload: json!({"text": "do work"}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        // Orphan: ToolCalled with NO matching ToolResult.
        EventRow {
            id: orphan_id,
            event_type: "ToolCalled".into(),
            payload: json!({"name": "edit_file", "args": {"path": "x"}, "description": "Edit x"}),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
    ];
    let (blocks, skip_ids) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        5,
        &std::collections::HashSet::new(),
    );

    assert_eq!(
        blocks.len(),
        2,
        "orphan must still produce assistant ToolUse + user (synthetic) ToolResult"
    );

    use crate::llm::{ContentBlock, MessageContent};
    let use_id = match &blocks[0].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolUse { id, name, .. } => {
                assert_eq!(name, "edit_file");
                id.clone()
            }
            other => panic!("expected ToolUse, got {:?}", other),
        },
        _ => panic!("expected Blocks"),
    };
    match &blocks[1].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
            } => {
                assert_eq!(tool_use_id, &use_id, "stub must reference the orphan's id");
                assert!(
                    content.contains("orphan") || content.contains("unavailable"),
                    "stub content should self-identify, got {:?}",
                    content
                );
            }
            other => panic!("expected ToolResult, got {:?}", other),
        },
        _ => panic!("expected Blocks"),
    }
    assert!(
        skip_ids.contains(&orphan_id.to_string()),
        "skip set must include the orphan so stringified history doesn't double-render"
    );
}

/// `ToolCalled` followed by `Thinking` then matching `ToolResult` must still
/// pair correctly — interleaving non-tool events between the call and result
/// is the mid-flight `Thinking` / `ContextCaptured` shape.
#[test]
fn build_resume_tool_blocks_pairs_across_intervening_thinking_event() {
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let events = vec![
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolCalled".into(),
            payload: json!({"name": "read_file", "args": {"path": "/a"}, "description": "d"}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "Thinking".into(),
            payload: json!({"text": "Context: 100 tokens"}),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "read_file", "result": "file body", "success": true}),
            created: now + chrono::Duration::seconds(2),
            thread_id: None,
            sequence: Some(3),
        },
    ];
    let (blocks, _skip) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        5,
        &std::collections::HashSet::new(),
    );
    assert_eq!(blocks.len(), 2, "Thinking between must not break pairing");
    use crate::llm::{ContentBlock, MessageContent};
    let use_id = match &blocks[0].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolUse { id, .. } => id.clone(),
            _ => panic!("expected ToolUse"),
        },
        _ => panic!("expected Blocks"),
    };
    match &blocks[1].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
            } => {
                assert_eq!(tool_use_id, &use_id);
                assert!(content.contains("file body"));
            }
            _ => panic!("expected ToolResult"),
        },
        _ => panic!("expected Blocks"),
    }
}

/// `ToolResult` with empty / missing-content payload must still produce a
/// valid `tool_result` block (not be silently skipped). Anthropic accepts an
/// empty string for `content`; what it rejects is a missing pairing.
#[test]
fn build_resume_tool_blocks_emits_block_for_empty_result_content() {
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let events = vec![
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolCalled".into(),
            payload: json!({"name": "tool_x", "args": {}, "description": "d"}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        // ToolResult with empty result content.
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "tool_x", "result": "", "success": true}),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
    ];
    let (blocks, _skip) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        5,
        &std::collections::HashSet::new(),
    );
    assert_eq!(blocks.len(), 2, "empty content still emits a valid pair");
    use crate::llm::{ContentBlock, MessageContent};
    let use_id = match &blocks[0].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolUse { id, .. } => id.clone(),
            _ => panic!("expected ToolUse"),
        },
        _ => panic!("expected Blocks"),
    };
    match &blocks[1].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolResult { tool_use_id, .. } => {
                assert_eq!(tool_use_id, &use_id);
            }
            _ => panic!("expected ToolResult"),
        },
        _ => panic!("expected Blocks"),
    }
}

/// Two parallel `ToolCalled`s of the SAME name in one assistant turn, results
/// arriving in reverse order. Today the builder pairs by name with `rposition`,
/// so both pairs exist but the `(args, result)` mapping may be wrong. The
/// post-fix invariant: every assistant `ToolUse` has a matching user
/// `ToolResult` with the same id, and the wire payload passes the validator.
#[test]
fn build_resume_tool_blocks_pairs_parallel_same_name_calls_into_valid_payload() {
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let events = vec![
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolCalled".into(),
            payload: json!({"name": "read_file", "args": {"path": "/a"}, "description": "d"}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolCalled".into(),
            payload: json!({"name": "read_file", "args": {"path": "/b"}, "description": "d"}),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
        // Results arrive reversed.
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "read_file", "result": "body_b", "success": true}),
            created: now + chrono::Duration::seconds(2),
            thread_id: None,
            sequence: Some(3),
        },
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "read_file", "result": "body_a", "success": true}),
            created: now + chrono::Duration::seconds(3),
            thread_id: None,
            sequence: Some(4),
        },
    ];
    let (blocks, _skip) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        5,
        &std::collections::HashSet::new(),
    );
    assert_eq!(blocks.len(), 4, "two pairs => 4 messages");

    use crate::llm::{ContentBlock, MessageContent};
    // Walk pairs and assert assistant.tool_use_id == user.tool_use_id for each.
    for chunk in blocks.chunks(2) {
        let use_id = match &chunk[0].content {
            MessageContent::Blocks(b) => match &b[0] {
                ContentBlock::ToolUse { id, .. } => id.clone(),
                _ => panic!("expected ToolUse at start of pair"),
            },
            _ => panic!("expected Blocks"),
        };
        match &chunk[1].content {
            MessageContent::Blocks(b) => match &b[0] {
                ContentBlock::ToolResult { tool_use_id, .. } => {
                    assert_eq!(
                        tool_use_id, &use_id,
                        "every assistant ToolUse must have a same-id ToolResult"
                    );
                }
                _ => panic!("expected ToolResult after ToolUse"),
            },
            _ => panic!("expected Blocks"),
        }
    }

    // End-to-end: the resulting messages must pass the pre-flight validator
    // with zero stubs needed.
    let mut all_messages = blocks;
    all_messages.push(crate::llm::Message {
        role: "user".to_string(),
        content: crate::llm::MessageContent::Text("follow-up prompt".to_string()),
    });
    let stubs = crate::llm::validate::validate_tool_use_pairing(&mut all_messages);
    assert_eq!(
        stubs, 0,
        "well-paired resume blocks must require zero defensive stubs"
    );
}

/// Reproduces the thread `b101c3d7` shape: assistant has multiple tool_use
/// blocks (parallel calls), one result is missing, and the next user message
/// is a plain `Text` prompt. End-to-end: feed the events through the builder,
/// run the pre-flight validator, assert no orphan tool_use survives.
#[test]
fn b101c3d7_shape_orphan_tool_use_repaired_end_to_end() {
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let orphan_called = uuid::Uuid::new_v4();
    let paired_called = uuid::Uuid::new_v4();
    let events = vec![
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "MessageReceived".into(),
            payload: json!({"text": "go"}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        // Two sibling tool calls in one assistant turn.
        EventRow {
            id: orphan_called,
            event_type: "ToolCalled".into(),
            payload: json!({"name": "read_file", "args": {"path": "/x"}, "description": "d"}),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
        EventRow {
            id: paired_called,
            event_type: "ToolCalled".into(),
            payload: json!({"name": "read_file", "args": {"path": "/y"}, "description": "d"}),
            created: now + chrono::Duration::seconds(2),
            thread_id: None,
            sequence: Some(3),
        },
        // Only the second call gets a result — the first is orphaned.
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "read_file", "result": "body_y", "success": true}),
            created: now + chrono::Duration::seconds(3),
            thread_id: None,
            sequence: Some(4),
        },
    ];

    // Build the resume blocks the way `chat::process` does, then append the
    // user prompt as plain Text content (the no-images path).
    let (resume_blocks, _skip) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        5,
        &std::collections::HashSet::new(),
    );
    let mut messages = resume_blocks;
    messages.push(crate::llm::Message {
        role: "user".to_string(),
        content: crate::llm::MessageContent::Text("follow-up prompt".to_string()),
    });

    // Defense in depth: the pre-flight validator must produce a payload where
    // every assistant tool_use has a matching tool_result with the same id.
    let _ = crate::llm::validate::validate_tool_use_pairing(&mut messages);

    use crate::llm::{ContentBlock, MessageContent};
    let mut tool_use_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut tool_result_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in &messages {
        if let MessageContent::Blocks(blocks) = &m.content {
            for b in blocks {
                match b {
                    ContentBlock::ToolUse { id, .. } => {
                        tool_use_ids.insert(id.clone());
                    }
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        tool_result_ids.insert(tool_use_id.clone());
                    }
                    _ => {}
                }
            }
        }
    }
    assert!(
        tool_use_ids.is_subset(&tool_result_ids),
        "every tool_use must have a matching tool_result.\ntool_use_ids: {:?}\ntool_result_ids: {:?}",
        tool_use_ids,
        tool_result_ids
    );
}

// ----------------------------------------------------------------------------
// `find_orphan_tool_called_ids` — direct-call tests for the shared helper
// the startup recovery sweep uses to settle dangling `ToolCalled` events
// that lost their `ToolResult` on a mid-tool engine crash.
// ----------------------------------------------------------------------------

#[test]
fn find_orphan_tool_called_ids_no_orphans_when_every_call_paired() {
    let events = vec![
        tool_event("ToolCalled", "edit_file", 0),
        tool_event("ToolResult", "edit_file", 1),
        tool_event("ToolCalled", "read_file", 2),
        tool_event("ToolResult", "read_file", 3),
    ];
    assert!(crate::core::store::find_orphan_tool_called_ids(&events).is_empty());
}

#[test]
fn find_orphan_tool_called_ids_unpaired_call_at_end_is_orphan_and_idempotent() {
    let mut events = vec![
        tool_event("ToolCalled", "edit_file", 0),
        tool_event("ToolResult", "edit_file", 1),
        tool_event("ToolCalled", "bash", 2),
        // engine died here — no ToolResult for `bash`
    ];
    let orphan_id = events[2].id;
    let orphans = crate::core::store::find_orphan_tool_called_ids(&events);
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].0, orphan_id);
    assert_eq!(orphans[0].1, "bash");

    // Idempotency: simulate the recovery emitting a synthetic ToolResult,
    // then re-running the sweep — no new orphans found.
    events.push(tool_event("ToolResult", "bash", 3));
    assert!(crate::core::store::find_orphan_tool_called_ids(&events).is_empty());
}

#[test]
fn find_orphan_tool_called_ids_parallel_calls_some_unpaired() {
    // Three sibling calls, only one result lands.
    let events = vec![
        tool_event("ToolCalled", "read_file", 0),
        tool_event("ToolCalled", "read_file", 1),
        tool_event("ToolCalled", "read_file", 2),
        tool_event("ToolResult", "read_file", 3),
    ];
    let orphans = crate::core::store::find_orphan_tool_called_ids(&events);
    assert_eq!(orphans.len(), 2, "two of three siblings still pending");
}

#[test]
fn find_orphan_tool_called_ids_name_mismatch_falls_back_to_most_recent_pending() {
    // Defensive: legacy events / racing tools may have mismatched names.
    let events = vec![
        tool_event("ToolCalled", "tool_a", 0),
        tool_event("ToolResult", "different_name", 1), // name doesn't match
    ];
    // Falls back to "most recent pending" → tool_a is paired, no orphans.
    assert!(crate::core::store::find_orphan_tool_called_ids(&events).is_empty());
}

// ----------------------------------------------------------------------------
// Phase 4 of the loaded-knowhow plan: when a `load_knowhow` doc is in the
// per-thread loaded set, its result body in the resume tool blocks must be
// stubbed out — the full body is injected into the user message's
// `[LOADED KNOWHOW]` section, so sending it in the resume blocks too would
// double-bill the tokens.
// ----------------------------------------------------------------------------

#[test]
fn resume_blocks_stub_load_knowhow_result_when_doc_in_loaded_set() {
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let events = vec![
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolCalled".into(),
            payload: json!({"name": "load_knowhow", "args": {"id": "my-doc"}, "description": "Loading my-doc"}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "load_knowhow", "result": "ORIGINAL DOC BODY", "success": true}),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
    ];
    let mut loaded = std::collections::HashSet::new();
    loaded.insert("my-doc".to_string());
    let (messages, _skip) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 5, &loaded);
    assert_eq!(messages.len(), 2);

    use crate::llm::{ContentBlock, MessageContent};
    let result_msg = messages
        .iter()
        .rfind(|m| m.role == "user")
        .expect("expect a user-role ToolResult message");
    if let MessageContent::Blocks(blocks) = &result_msg.content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert!(
                content.contains("LOADED KNOWHOW"),
                "stub should point at the [LOADED KNOWHOW] section, got: {}",
                content
            );
            assert!(
                content.contains("my-doc"),
                "stub should name the doc id, got: {}",
                content
            );
            assert!(
                !content.contains("ORIGINAL DOC BODY"),
                "stub must replace the original body, got: {}",
                content
            );
        } else {
            panic!("expected ToolResult block, got {:?}", &blocks[0]);
        }
    } else {
        panic!("expected Blocks content");
    }
}

#[test]
fn resume_blocks_keep_load_knowhow_body_when_doc_not_in_loaded_set() {
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let events = vec![
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolCalled".into(),
            payload: json!({"name": "load_knowhow", "args": {"id": "my-doc"}, "description": "Loading my-doc"}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "load_knowhow", "result": "ORIGINAL DOC BODY", "success": true}),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
    ];
    // Empty loaded set — no recovery yet, or the doc was unloaded. The body
    // must survive verbatim so the LLM doesn't lose context.
    let loaded = std::collections::HashSet::new();
    let (messages, _skip) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 5, &loaded);

    use crate::llm::{ContentBlock, MessageContent};
    let result_msg = messages
        .iter()
        .rfind(|m| m.role == "user")
        .expect("expect a user-role ToolResult message");
    if let MessageContent::Blocks(blocks) = &result_msg.content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert_eq!(content, "ORIGINAL DOC BODY");
        } else {
            panic!("expected ToolResult block");
        }
    } else {
        panic!("expected Blocks content");
    }
}

#[test]
fn resume_blocks_keep_other_tools_unchanged_regardless_of_loaded_set() {
    // Loaded set affects only `load_knowhow`. Other tools (e.g. query_events)
    // pass through verbatim no matter what's in the set.
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let events = vec![
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolCalled".into(),
            payload: json!({"name": "query_events", "args": {"limit": 5}, "description": "querying"}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "query_events", "result": "[1,2,3]", "success": true}),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
    ];
    let mut loaded = std::collections::HashSet::new();
    // Add an unrelated id — must not affect query_events.
    loaded.insert("unrelated-doc".to_string());
    let (messages, _skip) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 5, &loaded);

    use crate::llm::{ContentBlock, MessageContent};
    let result_msg = messages
        .iter()
        .rfind(|m| m.role == "user")
        .expect("expect a user-role ToolResult message");
    if let MessageContent::Blocks(blocks) = &result_msg.content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert_eq!(content, "[1,2,3]");
        } else {
            panic!("expected ToolResult block");
        }
    } else {
        panic!("expected Blocks content");
    }
}

#[test]
fn resume_blocks_orphan_load_knowhow_emits_orphan_stub_even_when_id_in_loaded_set() {
    // Edge case: a `load_knowhow` ToolCalled with no matching ToolResult
    // (orphan). Even if the doc id is in the loaded set, there was no real
    // result body to dedupe — the orphan stub must win, because the loaded-
    // knowhow stub assumes the body lives in the user message AND that there
    // was a paired result to begin with. Anthropic 400s if any tool_use lacks
    // a paired tool_result, so the orphan stub's job (every assistant
    // tool_use gets a paired user tool_result) is the higher-priority
    // invariant.
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let events = vec![
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolCalled".into(),
            payload: json!({"name": "load_knowhow", "args": {"id": "my-doc"}, "description": "Loading my-doc"}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        // No ToolResult — orphan.
    ];
    let mut loaded = std::collections::HashSet::new();
    loaded.insert("my-doc".to_string());
    let (messages, _skip) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 5, &loaded);
    assert_eq!(
        messages.len(),
        2,
        "orphan still produces ToolUse + stub ToolResult"
    );

    use crate::llm::{ContentBlock, MessageContent};
    let result_msg = messages
        .iter()
        .rfind(|m| m.role == "user")
        .expect("expect a user-role ToolResult message");
    if let MessageContent::Blocks(blocks) = &result_msg.content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert!(
                content.contains("orphan") || content.contains("unavailable"),
                "orphan stub must win over the loaded-knowhow stub, got: {}",
                content
            );
            assert!(
                !content.contains("LOADED KNOWHOW"),
                "loaded-knowhow stub must NOT be applied to orphans, got: {}",
                content
            );
        } else {
            panic!("expected ToolResult block");
        }
    } else {
        panic!("expected Blocks content");
    }
}
