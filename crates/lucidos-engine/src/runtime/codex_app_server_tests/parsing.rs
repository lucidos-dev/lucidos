use crate::engine::thread_events::{TurnEndFacts, TurnEndStatus};
use crate::runtime::agent_runtime::AgentEvent;
use crate::runtime::codex::UNREQUESTED_INTERRUPT_ERROR;
use crate::runtime::codex_app_server_parse::*;

fn note(
    tracker: &mut AppServerTracker,
    method: &str,
    params: serde_json::Value,
) -> Vec<AgentEvent> {
    tracker.map_notification(method, &params, 42)
}

#[test]
fn resumed_startup_usage_is_not_a_new_call() {
    let mut tracker = AppServerTracker::new(Some("t-resumed".into()));
    let params = serde_json::json!({
        "threadId": "t-resumed", "turnId": "old-turn",
        "tokenUsage": {"last": {"inputTokens": 100, "cachedInputTokens": 60,
            "cacheWriteInputTokens": 10, "outputTokens": 5}}
    });
    let events = note(&mut tracker, "thread/tokenUsage/updated", params);
    assert_eq!(
        events.len(),
        1,
        "restored usage must still be recorded once"
    );
    assert!(matches!(&events[0], AgentEvent::UsageRestored { snapshot }
        if snapshot.session_id == "t-resumed" && snapshot.turn_id.as_deref() == Some("old-turn")
            && snapshot.input_tokens == 30 && snapshot.cache_read_tokens == 60
            && snapshot.cache_creation_tokens == 10 && snapshot.output_tokens == 5));
}

#[test]
fn codex_resume_an_empty_trailing_turn_cannot_replace_observed_snapshot_identity() {
    let mut tracker = AppServerTracker::new(Some("t-1".into()));
    let usage = serde_json::json!({"threadId": "t-1", "turnId": "last-billed-turn",
        "tokenUsage": {"last": {"inputTokens": 10, "outputTokens": 2}}});
    let initial = note(&mut tracker, "thread/tokenUsage/updated", usage.clone());
    assert!(matches!(&initial[..], [AgentEvent::UsageRestored { .. }]));
    tracker.note_resume_response(&serde_json::json!({"thread": {"id": "t-1",
        "turns": [{"id": "last-billed-turn"}, {"id": "empty-interrupted-turn"}]}}));
    tracker.begin_turn(None);
    tracker.current_turn_id = Some("new-turn".into());
    let late = note(&mut tracker, "thread/tokenUsage/updated", usage);
    assert!(matches!(&late[..], [AgentEvent::UsageRestored { .. }]));
}

#[test]
fn codex_resume_late_snapshot_and_identical_new_usage_stay_distinct() {
    let mut tracker = AppServerTracker::new(Some("t-resumed".into()));
    tracker.note_resume_response(&serde_json::json!({"thread": {
        "id": "t-resumed", "turns": [{"id": "old-turn"}]}}));
    tracker.begin_turn(Some("high".into()));
    tracker.current_turn_id = Some("new-turn".into());
    let usage = |thread: &str, turn: &str| {
        serde_json::json!({
            "threadId": thread, "turnId": turn,
            "tokenUsage": {"last": {"inputTokens": 100, "outputTokens": 5}}
        })
    };
    let restored = note(
        &mut tracker,
        "thread/tokenUsage/updated",
        usage("t-resumed", "old-turn"),
    );
    assert!(matches!(&restored[..], [AgentEvent::UsageRestored { .. }]));
    for (thread, turn) in [("t-resumed", "new-turn"), ("child", "old-turn")] {
        let live = note(
            &mut tracker,
            "thread/tokenUsage/updated",
            usage(thread, turn),
        );
        assert!(matches!(
            &live[..],
            [AgentEvent::Usage {
                input_tokens: 100,
                output_tokens: 5,
                ..
            }]
        ));
    }
    tracker.note_thread_started("fresh-thread".into(), None);
    tracker.current_turn_id = Some("old-turn".into());
    let fresh = note(
        &mut tracker,
        "thread/tokenUsage/updated",
        usage("fresh-thread", "old-turn"),
    );
    assert!(matches!(&fresh[..], [AgentEvent::Usage { .. }]));
}

#[test]
fn codex_current_turn_identity_takes_precedence_over_restored_identity() {
    let mut tracker = AppServerTracker::new(Some("t-1".into()));
    tracker.note_resume_response(&serde_json::json!({"thread": {
        "id": "t-1", "turns": [{"id": "reused-turn"}]}}));
    tracker.begin_turn(None);
    tracker.current_turn_id = Some("reused-turn".into());
    let events = note(
        &mut tracker,
        "thread/tokenUsage/updated",
        serde_json::json!({"threadId": "t-1", "turnId": "reused-turn",
            "tokenUsage": {"last": {"inputTokens": 10, "outputTokens": 2}}}),
    );
    assert!(matches!(&events[..], [AgentEvent::Usage { .. }]));
}

#[test]
fn codex_usage_with_no_proven_current_or_restored_turn_stays_unscoped() {
    for resume in [None, Some("t-1".into())] {
        let mut tracker = AppServerTracker::new(resume);
        tracker.begin_turn(None);
        tracker.current_turn_id = Some("new-turn".into());
        for turn in [
            serde_json::Value::Null,
            serde_json::json!("unidentified-turn"),
        ] {
            let events = note(
                &mut tracker,
                "thread/tokenUsage/updated",
                serde_json::json!({
                    "threadId": "t-1", "turnId": turn,
                    "tokenUsage": {"last": {"inputTokens": 10, "outputTokens": 2}}
                }),
            );
            assert!(
                matches!(&events[..], [AgentEvent::UsageUnscoped { snapshot }]
                if snapshot.input_tokens == 10 && snapshot.output_tokens == 2)
            );
        }
    }
}

#[test]
fn parse_line_classifies_the_three_frame_shapes() {
    assert_eq!(
        parse_app_server_line(r#"{"id":3,"result":{"ok":true}}"#),
        AppServerLine::Response {
            id: 3,
            result: serde_json::json!({"ok": true}),
            error: None,
        }
    );
    assert_eq!(
        parse_app_server_line(r#"{"id":4,"error":{"code":-1,"message":"boom"}}"#),
        AppServerLine::Response {
            id: 4,
            result: serde_json::Value::Null,
            error: Some("boom".to_string()),
        }
    );
    assert_eq!(
        parse_app_server_line(r#"{"method":"turn/started","params":{"threadId":"t"}}"#),
        AppServerLine::Notification {
            method: "turn/started".to_string(),
            params: serde_json::json!({"threadId": "t"}),
        }
    );
    assert_eq!(
        parse_app_server_line(
            r#"{"id":"req-1","method":"item/commandExecution/requestApproval","params":{}}"#
        ),
        AppServerLine::ServerRequest {
            id: serde_json::json!("req-1"),
            method: "item/commandExecution/requestApproval".to_string(),
            params: serde_json::json!({}),
        }
    );
    assert_eq!(parse_app_server_line("not json"), AppServerLine::Other);
    assert_eq!(parse_app_server_line(""), AppServerLine::Other);
}

#[test]
fn thread_started_emits_init_once() {
    let mut t = AppServerTracker::new(None);
    let evs = t.note_thread_started("t-1".to_string(), Some("gpt-5.5".to_string()));
    assert!(matches!(
        &evs[..],
        [AgentEvent::Init { session_id, model: Some(m), .. }]
            if session_id == "t-1" && m == "gpt-5.5"
    ));
    let evs = note(
        &mut t,
        "thread/started",
        serde_json::json!({"thread": {"id": "t-1"}}),
    );
    assert!(evs.is_empty(), "duplicate Init must be suppressed");
    assert_eq!(t.session_id.as_deref(), Some("t-1"));
}

#[test]
fn turn_result_prefers_final_answers_and_preserves_streamed_commentary() {
    assert_turn_messages(
        &[
            (Some("commentary"), "Checking"),
            (Some("final_answer"), "Done"),
        ],
        "Done",
    );
}

#[test]
fn turn_result_joins_multiple_final_answers() {
    assert_turn_messages(
        &[
            (Some("commentary"), "Checking"),
            (Some("final_answer"), "First"),
            (Some("partial_answer"), "Interim"),
            (Some("final_answer"), "Second"),
        ],
        "First\n\nSecond",
    );
}

#[test]
fn turn_result_without_phases_keeps_all_messages() {
    assert_turn_messages(&[(None, "Checking"), (None, "Done")], "Checking\n\nDone");
}

#[test]
fn turn_result_without_final_answer_keeps_all_messages() {
    assert_turn_messages(
        &[
            (Some("commentary"), "Checking"),
            (Some("partial_answer"), "Interim"),
        ],
        "Checking\n\nInterim",
    );
}

fn assert_turn_messages(messages: &[(Option<&str>, &str)], expected: &str) {
    let mut tracker = AppServerTracker::new(Some("t-1".into()));
    tracker.begin_turn(None);
    for (index, (phase, text)) in messages.iter().enumerate() {
        let id = format!("i{index}");
        let streamed = note(
            &mut tracker,
            "item/agentMessage/delta",
            serde_json::json!({"itemId": id, "delta": text, "threadId": "t-1", "turnId": "u"}),
        );
        assert!(
            matches!(&streamed[..], [AgentEvent::Message { text: actual, .. }] if actual == text)
        );
        let completed = note(
            &mut tracker,
            "item/completed",
            serde_json::json!({"item": {"id": id, "type": "agentMessage", "text": text, "phase": phase}}),
        );
        assert!(completed.is_empty(), "streamed text must not be duplicated");
    }
    let completed = note(
        &mut tracker,
        "turn/completed",
        serde_json::json!({"turn": {"id": "u", "status": "completed"}}),
    );
    assert!(
        matches!(&completed[..], [AgentEvent::Result { text, .. }] if text == expected),
        "{completed:?}"
    );
    tracker.begin_turn(None);
    assert_eq!(tracker.turn_text(), "", "a new turn clears prior answers");
}

#[test]
fn agent_message_deltas_stream_and_completed_emits_only_remainder() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let evs = note(
        &mut t,
        "item/agentMessage/delta",
        serde_json::json!({"itemId": "i1", "delta": "Hel", "threadId": "t-1", "turnId": "u"}),
    );
    assert!(matches!(&evs[..], [AgentEvent::Message { text, .. }] if text == "Hel"));
    note(
        &mut t,
        "item/agentMessage/delta",
        serde_json::json!({"itemId": "i1", "delta": "lo", "threadId": "t-1", "turnId": "u"}),
    );
    let evs = note(
        &mut t,
        "item/completed",
        serde_json::json!({"item": {"id": "i1", "type": "agentMessage", "text": "Hello!"}}),
    );
    assert!(
        matches!(&evs[..], [AgentEvent::Message { text, .. }] if text == "!"),
        "completed agentMessage must emit only the remainder; got {evs:?}"
    );
    assert_eq!(
        t.turn_text(),
        "Hello!",
        "Result.text carries the full message"
    );
}

#[test]
fn only_the_first_delta_of_an_agent_message_opens_a_block() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let mut delta = |item: &str, text: &str| {
        note(
            &mut t,
            "item/agentMessage/delta",
            serde_json::json!({"itemId": item, "delta": text, "threadId": "t-1", "turnId": "u"}),
        )
    };
    let opens: Vec<bool> = [("i1", "Hel"), ("i1", "lo."), ("i2", "Next"), ("i2", " one")]
        .into_iter()
        .flat_map(|(item, text)| delta(item, text))
        .map(|ev| match ev {
            AgentEvent::Message { opens_block, .. } => opens_block,
            other => panic!("expected Message, got {other:?}"),
        })
        .collect();
    assert_eq!(opens, [true, false, true, false]);
}

#[test]
fn a_completed_remainder_continues_its_block_but_an_unstreamed_message_opens_one() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    note(
        &mut t,
        "item/agentMessage/delta",
        serde_json::json!({"itemId": "i1", "delta": "Hel", "threadId": "t-1", "turnId": "u"}),
    );
    let remainder = note(
        &mut t,
        "item/completed",
        serde_json::json!({"item": {"id": "i1", "type": "agentMessage", "text": "Hello"}}),
    );
    assert!(
        matches!(&remainder[..], [AgentEvent::Message { text, opens_block: false, .. }] if text == "lo"),
        "got {remainder:?}"
    );
    let whole = note(
        &mut t,
        "item/completed",
        serde_json::json!({"item": {"id": "i2", "type": "agentMessage", "text": "whole"}}),
    );
    assert!(
        matches!(&whole[..], [AgentEvent::Message { text, opens_block: true, .. }] if text == "whole"),
        "got {whole:?}"
    );
}

#[test]
fn plan_update_emits_normalized_todo_list_pair() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let evs = note(
        &mut t,
        "turn/plan/updated",
        serde_json::json!({
            "threadId": "t-1", "turnId": "u", "explanation": null,
            "plan": [
                {"step": "Map the code", "status": "completed"},
                {"step": "Fix the bug", "status": "inProgress"},
                {"step": "Run tests", "status": "pending"},
            ],
        }),
    );
    match &evs[..] {
        [AgentEvent::ToolUse {
            name, input, id, ..
        }, AgentEvent::ToolResult {
            status,
            id: result_id,
            ..
        }] => {
            assert_eq!(name, "todo_list");
            assert_eq!(id, result_id, "pair must share an id");
            assert_eq!(status, "success");
            assert_eq!(
                input,
                &serde_json::json!({"items": [
                    {"text": "Map the code", "completed": true},
                    {"text": "Fix the bug", "completed": false},
                    {"text": "Run tests", "completed": false},
                ]}),
                "plan steps normalize to the exec todo_list shape"
            );
        }
        other => panic!("expected a ToolUse/ToolResult pair, got {other:?}"),
    }
}

#[test]
fn plan_update_dedupes_identical_snapshots_across_turns() {
    let plan = serde_json::json!({
        "threadId": "t-1", "turnId": "u",
        "plan": [{"step": "a", "status": "inProgress"}],
    });
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    assert_eq!(note(&mut t, "turn/plan/updated", plan.clone()).len(), 2);
    assert!(note(&mut t, "turn/plan/updated", plan.clone()).is_empty());
    t.begin_turn(None);
    assert!(note(&mut t, "turn/plan/updated", plan).is_empty());
    let evs = note(
        &mut t,
        "turn/plan/updated",
        serde_json::json!({
            "threadId": "t-1", "turnId": "u",
            "plan": [{"step": "a", "status": "completed"}],
        }),
    );
    assert_eq!(evs.len(), 2);
    let (first_id, second_id) = (
        match &evs[0] {
            AgentEvent::ToolUse { id, .. } => id.clone(),
            other => panic!("expected ToolUse, got {other:?}"),
        },
        "plan_1".to_string(),
    );
    assert_ne!(first_id, second_id, "each emission gets a unique id");
    assert!(note(
        &mut t,
        "turn/plan/updated",
        serde_json::json!({"threadId": "t-1", "turnId": "u", "plan": []}),
    )
    .is_empty());
}

#[test]
fn reasoning_deltas_emit_thoughts() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let evs = note(
        &mut t,
        "item/reasoning/textDelta",
        serde_json::json!({"itemId": "r1", "delta": "Think", "threadId": "t-1", "turnId": "u", "contentIndex": 0}),
    );
    assert!(matches!(&evs[..], [AgentEvent::Thought { text }] if text == "Think"));
    let evs = note(
        &mut t,
        "item/reasoning/summaryTextDelta",
        serde_json::json!({"itemId": "r1", "delta": "ing", "threadId": "t-1", "turnId": "u", "summaryIndex": 0}),
    );
    assert!(matches!(&evs[..], [AgentEvent::Thought { text }] if text == "ing"));
    let evs = note(
        &mut t,
        "item/reasoning/textDelta",
        serde_json::json!({"itemId": "r1", "delta": "", "threadId": "t-1", "turnId": "u", "contentIndex": 1}),
    );
    assert!(
        evs.is_empty(),
        "empty reasoning delta emits nothing; got {evs:?}"
    );
}

#[test]
fn fully_streamed_agent_message_emits_nothing_at_completion() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    note(
        &mut t,
        "item/agentMessage/delta",
        serde_json::json!({"itemId": "i1", "delta": "done", "threadId": "t-1", "turnId": "u"}),
    );
    let evs = note(
        &mut t,
        "item/completed",
        serde_json::json!({"item": {"id": "i1", "type": "agentMessage", "text": "done"}}),
    );
    assert!(evs.is_empty(), "no duplicate text; got {evs:?}");
    assert_eq!(t.turn_text(), "done");
}

#[test]
fn unstreamed_agent_message_emits_full_text_at_completion() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let evs = note(
        &mut t,
        "item/completed",
        serde_json::json!({"item": {"id": "i1", "type": "agentMessage", "text": "whole"}}),
    );
    assert!(matches!(&evs[..], [AgentEvent::Message { text, .. }] if text == "whole"));
}

#[test]
fn command_execution_pairs_tool_use_and_result() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let evs = note(
        &mut t,
        "item/started",
        serde_json::json!({"item": {
            "id": "i2", "type": "commandExecution", "command": "ls -la",
            "commandActions": [], "cwd": "/wt", "status": "inProgress"
        }}),
    );
    assert!(matches!(
        &evs[..],
        [AgentEvent::ToolUse { name, id, input, .. }]
            if name == "command_execution" && id == "i2"
               && input["command"] == "ls -la"
    ));
    let evs = note(
        &mut t,
        "item/completed",
        serde_json::json!({"item": {
            "id": "i2", "type": "commandExecution", "command": "ls -la",
            "commandActions": [], "cwd": "/wt", "status": "completed",
            "aggregatedOutput": "file.txt\n", "exitCode": 0
        }}),
    );
    assert!(matches!(
        &evs[..],
        [AgentEvent::ToolResult { id, status, output, .. }]
            if id == "i2" && status == "success" && output == "file.txt\n"
    ));
}

#[test]
fn a_missing_exit_code_reads_as_unknown_not_success() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    note(
        &mut t,
        "item/started",
        serde_json::json!({"item": {
            "id": "i9", "type": "commandExecution", "command": "sleep 99",
            "commandActions": [], "cwd": "/wt", "status": "inProgress"
        }}),
    );
    let evs = note(
        &mut t,
        "item/completed",
        serde_json::json!({"item": {
            "id": "i9", "type": "commandExecution", "command": "sleep 99",
            "commandActions": [], "cwd": "/wt", "status": "failed",
            "aggregatedOutput": ""
        }}),
    );
    assert!(matches!(
        &evs[..],
        [AgentEvent::ToolResult { id, output, .. }]
            if id == "i9" && output == "exit_code: unknown"
    ));
}

#[test]
fn declined_command_maps_to_error_status() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    note(
        &mut t,
        "item/started",
        serde_json::json!({"item": {
            "id": "i3", "type": "commandExecution", "command": "sudo x",
            "commandActions": [], "cwd": "/wt", "status": "inProgress"
        }}),
    );
    let evs = note(
        &mut t,
        "item/completed",
        serde_json::json!({"item": {
            "id": "i3", "type": "commandExecution", "command": "sudo x",
            "commandActions": [], "cwd": "/wt", "status": "declined"
        }}),
    );
    assert!(matches!(
        &evs[..],
        [AgentEvent::ToolResult { status, .. }] if status == "error"
    ));
}

#[test]
fn approval_gated_command_started_waits_until_acceptance() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    t.note_approval_request(&ApprovalRequest {
        item_id: "i7".into(),
        tool_name: "command_execution".into(),
        input: serde_json::json!({"command": "sudo ls", "cwd": "/wt"}),
    });

    let evs = note(
        &mut t,
        "item/started",
        serde_json::json!({"item": {
            "id": "i7", "type": "commandExecution", "command": "sudo ls",
            "commandActions": [], "cwd": "/wt", "status": "inProgress"
        }}),
    );
    assert!(
        evs.is_empty(),
        "approval-gated command must not render a working step before acceptance"
    );

    let evs = t.note_approval_resolved("i7", true);
    assert!(matches!(
        &evs[..],
        [AgentEvent::ToolUse { name, id, input, .. }]
            if name == "command_execution" && id == "i7" && input["command"] == "sudo ls"
    ));

    let evs = note(
        &mut t,
        "item/completed",
        serde_json::json!({"item": {
            "id": "i7", "type": "commandExecution", "command": "sudo ls",
            "commandActions": [], "cwd": "/wt", "status": "completed",
            "aggregatedOutput": "ok\n", "exitCode": 0
        }}),
    );
    assert!(matches!(
        &evs[..],
        [AgentEvent::ToolResult { id, status, output, .. }]
            if id == "i7" && status == "success" && output == "ok\n"
    ));
}

#[test]
fn approval_gated_command_decline_suppresses_deferred_step_and_result() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    t.note_approval_request(&ApprovalRequest {
        item_id: "i8".into(),
        tool_name: "command_execution".into(),
        input: serde_json::json!({"command": "rm -rf /", "cwd": "/wt"}),
    });

    let evs = note(
        &mut t,
        "item/started",
        serde_json::json!({"item": {
            "id": "i8", "type": "commandExecution", "command": "rm -rf /",
            "commandActions": [], "cwd": "/wt", "status": "inProgress"
        }}),
    );
    assert!(evs.is_empty());
    assert!(t.note_approval_resolved("i8", false).is_empty());

    let evs = note(
        &mut t,
        "item/completed",
        serde_json::json!({"item": {
            "id": "i8", "type": "commandExecution", "command": "rm -rf /",
            "commandActions": [], "cwd": "/wt", "status": "declined"
        }}),
    );
    assert!(
        evs.is_empty(),
        "declined approval should not create a dangling failed tool step"
    );
}

#[test]
fn mcp_tool_call_uses_cc_compatible_name() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let evs = note(
        &mut t,
        "item/started",
        serde_json::json!({"item": {
            "id": "i4", "type": "mcpToolCall", "server": "lucidos",
            "tool": "ask_user_question", "arguments": {"question": "Q?"},
            "status": "inProgress"
        }}),
    );
    assert!(
        matches!(
            &evs[..],
            [AgentEvent::ToolUse { name, .. }]
                if name == crate::runtime::CODEX_ASK_USER_QUESTION_TOOL
        ),
        "the app-server driver must produce the same mcp__<server>__<tool> \
         name the exec driver and the run-loop suppression gate use; got {evs:?}"
    );
}

#[test]
fn turn_completed_closes_open_tools_then_emits_result() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    note(
        &mut t,
        "item/started",
        serde_json::json!({"item": {
            "id": "i5", "type": "commandExecution", "command": "sleep 99",
            "commandActions": [], "cwd": "/wt", "status": "inProgress"
        }}),
    );
    let evs = note(
        &mut t,
        "turn/completed",
        serde_json::json!({"threadId": "t-1", "turn": {"id": "u1", "items": [], "status": "completed"}}),
    );
    assert!(matches!(
        &evs[..],
        [
            AgentEvent::ToolResult { id, status, .. },
            AgentEvent::Result { error: None, .. },
        ] if id == "i5" && status == "error"
    ));
    assert!(t.turn_terminal_seen);
}

#[test]
fn failed_turn_carries_the_error_message() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let evs = note(
        &mut t,
        "turn/completed",
        serde_json::json!({"threadId": "t-1", "turn": {
            "id": "u1", "items": [], "status": "failed",
            "error": {"message": "usage limit exceeded"}
        }}),
    );
    assert!(matches!(
        &evs[..],
        [AgentEvent::Result { error: Some(e), .. }] if e == "usage limit exceeded"
    ));
}

fn policy_blocked_turn() -> serde_json::Value {
    serde_json::json!({"threadId": "t-1", "turn": {
        "id": "u1", "items": [], "status": "failed",
        "error": {
            "message": "This request was blocked by our safety systems. Reason: Potentially unintended activity.",
            "codexErrorInfo": "misalignmentPolicyViolation"
        }
    }})
}

#[test]
fn a_policy_blocked_turn_carries_its_provider_error() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let evs = note(&mut t, "turn/completed", policy_blocked_turn());
    let [AgentEvent::Result {
        error: Some(error),
        provider_error: Some(provider_error),
        ..
    }] = &evs[..]
    else {
        panic!("expected a failed Result with a provider error, got {evs:?}");
    };
    assert!(error.starts_with("This request was blocked"));
    assert!(provider_error.is_policy_block());
}

#[test]
fn the_same_message_without_a_code_is_no_block() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let mut turn = policy_blocked_turn();
    turn["turn"]["error"]
        .as_object_mut()
        .unwrap()
        .remove("codexErrorInfo");
    let evs = note(&mut t, "turn/completed", turn);
    assert!(matches!(
        &evs[..],
        [AgentEvent::Result {
            error: Some(_),
            provider_error: None,
            ..
        }]
    ));
}

#[test]
fn an_error_notification_code_survives_to_a_codeless_failed_turn() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    note(
        &mut t,
        "error",
        serde_json::json!({"threadId": "t-1", "error": {
            "message": "blocked", "codexErrorInfo": "cyberPolicy"
        }}),
    );
    let evs = note(
        &mut t,
        "turn/completed",
        serde_json::json!({"threadId": "t-1", "turn": {
            "id": "u1", "items": [], "status": "failed", "error": {"message": "blocked"}
        }}),
    );
    assert!(matches!(
        &evs[..],
        [AgentEvent::Result { provider_error: Some(p), .. }] if p.is_policy_block()
    ));
}

#[test]
fn an_unknown_code_is_relayed_and_is_no_block() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let mut turn = policy_blocked_turn();
    turn["turn"]["error"]["codexErrorInfo"] = serde_json::json!("brandNewFailure");
    let evs = note(&mut t, "turn/completed", turn);
    assert!(matches!(
        &evs[..],
        [AgentEvent::Result { provider_error: Some(p), .. }]
            if p.code == "brand_new_failure" && !p.is_policy_block()
    ));
}

#[test]
fn requested_interrupt_emits_error_free_result() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    t.note_interrupt_requested();
    let evs = interrupted_turn_end(&mut t);
    assert!(matches!(&evs[..], [AgentEvent::Result { error: None, .. }]));
}

#[test]
fn unrequested_interrupt_fails_the_turn() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let evs = interrupted_turn_end(&mut t);
    assert!(matches!(
        &evs[..],
        [AgentEvent::Result { error: Some(e), .. }] if e == UNREQUESTED_INTERRUPT_ERROR
    ));
}

#[test]
fn an_interrupt_request_does_not_outlive_its_turn() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    t.note_interrupt_requested();
    interrupted_turn_end(&mut t);
    t.begin_turn(None);
    let evs = interrupted_turn_end(&mut t);
    assert!(matches!(
        &evs[..],
        [AgentEvent::Result { error: Some(e), .. }] if e == UNREQUESTED_INTERRUPT_ERROR
    ));
}

fn interrupted_turn_end(t: &mut AppServerTracker) -> Vec<AgentEvent> {
    note(
        t,
        "turn/completed",
        serde_json::json!({"threadId": "t-1", "turn": {"id": "u1", "items": [], "status": "interrupted"}}),
    )
}

fn complete_message(t: &mut AppServerTracker, id: &str, phase: Option<&str>) {
    note(
        t,
        "item/completed",
        serde_json::json!({"item": {"id": id, "type": "agentMessage", "text": id, "phase": phase}}),
    );
}

fn complete_command(t: &mut AppServerTracker, id: &str) {
    note(
        t,
        "item/completed",
        serde_json::json!({"item": {
            "id": id, "type": "commandExecution", "command": "true",
            "commandActions": [], "cwd": "/wt", "status": "completed", "exitCode": 0
        }}),
    );
}

fn turn_end_of(t: &mut AppServerTracker, status: &str) -> TurnEndFacts {
    let evs = note(
        t,
        "turn/completed",
        serde_json::json!({"threadId": "t-1", "turn": {"id": "u1", "items": [], "status": status}}),
    );
    match evs.last() {
        Some(AgentEvent::Result {
            turn_end: Some(facts),
            ..
        }) => facts.clone(),
        other => panic!("expected a Result with turn-end facts, got {other:?}"),
    }
}

#[test]
fn a_turn_end_with_a_final_answer_records_it() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    complete_message(&mut t, "m1", Some("commentary"));
    complete_command(&mut t, "c1");
    complete_message(&mut t, "m2", Some("final_answer"));
    assert_eq!(
        turn_end_of(&mut t, "completed"),
        TurnEndFacts {
            status: TurnEndStatus::Completed,
            final_answer: true,
            agent_messages: 2,
            steered_inputs: 0,
            last_item: Some("agentMessage".to_string()),
        }
    );
}

#[test]
fn a_commentary_only_turn_end_records_no_final_answer() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    complete_message(&mut t, "m1", Some("commentary"));
    complete_command(&mut t, "c1");
    let facts = turn_end_of(&mut t, "completed");
    assert!(!facts.final_answer);
    assert_eq!(facts.agent_messages, 1);
    assert_eq!(facts.last_item.as_deref(), Some("commandExecution"));
}

#[test]
fn a_turn_end_counts_the_steers_codex_accepted() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    t.note_steer_accepted();
    t.note_steer_accepted();
    assert_eq!(turn_end_of(&mut t, "completed").steered_inputs, 2);
    t.begin_turn(None);
    assert_eq!(
        turn_end_of(&mut t, "completed").steered_inputs,
        0,
        "a new turn starts its own count"
    );
}

#[test]
fn a_failed_turn_end_records_its_status() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    assert_eq!(turn_end_of(&mut t, "failed").status, TurnEndStatus::Failed);
}

#[test]
fn an_interrupted_turn_end_records_its_status() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    assert_eq!(
        turn_end_of(&mut t, "interrupted").status,
        TurnEndStatus::Interrupted
    );
}

#[test]
fn a_synthesized_turn_end_keeps_what_the_turn_did() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    complete_message(&mut t, "m1", Some("commentary"));
    let facts = t.turn_end_facts(TurnEndStatus::Synthesized);
    assert_eq!(facts.status, TurnEndStatus::Synthesized);
    assert_eq!(facts.agent_messages, 1);
    assert_eq!(facts.last_item.as_deref(), Some("agentMessage"));
}

#[test]
fn a_sub_agents_items_never_count_as_the_sessions_last_item() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    complete_message(&mut t, "m1", Some("final_answer"));
    note(
        &mut t,
        "item/completed",
        serde_json::json!({"threadId": "sub-1", "item": {
            "id": "c9", "type": "commandExecution", "command": "true",
            "commandActions": [], "cwd": "/wt", "status": "completed", "exitCode": 0
        }}),
    );
    assert_eq!(
        turn_end_of(&mut t, "completed").last_item.as_deref(),
        Some("agentMessage")
    );
}

#[test]
fn turn_started_records_the_interrupt_target() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    note(
        &mut t,
        "turn/started",
        serde_json::json!({"threadId": "t-1", "turn": {"id": "turn-7", "items": [], "status": "inProgress"}}),
    );
    assert_eq!(t.current_turn_id.as_deref(), Some("turn-7"));
}

#[test]
fn token_usage_maps_to_uncached_input_convention() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    t.current_turn_id = Some("u1".into());
    let evs = note(
        &mut t,
        "thread/tokenUsage/updated",
        serde_json::json!({"threadId": "t-1", "turnId": "u1", "tokenUsage": {
            "last": {"inputTokens": 100, "cachedInputTokens": 60, "outputTokens": 5,
                     "reasoningOutputTokens": 0, "totalTokens": 105},
            "total": {"inputTokens": 100, "cachedInputTokens": 60, "outputTokens": 5,
                      "reasoningOutputTokens": 0, "totalTokens": 105}
        }}),
    );
    assert!(matches!(
        &evs[..],
        [AgentEvent::Usage {
            input_tokens: 40,
            cache_read_tokens: 60,
            output_tokens: 5,
            ..
        }]
    ));
}

#[test]
fn error_notification_is_recorded_not_terminal() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let evs = note(
        &mut t,
        "error",
        serde_json::json!({"threadId": "t-1", "turnId": "u", "willRetry": true,
            "error": {"message": "503 retrying"}}),
    );
    assert!(evs.is_empty(), "transient errors must not end the turn");
    assert_eq!(t.last_error.as_deref(), Some("503 retrying"));
}

#[test]
fn file_change_strips_diffs_from_the_persisted_input() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let evs = note(
        &mut t,
        "item/started",
        serde_json::json!({"item": {
            "id": "i6", "type": "fileChange", "status": "inProgress",
            "changes": [{"path": "src/x.rs", "kind": "update", "diff": "---huge---"}]
        }}),
    );
    match &evs[..] {
        [AgentEvent::ToolUse { name, input, .. }] => {
            assert_eq!(name, "file_change");
            assert_eq!(input["changes"][0]["path"], "src/x.rs");
            assert!(
                input["changes"][0].get("diff").is_none(),
                "inline diffs must be dropped — a multi-file patch would balloon the event"
            );
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
}

fn file_change_approval(item_id: &str) -> ApprovalRequest {
    parse_approval_request(
        "item/fileChange/requestApproval",
        &serde_json::json!({
            "threadId": "t-1", "turnId": "u", "itemId": item_id,
            "startedAtMs": 1, "reason": null, "grantRoot": null
        }),
    )
    .expect("file-change approval parses")
}

fn started_file_change(t: &mut AppServerTracker, id: &str, path: &str) {
    note(
        t,
        "item/started",
        serde_json::json!({"item": {
            "id": id, "type": "fileChange", "status": "inProgress",
            "changes": [{"path": path, "kind": {"type": "add"}, "diff": "---huge---"}]
        }}),
    );
}

#[test]
fn file_change_approval_is_given_the_paths_from_its_item() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    started_file_change(&mut t, "i6", "/Users/me/notes.txt");

    let mut approval = file_change_approval("i6");
    assert!(
        approval.input.get("changes").is_none(),
        "the raw approval params carry no paths"
    );
    t.attach_known_file_changes(&mut approval);
    assert_eq!(approval.input["changes"][0]["path"], "/Users/me/notes.txt");
    assert_eq!(approval.input["changes"][0]["kind"]["type"], "add");
    assert!(
        approval.input["changes"][0].get("diff").is_none(),
        "the approval is persisted verbatim, so inline diffs must stay out of it"
    );
}

#[test]
fn file_change_approvals_never_borrow_another_items_paths() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    started_file_change(&mut t, "i1", "/one.txt");
    started_file_change(&mut t, "i2", "/two.txt");

    let mut first = file_change_approval("i1");
    t.attach_known_file_changes(&mut first);
    let mut second = file_change_approval("i2");
    t.attach_known_file_changes(&mut second);

    assert_eq!(first.input["changes"][0]["path"], "/one.txt");
    assert_eq!(first.input["changes"].as_array().unwrap().len(), 1);
    assert_eq!(second.input["changes"][0]["path"], "/two.txt");
}

#[test]
fn an_unknown_item_leaves_the_approval_exactly_as_it_arrived() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let mut approval = file_change_approval("never-announced");
    let before = approval.input.clone();
    t.attach_known_file_changes(&mut approval);
    assert_eq!(approval.input, before);

    let mut cmd = parse_approval_request(
        "item/commandExecution/requestApproval",
        &serde_json::json!({"itemId": "i1", "command": "sudo ls", "cwd": "/wt"}),
    )
    .expect("command approval parses");
    let before = cmd.input.clone();
    t.attach_known_file_changes(&mut cmd);
    assert_eq!(cmd.input, before);
}

#[test]
fn remembered_file_changes_do_not_outlive_their_item_or_their_turn() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    started_file_change(&mut t, "i6", "/Users/me/notes.txt");
    note(
        &mut t,
        "item/completed",
        serde_json::json!({"item": {
            "id": "i6", "type": "fileChange", "status": "completed", "changes": []
        }}),
    );
    let mut approval = file_change_approval("i6");
    t.attach_known_file_changes(&mut approval);
    assert!(
        approval.input.get("changes").is_none(),
        "a completed item's paths are dropped: its approval already resolved"
    );

    started_file_change(&mut t, "i7", "/Users/me/other.txt");
    t.begin_turn(None);
    let mut approval = file_change_approval("i7");
    t.attach_known_file_changes(&mut approval);
    assert!(
        approval.input.get("changes").is_none(),
        "a new turn starts with no remembered items"
    );
}

#[test]
fn token_usage_carries_the_effort_the_turn_was_sent_with() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(Some("low".into()));
    t.current_turn_id = Some("u1".into());
    let evs = note(
        &mut t,
        "thread/tokenUsage/updated",
        serde_json::json!({"threadId": "t-1", "turnId": "u1", "tokenUsage": {
            "last": {"inputTokens": 100, "cachedInputTokens": 60, "outputTokens": 5,
                     "reasoningOutputTokens": 0, "totalTokens": 105}
        }}),
    );

    assert!(
        matches!(
            &evs[..],
            [AgentEvent::Usage { reasoning_effort: Some(effort), .. }] if effort == "low"
        ),
        "{evs:?}"
    );
}

#[test]
fn token_usage_maps_the_cache_write() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    t.current_turn_id = Some("u1".into());
    let evs = note(
        &mut t,
        "thread/tokenUsage/updated",
        serde_json::json!({"threadId": "t-1", "turnId": "u1", "tokenUsage": {
            "last": {"inputTokens": 11974, "cachedInputTokens": 0,
                     "cacheWriteInputTokens": 11971, "outputTokens": 40,
                     "reasoningOutputTokens": 0, "totalTokens": 12014},
            "total": {"inputTokens": 11974, "cachedInputTokens": 0,
                      "cacheWriteInputTokens": 11971, "outputTokens": 40,
                      "reasoningOutputTokens": 0, "totalTokens": 12014}
        }}),
    );
    assert!(
        matches!(
            &evs[..],
            [AgentEvent::Usage {
                input_tokens: 3,
                cache_read_tokens: 0,
                cache_creation_tokens: 11_971,
                cache_creation_1h_tokens: None,
                ..
            }]
        ),
        "{evs:?}"
    );
}

fn foreign_turn_completed() -> serde_json::Value {
    serde_json::json!({"threadId": "t-sub", "turn": {"id": "u9", "items": [], "status": "completed"}})
}

#[test]
fn a_sub_agents_turn_completed_leaves_the_session_turn_running() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    note(
        &mut t,
        "item/started",
        serde_json::json!({"threadId": "t-1", "item": {
            "id": "i7", "type": "commandExecution", "command": "./harden.sh",
            "commandActions": [], "cwd": "/wt", "status": "inProgress"
        }}),
    );
    note(
        &mut t,
        "turn/started",
        serde_json::json!({"threadId": "t-1", "turn": {"id": "turn-1"}}),
    );
    let evs = note(&mut t, "turn/completed", foreign_turn_completed());
    assert!(evs.is_empty(), "a foreign turn end emits nothing: {evs:?}");
    assert!(!t.turn_terminal_seen);
    assert_eq!(t.current_turn_id.as_deref(), Some("turn-1"));
    assert!(!t.owns("turn/completed", &foreign_turn_completed()));
    let evs = note(
        &mut t,
        "item/completed",
        serde_json::json!({"threadId": "t-1", "item": {
            "id": "i7", "type": "commandExecution", "command": "./harden.sh",
            "commandActions": [], "cwd": "/wt", "status": "completed",
            "aggregatedOutput": "ok", "exitCode": 0
        }}),
    );
    assert!(matches!(
        &evs[..],
        [AgentEvent::ToolResult { status, .. }] if status == "success"
    ));
}

#[test]
fn a_sub_agents_items_and_deltas_stay_out_of_the_session_turn() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let evs = note(
        &mut t,
        "item/agentMessage/delta",
        serde_json::json!({"threadId": "t-sub", "itemId": "x", "delta": "hi"}),
    );
    assert!(evs.is_empty());
    let evs = note(
        &mut t,
        "item/completed",
        serde_json::json!({"threadId": "t-sub", "item": {"id": "x", "type": "agentMessage", "text": "hi"}}),
    );
    assert!(evs.is_empty());
    assert_eq!(t.turn_text(), "");
}

#[test]
fn a_sub_agent_thread_started_never_replaces_the_session_id() {
    let sub = serde_json::json!({"thread": {"id": "t-sub", "parentThreadId": "t-1"}});

    let mut t = AppServerTracker::new(Some("t-1".into()));
    assert!(note(&mut t, "thread/started", sub.clone()).is_empty());
    assert_eq!(t.session_id.as_deref(), Some("t-1"));

    let mut t = AppServerTracker::new(None);
    assert!(note(&mut t, "thread/started", sub).is_empty());
    assert_eq!(t.session_id, None);
}

#[test]
fn the_sessions_own_thread_started_is_adopted_on_a_fresh_start() {
    let mut t = AppServerTracker::new(None);
    let evs = note(
        &mut t,
        "thread/started",
        serde_json::json!({"thread": {"id": "t-1", "parentThreadId": null}}),
    );
    assert!(matches!(&evs[..], [AgentEvent::Init { session_id, .. }] if session_id == "t-1"));
    assert_eq!(t.session_id.as_deref(), Some("t-1"));
}

#[test]
fn a_sub_agents_token_usage_is_recorded_under_its_thread_id() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(Some("xhigh".into()));
    t.current_turn_id = Some("u1".into());
    let usage = |thread: &str| {
        serde_json::json!({"threadId": thread, "turnId": "u1", "tokenUsage": {
            "last": {"inputTokens": 100, "cachedInputTokens": 60, "outputTokens": 5,
                     "reasoningOutputTokens": 0, "totalTokens": 105},
            "total": {"inputTokens": 100, "cachedInputTokens": 60, "outputTokens": 5,
                      "reasoningOutputTokens": 0, "totalTokens": 105}
        }})
    };
    let evs = note(&mut t, "thread/tokenUsage/updated", usage("t-sub"));
    assert!(matches!(
        &evs[..],
        [AgentEvent::Usage {
            input_tokens: 40,
            parent_tool_use_id: Some(p),
            reasoning_effort: None,
            ..
        }] if p == "t-sub"
    ));
    let evs = note(&mut t, "thread/tokenUsage/updated", usage("t-1"));
    assert!(matches!(
        &evs[..],
        [AgentEvent::Usage {
            parent_tool_use_id: None,
            reasoning_effort: Some(effort),
            ..
        }] if effort == "xhigh"
    ));
}

#[test]
fn a_sub_agents_file_change_approval_is_given_its_paths() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let item = |completed: bool| {
        serde_json::json!({"threadId": "t-sub", "item": {
            "id": "s1", "type": "fileChange",
            "status": if completed { "completed" } else { "inProgress" },
            "changes": [{"path": "/Users/me/sub.txt", "kind": {"type": "add"}, "diff": "-"}]
        }})
    };
    let evs = note(&mut t, "item/started", item(false));
    assert!(
        evs.is_empty(),
        "a sub-agent's item is no session step: {evs:?}"
    );

    let mut approval = file_change_approval("s1");
    t.attach_known_file_changes(&mut approval);
    assert_eq!(approval.input["changes"][0]["path"], "/Users/me/sub.txt");

    assert!(note(&mut t, "item/completed", item(true)).is_empty());
    let mut late = file_change_approval("s1");
    t.attach_known_file_changes(&mut late);
    assert!(
        late.input.get("changes").is_none(),
        "completion drops the paths"
    );
}

#[test]
fn a_notification_without_a_thread_id_is_the_sessions_own() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn(None);
    let evs = note(
        &mut t,
        "turn/completed",
        serde_json::json!({"turn": {"id": "u1", "items": [], "status": "completed"}}),
    );
    assert!(matches!(&evs[..], [AgentEvent::Result { .. }]));
    assert!(t.turn_terminal_seen);
}

#[test]
fn parse_mcp_server_statuses_uses_structured_auth_and_connection_states() {
    let result = serde_json::json!({"data": [
        {"name": "calendar", "authStatus": "notLoggedIn"},
        {"name": "storage", "connectionStatus": "authenticationRequired", "authStatus": "unknown"},
        {"name": "search", "connectionStatus": "connected", "authStatus": "oAuth"}
    ]});
    let (statuses, cursor) = super::parse_mcp_server_statuses(&result).unwrap();
    assert_eq!(cursor, None);
    assert!(super::parse_mcp_server_statuses(&serde_json::json!({})).is_none());
    assert!(
        super::parse_mcp_server_statuses(&serde_json::json!({"data": [], "nextCursor": 42}))
            .is_none()
    );
    assert!(
        super::parse_mcp_server_statuses(&serde_json::json!({"data": [], "nextCursor": null}))
            .unwrap()
            .0
            .is_empty()
    );
    assert_eq!(statuses.len(), 3);
    assert!(statuses[0].needs_sign_in);
    assert!(statuses[1].needs_sign_in);
    assert!(!statuses[2].needs_sign_in);
}
