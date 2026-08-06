use crate::runtime::agent_runtime::AgentEvent;
use crate::runtime::codex_app_server_parse::*;

fn note(
    tracker: &mut AppServerTracker,
    method: &str,
    params: serde_json::Value,
) -> Vec<AgentEvent> {
    tracker.map_notification(method, &params, 42)
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
    // id + method = server request — the approval shape that MUST be answered.
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
    // The thread/started notification re-announces the same id — no second Init.
    let evs = note(
        &mut t,
        "thread/started",
        serde_json::json!({"thread": {"id": "t-1"}}),
    );
    assert!(evs.is_empty(), "duplicate Init must be suppressed");
    assert_eq!(t.session_id.as_deref(), Some("t-1"));
}

#[test]
fn agent_message_deltas_stream_and_completed_emits_only_remainder() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
    let evs = note(
        &mut t,
        "item/agentMessage/delta",
        serde_json::json!({"itemId": "i1", "delta": "Hel", "threadId": "t", "turnId": "u"}),
    );
    assert!(matches!(&evs[..], [AgentEvent::Message { text, .. }] if text == "Hel"));
    note(
        &mut t,
        "item/agentMessage/delta",
        serde_json::json!({"itemId": "i1", "delta": "lo", "threadId": "t", "turnId": "u"}),
    );
    // Completed item carries the FULL text — only the unstreamed tail may be
    // re-emitted, or the engine's buffer shows the message twice.
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

// The plan tool (codex's TodoWrite analog) arrives as `turn/plan/updated`
// with `{plan: [{step, status}]}` — verified live against codex-cli 0.142.5.
// It maps to the exec protocol's `todo_list` shape (`{items: [{text,
// completed}]}`) as a synthesized ToolUse/ToolResult pair per *distinct*
// list, so plan progress renders on the default protocol too.
#[test]
fn plan_update_emits_normalized_todo_list_pair() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
    let evs = note(
        &mut t,
        "turn/plan/updated",
        serde_json::json!({
            "threadId": "t", "turnId": "u", "explanation": null,
            "plan": [
                {"step": "Map the code", "status": "completed"},
                {"step": "Fix the bug", "status": "inProgress"},
                {"step": "Run tests", "status": "pending"},
            ],
        }),
    );
    match &evs[..] {
        [AgentEvent::ToolUse { name, input, id }, AgentEvent::ToolResult {
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
        "threadId": "t", "turnId": "u",
        "plan": [{"step": "a", "status": "inProgress"}],
    });
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
    assert_eq!(note(&mut t, "turn/plan/updated", plan.clone()).len(), 2);
    // Identical snapshot — nothing new to show.
    assert!(note(&mut t, "turn/plan/updated", plan.clone()).is_empty());
    // The plan persists across turns: a new turn re-announcing the unchanged
    // list must not re-emit a card (dedup state survives begin_turn).
    t.begin_turn();
    assert!(note(&mut t, "turn/plan/updated", plan).is_empty());
    // A changed snapshot emits a fresh pair with a distinct id.
    let evs = note(
        &mut t,
        "turn/plan/updated",
        serde_json::json!({
            "threadId": "t", "turnId": "u",
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
    // An empty or missing plan emits nothing.
    assert!(note(
        &mut t,
        "turn/plan/updated",
        serde_json::json!({"threadId": "t", "turnId": "u", "plan": []}),
    )
    .is_empty());
}

// Reasoning deltas (raw `textDelta` and summary `summaryTextDelta`) surface as
// Thoughts so the timeline can render a live "Thinking" step; an empty delta
// emits nothing.
#[test]
fn reasoning_deltas_emit_thoughts() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
    let evs = note(
        &mut t,
        "item/reasoning/textDelta",
        serde_json::json!({"itemId": "r1", "delta": "Think", "threadId": "t", "turnId": "u", "contentIndex": 0}),
    );
    assert!(matches!(&evs[..], [AgentEvent::Thought { text }] if text == "Think"));
    let evs = note(
        &mut t,
        "item/reasoning/summaryTextDelta",
        serde_json::json!({"itemId": "r1", "delta": "ing", "threadId": "t", "turnId": "u", "summaryIndex": 0}),
    );
    assert!(matches!(&evs[..], [AgentEvent::Thought { text }] if text == "ing"));
    let evs = note(
        &mut t,
        "item/reasoning/textDelta",
        serde_json::json!({"itemId": "r1", "delta": "", "threadId": "t", "turnId": "u", "contentIndex": 1}),
    );
    assert!(
        evs.is_empty(),
        "empty reasoning delta emits nothing; got {evs:?}"
    );
}

#[test]
fn fully_streamed_agent_message_emits_nothing_at_completion() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
    note(
        &mut t,
        "item/agentMessage/delta",
        serde_json::json!({"itemId": "i1", "delta": "done", "threadId": "t", "turnId": "u"}),
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
    // No deltas at all (codex may batch) — the completed item is the only
    // carrier of the text and must emit it whole.
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
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
    t.begin_turn();
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
        [AgentEvent::ToolUse { name, id, input }]
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
        [AgentEvent::ToolResult { id, status, output }]
            if id == "i2" && status == "success" && output == "file.txt\n"
    ));
}

#[test]
fn declined_command_maps_to_error_status() {
    // `declined` = the user denied the approval; the command never ran.
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
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
    t.begin_turn();
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
        [AgentEvent::ToolUse { name, id, input }]
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
        [AgentEvent::ToolResult { id, status, output }]
            if id == "i7" && status == "success" && output == "ok\n"
    ));
}

#[test]
fn approval_gated_command_decline_suppresses_deferred_step_and_result() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
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
    t.begin_turn();
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
    t.begin_turn();
    note(
        &mut t,
        "item/started",
        serde_json::json!({"item": {
            "id": "i5", "type": "commandExecution", "command": "sleep 99",
            "commandActions": [], "cwd": "/wt", "status": "inProgress"
        }}),
    );
    // Abandoned in-flight tool — the closing ToolResult must precede the
    // Result so the engine's paired counter re-arms its watchdog.
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
    t.begin_turn();
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

#[test]
fn interrupted_turn_emits_error_free_result() {
    // The engine's user_hit_stop latch turns this into ResponseCanceled —
    // an error here would mislabel a user cancel as a failure.
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
    let evs = note(
        &mut t,
        "turn/completed",
        serde_json::json!({"threadId": "t-1", "turn": {"id": "u1", "items": [], "status": "interrupted"}}),
    );
    assert!(matches!(&evs[..], [AgentEvent::Result { error: None, .. }]));
}

#[test]
fn turn_started_records_the_interrupt_target() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
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
    // Codex reports TOTAL input with the cached share inside; Usage carries
    // the uncached portion (Anthropic convention the consumer re-totals).
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
    t.begin_turn();
    let evs = note(
        &mut t,
        "error",
        serde_json::json!({"threadId": "t", "turnId": "u", "willRetry": true,
            "error": {"message": "503 retrying"}}),
    );
    assert!(evs.is_empty(), "transient errors must not end the turn");
    assert_eq!(t.last_error.as_deref(), Some("503 retrying"));
}

#[test]
fn file_change_strips_diffs_from_the_persisted_input() {
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
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

/// Build the approval an out-of-sandbox patch actually raises: no paths, and
/// both optional fields null (verified live against codex-cli 0.146.1).
fn file_change_approval(item_id: &str) -> ApprovalRequest {
    parse_approval_request(
        "item/fileChange/requestApproval",
        &serde_json::json!({
            "threadId": "t", "turnId": "u", "itemId": item_id,
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
    // The whole point: the approval params carry no paths, so without this the
    // permission card reads as a bare "file_change" and the user is asked to
    // authorize a write they cannot see.
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
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
    // Two concurrent patches each get their own card (the item id is in the
    // input for exactly that reason); enriching one must not leak the other.
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
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
    // Degrade, never block: a reordered or dropped notification on some future
    // codex must cost the card its detail, not the approval its round-trip.
    let mut t = AppServerTracker::new(Some("t-1".into()));
    t.begin_turn();
    let mut approval = file_change_approval("never-announced");
    let before = approval.input.clone();
    t.attach_known_file_changes(&mut approval);
    assert_eq!(approval.input, before);

    // Same for a command approval, which carries its own detail already.
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
    t.begin_turn();
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
    t.begin_turn();
    let mut approval = file_change_approval("i7");
    t.attach_known_file_changes(&mut approval);
    assert!(
        approval.input.get("changes").is_none(),
        "a new turn starts with no remembered items"
    );
}
