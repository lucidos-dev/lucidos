use super::*;
use crate::runtime::codex_parse::{parse_codex_line, CodexLine, ItemKind, ItemPhase};

/// Real lines captured from `codex exec --json` (codex-cli 0.139.0,
/// 2026-06-12) — the fixtures the parser contract is pinned to.
const REAL_THREAD_STARTED: &str =
    r#"{"type":"thread.started","thread_id":"019ebbf1-e752-7fb2-ab6b-d03f5dc7686d"}"#;
const REAL_TURN_STARTED: &str = r#"{"type":"turn.started"}"#;
const REAL_CMD_STARTED: &str = r#"{"type":"item.started","item":{"id":"item_0","type":"command_execution","command":"/bin/zsh -lc ls","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
const REAL_CMD_COMPLETED: &str = r#"{"type":"item.completed","item":{"id":"item_0","type":"command_execution","command":"/bin/zsh -lc ls","aggregated_output":"run1.jsonl\nsample.txt\n","exit_code":0,"status":"completed"}}"#;
const REAL_AGENT_MESSAGE: &str =
    r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"5 files"}}"#;
const REAL_TURN_COMPLETED: &str = r#"{"type":"turn.completed","usage":{"input_tokens":24171,"cached_input_tokens":21760,"output_tokens":63,"reasoning_output_tokens":10}}"#;
const REAL_TURN_FAILED: &str = r#"{"type":"turn.failed","error":{"message":"The requested model 'bogus-model-name' does not exist."}}"#;
const REAL_STREAM_ERROR: &str = r#"{"type":"error","message":"stream error: broken pipe"}"#;

#[test]
fn parses_real_thread_started() {
    assert_eq!(
        parse_codex_line(REAL_THREAD_STARTED),
        CodexLine::ThreadStarted {
            thread_id: "019ebbf1-e752-7fb2-ab6b-d03f5dc7686d".into()
        }
    );
}

#[test]
fn parses_real_command_execution_phases() {
    let started = parse_codex_line(REAL_CMD_STARTED);
    match started {
        CodexLine::Item {
            phase: ItemPhase::Started,
            id,
            kind: ItemKind::CommandExecution {
                command, status, ..
            },
        } => {
            assert_eq!(id, "item_0");
            assert_eq!(command, "/bin/zsh -lc ls");
            assert_eq!(status, "in_progress");
        }
        other => panic!("expected started command_execution, got {:?}", other),
    }
    let completed = parse_codex_line(REAL_CMD_COMPLETED);
    match completed {
        CodexLine::Item {
            phase: ItemPhase::Completed,
            kind:
                ItemKind::CommandExecution {
                    aggregated_output,
                    exit_code,
                    status,
                    ..
                },
            ..
        } => {
            assert_eq!(aggregated_output, "run1.jsonl\nsample.txt\n");
            assert_eq!(exit_code, Some(0));
            assert_eq!(status, "completed");
        }
        other => panic!("expected completed command_execution, got {:?}", other),
    }
}

#[test]
fn parses_turn_failed_message() {
    assert_eq!(
        parse_codex_line(REAL_TURN_FAILED),
        CodexLine::TurnFailed {
            message: "The requested model 'bogus-model-name' does not exist.".into()
        }
    );
}

#[test]
fn non_json_and_unknown_lines_map_to_other() {
    assert_eq!(parse_codex_line("not json"), CodexLine::Other);
    assert_eq!(parse_codex_line(""), CodexLine::Other);
    assert_eq!(
        parse_codex_line(r#"{"type":"future.event","x":1}"#),
        CodexLine::Other
    );
}

fn track(lines: &[&str]) -> (Vec<AgentEvent>, crate::runtime::codex_parse::TurnTracker) {
    let mut tracker = crate::runtime::codex_parse::TurnTracker::new(None);
    tracker.begin_turn();
    let mut events = Vec::new();
    for line in lines {
        events.extend(tracker.map_line(parse_codex_line(line), 1234));
    }
    (events, tracker)
}

#[test]
fn full_real_turn_maps_to_canonical_events() {
    // The exact sequence captured from the probe run: init handshake, a
    // command with paired tool events, the final message, the turn terminal.
    let (events, tracker) = track(&[
        REAL_THREAD_STARTED,
        REAL_TURN_STARTED,
        REAL_CMD_STARTED,
        REAL_CMD_COMPLETED,
        REAL_AGENT_MESSAGE,
        REAL_TURN_COMPLETED,
    ]);

    match &events[0] {
        AgentEvent::Init {
            session_id,
            model,
            slash_commands,
            skills,
        } => {
            assert_eq!(session_id, "019ebbf1-e752-7fb2-ab6b-d03f5dc7686d");
            assert_eq!(*model, None, "JSONL stream doesn't echo the model");
            assert!(slash_commands.is_empty());
            assert!(skills.is_empty());
        }
        other => panic!("expected Init first, got {:?}", other),
    }
    match &events[1] {
        AgentEvent::ToolUse { name, input, id } => {
            assert_eq!(name, "command_execution");
            assert_eq!(input["command"], "/bin/zsh -lc ls");
            assert_eq!(id, "item_0");
        }
        other => panic!("expected ToolUse second, got {:?}", other),
    }
    match &events[2] {
        AgentEvent::ToolResult { output, status, id } => {
            assert_eq!(output, "run1.jsonl\nsample.txt\n");
            assert_eq!(status, "success");
            assert_eq!(id, "item_0", "result must pair with its call");
        }
        other => panic!("expected ToolResult third, got {:?}", other),
    }
    match &events[3] {
        AgentEvent::Message { role, text } => {
            assert_eq!(role, "assistant");
            assert_eq!(text, "5 files");
        }
        other => panic!("expected Message fourth, got {:?}", other),
    }
    match &events[4] {
        AgentEvent::Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
            ..
        } => {
            // Codex input_tokens is the TOTAL; the canonical convention
            // carries the uncached portion with the cache split out.
            assert_eq!(*input_tokens, 24171 - 21760);
            assert_eq!(*cache_read_tokens, 21760);
            assert_eq!(*output_tokens, 63);
            assert_eq!(*cache_creation_tokens, 0);
        }
        other => panic!("expected Usage fifth, got {:?}", other),
    }
    match &events[5] {
        AgentEvent::Result {
            text,
            duration_ms,
            error,
        } => {
            assert_eq!(text, "5 files", "Result.text carries the streamed answer so the engine's empty-result detection works");
            assert_eq!(*duration_ms, 1234, "driver-measured duration");
            assert_eq!(*error, None);
        }
        other => panic!("expected Result last, got {:?}", other),
    }
    assert_eq!(events.len(), 6);
    assert!(tracker.turn_terminal_seen);
}

#[test]
fn duplicate_thread_started_emits_init_once() {
    // Every resumed turn re-announces the thread id — the engine must not
    // see a second Init (it would re-persist settings noise).
    let (events, _) = track(&[REAL_THREAD_STARTED, REAL_THREAD_STARTED]);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Init { .. }))
            .count(),
        1
    );
}

#[test]
fn turn_failed_maps_to_result_with_error() {
    let (events, tracker) = track(&[REAL_THREAD_STARTED, REAL_TURN_FAILED]);
    match events.last() {
        Some(AgentEvent::Result { error: Some(e), .. }) => {
            assert!(e.contains("bogus-model-name"));
        }
        other => panic!("expected failed Result, got {:?}", other),
    }
    assert!(tracker.turn_terminal_seen);
}

#[test]
fn stream_error_is_recorded_but_not_terminal() {
    // "Reconnecting…"-style transient errors must not end the turn; the
    // driver only promotes last_error when the process dies without a
    // turn terminal.
    let (events, tracker) = track(&[REAL_THREAD_STARTED, REAL_STREAM_ERROR]);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::Result { .. })),
        "stream error alone must not produce a Result"
    );
    assert!(!tracker.turn_terminal_seen);
    assert_eq!(
        tracker.last_error.as_deref(),
        Some("stream error: broken pipe")
    );
}

#[test]
fn file_change_synthesizes_balanced_tool_pair() {
    // file_change arrives completed-only — both ToolUse and ToolResult must
    // be synthesized so the engine's tools_in_flight counter stays balanced
    // (an unpaired ToolUse permanently disarms the hang watchdog).
    let line = r#"{"type":"item.completed","item":{"id":"item_4","type":"file_change","changes":[{"path":"src/a.rs","kind":"update"}],"status":"completed"}}"#;
    let (events, _) = track(&[line]);
    assert_eq!(events.len(), 2);
    assert!(
        matches!(&events[0], AgentEvent::ToolUse { name, id, .. } if name == "file_change" && id == "item_4")
    );
    assert!(
        matches!(&events[1], AgentEvent::ToolResult { status, id, .. } if status == "success" && id == "item_4")
    );
}

#[test]
fn failed_command_maps_to_error_status() {
    let line = r#"{"type":"item.completed","item":{"id":"item_2","type":"command_execution","command":"false","aggregated_output":"","exit_code":1,"status":"failed"}}"#;
    let (events, _) = track(&[line]);
    match &events[0] {
        AgentEvent::ToolResult { output, status, .. } => {
            assert_eq!(status, "error");
            assert_eq!(
                output, "exit_code: 1",
                "empty output falls back to the exit code"
            );
        }
        other => panic!("expected ToolResult, got {:?}", other),
    }
}

#[test]
fn a_null_exit_code_reads_as_unknown_not_success() {
    // A killed or abandoned command reports no code. `exit_code: 0` here would
    // tell the model the command succeeded.
    let line = r#"{"type":"item.completed","item":{"id":"item_3","type":"command_execution","command":"sleep 99","aggregated_output":"","exit_code":null,"status":"failed"}}"#;
    let (events, _) = track(&[line]);
    match &events[0] {
        AgentEvent::ToolResult { output, .. } => assert_eq!(output, "exit_code: unknown"),
        other => panic!("expected ToolResult, got {:?}", other),
    }
}

#[test]
fn mcp_tool_call_uses_cc_naming_convention() {
    let started = r#"{"type":"item.started","item":{"id":"item_5","type":"mcp_tool_call","server":"docs","tool":"search","arguments":{"q":"x"},"result":null,"error":null,"status":"in_progress"}}"#;
    let completed = r#"{"type":"item.completed","item":{"id":"item_5","type":"mcp_tool_call","server":"docs","tool":"search","arguments":{"q":"x"},"result":{"matches":3},"error":null,"status":"completed"}}"#;
    let (events, _) = track(&[started, completed]);
    assert!(matches!(&events[0], AgentEvent::ToolUse { name, .. } if name == "mcp__docs__search"));
    assert!(
        matches!(&events[1], AgentEvent::ToolResult { output, .. } if output.contains("\"matches\":3"))
    );
}

#[test]
fn todo_list_dedupes_identical_snapshots() {
    let started = r#"{"type":"item.started","item":{"id":"item_8","type":"todo_list","items":[{"text":"a","completed":false}]}}"#;
    let updated_same = r#"{"type":"item.updated","item":{"id":"item_8","type":"todo_list","items":[{"text":"a","completed":false}]}}"#;
    let updated_changed = r#"{"type":"item.updated","item":{"id":"item_8","type":"todo_list","items":[{"text":"a","completed":true}]}}"#;
    let (events, _) = track(&[started, updated_same, updated_changed]);
    // started → pair; identical update → nothing; changed update → pair.
    assert_eq!(events.len(), 4);
    assert!(matches!(&events[0], AgentEvent::ToolUse { name, .. } if name == "todo_list"));
    assert!(matches!(&events[2], AgentEvent::ToolUse { name, .. } if name == "todo_list"));
}

// A completed reasoning item surfaces its summary text as a Thought so the
// timeline can render a "Thinking" step.
#[test]
fn reasoning_item_completed_emits_thought() {
    let line = r#"{"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"**Scanning**"}}"#;
    let (events, _) = track(&[line]);
    match &events[..] {
        [AgentEvent::Thought { text }] => assert_eq!(text, "**Scanning**"),
        other => panic!("expected [Thought], got {:?}", other),
    }
}

// A reasoning item that is only started/updated (not completed) emits nothing —
// Codex exec sends the full summary on completion, mirroring agent_message.
#[test]
fn reasoning_item_started_emits_nothing() {
    let line =
        r#"{"type":"item.started","item":{"id":"item_0","type":"reasoning","text":"partial"}}"#;
    let (events, _) = track(&[line]);
    assert!(events.is_empty());
}

#[test]
fn error_item_records_last_error_without_events() {
    let line = r#"{"type":"item.completed","item":{"id":"item_9","type":"error","message":"command output truncated"}}"#;
    let (events, tracker) = track(&[line]);
    assert!(events.is_empty());
    assert_eq!(
        tracker.last_error.as_deref(),
        Some("command output truncated")
    );
}

#[test]
fn multiple_agent_messages_join_in_result_text() {
    let msg1 =
        r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"first"}}"#;
    let msg2 =
        r#"{"type":"item.completed","item":{"id":"i2","type":"agent_message","text":"second"}}"#;
    let (events, _) = track(&[msg1, msg2, REAL_TURN_COMPLETED]);
    match events.last() {
        Some(AgentEvent::Result { text, .. }) => assert_eq!(text, "first\n\nsecond"),
        other => panic!("expected Result, got {:?}", other),
    }
}

#[test]
fn begin_turn_resets_per_turn_state_but_keeps_session() {
    let mut tracker = crate::runtime::codex_parse::TurnTracker::new(None);
    tracker.begin_turn();
    let _ = tracker.map_line(parse_codex_line(REAL_THREAD_STARTED), 0);
    let _ = tracker.map_line(parse_codex_line(REAL_AGENT_MESSAGE), 0);
    let _ = tracker.map_line(parse_codex_line(REAL_TURN_COMPLETED), 0);
    assert!(tracker.turn_terminal_seen);

    tracker.begin_turn();
    assert!(!tracker.turn_terminal_seen);
    assert_eq!(tracker.turn_text(), "");
    assert_eq!(
        tracker.session_id.as_deref(),
        Some("019ebbf1-e752-7fb2-ab6b-d03f5dc7686d"),
        "session id must survive turn boundaries — it's the resume handle"
    );
}

#[test]
fn unknown_item_type_degrades_to_no_events() {
    let line = r#"{"type":"item.completed","item":{"id":"i9","type":"hologram","payload":{}}}"#;
    let (events, _) = track(&[line]);
    assert!(
        events.is_empty(),
        "a new Codex item type must not wedge the turn"
    );
}

#[test]
fn empty_usage_turn_completed_still_emits_result() {
    let line = r#"{"type":"turn.completed","usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0}}"#;
    let (events, _) = track(&[line]);
    assert_eq!(events.len(), 1, "zero usage is skipped, Result still fires");
    assert!(matches!(&events[0], AgentEvent::Result { error: None, .. }));
}
