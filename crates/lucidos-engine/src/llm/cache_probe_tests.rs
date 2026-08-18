//! Tests for the prompt-cache wire probe.
//!
//! They call the pure line builders directly. `enabled()` memoises the env var
//! in a `OnceLock`, so a test process cannot see both states. Gating the
//! builders behind it would make every assertion here untestable.

use super::*;
use crate::llm::anthropic_wire::{ClaudeMessage, ClaudeRequest, ClaudeTool};

fn tool(name: &str, description: &str) -> ClaudeTool {
    ClaudeTool {
        name: name.into(),
        description: description.into(),
        input_schema: serde_json::json!({"type": "object"}),
        cache_control: None,
    }
}

fn marked(mut tool: ClaudeTool) -> ClaudeTool {
    tool.cache_control = Some(serde_json::json!({"type": "ephemeral"}));
    tool
}

fn marked_block(text: &str) -> serde_json::Value {
    serde_json::json!({"type": "text", "text": text, "cache_control": {"type": "ephemeral"}})
}

fn message(role: &str, content: serde_json::Value) -> ClaudeMessage {
    ClaudeMessage {
        role: role.into(),
        content,
    }
}

/// A request shaped the way `build_claude_request` shapes one: a marker on the
/// last tool, on the system block, and on the last message's last block.
fn request(tools: Vec<ClaudeTool>, messages: Vec<ClaudeMessage>) -> ClaudeRequest {
    ClaudeRequest {
        anthropic_version: Some("vertex-2023-10-16".into()),
        model: None,
        max_tokens: 32768,
        stream: true,
        system: Some(serde_json::Value::Array(vec![marked_block("system body")])),
        messages,
        tools: Some(tools),
        thinking: None,
        output_config: None,
        anthropic_beta: None,
    }
}

fn two_tools() -> Vec<ClaudeTool> {
    vec![
        tool("read_file", "read a file"),
        marked(tool("todo", "todos")),
    ]
}

fn one_turn() -> Vec<ClaudeMessage> {
    vec![
        message("user", serde_json::Value::String("first turn".into())),
        message("assistant", serde_json::Value::String("answer".into())),
        message("user", serde_json::json!([marked_block("follow-up")])),
    ]
}

const URL: &str = "https://aiplatform.eu.rep.googleapis.com/v1/projects/p/locations/eu/x";

fn line(request: &ClaudeRequest) -> String {
    request_line(request, "claude-opus-5@default[1m]", URL, "Vertex", None)
}

/// Pull one `key=value` field out of a probe line. Panics with the whole line
/// on a miss, so a renamed field fails loudly rather than silently.
fn field<'a>(line: &'a str, key: &str) -> &'a str {
    let prefix = format!("{key}=");
    line.split_whitespace()
        .find_map(|kv| kv.strip_prefix(prefix.as_str()))
        .unwrap_or_else(|| panic!("no field {key} in: {line}"))
}

#[test]
fn probe_is_off_unless_the_var_says_otherwise() {
    // The default matters more than the parsing: an unset var must leave the
    // probe fully inert on every Claude call in every workspace.
    assert!(!probe_enabled_value(None));
    assert!(!probe_enabled_value(Some("")));
    assert!(!probe_enabled_value(Some("0")));
    assert!(!probe_enabled_value(Some("false")));
    assert!(!probe_enabled_value(Some("off")));
    for on in ["1", "true", "yes", "on", " 1 "] {
        assert!(probe_enabled_value(Some(on)), "expected on for {on:?}");
    }
}

#[test]
fn identical_requests_produce_an_identical_line() {
    // The whole method is diffing two consecutive calls, so a hash that moved
    // on unchanged input would make every comparison meaningless.
    assert_eq!(
        line(&request(two_tools(), one_turn())),
        line(&request(two_tools(), one_turn()))
    );
}

#[test]
fn reordering_tools_changes_the_hash_and_not_the_byte_count() {
    // The exact shape a `char_count` comparison cannot see, and the reason the
    // probe hashes bytes instead of counting them. Same tools, same total
    // size, different prefix, so the cache lookup cannot match.
    let forward = line(&request(
        vec![
            tool("read_file", "read a file"),
            marked(tool("todo", "todos")),
        ],
        one_turn(),
    ));
    let swapped = line(&request(
        vec![
            tool("todo", "todos"),
            marked(tool("read_file", "read a file")),
        ],
        one_turn(),
    ));

    assert_ne!(field(&forward, "tools_hash"), field(&swapped, "tools_hash"));
    assert_ne!(
        field(&forward, "tool_names_hash"),
        field(&swapped, "tool_names_hash")
    );
    assert_eq!(
        field(&forward, "tools_bytes"),
        field(&swapped, "tools_bytes"),
        "a reorder is invisible to a byte count"
    );
}

#[test]
fn a_changed_schema_moves_the_body_hash_and_not_the_name_hash() {
    // Separates "the tool order changed" from "a tool schema changed", so one
    // log line says which of the two happened.
    let before = line(&request(two_tools(), one_turn()));
    let after = line(&request(
        vec![
            tool("read_file", "read a file, now with flags"),
            marked(tool("todo", "todos")),
        ],
        one_turn(),
    ));

    assert_ne!(field(&before, "tools_hash"), field(&after, "tools_hash"));
    assert_eq!(
        field(&before, "tool_names_hash"),
        field(&after, "tool_names_hash")
    );
}

#[test]
fn census_reports_the_three_intended_breakpoints_by_position() {
    let line = line(&request(two_tools(), one_turn()));

    assert_eq!(field(&line, "marker_count"), "3");
    assert_eq!(field(&line, "markers"), "tools[1],system[0],messages[2][0]");
    assert_eq!(field(&line, "tools_prefix"), "1");
    assert_eq!(field(&line, "msgs_prefix"), "2");
    assert_eq!(field(&line, "tools_last"), "todo");
}

#[test]
fn census_sees_a_marker_left_behind_in_history() {
    // The 4-breakpoint limit is a live suspect. A scan of only the last message
    // would report 3 here and hide exactly the accumulation we are looking for.
    let messages = vec![
        message("user", serde_json::json!([marked_block("an earlier turn")])),
        message("assistant", serde_json::Value::String("answer".into())),
        message("user", serde_json::json!([marked_block("follow-up")])),
    ];
    let line = line(&request(two_tools(), messages));

    assert_eq!(field(&line, "marker_count"), "4");
    assert_eq!(
        field(&line, "markers"),
        "tools[1],system[0],messages[0][0],messages[2][0]"
    );
    // The prefix still ends at the LAST marked message, which is what
    // Anthropic looks up.
    assert_eq!(field(&line, "msgs_prefix"), "2");
}

#[test]
fn a_missing_breakpoint_reads_as_absent_rather_than_as_zero() {
    // A marker that silently stopped being applied is itself a finding, so an
    // empty prefix must not look like a prefix of one element.
    let unmarked = vec![tool("read_file", "read a file"), tool("todo", "todos")];
    let line = line(&request(unmarked, one_turn()));

    assert_eq!(field(&line, "tools_prefix"), "-");
    assert_eq!(field(&line, "tools_last"), "-");
    assert_eq!(field(&line, "tools_bytes"), "2", "an empty JSON array");
    assert_eq!(field(&line, "marker_count"), "2");
}

#[test]
fn the_messages_prefix_stops_at_the_marker() {
    // Everything after the last breakpoint is uncached by construction, so it
    // must not be folded into the hash we compare across calls.
    let short = request(two_tools(), one_turn());
    let mut long = request(two_tools(), one_turn());
    long.messages.push(message(
        "assistant",
        serde_json::Value::String("later".into()),
    ));

    let (short, long) = (line(&short), line(&long));
    assert_eq!(field(&short, "msgs_hash"), field(&long, "msgs_hash"));
    assert_eq!(field(&short, "msgs_bytes"), field(&long, "msgs_bytes"));
    // The count still reports the whole array, so the growth stays visible.
    assert_eq!(field(&short, "msgs_n"), "3");
    assert_eq!(field(&long, "msgs_n"), "4");
}

#[test]
fn transport_facts_reach_the_line() {
    let mut req = request(two_tools(), one_turn());
    req.anthropic_beta = Some(vec!["context-1m-2025-08-07".into()]);
    let line = line(&req);

    assert_eq!(field(&line, "model"), "claude-opus-5@default[1m]");
    assert_eq!(field(&line, "host"), "aiplatform.eu.rep.googleapis.com");
    assert_eq!(field(&line, "anthropic_version"), "vertex-2023-10-16");
    assert_eq!(field(&line, "anthropic_beta"), "context-1m-2025-08-07");
    assert_eq!(field(&line, "provider"), "Vertex");
}

#[test]
fn a_direct_body_reports_its_absent_fields_rather_than_omitting_them() {
    // Direct framing carries the version and betas as HTTP headers, so the
    // body has neither. A missing key would break a mechanical diff; a `-`
    // does not.
    let mut req = request(two_tools(), one_turn());
    req.anthropic_version = None;
    req.model = Some("claude-opus-5".into());
    let line = request_line(
        &req,
        "claude-opus-5[1m]",
        "https://api.anthropic.com/v1/messages",
        "Anthropic",
        None,
    );

    assert_eq!(field(&line, "anthropic_version"), "-");
    assert_eq!(field(&line, "anthropic_beta"), "-");
    assert_eq!(field(&line, "host"), "api.anthropic.com");
    // The `[1m]` suffix survives, which is what determines the Direct beta
    // header the body cannot show.
    assert_eq!(field(&line, "model"), "claude-opus-5[1m]");
}

#[test]
fn request_and_response_lines_join_on_the_same_triple() {
    let call = ProbeCall {
        thread_id: Uuid::nil(),
        turn_id: Uuid::from_u128(7),
        round: 1,
    };
    let meta = TurnMeta {
        cache_creation_tokens: Some(67_500),
        input_tokens: Some(67_500),
        output_tokens: Some(12),
        ..TurnMeta::default()
    };

    let req = request_line(
        &request(two_tools(), one_turn()),
        "m",
        URL,
        "Vertex",
        Some(call),
    );
    let resp = response_line(&meta, "Vertex", Some(call));

    for key in ["thread", "turn", "round", "first_of_turn"] {
        assert_eq!(field(&req, key), field(&resp, key), "field {key}");
    }
    assert_eq!(field(&req, "first_of_turn"), "true");
    assert_eq!(field(&resp, "cache_read"), "0");
    assert_eq!(field(&resp, "cache_creation"), "67500");
}

/// The zero-read case is the whole point of the probe, and it is the one the
/// parser cannot express: `process_sse_data` records a cache field only when
/// it is non-zero, so a real zero arrives as `None`. Rendering that as `-`
/// would hide every confirmed miss behind the same glyph as "no usage
/// reported", and the line would never once say `cache_read=0`.
#[test]
fn a_real_zero_read_prints_zero_and_not_the_absent_glyph() {
    let meta = TurnMeta {
        cache_creation_tokens: Some(67_500),
        input_tokens: Some(67_500),
        ..TurnMeta::default()
    };
    let resp = response_line(&meta, "Vertex", None);

    assert_eq!(field(&resp, "cache_read"), "0");
    assert_eq!(field(&resp, "cache_creation"), "67500");
}

#[test]
fn a_stream_that_reported_no_usage_stays_absent() {
    // No `message_start`, so nothing is known. That must not read as a
    // confirmed zero, which is a finding rather than an absence.
    let resp = response_line(&TurnMeta::default(), "Vertex", None);

    assert_eq!(field(&resp, "cache_read"), "-");
    assert_eq!(field(&resp, "cache_creation"), "-");
    assert_eq!(field(&resp, "input"), "-");
}

#[test]
fn a_later_round_is_not_first_of_turn() {
    let call = ProbeCall {
        thread_id: Uuid::nil(),
        turn_id: Uuid::from_u128(7),
        round: 2,
    };
    let resp = response_line(&TurnMeta::default(), "Vertex", Some(call));

    assert_eq!(field(&resp, "round"), "2");
    assert_eq!(field(&resp, "first_of_turn"), "false");
}

#[test]
fn a_call_outside_a_turn_says_so() {
    // Memory extraction and web search reach the same chokepoint with no turn.
    let resp = response_line(&TurnMeta::default(), "Anthropic", None);

    assert_eq!(field(&resp, "thread"), "-");
    assert_eq!(field(&resp, "round"), "-");
    assert_eq!(field(&resp, "first_of_turn"), "-");
    assert_eq!(field(&resp, "cache_read"), "-");
}

#[test]
fn host_of_survives_shapes_that_are_not_urls() {
    assert_eq!(
        host_of("https://api.anthropic.com/v1/messages"),
        "api.anthropic.com"
    );
    assert_eq!(host_of("https://host.example"), "host.example");
    assert_eq!(host_of("host.example/path"), "host.example");
    assert_eq!(host_of(""), "");
}
