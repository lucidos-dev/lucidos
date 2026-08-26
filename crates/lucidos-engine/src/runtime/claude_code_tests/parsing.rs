use super::*;

/// Parse one line against a fresh stream, for the majority of tests below that
/// judge a single frame in isolation. Anything about state that spans lines
/// (the usage dedup) drives one `CcStreamState` across several calls instead.
fn parse_one_line(line: &str) -> Vec<AgentEvent> {
    parse_line(&mut CcStreamState::default(), line)
}

/// Build an assistant frame carrying `usage`, with an optional `message.id`.
/// The dedup tests differ only in that id, so spelling the rest of the JSON out
/// per test would bury the one thing under examination.
fn assistant_usage_frame(message_id: Option<&str>, input: u64, output: u64) -> String {
    let id_field = match message_id {
        Some(id) => format!(r#""id":"{}","#, id),
        None => String::new(),
    };
    format!(
        r#"{{"type":"assistant","message":{{{}"role":"assistant","model":"claude-opus-4-7","content":[{{"type":"text","text":"hi"}}],"usage":{{"input_tokens":{},"output_tokens":{},"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#,
        id_field, input, output
    )
}

fn usage_count(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Usage { .. }))
        .count()
}

#[test]
fn parse_system_init() {
    let line = r#"{"type":"system","subtype":"init","session_id":"abc-123","tools":[]}"#;
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
    assert_eq!(
        events.len(),
        1,
        "all-zero usage frames must produce only the Message"
    );
    assert!(matches!(&events[0], AgentEvent::Message { .. }));
}

/// The duplicate-`ContextCaptured` bug. CC splits one assistant message into
/// one frame per content block (thinking, text, each tool_use), and every frame
/// repeats the same `message.id` and the same cumulative usage, so one API call
/// was reported 2 to 4 times (1.73x the real call count across 521,038 rows in
/// the dev workspace, measured 2026-08-11). The repeats must collapse to a
/// single `Usage`, and the frames' own content must still come through.
#[test]
fn parse_assistant_usage_reported_once_per_message_id() {
    let mut state = CcStreamState::default();
    let frame = assistant_usage_frame(Some("msg_repeat"), 208_908, 2);

    let all: Vec<AgentEvent> = (0..3)
        .flat_map(|_| parse_line(&mut state, &frame))
        .collect();

    assert_eq!(
        usage_count(&all),
        1,
        "three frames of one assistant message are one API call, got {:?}",
        all
    );
    assert_eq!(
        all.iter()
            .filter(|e| matches!(e, AgentEvent::Message { .. }))
            .count(),
        3,
        "only the Usage is deduped: each frame's own content block still emits"
    );
}

/// Keyed on the id, NOT on the numbers. Two genuine API calls can report byte
/// identical usage (a cache-warm retry of the same prompt), and dropping the
/// second would under-report spend. Value equality is the heuristic the
/// historical rollup had to use because it cannot see the id; the engine has
/// the exact key.
#[test]
fn parse_assistant_usage_distinct_message_ids_both_report() {
    let mut state = CcStreamState::default();
    let first = parse_line(&mut state, &assistant_usage_frame(Some("msg_a"), 1000, 7));
    let second = parse_line(&mut state, &assistant_usage_frame(Some("msg_b"), 1000, 7));

    assert_eq!(usage_count(&first), 1);
    assert_eq!(
        usage_count(&second),
        1,
        "a different message id is a different API call however identical its usage"
    );
}

/// A parallel sub-agent's frames ride the parent's stream, so two messages can
/// interleave. Dedup must survive that: the id claimed by the first frame is
/// still claimed when the second one arrives.
#[test]
fn parse_assistant_usage_dedupes_across_interleaved_messages() {
    let mut state = CcStreamState::default();
    let a = assistant_usage_frame(Some("msg_parent"), 5000, 11);
    let b = assistant_usage_frame(Some("msg_subagent"), 900, 4);

    let all: Vec<AgentEvent> = [&a, &b, &a, &b]
        .iter()
        .flat_map(|f| parse_line(&mut state, f))
        .collect();

    assert_eq!(
        usage_count(&all),
        2,
        "two interleaved messages are two API calls, got {:?}",
        all
    );
}

/// The same, at the width a fleet of parallel sub-agents can reach. Every id
/// claimed this turn is remembered, so no amount of interleaving between a
/// message's frames lets the later one report its call a second time. A fixed
/// window of recent ids would evict the first message and do exactly that.
#[test]
fn parse_assistant_usage_dedup_survives_wide_interleaving() {
    let mut state = CcStreamState::default();
    let frames: Vec<String> = (0..40)
        .map(|i| assistant_usage_frame(Some(&format!("msg_{}", i)), 1000 + i, 5))
        .collect();

    let first_pass: Vec<AgentEvent> = frames
        .iter()
        .flat_map(|f| parse_line(&mut state, f))
        .collect();
    let second_pass: Vec<AgentEvent> = frames
        .iter()
        .flat_map(|f| parse_line(&mut state, f))
        .collect();

    assert_eq!(
        usage_count(&first_pass),
        40,
        "40 distinct calls, 40 reports"
    );
    assert_eq!(
        usage_count(&second_pass),
        0,
        "a second frame of each of those 40 messages is no new call at all"
    );
}

/// The claimed ids are turn-scoped: a `result` frame releases them, which is
/// what keeps the set the size of one turn rather than of a session that stays
/// up for days. Nothing is lost, because CC never reuses a message id.
#[test]
fn parse_assistant_usage_claims_are_released_at_the_turn_terminal() {
    let mut state = CcStreamState::default();
    let frame = assistant_usage_frame(Some("msg_turn_1"), 7000, 21);

    let before = parse_line(&mut state, &frame);
    let repeat_within_turn = parse_line(&mut state, &frame);
    parse_line(
        &mut state,
        r#"{"type":"result","result":"Done.","duration_ms":10}"#,
    );
    let after_terminal = parse_line(&mut state, &frame);

    assert_eq!(usage_count(&before), 1);
    assert_eq!(usage_count(&repeat_within_turn), 0);
    assert_eq!(
        usage_count(&after_terminal),
        1,
        "past the terminal the id is forgotten, and re-reporting a straggler is \
         the safe direction"
    );
}

/// Defensive: no `message.id` means no key, and with no key a repeat cannot be
/// told from a fresh call. Emit, every time. Over-counting spend is the
/// recoverable direction; silently dropping usage is not.
#[test]
fn parse_assistant_usage_without_message_id_always_reports() {
    let mut state = CcStreamState::default();
    let frame = assistant_usage_frame(None, 1234, 56);

    let all: Vec<AgentEvent> = (0..2)
        .flat_map(|_| parse_line(&mut state, &frame))
        .collect();

    assert_eq!(
        usage_count(&all),
        2,
        "an id-less frame must never be suppressed, got {:?}",
        all
    );
}

/// The all-zero skip and the dedup are orthogonal, and an all-zero frame must
/// not consume its id: CC stamps zeroed usage on synthetic continuation
/// messages, and a real frame later carrying the same id still has to report.
#[test]
fn parse_assistant_all_zero_usage_does_not_consume_its_message_id() {
    let mut state = CcStreamState::default();
    let zeroed = parse_line(&mut state, &assistant_usage_frame(Some("msg_z"), 0, 0));
    let real = parse_line(&mut state, &assistant_usage_frame(Some("msg_z"), 4321, 9));

    assert_eq!(
        usage_count(&zeroed),
        0,
        "the all-zero skip still holds, got {:?}",
        zeroed
    );
    assert_eq!(
        usage_count(&real),
        1,
        "a real call must report even when a zeroed frame claimed its id first"
    );
}

#[test]
fn parse_assistant_no_usage_block() {
    // Defensive: assistant frames without a usage block (e.g. older CC
    // format) must still parse text/tool_use without panicking.
    let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#;
    let events = parse_one_line(line);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], AgentEvent::Message { .. }));
}

/// CC's own error banner is a SYNTHETIC assistant message flagged
/// `is_api_error_message: true`. It is CC's error surface, and the identical
/// string returns as the turn's `result` error, which the transcript renders in
/// the failure card. Taking it as prose printed the failure twice: a paragraph
/// mashed into the response body, then the red card underneath it (reported
/// 2026-08-10, the exact line below).
#[test]
fn parse_assistant_skips_cc_own_api_error_banner() {
    let line = r#"{"type":"assistant","is_api_error_message":true,"message":{"role":"assistant","model":"<synthetic>","content":[{"type":"text","text":"API Error: Stream idle timeout - no chunks received"}],"usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    assert!(
        parse_one_line(line).is_empty(),
        "CC's API-error banner must not become assistant text: the failure card already states it"
    );
}

/// A sub-agent's banner rides the PARENT's stream carrying the same flag (plus a
/// `parent_tool_use_id`), so it was leaking into the parent's prose too. It never
/// reaches the parent's `result`, so it is dropped rather than moved.
#[test]
fn parse_assistant_skips_subagent_api_error_banner() {
    let line = r#"{"type":"assistant","is_api_error_message":true,"parent_tool_use_id":"toolu_1","message":{"role":"assistant","model":"<synthetic>","content":[{"type":"text","text":"API Error: Response stalled mid-stream. The response above may be incomplete."}]}}"#;
    assert!(parse_one_line(line).is_empty());
}

/// The flag is what disqualifies the text, not its wording. A turn where the
/// model itself writes about an API error is ordinary prose and stays.
#[test]
fn parse_assistant_keeps_text_that_merely_looks_like_an_api_error() {
    let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"API Error: Stream idle timeout - no chunks received"}]}}"#;
    let events = parse_one_line(line);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::Message { text, .. } => {
            assert_eq!(text, "API Error: Stream idle timeout - no chunks received");
        }
        other => panic!("Expected Message, got {:?}", other),
    }
}

/// `is_api_error_message: false` is the ordinary shape for every non-error
/// assistant line CC bothers to stamp, so it must read as "keep".
#[test]
fn parse_assistant_keeps_text_when_the_error_flag_is_false() {
    let line = r#"{"type":"assistant","is_api_error_message":false,"message":{"role":"assistant","content":[{"type":"text","text":"Reading the file."}]}}"#;
    let events = parse_one_line(line);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], AgentEvent::Message { .. }));
}

#[test]
fn parse_legacy_tool_result() {
    let line = r#"{"type":"tool_result","content":"file contents here","is_error":false,"tool_use_id":"toolu_legacy"}"#;
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
    assert_eq!(events.len(), 2);
    assert!(matches!(&events[0], AgentEvent::ToolResult { .. }));
    assert!(matches!(&events[1], AgentEvent::ToolResult { .. }));
}

/// Pull the one `ToolResult` a line is expected to produce.
fn one_tool_result(line: &str) -> (String, String, String) {
    let events = parse_one_line(line);
    assert_eq!(events.len(), 1, "expected one event from {line}");
    match &events[0] {
        AgentEvent::ToolResult { output, status, id } => {
            (output.clone(), status.clone(), id.clone())
        }
        other => panic!("Expected ToolResult, got {other:?}"),
    }
}

/// A subagent's whole report arrives as `content: [{type: text}]`, and reading
/// that field as a string alone blanked every `Agent` step in the UI.
#[test]
fn parse_user_tool_result_joins_an_array_of_text_blocks() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":[{"type":"text","text":"Findings"},{"type":"text","text":"Nothing broke."}],"is_error":false}]}}"#;
    let (output, status, id) = one_tool_result(line);
    assert_eq!(output, "Findings\nNothing broke.");
    assert_eq!(status, "success");
    assert_eq!(id, "tu_1");
}

/// The legacy line shape takes the same array. The bug lived in both arms, so
/// a fix landing on only one is the failure mode to prevent.
#[test]
fn parse_legacy_tool_result_joins_an_array_of_text_blocks() {
    let line = r#"{"type":"tool_result","tool_use_id":"toolu_legacy","content":[{"type":"text","text":"first"},{"type":"text","text":"second"}],"is_error":false}"#;
    let (output, status, id) = one_tool_result(line);
    assert_eq!(output, "first\nsecond");
    assert_eq!(status, "success");
    assert_eq!(id, "toolu_legacy");
}

/// An image read must reach the step as a LABEL. One full-page screenshot is
/// hundreds of KB of base64, and inlining it would ride the event payload and
/// every SSE frame carrying it.
#[test]
fn parse_tool_result_labels_an_image_without_inlining_its_data() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgoAAAANSUhEUg"}}],"is_error":false}]}}"#;
    let (output, ..) = one_tool_result(line);
    assert_eq!(output, "[image]");
    assert!(
        !output.contains("iVBORw0KGgo"),
        "base64 image data must never reach the event",
    );
}

/// `ToolSearch` answers with `tool_reference` blocks, whose only readable part
/// is the tool they name.
#[test]
fn parse_tool_result_names_the_subject_of_a_reference_block() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":[{"type":"tool_reference","tool_name":"TaskOutput"}],"is_error":false}]}}"#;
    let (output, ..) = one_tool_result(line);
    assert_eq!(output, "[tool_reference: TaskOutput]");
}

/// The default that matters: a block type nobody has seen yet is VISIBLE. An
/// unrecognised shape reading as `""` is exactly how `tool_reference` blanked
/// 583 steps in 30 days without anyone noticing.
#[test]
fn parse_tool_result_labels_an_unrecognised_block_rather_than_blanking() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":[{"type":"some_future_block"}],"is_error":false}]}}"#;
    let (output, ..) = one_tool_result(line);
    assert_eq!(output, "[some_future_block]");
}

/// A block with no `type` at all still says something.
#[test]
fn parse_tool_result_labels_a_typeless_block() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":[{"unexpected":true}],"is_error":false}]}}"#;
    let (output, ..) = one_tool_result(line);
    assert_eq!(output, "[unknown]");
}

/// An array-shaped result still reports its error status and pairs with its
/// call, which is what re-arms the session watchdog.
#[test]
fn parse_tool_result_keeps_error_status_on_an_array_shape() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_9","content":[{"type":"text","text":"boom"}],"is_error":true}]}}"#;
    let (output, status, id) = one_tool_result(line);
    assert_eq!(output, "boom");
    assert_eq!(status, "error");
    assert_eq!(id, "tu_9");
}

/// A missing `content` stays empty, as it was before the array handling.
/// Absent is the one shape that genuinely carries nothing.
#[test]
fn parse_tool_result_with_no_content_is_empty() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","is_error":false}]}}"#;
    let (output, ..) = one_tool_result(line);
    assert!(output.is_empty());
}

/// An explicit `null` reads the same as absent.
#[test]
fn parse_tool_result_with_null_content_is_empty() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":null,"is_error":false}]}}"#;
    let (output, ..) = one_tool_result(line);
    assert!(output.is_empty());
}

/// A bare string inside the array carries its own text and has no `type`.
/// Labelling it would drop the very content the label exists to preserve.
#[test]
fn parse_tool_result_keeps_a_bare_string_array_element() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":["plain text","more"],"is_error":false}]}}"#;
    let (output, ..) = one_tool_result(line);
    assert_eq!(output, "plain text\nmore");
}

/// A `text` block whose `text` key is missing takes the label path. The empty
/// string is reserved for content that is genuinely absent.
#[test]
fn parse_tool_result_labels_a_text_block_with_no_text() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":[{"type":"text"}],"is_error":false}]}}"#;
    let (output, ..) = one_tool_result(line);
    assert_eq!(output, "[text]");
}

/// A `content` that is neither string nor array, and carries no block `type`
/// to dispatch on, still reaches the step as its JSON. CC has never sent this
/// shape. What it buys is that present content is labelled or rendered, never
/// dropped for being unrecognised.
#[test]
fn parse_tool_result_renders_an_unexpected_content_shape() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":{"unexpected":true},"is_error":false}]}}"#;
    let (output, ..) = one_tool_result(line);
    assert!(
        !output.is_empty(),
        "an object content must not read as empty"
    );
    assert!(output.contains("unexpected"));
}

/// A lone block sent unwrapped takes the same path a wrapped one does. The
/// JSON fallback above must never claim an image, or an unwrapped screenshot
/// spills the base64 that the array form is careful to label.
#[test]
fn parse_tool_result_labels_an_unwrapped_image_block() {
    let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":{"type":"image","source":{"type":"base64","data":"iVBORw0KGgoAAAANSUhEUg"}},"is_error":false}]}}"#;
    let (output, ..) = one_tool_result(line);
    assert_eq!(output, "[image]");
    assert!(
        !output.contains("iVBORw0KGgo"),
        "an unwrapped image must not spill base64 either",
    );
}

#[test]
fn parse_user_non_tool_result_ignored() {
    // A user message with text content (not tool_result) should produce no events
    let line =
        r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}"#;
    let events = parse_one_line(line);
    assert_eq!(events.len(), 0);
}

// control_response from CC after a control_request (e.g. interrupt) — silently ignored
#[test]
fn parse_control_response_ignored() {
    let line =
        r#"{"type":"control_response","request_id":"abc-123","response":{"subtype":"success"}}"#;
    let events = parse_one_line(line);
    assert!(events.is_empty());
}

// CC 2.1.76+ sends streaming deltas as "stream_event" wrappers. They carry no
// content to persist (the complete text/tool call arrives later as
// assistant/tool_result), but each one PROVES the subprocess is alive and
// actively producing output — so the parser emits a content-free
// `StreamActivity` liveness ping. Without it the watchdog's inactivity clock
// only ticks at step boundaries, and one long step (extended thinking on a
// hard problem) is killed mid-work even though CC is streaming the whole time.
#[test]
fn parse_stream_event_emits_liveness() {
    let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}}"#;
    let events = parse_one_line(line);
    assert!(
        matches!(&events[..], [AgentEvent::StreamActivity]),
        "stream_event must yield a single StreamActivity liveness ping, got {:?}",
        events
    );
}

// A `thinking_delta` carries plaintext reasoning that exists ONLY on the live
// stream (the persisted JSONL keeps just an encrypted signature). The parser must
// extract it as a `Thought` AND still emit the `StreamActivity` ping so the
// watchdog contract is unchanged.
#[test]
fn parse_stream_event_extracts_thinking_delta_and_keeps_liveness() {
    let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me reason"}}}"#;
    let events = parse_one_line(line);
    match &events[..] {
        [AgentEvent::Thought { text }, AgentEvent::StreamActivity] => {
            assert_eq!(text, "Let me reason");
        }
        other => panic!("expected [Thought, StreamActivity], got {:?}", other),
    }
}

// Regression: the reasoning text rides on `delta.thinking`, not `delta.text`
// (only `text_delta` uses `text`). A `thinking_delta` carrying ONLY a `text`
// field must NOT yield a Thought — reading `text` here was the original bug that
// dropped every thought.
#[test]
fn parse_stream_event_thinking_delta_ignores_text_field() {
    let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","text":"wrong field"}}}"#;
    let events = parse_one_line(line);
    assert!(
        matches!(&events[..], [AgentEvent::StreamActivity]),
        "a thinking_delta with only a `text` field must yield liveness only, got {:?}",
        events
    );
}

// An empty thinking delta must not emit a Thought (no empty "Thinking" steps),
// but the liveness ping is still emitted.
#[test]
fn parse_stream_event_empty_thinking_delta_is_liveness_only() {
    let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}}}"#;
    let events = parse_one_line(line);
    assert!(
        matches!(&events[..], [AgentEvent::StreamActivity]),
        "empty thinking_delta must yield only StreamActivity, got {:?}",
        events
    );
}

#[test]
fn parse_result() {
    let line = r#"{"type":"result","result":"Done.","duration_ms":1234}"#;
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
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
    let events = parse_one_line(line);
    match &events[0] {
        AgentEvent::Result { text, error, .. } => {
            assert_eq!(text, "All good.");
            assert!(error.is_none());
        }
        other => panic!("Expected Result, got {:?}", other),
    }
}

/// CC sometimes emits `is_error: true` with `subtype: "success"` and no
/// `errors[]` — a self-contradictory terminal signal. Observed on BOTH a
/// genuinely completed turn that streamed a full response and committed work,
/// AND on an upstream API drop that streamed an "API Error: …" message before
/// CC decided the conversation succeeded structurally. With no `errors[]` and
/// the non-informative `success` subtype there is nothing actionable to report.
/// The parser must return `error: None` — NOT a fabricated "Unknown error" —
/// so `classify_result` classifies on the turn's real content instead of
/// flipping an otherwise-successful turn to `Failed` (the "Event stream error /
/// Unknown error" false-failure on the OPUS Brand Title Badge Updates thread).
/// A turn that produced no content still fails, via the empty-response branch.
#[test]
fn parse_result_is_error_with_success_subtype_yields_no_error() {
    let line =
        r#"{"type":"result","subtype":"success","is_error":true,"result":"","duration_ms":100}"#;
    let events = parse_one_line(line);
    match &events[0] {
        AgentEvent::Result { error, .. } => {
            assert!(
                error.is_none(),
                "is_error:true + subtype:success + no errors[] carries no usable \
                 failure reason — must be None so a successful turn isn't flipped \
                 to Failed with a fabricated 'Unknown error'; got {:?}",
                error
            );
        }
        other => panic!("Expected Result, got {:?}", other),
    }
}

/// The load-bearing case behind the fix above: a turn that produced a real
/// final message but whose `result` message still carried the contradictory
/// `is_error: true` + `subtype: "success"`. `error` must be `None` so the turn
/// classifies as `Generated` and the streamed answer + proposed change are not
/// buried under a red error dot.
#[test]
fn parse_result_is_error_with_success_subtype_and_text_yields_no_error() {
    let line = r#"{"type":"result","subtype":"success","is_error":true,"result":"Done — changes committed.","duration_ms":100}"#;
    let events = parse_one_line(line);
    match &events[0] {
        AgentEvent::Result { text, error, .. } => {
            assert_eq!(text, "Done — changes committed.");
            assert!(
                error.is_none(),
                "a successful turn with real output must not carry an error; got {:?}",
                error
            );
        }
        other => panic!("Expected Result, got {:?}", other),
    }
}

/// The genuine-drop counterpart of the fix: an upstream API drop that CC still
/// labels `subtype: "success"` (with `is_error: true`, no `errors[]`) surfaces
/// its "API Error: …" message as the final result text. That IS an actionable
/// failure, so `error` must be preserved (the real text, not a generic "Unknown
/// error") and the turn stays `Failed` — a successful turn's result text never
/// *starts* with the `API Error` prefix, so this cannot re-flag a real success.
#[test]
fn parse_result_is_error_success_subtype_preserves_api_error_result_text() {
    let line = r#"{"type":"result","subtype":"success","is_error":true,"result":"API Error: Stream idle timeout - partial response received","duration_ms":100}"#;
    let events = parse_one_line(line);
    match &events[0] {
        AgentEvent::Result { error, .. } => {
            assert_eq!(
                error.as_deref(),
                Some("API Error: Stream idle timeout - partial response received"),
                "a genuine upstream drop (result text starts with `API Error`) must \
                 stay Failed with the real error text"
            );
        }
        other => panic!("Expected Result, got {:?}", other),
    }
}

/// Guard the tightness of the discriminator: a genuinely successful turn whose
/// result text merely *mentions* "api error" mid-sentence (not a leading `API
/// Error` prefix) must NOT be re-flagged as Failed — that would reintroduce the
/// exact false-failure the fix removes.
#[test]
fn parse_result_is_error_success_subtype_ignores_incidental_api_error_mention() {
    let line = r#"{"type":"result","subtype":"success","is_error":true,"result":"Fixed the api error handling in client.ts.","duration_ms":100}"#;
    let events = parse_one_line(line);
    match &events[0] {
        AgentEvent::Result { error, .. } => {
            assert!(
                error.is_none(),
                "an incidental mid-sentence 'api error' mention is not CC's error \
                 message — must stay None; got {:?}",
                error
            );
        }
        other => panic!("Expected Result, got {:?}", other),
    }
}

/// An absent subtype is likewise non-actionable: `is_error: true` with neither
/// `errors[]` nor a subtype gives no failure reason, so `error` must be `None`
/// (the empty-response branch still catches a genuinely output-less turn).
#[test]
fn parse_result_is_error_with_no_subtype_and_no_errors_yields_no_error() {
    let line = r#"{"type":"result","is_error":true,"result":"","duration_ms":100}"#;
    let events = parse_one_line(line);
    match &events[0] {
        AgentEvent::Result { error, .. } => {
            assert!(
                error.is_none(),
                "is_error:true with no subtype and no errors[] carries no usable \
                 reason — must be None; got {:?}",
                error
            );
        }
        other => panic!("Expected Result, got {:?}", other),
    }
}

#[test]
fn parse_empty_line() {
    assert!(parse_one_line("").is_empty());
    assert!(parse_one_line("   ").is_empty());
    assert!(parse_one_line("\n").is_empty());
}

#[test]
fn parse_invalid_json() {
    assert!(parse_one_line("not json at all").is_empty());
}

#[test]
fn parse_unknown_type_logged() {
    let line = r#"{"type":"something_new","data":"test"}"#;
    let events = parse_one_line(line);
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

    // One state for the whole sequence, as the driver task does it.
    let mut state = CcStreamState::default();
    let all_events: Vec<AgentEvent> = lines
        .iter()
        .flat_map(|l| parse_line(&mut state, l))
        .collect();

    // system → Init
    assert!(
        matches!(&all_events[0], AgentEvent::Init { session_id, .. } if session_id == "sess-1")
    );
    // stream_event → StreamActivity liveness ping (keeps the watchdog clock fresh)
    assert!(matches!(&all_events[1], AgentEvent::StreamActivity));
    // assistant text + tool_use
    assert!(matches!(&all_events[2], AgentEvent::Message { text, .. } if text == "Reading file."));
    assert!(matches!(&all_events[3], AgentEvent::ToolUse { name, .. } if name == "Read"));
    // user tool_result
    assert!(
        matches!(&all_events[4], AgentEvent::ToolResult { output, status, .. } if output == "file contents" && status == "success")
    );
    // second stream_event → StreamActivity
    assert!(matches!(&all_events[5], AgentEvent::StreamActivity));
    // second assistant text
    assert!(
        matches!(&all_events[6], AgentEvent::Message { text, .. } if text == "Here are the contents.")
    );
    // result
    assert!(matches!(
        &all_events[7],
        AgentEvent::Result {
            duration_ms: 5000,
            error: None,
            ..
        }
    ));
    // 6 content events + the 2 stream_event liveness pings
    assert_eq!(all_events.len(), 8);
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
        parse_one_line(hook_started).is_empty(),
        "hook_started should not produce Init"
    );
    assert!(
        parse_one_line(hook_response).is_empty(),
        "hook_response should not produce Init"
    );
    assert!(
        parse_one_line(hook_progress).is_empty(),
        "hook_progress should not produce Init"
    );
}

#[test]
fn parse_line_extracts_init_from_system_init_event() {
    let init = r#"{"type":"system","subtype":"init","session_id":"s1","model":"opus","slash_commands":["compact","commit","review"],"skills":["commit","review"]}"#;
    let events = parse_one_line(init);
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
