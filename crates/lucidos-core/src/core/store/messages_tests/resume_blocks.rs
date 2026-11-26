use super::msg_helpers::*;
use super::*;
use crate::core::changes::ChangeStatus;

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
            payload: json!({"name": "read_file", "args": {"path": "x"}, "description": "Reading"}),
            created: now,
            thread_id: None,
            sequence: Some(2),
        },
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "read_file", "result": "PROCEDURE BODY", "success": true}),
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
    let (blocks, _skip_ids) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        3,
        &std::collections::HashSet::new(),
    );
    assert_eq!(
        blocks.len(),
        2,
        "expect assistant ToolUse + user ToolResult"
    );

    use crate::llm::{ContentBlock, MessageContent};
    match &blocks[0].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "x");
                assert!(id.starts_with("evt-"), "id should be synthesized: {}", id);
            }
            _ => panic!("expected ToolUse block"),
        },
        _ => panic!("expected Blocks content"),
    }
    match &blocks[1].content {
        MessageContent::Blocks(b) => match &b[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
            } => {
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
fn a_retired_execute_intent_pair_still_replays_on_resume() {
    use crate::core::EventRow;
    use crate::llm::{ContentBlock, MessageContent};
    let now = chrono::Utc::now();
    let row = |event_type: &str, payload: serde_json::Value, sequence| EventRow {
        id: uuid::Uuid::new_v4(),
        event_type: event_type.into(),
        payload,
        created: now,
        thread_id: None,
        sequence: Some(sequence),
    };
    let events = vec![
        row("MessageReceived", json!({"text": "Check my calendar."}), 1),
        row(
            "ToolCalled",
            json!({"name": "execute_intent", "args": {"intent_id": "calendar/check"}}),
            2,
        ),
        row(
            "ToolResult",
            json!({"name": "execute_intent", "result": "Calendar is clear.", "success": true}),
            3,
        ),
        row(
            "ResponseGenerated",
            json!({"text": "Your calendar is clear."}),
            4,
        ),
    ];
    let (blocks, _skip_ids) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        3,
        &std::collections::HashSet::new(),
    );
    assert_eq!(
        blocks.len(),
        2,
        "expect assistant ToolUse + user ToolResult"
    );
    let MessageContent::Blocks(call) = &blocks[0].content else {
        panic!("expected Blocks content");
    };
    assert!(
        matches!(&call[0], ContentBlock::ToolUse { name, input, .. }
            if name == "execute_intent" && input["intent_id"] == "calendar/check"),
        "{call:?}"
    );
    let MessageContent::Blocks(result) = &blocks[1].content else {
        panic!("expected Blocks content");
    };
    assert!(
        matches!(&result[0], ContentBlock::ToolResult { content, .. }
            if content.contains("Calendar is clear.")),
        "{result:?}"
    );
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
    let (blocks, skip_ids) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        3,
        &std::collections::HashSet::new(),
    );
    assert!(blocks.is_empty());
    assert!(skip_ids.is_empty());
}

#[test]
fn build_resume_tool_blocks_caps_at_n() {
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
    let (blocks, _skip_ids) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        2,
        &std::collections::HashSet::new(),
    );
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
fn build_resume_tool_blocks_drop_an_old_load_knowhow_like_any_tool() {
    use crate::core::EventRow;
    use chrono::Utc;
    let now = Utc::now();
    let mut events = Vec::new();
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

    let (blocks, _skip_ids) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        2,
        &std::collections::HashSet::new(),
    );
    assert_eq!(blocks.len(), 4, "2 most recent pairs * 2 messages each");

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
    assert_eq!(names, vec!["tool_3".to_string(), "tool_4".into()]);

    let body_present = blocks.iter().any(|m| {
        if let MessageContent::Blocks(b) = &m.content {
            if let ContentBlock::ToolResult { content, .. } = &b[0] {
                return content.contains("RECIPE BODY");
            }
        }
        false
    });
    assert!(
        !body_present,
        "an old knowhow body must not ride in the resume blocks"
    );
}

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
    let (_blocks, skip_ids) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        2,
        &std::collections::HashSet::new(),
    );
    assert_eq!(skip_ids.len(), 2);
    assert!(skip_ids.contains(&tool_called_ids[3].to_string()));
    assert!(skip_ids.contains(&tool_called_ids[4].to_string()));
    assert!(!skip_ids.contains(&tool_called_ids[0].to_string()));
    assert!(!skip_ids.contains(&tool_called_ids[1].to_string()));
    assert!(!skip_ids.contains(&tool_called_ids[2].to_string()));
}

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
    let (blocks, _skip_ids) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        5,
        &std::collections::HashSet::new(),
    );
    assert_eq!(blocks.len(), 4);
    use crate::llm::{ContentBlock, MessageContent};
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
            ContentBlock::ToolResult {
                tool_use_id,
                content,
            } => {
                assert!(content.contains("result_a"));
                tool_use_id.clone()
            }
            _ => panic!("expected ToolResult"),
        },
        _ => panic!("expected Blocks"),
    };
    assert_eq!(use_a_id, result_a_id, "tool_a's use+result must share id");
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
    let (blocks, skip_ids) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        5,
        &std::collections::HashSet::new(),
    );
    assert!(blocks.is_empty(), "dismissed pair should be excluded");
    assert!(
        skip_ids.is_empty(),
        "dismissed pair must NOT survive in skip_ids — the agent asked the entry to be gone, not just shrunk"
    );
}

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
    let (blocks, skip_ids) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        5,
        &std::collections::HashSet::new(),
    );
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
    let (blocks, skip_ids) = crate::core::store::build_resume_tool_blocks_with_skip_ids(
        &events,
        5,
        &std::collections::HashSet::new(),
    );
    assert!(blocks.is_empty(), "dismissed pair should still be excluded");
    assert!(skip_ids.is_empty());
}

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
        !msgs
            .iter()
            .any(|m| m.content.contains("[CHILD THREAD COMPLETED]")),
        "dismissed ChildThreadCompleted must not appear in projected messages: {:?}",
        msgs.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
}

#[test]
fn build_session_messages_projects_undismissed_child_thread_completed() {
    use chrono::Utc;
    let now = Utc::now();
    let child_id = Uuid::new_v4();
    let cc_event_id = Uuid::new_v4();
    let events = vec![EventRow {
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
    }];
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
    let expected = format!("event_id: {}", cc_event_id);
    assert!(
        cc.content.contains(&expected),
        "projection must surface the event_id; got:\n{}",
        cc.content
    );
    assert!(
        cc.content.contains("session can finish") && cc.content.contains("child subprocess"),
        "projection must carry the 'session can finish' guard rail; got:\n{}",
        cc.content
    );
}

#[test]
fn build_session_messages_projects_child_thread_stopped_as_alive() {
    use chrono::Utc;
    let child_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let events = vec![EventRow {
        id: event_id,
        event_type: "ChildThreadStopped".into(),
        payload: json!({
            "child_thread_id": child_id.to_string(),
            "child_thread_title": "Fix the ticket",
        }),
        created: Utc::now(),
        thread_id: None,
        sequence: Some(1),
    }];
    let msgs = build_session_messages(&events);
    let note = msgs
        .iter()
        .find(|m| m.content.contains("[CHILD THREAD STOPPED]"))
        .expect("child thread stopped message");
    assert_eq!(note.role, "user");
    for needle in [
        child_id.to_string(),
        format!("event_id: {event_id}"),
        "Fix the ticket".to_string(),
        "NOT finished".to_string(),
        "[CHILD THREAD COMPLETED]".to_string(),
        "do not roll back".to_string(),
    ] {
        assert!(
            note.content.contains(&needle),
            "missing {needle:?} in:\n{}",
            note.content
        );
    }
}

#[test]
fn build_session_messages_projects_a_filed_report_with_its_note() {
    use chrono::Utc;
    let child_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    let events = vec![EventRow {
        id: Uuid::new_v4(),
        event_type: "ChildReportFiled".into(),
        payload: json!({
            "child_thread_id": child_id.to_string(),
            "child_thread_title": "Check the venue",
            "status": "success",
            "summary": "It opens at nine.",
            "pending_change_ids": [change_id.to_string()],
        }),
        created: Utc::now(),
        thread_id: None,
        sequence: Some(1),
    }];
    assert_eq!(super::build::reported_change_ids(&events), vec![change_id]);
    let msgs = build_session_messages(&events);
    let report = msgs
        .iter()
        .find(|m| m.content.contains("[CHILD THREAD REPORT FILED]"))
        .expect("filed report message");
    assert_eq!(report.role, "user");
    for needle in [
        child_id.to_string(),
        "completed (success)".to_string(),
        "It opens at nine.".to_string(),
        super::build::REPORT_FILED_NOTE.trim().to_string(),
    ] {
        assert!(
            report.content.contains(&needle),
            "missing {needle:?} in:\n{}",
            report.content
        );
    }
}

#[test]
fn build_session_messages_projects_child_thread_detached_as_moved_out() {
    use chrono::Utc;
    let child_id = Uuid::new_v4();
    let events = vec![EventRow {
        id: Uuid::new_v4(),
        event_type: "ChildThreadDetached".into(),
        payload: json!({
            "child_thread_id": child_id.to_string(),
            "child_thread_title": "Fix the ticket",
        }),
        created: Utc::now(),
        thread_id: None,
        sequence: Some(1),
    }];
    let msgs = build_session_messages(&events);
    let note = msgs
        .iter()
        .find(|m| m.content.contains("[CHILD THREAD MOVED OUT]"))
        .expect("child thread moved-out message");
    assert_eq!(note.role, "user");
    for needle in [
        child_id.to_string(),
        "Fix the ticket".to_string(),
        "no longer your child".to_string(),
        "Do not wait for it or respawn it".to_string(),
    ] {
        assert!(
            note.content.contains(&needle),
            "missing {needle:?} in:\n{}",
            note.content
        );
    }
}

#[test]
fn a_canceled_card_reads_as_the_users_final_word() {
    use chrono::Utc;
    let events = vec![EventRow {
        id: Uuid::new_v4(),
        event_type: "ChildThreadCompleted".into(),
        payload: json!({
            "child_thread_id": Uuid::new_v4().to_string(),
            "status": "canceled",
            "summary": "The user archived this child thread.",
        }),
        created: Utc::now(),
        thread_id: None,
        sequence: Some(1),
    }];
    let msgs = build_session_messages(&events);
    let card = &msgs[0].content;
    assert!(card.contains(" canceled\n"), "got:\n{card}");
    assert!(!card.contains("user stop"), "got:\n{card}");
}

#[test]
fn every_rebuilt_tool_result_carries_its_event_address() {
    use crate::core::EventRow;
    use crate::llm::{ContentBlock, MessageContent};
    use chrono::Utc;
    let now = Utc::now();
    let plain_id = uuid::Uuid::new_v4();
    let orphan_id = uuid::Uuid::new_v4();
    let knowhow_id = uuid::Uuid::new_v4();
    let events = vec![
        EventRow {
            id: knowhow_id,
            event_type: "ToolCalled".into(),
            payload: json!({"name": "load_knowhow", "args": {"id": "recipe"}}),
            created: now,
            thread_id: None,
            sequence: Some(1),
        },
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "load_knowhow", "result": "BODY", "success": true}),
            created: now + chrono::Duration::seconds(1),
            thread_id: None,
            sequence: Some(2),
        },
        EventRow {
            id: plain_id,
            event_type: "ToolCalled".into(),
            payload: json!({"name": "read_file", "args": {}}),
            created: now + chrono::Duration::seconds(2),
            thread_id: None,
            sequence: Some(3),
        },
        EventRow {
            id: uuid::Uuid::new_v4(),
            event_type: "ToolResult".into(),
            payload: json!({"name": "read_file", "result": "file body", "success": true}),
            created: now + chrono::Duration::seconds(3),
            thread_id: None,
            sequence: Some(4),
        },
        EventRow {
            id: orphan_id,
            event_type: "ToolCalled".into(),
            payload: json!({"name": "run_bash", "args": {}}),
            created: now + chrono::Duration::seconds(4),
            thread_id: None,
            sequence: Some(5),
        },
    ];
    let mut loaded = std::collections::HashSet::new();
    loaded.insert("recipe".to_string());
    let (blocks, _skip) =
        crate::core::store::build_resume_tool_blocks_with_skip_ids(&events, 3, &loaded);

    let results: Vec<(&String, &String)> = blocks
        .iter()
        .filter_map(|m| match &m.content {
            MessageContent::Blocks(b) => match b.first() {
                Some(ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                }) => Some((tool_use_id, content)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 3, "one result per call, orphan included");

    for (tool_use_id, content) in results {
        let trailer = format!("[{tool_use_id}]");
        assert_eq!(
            content.lines().next_back().map(str::trim),
            Some(trailer.as_str()),
            "last line must be the result's own address; got:\n{content}"
        );
    }

    let bodies: Vec<String> = blocks
        .iter()
        .filter_map(|m| match &m.content {
            MessageContent::Blocks(b) => match b.first() {
                Some(ContentBlock::ToolResult { content, .. }) => Some(content.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert!(bodies.iter().any(|c| c.contains("[LOADED KNOWHOW]")));
    assert!(bodies
        .iter()
        .any(|c| c.contains(crate::llm::validate::ORPHAN_TOOL_RESULT_STUB)));
    assert!(bodies.iter().any(|c| c.contains("file body")));
}

#[test]
fn child_completed_block_lists_sub_thread_changes_apart_from_its_own() {
    use chrono::Utc;
    let (settled_change, working_change) = (Uuid::new_v4(), Uuid::new_v4());
    let (settled_thread, working_thread) = (Uuid::new_v4(), Uuid::new_v4());
    let row = |payload: serde_json::Value| EventRow {
        id: Uuid::new_v4(),
        event_type: "ChildThreadCompleted".into(),
        payload,
        created: Utc::now(),
        thread_id: None,
        sequence: Some(1),
    };
    let block = format_child_thread_completed_block(
        &row(json!({
            "child_thread_id": Uuid::new_v4().to_string(),
            "child_thread_title": "orchestrator",
            "status": "success",
            "summary": "handed over",
            "sub_thread_pending_changes": [
                {
                    "change_id": settled_change,
                    "thread_id": settled_thread,
                    "thread_title": "milestone 9b",
                    "thread_unsettled": false,
                },
                {
                    "change_id": working_change,
                    "thread_id": working_thread,
                    "thread_unsettled": true,
                },
            ],
        })),
        &ChangeStatuses::new(),
    );
    assert!(
        block.contains("Pending changes: none\nPending changes in its sub-threads"),
        "the own line stays, and the sub-thread section follows it; got:\n{block}"
    );
    assert!(
        block.contains(&format!(
            "- {settled_change} from sub-thread \"milestone 9b\" ({settled_thread}): settled"
        )),
        "got:\n{block}"
    );
    assert!(
        block.contains(&format!(
            "- {working_change} from sub-thread \"untitled\" ({working_thread}): still working"
        )),
        "got:\n{block}"
    );

    let plain = format_child_thread_completed_block(
        &row(json!({
            "child_thread_id": Uuid::new_v4().to_string(),
            "status": "success",
            "summary": "done",
        })),
        &ChangeStatuses::new(),
    );
    assert!(!plain.contains("sub-threads"), "got:\n{plain}");
}

#[test]
fn child_completed_block_names_the_branch_an_integrated_child_hands_up() {
    let block = format_child_thread_completed_block(
        &child_report_row(json!({
            "child_thread_id": Uuid::new_v4().to_string(),
            "status": "success",
            "summary": "fixed the follow-up",
            "integrated_branch": {
                "branch_name": "claude-code/follow-up",
                "head_sha": "0123abcd",
            },
        })),
        &ChangeStatuses::new(),
    );
    assert!(
        block.contains("Integrated branch: claude-code/follow-up at 0123abcd."),
        "got:\n{block}"
    );
    assert!(
        block.contains("`git merge claude-code/follow-up`"),
        "got:\n{block}"
    );
    let plain = format_child_thread_completed_block(
        &child_report_row(json!({
            "child_thread_id": Uuid::new_v4().to_string(),
            "status": "success",
            "summary": "done",
        })),
        &ChangeStatuses::new(),
    );
    assert!(!plain.contains("Integrated branch"), "got:\n{plain}");
}

fn child_report_row(payload: serde_json::Value) -> EventRow {
    EventRow {
        id: Uuid::new_v4(),
        event_type: "ChildThreadCompleted".into(),
        payload,
        created: chrono::Utc::now(),
        thread_id: None,
        sequence: Some(1),
    }
}

#[test]
fn child_completed_block_names_what_became_of_each_change_since() {
    let (applied, waiting, discarded) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let sub_thread = Uuid::new_v4();
    let report = child_report_row(json!({
        "child_thread_id": Uuid::new_v4().to_string(),
        "status": "success",
        "summary": "3 commits, change pending your Apply",
        "pending_change_ids": [applied, waiting],
        "sub_thread_pending_changes": [{
            "change_id": discarded,
            "thread_id": sub_thread,
            "thread_title": "helper",
            "thread_unsettled": false,
        }],
    }));
    let statuses = ChangeStatuses::from([
        (applied, Some(ChangeStatus::Applied)),
        (waiting, Some(ChangeStatus::Pending)),
        (discarded, Some(ChangeStatus::Discarded)),
    ]);

    let block = format_child_thread_completed_block(&report, &statuses);

    assert!(
        block.contains(&format!(
            "Pending changes: {applied} (now applied), {waiting}\n"
        )),
        "an applied change says so, a pending one reads as before; got:\n{block}"
    );
    assert!(
        block.contains(&format!(
            "- {discarded} from sub-thread \"helper\" ({sub_thread}): now discarded"
        )),
        "the frozen sub-thread state gives way to the change's own; got:\n{block}"
    );
    assert!(
        block.contains(CHANGES_MOVED_ON_NOTE),
        "the card overrides the summary's stale claim; got:\n{block}"
    );

    let unchanged = format_child_thread_completed_block(&report, &ChangeStatuses::new());
    assert!(
        unchanged.contains(&format!("Pending changes: {applied}, {waiting}\n")),
        "got:\n{unchanged}"
    );
    assert!(!unchanged.contains("now "), "got:\n{unchanged}");
    assert!(
        !unchanged.contains(CHANGES_MOVED_ON_NOTE),
        "got:\n{unchanged}"
    );
}

#[tokio::test]
async fn store_built_messages_read_each_reported_change_as_it_is_now() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    let change_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO changes (id, request_id, branch_name, repo_root, status) \
         VALUES ($1, $2, 'b-applied', '/repo', 'applied')",
    )
    .bind(change_id)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .expect("insert an applied change");
    let deleted_id = Uuid::new_v4();
    let events = vec![child_report_row(json!({
        "child_thread_id": Uuid::new_v4().to_string(),
        "status": "success",
        "summary": "pending your Apply",
        "pending_change_ids": [change_id, deleted_id],
    }))];

    let messages = EventStore::new(pool.clone())
        .build_messages_now(&events)
        .await;

    assert_eq!(messages.len(), 1);
    assert!(
        messages[0].content.contains(&format!(
            "Pending changes: {change_id} (now applied), {deleted_id} (now deleted)"
        )),
        "got:\n{}",
        messages[0].content
    );
    teardown_test_db(&db).await;
}
