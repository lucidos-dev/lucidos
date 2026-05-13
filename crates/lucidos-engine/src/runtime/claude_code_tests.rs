use super::*;

#[test]
fn parse_system_init() {
    let line = r#"{"type":"system","subtype":"init","session_id":"abc-123","tools":[]}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::Init {
            session_id,
            model,
            slash_commands,
            ..
        } => {
            assert_eq!(session_id, "abc-123");
            assert!(model.is_none());
            assert!(slash_commands.is_empty());
        }
        other => panic!("Expected Init, got {:?}", other),
    }
}

#[test]
fn parse_system_init_with_model() {
    let line = r#"{"type":"system","subtype":"init","session_id":"s-1","model":"claude-opus-4-6","tools":[]}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::Init {
            session_id, model, ..
        } => {
            assert_eq!(session_id, "s-1");
            assert_eq!(model.as_deref(), Some("claude-opus-4-6"));
        }
        other => panic!("Expected Init, got {:?}", other),
    }
}

#[test]
fn parse_system_init_with_slash_commands() {
    let line = r#"{"type":"system","subtype":"init","session_id":"s-1","slash_commands":["compact","clear","help","my-skill"]}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::Init {
            session_id,
            slash_commands,
            ..
        } => {
            assert_eq!(session_id, "s-1");
            assert_eq!(slash_commands, &["compact", "clear", "help", "my-skill"]);
        }
        other => panic!("Expected Init, got {:?}", other),
    }
}

#[test]
fn parse_system_init_with_skills() {
    let line = r#"{"type":"system","subtype":"init","session_id":"s-1","slash_commands":["compact","clear","bugfix","superpowers:brainstorming"],"skills":["bugfix","superpowers:brainstorming"]}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::Init {
            session_id,
            slash_commands,
            skills,
            ..
        } => {
            assert_eq!(session_id, "s-1");
            assert_eq!(
                slash_commands,
                &["compact", "clear", "bugfix", "superpowers:brainstorming"]
            );
            assert_eq!(skills, &["bugfix", "superpowers:brainstorming"]);
        }
        other => panic!("Expected Init, got {:?}", other),
    }
}

#[test]
fn parse_assistant_text() {
    let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello world"}]}}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::Message { role, text } => {
            assert_eq!(role, "assistant");
            assert_eq!(text, "Hello world");
        }
        other => panic!("Expected Message, got {:?}", other),
    }
}

#[test]
fn parse_assistant_tool_use() {
    let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_1","name":"Read","input":{"file_path":"/tmp/test.rs"}}]}}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ToolUse { name, input, id } => {
            assert_eq!(name, "Read");
            assert_eq!(input["file_path"], "/tmp/test.rs");
            assert_eq!(id, "tu_1", "tool_use_id must be extracted so the QuestionCard can match answers to the originating question");
        }
        other => panic!("Expected ToolUse, got {:?}", other),
    }
}

#[test]
fn parse_assistant_tool_use_missing_id() {
    // Defensive: if CC ever emits a tool_use without id, parser must not panic.
    let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Read","input":{}}]}}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ToolUse { id, .. } => {
            assert!(id.is_empty(), "missing id should default to empty string");
        }
        other => panic!("Expected ToolUse, got {:?}", other),
    }
}

#[test]
fn parse_assistant_mixed_content() {
    let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Let me read that."},{"type":"tool_use","id":"tu_1","name":"Read","input":{"file_path":"/tmp/x"}}]}}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 2);
    assert!(matches!(&events[0], AgentEvent::Message { .. }));
    assert!(matches!(&events[1], AgentEvent::ToolUse { .. }));
}

#[test]
fn parse_assistant_extracts_usage() {
    // CC mirrors Anthropic's usage block on each assistant frame; the
    // parser must surface it as a separate `Usage` event so the
    // consumer can emit a real `ContextCaptured` with producer=CC.
    let line = r#"{"type":"assistant","message":{"role":"assistant","model":"claude-opus-4-7","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":1234,"output_tokens":56,"cache_read_input_tokens":900,"cache_creation_input_tokens":100}}}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 2, "expected one Message and one Usage");
    assert!(matches!(&events[0], AgentEvent::Message { .. }));
    match &events[1] {
        AgentEvent::Usage {
            model,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
        } => {
            assert_eq!(model.as_deref(), Some("claude-opus-4-7"));
            assert_eq!(*input_tokens, 1234);
            assert_eq!(*output_tokens, 56);
            assert_eq!(*cache_read_tokens, 900);
            assert_eq!(*cache_creation_tokens, 100);
        }
        other => panic!("Expected Usage, got {:?}", other),
    }
}

#[test]
fn parse_assistant_usage_skipped_when_all_zero() {
    // Continuation frames with zeroed usage shouldn't emit a misleading
    // snapshot — no real API call happened.
    let line = r#"{"type":"assistant","message":{"role":"assistant","model":"claude-opus-4-7","content":[{"type":"text","text":"continuing"}],"usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1, "all-zero usage frames must produce only the Message");
    assert!(matches!(&events[0], AgentEvent::Message { .. }));
}

#[test]
fn parse_assistant_no_usage_block() {
    // Defensive: assistant frames without a usage block (e.g. older CC
    // format) must still parse text/tool_use without panicking.
    let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], AgentEvent::Message { .. }));
}

#[test]
fn parse_legacy_tool_result() {
    let line = r#"{"type":"tool_result","content":"file contents here","is_error":false,"tool_use_id":"toolu_legacy"}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ToolResult { output, status, id } => {
            assert_eq!(output, "file contents here");
            assert_eq!(status, "success");
            assert_eq!(id, "toolu_legacy");
        }
        other => panic!("Expected ToolResult, got {:?}", other),
    }
}

#[test]
fn parse_legacy_tool_result_error() {
    let line = r#"{"type":"tool_result","content":"not found","is_error":true}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ToolResult { output, status, id } => {
            assert_eq!(output, "not found");
            assert_eq!(status, "error");
            // Missing tool_use_id in payload → empty (legacy frame).
            assert!(id.is_empty());
        }
        other => panic!("Expected ToolResult, got {:?}", other),
    }
}

// CC 2.1.76+ format: tool results come as "type": "user" messages
#[test]
fn parse_user_tool_result() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":"result text","is_error":false}]}}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ToolResult { output, status, id } => {
            assert_eq!(output, "result text");
            assert_eq!(status, "success");
            assert_eq!(id, "tu_1");
        }
        other => panic!("Expected ToolResult, got {:?}", other),
    }
}

#[test]
fn parse_user_tool_result_error() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":"permission denied","is_error":true}]}}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ToolResult { output, status, id } => {
            assert_eq!(output, "permission denied");
            assert_eq!(status, "error");
            assert_eq!(id, "tu_1");
        }
        other => panic!("Expected ToolResult, got {:?}", other),
    }
}

#[test]
fn parse_user_multiple_tool_results() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":"ok","is_error":false},{"type":"tool_result","tool_use_id":"tu_2","content":"also ok","is_error":false}]}}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 2);
    assert!(matches!(&events[0], AgentEvent::ToolResult { .. }));
    assert!(matches!(&events[1], AgentEvent::ToolResult { .. }));
}

#[test]
fn parse_user_non_tool_result_ignored() {
    // A user message with text content (not tool_result) should produce no events
    let line =
        r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 0);
}

// control_response from CC after a control_request (e.g. interrupt) — silently ignored
#[test]
fn parse_control_response_ignored() {
    let line =
        r#"{"type":"control_response","request_id":"abc-123","response":{"subtype":"success"}}"#;
    let events = parse_line(line);
    assert!(events.is_empty());
}

// CC 2.1.76+ sends streaming deltas as "stream_event" — silently ignored
#[test]
fn parse_stream_event_ignored() {
    let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 0);
}

#[test]
fn parse_result() {
    let line = r#"{"type":"result","result":"Done.","duration_ms":1234}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::Result {
            text,
            duration_ms,
            error,
        } => {
            assert_eq!(text, "Done.");
            assert_eq!(*duration_ms, 1234);
            assert!(error.is_none(), "success result has no error field");
        }
        other => panic!("Expected Result, got {:?}", other),
    }
}

/// CC mid-stream API failure (network drop, upstream 5xx) terminates the turn
/// with `subtype: "error_during_execution"`. The previous parser dropped the
/// signal, leaving `ResponseGenerated` to render the partial response as a
/// complete answer.
#[test]
fn parse_result_error_during_execution_carries_error() {
    let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"","duration_ms":2300,"errors":["Stream interrupted: connection reset"]}"#;
    let events = parse_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::Result {
            text,
            duration_ms,
            error,
        } => {
            assert_eq!(text, "");
            assert_eq!(*duration_ms, 2300);
            assert_eq!(
                error.as_deref(),
                Some("Stream interrupted: connection reset")
            );
        }
        other => panic!("Expected Result, got {:?}", other),
    }
}

/// `error_max_turns` and other non-success subtypes also carry `is_error: true`
/// but may omit `errors`. Use the subtype as the fallback error message so the
/// `ResponseFailed` event has something user-readable instead of an empty string.
#[test]
fn parse_result_error_max_turns_falls_back_to_subtype() {
    let line = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":"","duration_ms":42000}"#;
    let events = parse_line(line);
    match &events[0] {
        AgentEvent::Result { error, .. } => {
            assert_eq!(
                error.as_deref(),
                Some("error_max_turns"),
                "no errors[] present — subtype is the user-facing reason"
            );
        }
        other => panic!("Expected Result, got {:?}", other),
    }
}

/// Multiple errors join with `; ` so a single `ResponseFailed.error` string
/// captures every line CC reported, not just the first.
#[test]
fn parse_result_joins_multiple_errors() {
    let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"","duration_ms":100,"errors":["upstream 503","retry exhausted"]}"#;
    let events = parse_line(line);
    match &events[0] {
        AgentEvent::Result { error, .. } => {
            assert_eq!(error.as_deref(), Some("upstream 503; retry exhausted"));
        }
        other => panic!("Expected Result, got {:?}", other),
    }
}

/// `is_error: false` (or absent) with a `success` subtype: the legacy success
/// path. `error` MUST stay `None` so `classify_result` doesn't flip the turn
/// to `Failed`.
#[test]
fn parse_result_success_with_subtype_has_no_error() {
    let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"All good.","duration_ms":1500}"#;
    let events = parse_line(line);
    match &events[0] {
        AgentEvent::Result { text, error, .. } => {
            assert_eq!(text, "All good.");
            assert!(error.is_none());
        }
        other => panic!("Expected Result, got {:?}", other),
    }
}

/// CC sometimes emits `is_error: true` with `subtype: "success"` and no
/// `errors[]` (observed when an upstream API drop happens after CC has
/// streamed an "API Error: Unable to connect" message and decided the
/// conversation succeeded structurally). The previous parser fell back to the
/// `subtype` string, surfacing the literal text "success" as the error in
/// `ResponseFailed.error` — which renders to the user as
/// `[ERROR] **Error:** success`. Filter "success" out of the subtype fallback
/// so the user sees something honest instead.
#[test]
fn parse_result_is_error_with_success_subtype_does_not_surface_success() {
    let line = r#"{"type":"result","subtype":"success","is_error":true,"result":"","duration_ms":100}"#;
    let events = parse_line(line);
    match &events[0] {
        AgentEvent::Result { error, .. } => {
            let err = error
                .as_deref()
                .expect("is_error: true must produce Some(error)");
            assert_ne!(
                err, "success",
                "`success` is the no-error sentinel — surfacing it as the failure \
                 reason renders to the user as `[ERROR] **Error:** success`"
            );
        }
        other => panic!("Expected Result, got {:?}", other),
    }
}

#[test]
fn parse_empty_line() {
    assert!(parse_line("").is_empty());
    assert!(parse_line("   ").is_empty());
    assert!(parse_line("\n").is_empty());
}

#[test]
fn parse_invalid_json() {
    assert!(parse_line("not json at all").is_empty());
}

#[test]
fn parse_unknown_type_logged() {
    let line = r#"{"type":"something_new","data":"test"}"#;
    let events = parse_line(line);
    assert!(events.is_empty());
}

/// Full CC 2.1.76 sequence: system → assistant(tool_use) → user(tool_result) → result
#[test]
fn parse_full_cc_session() {
    let lines = [
        r#"{"type":"system","subtype":"init","session_id":"sess-1","tools":[]}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_start"}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Reading file."},{"type":"tool_use","id":"tu_1","name":"Read","input":{"file_path":"/tmp/x"}}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":"file contents","is_error":false}]}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta"}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Here are the contents."}]}}"#,
        r#"{"type":"result","result":"Here are the contents.","duration_ms":5000}"#,
    ];

    let all_events: Vec<AgentEvent> = lines.iter().flat_map(|l| parse_line(l)).collect();

    // system → Init
    assert!(
        matches!(&all_events[0], AgentEvent::Init { session_id, .. } if session_id == "sess-1")
    );
    // assistant text + tool_use
    assert!(matches!(&all_events[1], AgentEvent::Message { text, .. } if text == "Reading file."));
    assert!(matches!(&all_events[2], AgentEvent::ToolUse { name, .. } if name == "Read"));
    // user tool_result
    assert!(
        matches!(&all_events[3], AgentEvent::ToolResult { output, status, .. } if output == "file contents" && status == "success")
    );
    // second assistant text
    assert!(
        matches!(&all_events[4], AgentEvent::Message { text, .. } if text == "Here are the contents.")
    );
    // result
    assert!(matches!(
        &all_events[5],
        AgentEvent::Result {
            duration_ms: 5000,
            error: None,
            ..
        }
    ));
    // stream_events were silently ignored
    assert_eq!(all_events.len(), 6);
}

#[test]
fn cc_control_request_interrupt_serializes() {
    let json = cc_control_request_to_json(&ControlRequest::Interrupt, "test-id-123");
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "control_request");
    assert_eq!(parsed["request_id"], "test-id-123");
    assert_eq!(parsed["request"]["subtype"], "interrupt");
}

#[test]
fn cc_control_request_set_model_serializes() {
    let json = cc_control_request_to_json(
        &ControlRequest::SetModel {
            model: "claude-sonnet-4-6".to_string(),
        },
        "test-id-456",
    );
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "control_request");
    assert_eq!(parsed["request"]["subtype"], "set_model");
    assert_eq!(parsed["request"]["model"], "claude-sonnet-4-6");
}

#[test]
fn cc_control_request_set_permission_mode_serializes() {
    let json = cc_control_request_to_json(
        &ControlRequest::SetPermissionMode {
            mode: "plan".to_string(),
        },
        "test-id-789",
    );
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["request"]["subtype"], "set_permission_mode");
    assert_eq!(parsed["request"]["mode"], "plan");
}

fn assert_command_options(
    defs: &serde_json::Value,
    subtype: &str,
    key: &str,
    expected_values: &[&str],
) {
    let cmd = defs
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["subtype"] == subtype)
        .unwrap_or_else(|| panic!("{} command should exist", subtype));
    let param = &cmd["params"][0];
    assert_eq!(param["key"], key);
    let options = param["options"]
        .as_array()
        .unwrap_or_else(|| panic!("{} param should have options", key));
    assert!(
        options.len() >= expected_values.len(),
        "{}: expected at least {} options, got {}",
        subtype,
        expected_values.len(),
        options.len()
    );
    for opt in options {
        assert!(opt["value"].is_string(), "option missing value");
        assert!(opt["label"].is_string(), "option missing label");
        assert!(opt["description"].is_string(), "option missing description");
    }
    let values: Vec<&str> = options
        .iter()
        .map(|o| o["value"].as_str().unwrap())
        .collect();
    for ev in expected_values {
        assert!(values.contains(ev), "{}: missing {} option", subtype, ev);
    }
}

#[test]
fn command_definitions_include_model_options() {
    let defs = cc_command_definitions();
    assert_command_options(
        &defs,
        "set_model",
        "model",
        &["default", "sonnet", "opus", "haiku"],
    );
}

#[test]
fn command_definitions_include_reasoning_effort_options() {
    let defs = cc_command_definitions();
    assert_command_options(
        &defs,
        "set_reasoning_effort",
        "effort",
        &["low", "medium", "high", "xhigh", "max"],
    );
}

#[test]
fn parse_line_ignores_hook_system_events() {
    // Hook events have type "system" + session_id but are NOT init events.
    // They must NOT produce Init events (which would overwrite commands with empty arrays).
    let hook_started =
        r#"{"type":"system","subtype":"hook_started","hook_id":"abc","session_id":"s1"}"#;
    let hook_response = r#"{"type":"system","subtype":"hook_response","hook_id":"abc","session_id":"s1","output":"{}"}"#;
    let hook_progress =
        r#"{"type":"system","subtype":"hook_progress","hook_id":"abc","session_id":"s1"}"#;

    assert!(
        super::parse_line(hook_started).is_empty(),
        "hook_started should not produce Init"
    );
    assert!(
        super::parse_line(hook_response).is_empty(),
        "hook_response should not produce Init"
    );
    assert!(
        super::parse_line(hook_progress).is_empty(),
        "hook_progress should not produce Init"
    );
}

#[test]
fn parse_line_extracts_init_from_system_init_event() {
    let init = r#"{"type":"system","subtype":"init","session_id":"s1","model":"opus","slash_commands":["compact","commit","review"],"skills":["commit","review"]}"#;
    let events = super::parse_line(init);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::Init {
            session_id,
            model,
            slash_commands,
            skills,
        } => {
            assert_eq!(session_id, "s1");
            assert_eq!(model.as_deref(), Some("opus"));
            // builtin = slash_commands - skills = ["compact"]
            assert_eq!(
                slash_commands,
                &vec![
                    "compact".to_string(),
                    "commit".to_string(),
                    "review".to_string()
                ]
            );
            assert_eq!(skills, &vec!["commit".to_string(), "review".to_string()]);
        }
        other => panic!("Expected Init, got {:?}", other),
    }
}

#[test]
fn control_request_deserializes_all_variants() {
    let cases = vec![
        (r#"{"subtype":"interrupt"}"#, "interrupt"),
        (
            r#"{"subtype":"set_model","model":"claude-sonnet-4-6"}"#,
            "set_model",
        ),
        (
            r#"{"subtype":"set_permission_mode","mode":"plan"}"#,
            "set_permission_mode",
        ),
        (
            r#"{"subtype":"set_reasoning_effort","effort":"high"}"#,
            "set_reasoning_effort",
        ),
    ];
    for (json, expected_subtype) in cases {
        let req: ControlRequest = serde_json::from_str(json).unwrap();
        let serialized = cc_control_request_to_json(&req, "test-id");
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(
            parsed["request"]["subtype"], expected_subtype,
            "Failed for: {}",
            json
        );
    }
}

#[test]
fn read_cc_default_effort_reads_settings() {
    let result = read_cc_default_effort();
    if let Some(ref v) = result {
        assert!(is_valid_effort(v), "Unexpected effort level: {}", v);
    }
}

#[test]
fn normalize_cc_model_id_maps_aliases() {
    assert_eq!(normalize_cc_model_id("sonnet"), "sonnet");
    assert_eq!(normalize_cc_model_id("opus"), "opus");
    assert_eq!(normalize_cc_model_id("haiku"), "haiku");
    assert_eq!(normalize_cc_model_id("claude-opus-4-7"), "claude-opus-4-7");
    assert_eq!(normalize_cc_model_id("claude-opus-4-1"), "claude-opus-4-1");
}

#[test]
fn normalize_cc_model_id_maps_full_ids() {
    assert_eq!(normalize_cc_model_id("claude-sonnet-4-6"), "sonnet");
    assert_eq!(normalize_cc_model_id("claude-sonnet-4-20250514"), "sonnet");
    assert_eq!(normalize_cc_model_id("claude-opus-4-6"), "opus");
    assert_eq!(normalize_cc_model_id("claude-haiku-4-5-20251001"), "haiku");
    assert_eq!(normalize_cc_model_id("claude-haiku-4-5@20251001"), "haiku");
    assert_eq!(normalize_cc_model_id("claude-haiku-4-5"), "haiku");
}

#[test]
fn normalize_cc_model_id_preserves_unknown() {
    assert_eq!(normalize_cc_model_id("gpt-4o"), "gpt-4o");
    assert_eq!(normalize_cc_model_id("custom-model"), "custom-model");
}

#[test]
fn reconcile_cc_model_preserves_1m_suffix_when_cc_strips_it() {
    // CC strips the [1m] suffix when echoing the model in stream-json
    // (both Init and per-message Usage frames). The engine pinned the
    // 1M-context variant when invoking CC, so the reconciled name must
    // keep the [1m] marker — context_window_for needs it to return 1M.
    assert_eq!(
        reconcile_cc_model(Some("claude-opus-4-7[1m]"), "claude-opus-4-7"),
        "claude-opus-4-7[1m]"
    );
    assert_eq!(
        reconcile_cc_model(Some("opus[1m]"), "claude-opus-4-6"),
        "opus[1m]"
    );
    assert_eq!(
        reconcile_cc_model(Some("sonnet[1m]"), "claude-sonnet-4-6"),
        "sonnet[1m]"
    );
}

#[test]
fn reconcile_cc_model_drops_1m_when_user_switched_models() {
    // /model in CC can swap the active model mid-session. If the new model
    // doesn't share a base with the original [1m] alias, don't fabricate
    // a [1m] suffix on it.
    assert_eq!(
        reconcile_cc_model(Some("claude-opus-4-7[1m]"), "claude-sonnet-4-6"),
        "sonnet"
    );
    assert_eq!(
        reconcile_cc_model(Some("opus[1m]"), "claude-haiku-4-5"),
        "haiku"
    );
}

#[test]
fn reconcile_cc_model_passes_through_when_no_1m() {
    // No suffix on the original alias → behave exactly like normalize.
    assert_eq!(
        reconcile_cc_model(Some("claude-opus-4-7"), "claude-opus-4-7"),
        "claude-opus-4-7"
    );
    assert_eq!(
        reconcile_cc_model(Some("sonnet"), "claude-sonnet-4-6"),
        "sonnet"
    );
    assert_eq!(reconcile_cc_model(None, "claude-opus-4-7"), "claude-opus-4-7");
}

fn collect_envs(
    cmd: &tokio::process::Command,
) -> std::collections::HashMap<std::ffi::OsString, std::ffi::OsString> {
    cmd.as_std()
        .get_envs()
        .filter_map(|(k, v)| v.map(|v| (k.to_owned(), v.to_owned())))
        .collect()
}

fn test_spawn_args<'a>(
    worktree: &'a Path,
    workspace: &'a Path,
    thread_id: uuid::Uuid,
) -> SpawnArgs<'a> {
    SpawnArgs {
        worktree_path: worktree,
        workspace_path: workspace,
        allowed_tools: None,
        system_prompt: None,
        resume_session_id: None,
        model: None,
        reasoning_effort: None,
        thread_id,
        spawning_event_id: None,
        repo_name: None,
        interactive: false,
    }
}

fn test_spawn_args_with_event<'a>(
    worktree: &'a Path,
    workspace: &'a Path,
    thread_id: uuid::Uuid,
    spawning_event_id: Option<uuid::Uuid>,
) -> SpawnArgs<'a> {
    SpawnArgs {
        worktree_path: worktree,
        workspace_path: workspace,
        allowed_tools: None,
        system_prompt: None,
        resume_session_id: None,
        model: None,
        reasoning_effort: None,
        thread_id,
        spawning_event_id,
        repo_name: None,
        interactive: false,
    }
}

fn test_spawn_args_with_repo<'a>(
    worktree: &'a Path,
    workspace: &'a Path,
    thread_id: uuid::Uuid,
    repo_name: Option<&'a str>,
) -> SpawnArgs<'a> {
    SpawnArgs {
        worktree_path: worktree,
        workspace_path: workspace,
        allowed_tools: None,
        system_prompt: None,
        resume_session_id: None,
        model: None,
        reasoning_effort: None,
        thread_id,
        spawning_event_id: None,
        repo_name,
        interactive: false,
    }
}

#[test]
fn build_command_sets_lucidos_thread_id_env() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = build_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    let value = env
        .get(std::ffi::OsStr::new("LUCIDOS_THREAD_ID"))
        .expect("LUCIDOS_THREAD_ID env var must be set on the spawned subprocess");
    assert_eq!(value, std::ffi::OsStr::new(&thread_id.to_string()));
}

#[test]
fn build_command_sets_lucidos_workspace_env() {
    let thread_id = uuid::Uuid::new_v4();
    let workspace = std::path::Path::new("/some/workspace");
    let worktree = std::path::Path::new("/some/workspace/.lucidos/worktrees/abc");
    let cmd = build_command(&test_spawn_args(worktree, workspace, thread_id), None);
    let env = collect_envs(&cmd);
    assert_eq!(
        env.get(std::ffi::OsStr::new("LUCIDOS_WORKSPACE"))
            .map(|v| v.as_os_str()),
        Some(workspace.as_os_str())
    );
}

#[test]
fn build_command_sets_lucidos_event_id_when_spawning_event_id_set() {
    // The CC subprocess needs `LUCIDOS_EVENT_ID` so the `lucidos spawn-thread`
    // CLI can default `--caller-event-id` for cross-workspace POSTs without
    // the user having to thread the value through every invocation.
    let thread_id = uuid::Uuid::new_v4();
    let event_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = build_command(
        &test_spawn_args_with_event(p, p, thread_id, Some(event_id)),
        None,
    );
    let env = collect_envs(&cmd);
    let value = env
        .get(std::ffi::OsStr::new("LUCIDOS_EVENT_ID"))
        .expect("LUCIDOS_EVENT_ID must be set when spawning_event_id is provided");
    assert_eq!(value, std::ffi::OsStr::new(&event_id.to_string()));
}

#[test]
fn build_command_omits_lucidos_event_id_when_spawning_event_id_none() {
    // Recovery, hardening, and other engine-internal spawns have no parent
    // event — the env var must be unset so the CLI falls back to omitting
    // `caller_event_id` rather than stamping a stale or fabricated id.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = build_command(&test_spawn_args_with_event(p, p, thread_id, None), None);
    let env = collect_envs(&cmd);
    assert!(
        env.get(std::ffi::OsStr::new("LUCIDOS_EVENT_ID")).is_none(),
        "LUCIDOS_EVENT_ID must be unset when no spawning_event_id"
    );
}

#[test]
fn build_command_sets_lucidos_session_kind_when_interactive() {
    // Interactive sessions (chat / recovery / external-repo) must set
    // LUCIDOS_SESSION_KIND=interactive so the cc-stop-reminder hook knows
    // it can safely block CC with an AskUserQuestion redirect when CC ends
    // a turn with a plaintext question.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let mut args = test_spawn_args(p, p, thread_id);
    args.interactive = true;
    let cmd = build_command(&args, None);
    let env = collect_envs(&cmd);
    assert_eq!(
        env.get(std::ffi::OsStr::new("LUCIDOS_SESSION_KIND"))
            .map(|v| v.as_os_str()),
        Some(std::ffi::OsStr::new("interactive")),
    );
}

#[test]
fn build_command_omits_lucidos_session_kind_when_not_interactive() {
    // Conflict-resolution sessions are unattended — they would hang on a
    // question redirect waiting for an answer that's not coming. The
    // cc-stop-reminder hook treats absence of LUCIDOS_SESSION_KIND as
    // "unattended, skip the question redirect".
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let args = test_spawn_args(p, p, thread_id); // interactive=false by default
    let cmd = build_command(&args, None);
    let env = collect_envs(&cmd);
    assert!(
        env.get(std::ffi::OsStr::new("LUCIDOS_SESSION_KIND"))
            .is_none(),
    );
}

#[test]
fn build_command_sets_lucidos_repo_when_repo_name_set() {
    // The CC subprocess needs `LUCIDOS_REPO` so the `lucidos spawn-thread`
    // CLI defaults `--repo` to the calling thread's repo, keeping CC
    // sidequests in the same repo as their caller in workspaces hosting
    // worktrees from multiple repos.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = build_command(
        &test_spawn_args_with_repo(p, p, thread_id, Some("user-acquisition")),
        None,
    );
    let env = collect_envs(&cmd);
    let value = env
        .get(std::ffi::OsStr::new("LUCIDOS_REPO"))
        .expect("LUCIDOS_REPO must be set when repo_name is provided");
    assert_eq!(value, std::ffi::OsStr::new("user-acquisition"));
}

#[test]
fn build_command_omits_lucidos_repo_when_repo_name_none() {
    // Engine-internal spawns that don't know their repo (very early startup)
    // must leave the env var unset so the CLI falls back to the workspace
    // default repo rather than stamping a stale or fabricated name.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = build_command(&test_spawn_args_with_repo(p, p, thread_id, None), None);
    let env = collect_envs(&cmd);
    assert!(
        env.get(std::ffi::OsStr::new("LUCIDOS_REPO")).is_none(),
        "LUCIDOS_REPO must be unset when no repo_name"
    );
}

#[test]
fn build_command_prepends_cli_dir_to_path() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cli_dir = std::path::Path::new("/opt/lucidos/bin");
    let cmd = build_command(&test_spawn_args(p, p, thread_id), Some(cli_dir));
    let env = collect_envs(&cmd);
    let path = env
        .get(std::ffi::OsStr::new("PATH"))
        .expect("PATH should be set");
    let path_str = path.to_string_lossy();
    assert!(
        path_str.starts_with(cli_dir.to_string_lossy().as_ref()),
        "PATH {:?} should start with lucidos cli dir {:?}",
        path_str,
        cli_dir
    );
}

fn collect_args(cmd: &tokio::process::Command) -> Vec<String> {
    cmd.as_std()
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn build_command_uses_permission_prompt_tool_not_skip_permissions() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = build_command(&test_spawn_args(p, p, thread_id), None);
    let args = collect_args(&cmd);

    assert!(
        !args.iter().any(|a| a == "--dangerously-skip-permissions"),
        "must not pass --dangerously-skip-permissions (would short-circuit the prompt tool)"
    );
    assert!(
        args.iter().any(|a| a == "--permission-prompt-tool"),
        "--permission-prompt-tool must be set"
    );
    let prompt_tool_idx = args
        .iter()
        .position(|a| a == "--permission-prompt-tool")
        .unwrap();
    assert_eq!(args[prompt_tool_idx + 1], "mcp__lucidos_perm__approve");

    let mcp_config_idx = args
        .iter()
        .position(|a| a == "--mcp-config")
        .expect("--mcp-config must be present");
    let cfg: serde_json::Value = serde_json::from_str(&args[mcp_config_idx + 1])
        .expect("--mcp-config value must be valid JSON");
    assert_eq!(cfg["mcpServers"]["lucidos_perm"]["command"], "lucidos");
    assert_eq!(
        cfg["mcpServers"]["lucidos_perm"]["args"][0],
        "mcp-permission-server"
    );

    assert!(
        args.iter().any(|a| a == "--strict-mcp-config"),
        "--strict-mcp-config keeps the permission server isolated from the user's global MCP config"
    );
}

#[test]
fn build_command_passes_settings_flag_with_workspace_path() {
    let thread_id = uuid::Uuid::new_v4();
    let workspace = std::path::Path::new("/some/workspace");
    let worktree = std::path::Path::new("/some/workspace/.lucidos/worktrees/abc");
    let cmd = build_command(&test_spawn_args(worktree, workspace, thread_id), None);
    let args = collect_args(&cmd);

    let settings_idx = args
        .iter()
        .position(|a| a == "--settings")
        .expect("--settings must be present");
    let settings_path = args[settings_idx + 1].as_str();
    assert_eq!(
        settings_path,
        std::path::Path::new("/some/workspace/.lucidos/cc-settings.json")
            .to_string_lossy()
            .as_ref(),
        "--settings path must point at workspace .lucidos/cc-settings.json"
    );
}

#[test]
fn build_command_sets_permission_mode_accept_edits() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = build_command(&test_spawn_args(p, p, thread_id), None);
    let args = collect_args(&cmd);

    let mode_idx = args
        .iter()
        .position(|a| a == "--permission-mode")
        .expect("--permission-mode must be set");
    assert_eq!(
        args[mode_idx + 1],
        "acceptEdits",
        "acceptEdits auto-approves in-cwd writes; only out-of-cwd / Bash routes through the prompt tool"
    );
}

#[test]
fn build_command_sets_mcp_tool_timeout_to_effective_infinity() {
    // The engine permission handler waits indefinitely (matching
    // AskUserQuestion). CC's MCP client must not time out either — a deny on
    // timeout would push the model into a retry that surfaces another card.
    // Verify the env var is set to "effectively never" (≥ 1 hour) so any
    // realistic user delay is covered.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = build_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    let raw = env
        .get(std::ffi::OsStr::new("MCP_TOOL_TIMEOUT"))
        .expect("MCP_TOOL_TIMEOUT must be set so CC's MCP client doesn't retry the prompt");
    let ms: u64 = raw
        .to_string_lossy()
        .parse()
        .expect("MCP_TOOL_TIMEOUT must be a number of milliseconds");
    let one_hour_ms: u64 = 3_600 * 1_000;
    assert!(
        ms >= one_hour_ms,
        "MCP_TOOL_TIMEOUT must be ≥ 1 hour for indefinite-wait behavior; got {ms}ms"
    );
}

#[test]
fn build_command_sets_mcp_timeout_to_effective_infinity() {
    // CC's MCP client also has a per-request timeout (`MCP_TIMEOUT`, default
    // 30s) that fires *before* MCP_TOOL_TIMEOUT for the permission_prompt
    // RPC. Without overriding it, every permission request is canceled after
    // 30s, the engine sees the receiver dropped (gc'd), CC's model retries
    // the original tool — and the user sees an apparent loop of identical
    // permission cards every ~30s. Set it to "effectively never" so the only
    // bound is the user's patience.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = build_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    let raw = env
        .get(std::ffi::OsStr::new("MCP_TIMEOUT"))
        .expect("MCP_TIMEOUT must be set so CC's MCP client doesn't cancel the permission RPC at 30s");
    let ms: u64 = raw
        .to_string_lossy()
        .parse()
        .expect("MCP_TIMEOUT must be a number of milliseconds");
    let one_hour_ms: u64 = 3_600 * 1_000;
    assert!(
        ms >= one_hour_ms,
        "MCP_TIMEOUT must be ≥ 1 hour for indefinite-wait behavior; got {ms}ms"
    );
}

#[test]
fn build_command_skips_path_injection_when_no_cli_dir() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = build_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    assert!(
        env.get(std::ffi::OsStr::new("PATH")).is_none(),
        "PATH must not be set when lucidos CLI binary is missing — \
         otherwise we'd shadow the inherited PATH with an empty value"
    );
}

#[test]
fn format_user_input_text_only() {
    let input = AgentInput {
        text: "hello".into(),
        images: vec![],
    };
    let line = format_user_input(&input, Some("sess-1"));
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(parsed["type"], "user");
    assert_eq!(parsed["message"]["role"], "user");
    assert_eq!(parsed["message"]["content"], "hello");
    assert_eq!(parsed["session_id"], "sess-1");
}

#[test]
fn format_user_input_with_images_uses_blocks() {
    let input = AgentInput {
        text: "describe".into(),
        images: vec![crate::api::ChatImage {
            base64: "deadbeef".into(),
            mime_type: "image/png".into(),
        }],
    };
    let line = format_user_input(&input, None);
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    let content = parsed["message"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "describe");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["source"]["data"], "deadbeef");
    assert_eq!(parsed["session_id"], "default");
}

/// Spawn an arbitrary subprocess and wire it to `driver_task` so we can
/// integration-test the channel plumbing without requiring the `claude`
/// CLI to be installed.
async fn spawn_driver_for_test(program: &str, args: &[&str]) -> (RunningAgent, CancellationToken) {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn test child");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        child,
        stdin,
        BufReader::new(stdout),
        BufReader::new(stderr),
        events_tx,
        input_rx,
        control_rx,
        cancel.clone(),
        None,
    ));
    (
        RunningAgent {
            kind: AgentKind::ClaudeCode,
            events_rx,
            input_tx,
            control_tx,
        },
        cancel,
    )
}

#[tokio::test]
async fn driver_task_parses_stdout_into_typed_events() {
    // Subprocess prints two CC-format lines then exits. The driver must
    // forward both as typed AgentEvents and finish with Exited.
    let cmd = format!(
        "printf '{}\\n{}\\n'",
        r#"{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-1\"}"#,
        r#"{\"type\":\"result\",\"result\":\"done\",\"duration_ms\":42}"#,
    );
    let (mut agent, _cancel) = spawn_driver_for_test("sh", &["-c", &cmd]).await;

    let init = tokio::time::timeout(std::time::Duration::from_secs(5), agent.events_rx.recv())
        .await
        .expect("driver should emit Init within 5s")
        .expect("events channel should be open");
    match init {
        AgentEvent::Init { session_id, .. } => assert_eq!(session_id, "sess-1"),
        other => panic!("expected Init, got {:?}", other),
    }

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), agent.events_rx.recv())
        .await
        .expect("driver should emit Result")
        .expect("events channel should be open");
    match result {
        AgentEvent::Result {
            text,
            duration_ms,
            error,
        } => {
            assert_eq!(text, "done");
            assert_eq!(duration_ms, 42);
            assert!(error.is_none());
        }
        other => panic!("expected Result, got {:?}", other),
    }

    let exited = tokio::time::timeout(std::time::Duration::from_secs(5), agent.events_rx.recv())
        .await
        .expect("driver should emit Exited after EOF")
        .expect("events channel should be open");
    assert!(matches!(exited, AgentEvent::Exited));

    // Channel closes after Exited
    assert!(agent.events_rx.recv().await.is_none());
}

#[tokio::test]
async fn driver_task_cancellation_terminates_process() {
    // Spawn a long-running sleep — driver must kill it when cancel fires.
    let (mut agent, cancel) = spawn_driver_for_test("sh", &["-c", "sleep 30"]).await;

    cancel.cancel();

    let exited = tokio::time::timeout(std::time::Duration::from_secs(5), agent.events_rx.recv())
        .await
        .expect("driver should emit Exited within 5s of cancellation")
        .expect("events channel should be open");
    assert!(matches!(exited, AgentEvent::Exited));
}
