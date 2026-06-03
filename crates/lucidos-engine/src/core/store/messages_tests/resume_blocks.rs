use super::*;
use super::msg_helpers::*;

// ----------------------------------------------------------------------------
// build_resume_tool_blocks_with_skip_ids — Phase 3 of the
// trigger-knowhow-discovery plan.
//
// On resume, the most recent N (ToolCalled, ToolResult) pairs are
// reconstructed as full Message::Blocks(...) pairs prepended to the LLM
// messages vec. `load_knowhow` results are pinned regardless of N — their
// value is reference material that doesn't decay across turns.
// ----------------------------------------------------------------------------

#[test]
fn build_resume_tool_blocks_emits_paired_tool_use_and_result_for_recent_tool() {
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let tool_called_id = uuid::Uuid::new_v4();
    let events = vec![
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "MessageReceived".into(),
            payload: json!({"text": "Run the nightly pipeline."}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        EventRow {
            id: tool_called_id,
            event_type: "ToolCalled".into(),
            payload: json!({"name": "load_knowhow", "args": {"id": "x"}, "description": "Loading"}),
            created: now,
            thread_id: None,
            sequence: Some(2),
        },
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "load_knowhow", "result": "PROCEDURE BODY", "success": true}),
            created: now,
            thread_id: None,
            sequence: Some(3),
        },
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ResponseGenerated".into(),
            payload: json!({"text": "I see the procedure"}),
            created: now,
            thread_id: None,
            sequence: Some(4),
        },
    ];
    let (blocks, _skip_ids) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 3, &std::collections::HashSet::new());
    assert_eq!(blocks.len(), 2, "expect assistant ToolUse + user ToolResult");

    use crate::llm::{ContentBlock, MessageContent};
    match &blocks[0].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(name, "load_knowhow");
                assert_eq!(input["id"], "x");
                assert!(id.starts_with("evt-"), "id should be synthesized: {}", id);
            }
            _ => panic!("expected ToolUse block"),
        },
        _ => panic!("expected Blocks content"),
    }
    match &blocks[1].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolResult { tool_use_id, content } => {
                assert!(tool_use_id.starts_with("evt-"));
                assert!(
                    content.contains("PROCEDURE BODY"),
                    "result body should be verbatim, got: {}",
                    content
                );
            }
            _ => panic!("expected ToolResult block"),
        },
        _ => panic!("expected Blocks content"),
    }
}

#[test]
fn build_resume_tool_blocks_returns_empty_when_no_tools() {
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let events = vec![EventRow {
        id: uuid::Uuid::new_v4(),
        event_type: "MessageReceived".into(),
        payload: json!({"text": "hi"}),
        created: now,
        thread_id: None,
        sequence: Some(1),
    }];
    let (blocks, skip_ids) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 3, &std::collections::HashSet::new());
    assert!(blocks.is_empty());
    assert!(skip_ids.is_empty());
}

#[test]
fn build_resume_tool_blocks_caps_at_n_excluding_pinned() {
    // 5 unrelated tool calls, N=2 -> 2 most recent emitted
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let mut events = Vec::new();
    for i in 0..5_i64 {
        events.push(EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolCalled".into(),
            payload: json!({"name": format!("tool_{}", i), "args": {}, "description": "d"}),
            created: now + chrono::Duration::seconds(i * 2),
            thread_id: None,
            sequence: Some(i * 2 + 1),
        });
        events.push(EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": format!("tool_{}", i), "result": format!("result_{}", i), "success": true}),
            created: now + chrono::Duration::seconds(i * 2 + 1),
            thread_id: None,
            sequence: Some(i * 2 + 2),
        });
    }
    let (blocks, _skip_ids) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 2, &std::collections::HashSet::new());
    assert_eq!(blocks.len(), 4, "2 pairs * 2 messages each");

    use crate::llm::{ContentBlock, MessageContent};
    match &blocks[2].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolUse { name, .. } => assert_eq!(name, "tool_4"),
            _ => panic!("expected ToolUse"),
        },
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn build_resume_tool_blocks_pins_load_knowhow_beyond_n() {
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let mut events = Vec::new();
    // load_knowhow first (oldest)
    events.push(EventRow {
        id: uuid::Uuid::new_v4(),
        event_type: "ToolCalled".into(),
        payload: json!({"name": "load_knowhow", "args": {"id": "recipe"}, "description": "Loading recipe"}),
        created: now,
        thread_id: None,
        sequence: Some(1),
    });
    events.push(EventRow {
        id: uuid::Uuid::new_v4(),
        event_type: "ToolResult".into(),
        payload: json!({"name": "load_knowhow", "result": "RECIPE BODY", "success": true}),
        created: now + chrono::Duration::seconds(1),
        thread_id: None,
        sequence: Some(2),
    });
    // 5 unrelated tool calls after
    for i in 0..5_i64 {
        events.push(EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolCalled".into(),
            payload: json!({"name": format!("tool_{}", i), "args": {}, "description": "d"}),
            created: now + chrono::Duration::seconds(2 + i * 2),
            thread_id: None,
            sequence: Some(2 + i * 2 + 1),
        });
        events.push(EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": format!("tool_{}", i), "result": format!("r_{}", i), "success": true}),
            created: now + chrono::Duration::seconds(2 + i * 2 + 1),
            thread_id: None,
            sequence: Some(2 + i * 2 + 2),
        });
    }

    // N=2, but load_knowhow must be pinned even though it's older
    let (blocks, _skip_ids) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 2, &std::collections::HashSet::new());
    // Pinned load_knowhow pair + 2 most recent unrelated pairs = 3 pairs * 2 = 6
    assert_eq!(blocks.len(), 6);

    use crate::llm::{ContentBlock, MessageContent};
    let names: Vec<String> = blocks
        .iter()
        .filter_map(|m| {
            if let MessageContent::Blocks(b) = &m.content {
                if let ContentBlock::ToolUse { name, .. } = &b[0] {
                    return Some(name.clone());
                }
            }
            None
        })
        .collect();
    // Pinned load_knowhow first (chronological), then last 2 unrelated tools
    assert_eq!(
        names,
        vec!["load_knowhow".to_string(), "tool_3".into(), "tool_4".into()]
    );

    // Verify body preserved
    let body_present = blocks.iter().any(|m| {
        if let MessageContent::Blocks(b) = &m.content {
            if let ContentBlock::ToolResult { content, .. } = &b[0] {
                return content.contains("RECIPE BODY");
            }
        }
        false
    });
    assert!(body_present, "load_knowhow body should be present in pinned pair");
}

/// The skip set returned alongside the resume blocks must contain ONLY the
/// `ToolCalled` event ids that were emitted as full Message::Blocks pairs —
/// older non-pinned tools must NOT be in it, so their `[tools: ...]` summary
/// in stringified history survives. Without this, suppressing the summary
/// for the wrong rows would silently strip the orchestrator's older tool
/// context twice over (once dropped from blocks, once dropped from summary).
#[test]
fn build_resume_tool_blocks_skip_set_contains_only_emitted_pairs() {
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let mut events = Vec::new();
    let mut tool_called_ids = Vec::new();
    for i in 0..5_i64 {
        let id = uuid::Uuid::new_v4();
        tool_called_ids.push(id);
        events.push(EventRow {
            id,
            event_type: "ToolCalled".into(),
            payload: json!({"name": format!("tool_{}", i), "args": {}, "description": "d"}),
            created: now + chrono::Duration::seconds(i * 2),
            thread_id: None,
            sequence: Some(i * 2 + 1),
        });
        events.push(EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": format!("tool_{}", i), "result": format!("r_{}", i), "success": true}),
            created: now + chrono::Duration::seconds(i * 2 + 1),
            thread_id: None,
            sequence: Some(i * 2 + 2),
        });
    }
    let (_blocks, skip_ids) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 2, &std::collections::HashSet::new());
    // N=2 -> last two tool_called event ids only (tool_3, tool_4)
    assert_eq!(skip_ids.len(), 2);
    assert!(skip_ids.contains(&tool_called_ids[3].to_string()));
    assert!(skip_ids.contains(&tool_called_ids[4].to_string()));
    // Older tools NOT in the skip set — they keep their [tools: ...] summary
    assert!(!skip_ids.contains(&tool_called_ids[0].to_string()));
    assert!(!skip_ids.contains(&tool_called_ids[1].to_string()));
    assert!(!skip_ids.contains(&tool_called_ids[2].to_string()));
}

/// Two tools called back-to-back before either result arrives, then results
/// arrive in same call order. Exercises the `rposition`-by-name fallback
/// path in `collect_tool_pairs_chronological`: each `ToolResult` must be
/// paired with the `ToolCalled` of the same `name` (not blindly with the
/// most recent pending call), so the synthesized `tool_use_id` on the
/// reconstructed `ToolUse` matches its `ToolResult`'s `tool_use_id`.
#[test]
fn build_resume_tool_blocks_pairs_interleaved_tools_correctly() {
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let t1_called = uuid::Uuid::new_v4();
    let t2_called = uuid::Uuid::new_v4();
    let events = vec![
        EventRow {
            id: t1_called,
            event_type: "ToolCalled".into(),
            payload: json!({"name": "tool_a", "args": {"x": 1}, "description": "d"}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        EventRow {
            id: t2_called,
            event_type: "ToolCalled".into(),
            payload: json!({"name": "tool_b", "args": {"y": 2}, "description": "d"}),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
        // Result for tool_a arrives FIRST despite being called first
        // (interleaved — tool_b is still in flight).
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "tool_a", "result": "result_a", "success": true}),
            created: now + chrono::Duration::seconds(2),
            thread_id: None,
            sequence: Some(3),
        },
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "tool_b", "result": "result_b", "success": true}),
            created: now + chrono::Duration::seconds(3),
            thread_id: None,
            sequence: Some(4),
        },
    ];
    let (blocks, _skip_ids) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 5, &std::collections::HashSet::new());
    // Expect 4 messages (2 pairs).
    assert_eq!(blocks.len(), 4);
    use crate::llm::{ContentBlock, MessageContent};
    // First pair should be tool_a (ToolUse + ToolResult must reference same id).
    let (use_a_id, use_a_input) = match &blocks[0].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(name, "tool_a");
                (id.clone(), input.clone())
            }
            _ => panic!("expected ToolUse"),
        },
        _ => panic!("expected Blocks"),
    };
    assert_eq!(use_a_input["x"], 1);
    let result_a_id = match &blocks[1].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolResult { tool_use_id, content } => {
                assert!(content.contains("result_a"));
                tool_use_id.clone()
            }
            _ => panic!("expected ToolResult"),
        },
        _ => panic!("expected Blocks"),
    };
    assert_eq!(use_a_id, result_a_id, "tool_a's use+result must share id");
    // Second pair: tool_b
    match &blocks[2].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolUse { name, input, .. } => {
                assert_eq!(name, "tool_b");
                assert_eq!(input["y"], 2);
            }
            _ => panic!("expected ToolUse"),
        },
        _ => panic!("expected Blocks"),
    }
}

// ----------------------------------------------------------------------------
// dismiss_from_context — Phase 4 of the trigger-knowhow-discovery plan.
//
// `ContextDismissed` records produced by the `dismiss_from_context` tool let
// the agent prune individual `(ToolCalled, ToolResult)` pairs and
// `ChildThreadCompleted` blocks from its future resume context. The pruning
// happens in two layers:
// - `build_resume_tool_blocks_with_skip_ids` drops tool pairs (so they vanish
//   from BOTH the rebuilt verbatim blocks AND the stringified `[tools: ...]`
//   summary).
// - `build_session_messages` drops `ChildThreadCompleted` projections (so the
//   structured user-channel block stops appearing in history).
// ----------------------------------------------------------------------------

#[test]
fn build_resume_tool_blocks_with_skip_ids_excludes_dismissed_tool_pair() {
    use chrono::Utc;
    let now = Utc::now();
    let tool_a_called = Uuid::new_v4();
    let events = vec![
        EventRow {
            id: tool_a_called,
            event_type: "ToolCalled".into(),
            payload: json!({"name": "tool_a", "args": {}, "description": "d"}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        EventRow {
            id: Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "tool_a", "result": "result_a", "success": true}),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
        EventRow {
            id: Uuid::new_v4(),
            event_type: "ContextDismissed".into(),
            payload: json!({"dismissed_event_id": tool_a_called.to_string()}),
            created: now + chrono::Duration::seconds(2),
            thread_id: None,
            sequence: Some(3),
        },
    ];
    let (blocks, skip_ids) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 5, &std::collections::HashSet::new());
    assert!(blocks.is_empty(), "dismissed pair should be excluded");
    assert!(
        skip_ids.is_empty(),
        "dismissed pair must NOT survive in skip_ids — the agent asked the entry to be gone, not just shrunk"
    );
}

/// Two tool calls, only one dismissed — the other still appears in both
/// rebuilt blocks and skip_ids. Distinguishes "dismissal works" from
/// "dismissal is overzealous".
#[test]
fn build_resume_tool_blocks_with_skip_ids_keeps_undismissed_tool_pair() {
    use chrono::Utc;
    let now = Utc::now();
    let tool_a_called = Uuid::new_v4();
    let tool_b_called = Uuid::new_v4();
    let events = vec![
        EventRow {
            id: tool_a_called,
            event_type: "ToolCalled".into(),
            payload: json!({"name": "tool_a", "args": {}, "description": "d"}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        EventRow {
            id: Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "tool_a", "result": "result_a", "success": true}),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
        EventRow {
            id: tool_b_called,
            event_type: "ToolCalled".into(),
            payload: json!({"name": "tool_b", "args": {}, "description": "d"}),
            created: now + chrono::Duration::seconds(2),
            thread_id: None,
            sequence: Some(3),
        },
        EventRow {
            id: Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "tool_b", "result": "result_b", "success": true}),
            created: now + chrono::Duration::seconds(3),
            thread_id: None,
            sequence: Some(4),
        },
        EventRow {
            id: Uuid::new_v4(),
            event_type: "ContextDismissed".into(),
            payload: json!({"dismissed_event_id": tool_a_called.to_string()}),
            created: now + chrono::Duration::seconds(4),
            thread_id: None,
            sequence: Some(5),
        },
    ];
    let (blocks, skip_ids) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 5, &std::collections::HashSet::new());
    // tool_b survives → 2 messages (ToolUse + ToolResult).
    assert_eq!(blocks.len(), 2, "undismissed pair should survive");
    assert_eq!(skip_ids.len(), 1, "skip set has only the surviving pair");
    assert!(
        skip_ids.contains(&tool_b_called.to_string()),
        "surviving tool's id must be in skip set: {:?}",
        skip_ids
    );
    assert!(
        !skip_ids.contains(&tool_a_called.to_string()),
        "dismissed tool's id must NOT appear in skip set: {:?}",
        skip_ids
    );
}

/// Dismissal is order-independent — emitting the `ContextDismissed` BEFORE
/// the `ToolCalled` it targets (e.g. on a fresh resume reading a backfilled
/// out-of-order stream) still drops the pair. The set is collected up-front.
#[test]
fn build_resume_tool_blocks_with_skip_ids_dismissal_works_regardless_of_order() {
    use chrono::Utc;
    let now = Utc::now();
    let tool_called = Uuid::new_v4();
    let events = vec![
        EventRow {
            id: Uuid::new_v4(),
            event_type: "ContextDismissed".into(),
            payload: json!({"dismissed_event_id": tool_called.to_string()}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        EventRow {
            id: tool_called,
            event_type: "ToolCalled".into(),
            payload: json!({"name": "tool_x", "args": {}, "description": "d"}),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
        EventRow {
            id: Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "tool_x", "result": "result_x", "success": true}),
            created: now + chrono::Duration::seconds(2),
            thread_id: None,
            sequence: Some(3),
        },
    ];
    let (blocks, skip_ids) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 5, &std::collections::HashSet::new());
    assert!(blocks.is_empty(), "dismissed pair should still be excluded");
    assert!(skip_ids.is_empty());
}

/// `build_session_messages` drops a dismissed `ChildThreadCompleted` event
/// from history projection. The agent asked the structured callback to be
/// gone from its future resume context — keeping it in `messages[]` would
/// undo the dismissal at the projection layer.
#[test]
fn build_session_messages_excludes_dismissed_child_thread_completed() {
    use chrono::Utc;
    let now = Utc::now();
    let cc_event_id = Uuid::new_v4();
    let events = vec![
        EventRow {
            id: Uuid::new_v4(),
            event_type: "MessageReceived".into(),
            payload: json!({"text": "Run pipeline"}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        EventRow {
            id: cc_event_id,
            event_type: "ChildThreadCompleted".into(),
            payload: json!({
                "child_thread_id": Uuid::new_v4().to_string(),
                "child_thread_title": "Step 1",
                "status": "success",
                "summary": "all green",
                "pending_change_ids": [],
            }),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
        EventRow {
            id: Uuid::new_v4(),
            event_type: "ContextDismissed".into(),
            payload: json!({"dismissed_event_id": cc_event_id.to_string()}),
            created: now + chrono::Duration::seconds(2),
            thread_id: None,
            sequence: Some(3),
        },
    ];
    let msgs = build_session_messages(&events);
    assert!(
        !msgs.iter().any(|m| m.content.contains("[CHILD THREAD COMPLETED]")),
        "dismissed ChildThreadCompleted must not appear in projected messages: {:?}",
        msgs.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
}

/// Sanity: an undismissed `ChildThreadCompleted` IS projected as a
/// user-channel block. Pairs with the dismissal test above.
#[test]
fn build_session_messages_projects_undismissed_child_thread_completed() {
    use chrono::Utc;
    let now = Utc::now();
    let child_id = Uuid::new_v4();
    let cc_event_id = Uuid::new_v4();
    let events = vec![
        EventRow {
            id: cc_event_id,
            event_type: "ChildThreadCompleted".into(),
            payload: json!({
                "child_thread_id": child_id.to_string(),
                "child_thread_title": "Nightly",
                "status": "success",
                "summary": "all green",
                "pending_change_ids": ["change-1"],
            }),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
    ];
    let msgs = build_session_messages(&events);
    let cc = msgs
        .iter()
        .find(|m| m.content.contains("[CHILD THREAD COMPLETED]"))
        .expect("child thread completed message");
    assert_eq!(cc.role, "user");
    assert!(cc.content.contains("success"));
    assert!(cc.content.contains("Nightly"));
    assert!(cc.content.contains("all green"));
    assert!(cc.content.contains("change-1"));
    // C1: the EVENT id (not the child_thread_id) must be surfaced as an
    // `event_id: <uuid>` line so the LLM can pass it to dismiss_from_context.
    let expected = format!("event_id: {}", cc_event_id);
    assert!(
        cc.content.contains(&expected),
        "projection must surface the event_id; got:\n{}",
        cc.content
    );
    // The "session can finish refers to child only" guard rail lives
    // inside this projection (not the wake-up signal — there is no
    // wake_text after the child-completion-card refactor). Keep a narrow
    // assertion so the rule stays load-bearing.
    assert!(
        cc.content.contains("session can finish")
            && cc.content.contains("child subprocess"),
        "projection must carry the 'session can finish' guard rail; got:\n{}",
        cc.content
    );
}
