use super::*;
use crate::llm::provider::{ContentBlock, MessageContent};

#[test]
fn message_content_to_claude_value_filters_empty_text_blocks() {
    // When pasting images without text, empty text blocks must be filtered
    // or the Claude API rejects with "text content blocks must be non-empty"
    let content = MessageContent::Blocks(vec![
        ContentBlock::Text {
            text: String::new(),
        },
        ContentBlock::Image {
            source_type: "base64".to_string(),
            media_type: "image/png".to_string(),
            data: "AAAA".to_string(),
        },
    ]);
    let value = VertexProvider::message_content_to_claude_value(&content);
    let arr = value.as_array().unwrap();
    // Empty text block should be filtered out, leaving only the image
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["type"], "image");
}

#[test]
fn message_content_to_claude_value_keeps_nonempty_text() {
    let content = MessageContent::Blocks(vec![
        ContentBlock::Text {
            text: "describe this".to_string(),
        },
        ContentBlock::Image {
            source_type: "base64".to_string(),
            media_type: "image/png".to_string(),
            data: "AAAA".to_string(),
        },
    ]);
    let value = VertexProvider::message_content_to_claude_value(&content);
    let arr = value.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[0]["text"], "describe this");
    assert_eq!(arr[1]["type"], "image");
}

#[test]
fn process_sse_captures_input_tokens_from_message_start() {
    // Anthropic streams `message_start` early in every response with the
    // exact prompt-token cost. Capturing it lets the UI replace the
    // chars/4 estimate (which over-counts base64 image bytes by orders
    // of magnitude) with the real number.
    let mut blocks = Vec::new();
    let mut meta = TurnMeta::default();
    let event = r#"{"type":"message_start","message":{"id":"msg_x","type":"message","role":"assistant","content":[],"model":"claude-opus-4-7","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":4321,"cache_creation_input_tokens":1000,"cache_read_input_tokens":500,"output_tokens":1}}}"#;

    VertexProvider::process_sse_data(event, &mut blocks, &mut meta).unwrap();

    // Real prompt size = uncached input + cache writes + cache reads
    // (everything the model actually processed). 4321 + 1000 + 500 = 5821.
    assert_eq!(meta.input_tokens, Some(5821));
    // Cache breakdown survives separately so the modal can show hit rate.
    assert_eq!(meta.cache_creation_tokens, Some(1000));
    assert_eq!(meta.cache_read_tokens, Some(500));
}

#[test]
fn system_with_cache_control_none_returns_none() {
    assert!(system_with_cache_control(None).is_none());
}

#[test]
fn system_with_cache_control_empty_string_returns_none() {
    assert!(system_with_cache_control(Some("")).is_none());
}

#[test]
fn system_with_cache_control_wraps_string_in_block_with_marker() {
    let value = system_with_cache_control(Some("you are a helpful assistant")).unwrap();
    let arr = value.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[0]["text"], "you are a helpful assistant");
    assert_eq!(arr[0]["cache_control"]["type"], "ephemeral");
}

#[test]
fn apply_cache_control_to_last_tool_marks_only_last() {
    let mut tools = vec![
        ClaudeTool {
            name: "a".into(),
            description: "first".into(),
            input_schema: serde_json::json!({}),
            cache_control: None,
        },
        ClaudeTool {
            name: "b".into(),
            description: "second".into(),
            input_schema: serde_json::json!({}),
            cache_control: None,
        },
    ];
    apply_cache_control_to_last_tool(&mut tools);
    assert!(tools[0].cache_control.is_none());
    assert_eq!(
        tools[1].cache_control.as_ref().unwrap()["type"],
        "ephemeral"
    );
}

#[test]
fn apply_cache_control_to_last_tool_empty_is_noop() {
    let mut tools: Vec<ClaudeTool> = Vec::new();
    apply_cache_control_to_last_tool(&mut tools);
    assert!(tools.is_empty());
}

#[test]
fn apply_cache_control_to_last_message_string_content_becomes_block() {
    let mut messages = vec![ClaudeMessage {
        role: "user".into(),
        content: serde_json::Value::String("hello there".into()),
    }];
    apply_cache_control_to_last_message(&mut messages);
    let arr = messages[0].content.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[0]["text"], "hello there");
    assert_eq!(arr[0]["cache_control"]["type"], "ephemeral");
}

#[test]
fn apply_cache_control_to_last_message_array_content_marks_last_block_only() {
    let mut messages = vec![ClaudeMessage {
        role: "user".into(),
        content: serde_json::json!([
            {"type": "text", "text": "first block"},
            {"type": "text", "text": "second block"},
        ]),
    }];
    apply_cache_control_to_last_message(&mut messages);
    let arr = messages[0].content.as_array().unwrap();
    assert!(arr[0].get("cache_control").is_none());
    assert_eq!(arr[1]["cache_control"]["type"], "ephemeral");
}

#[test]
fn apply_cache_control_to_last_message_only_touches_final_message() {
    let mut messages = vec![
        ClaudeMessage {
            role: "user".into(),
            content: serde_json::Value::String("first turn".into()),
        },
        ClaudeMessage {
            role: "assistant".into(),
            content: serde_json::Value::String("second turn".into()),
        },
    ];
    apply_cache_control_to_last_message(&mut messages);
    // First message untouched (still a bare string)
    assert!(messages[0].content.is_string());
    // Last message converted to a block array with cache_control
    assert!(messages[1].content.is_array());
}

#[test]
fn apply_cache_control_to_last_message_empty_is_noop() {
    let mut messages: Vec<ClaudeMessage> = Vec::new();
    apply_cache_control_to_last_message(&mut messages);
    assert!(messages.is_empty());
}

#[test]
fn apply_cache_control_to_last_message_skips_empty_string() {
    // An empty string would round-trip into an empty text block, which
    // Anthropic rejects. Cache_control on nothing is meaningless anyway.
    let mut messages = vec![ClaudeMessage {
        role: "user".into(),
        content: serde_json::Value::String(String::new()),
    }];
    apply_cache_control_to_last_message(&mut messages);
    // Untouched
    assert!(messages[0].content.is_string());
    assert_eq!(messages[0].content.as_str(), Some(""));
}

#[test]
fn cache_control_serializes_into_wire_format() {
    // End-to-end: build a request the way chat_claude does, serialize it,
    // and check cache_control lands on tools[-1], the system block, and
    // messages[-1]'s last content block.
    let mut tools = vec![
        ClaudeTool {
            name: "search".into(),
            description: "search the web".into(),
            input_schema: serde_json::json!({"type": "object"}),
            cache_control: None,
        },
        ClaudeTool {
            name: "calculator".into(),
            description: "do math".into(),
            input_schema: serde_json::json!({"type": "object"}),
            cache_control: None,
        },
    ];
    apply_cache_control_to_last_tool(&mut tools);

    let mut messages = vec![
        ClaudeMessage {
            role: "user".into(),
            content: serde_json::Value::String("first turn".into()),
        },
        ClaudeMessage {
            role: "assistant".into(),
            content: serde_json::Value::String("response".into()),
        },
        ClaudeMessage {
            role: "user".into(),
            content: serde_json::Value::String("follow-up".into()),
        },
    ];
    apply_cache_control_to_last_message(&mut messages);

    let req = ClaudeRequest {
        anthropic_version: "vertex-2023-10-16".into(),
        max_tokens: 1024,
        stream: true,
        system: system_with_cache_control(Some("system prompt body")),
        messages,
        tools: Some(tools),
        thinking: None,
        output_config: None,
        anthropic_beta: None,
    };

    let json = serde_json::to_value(&req).unwrap();

    // Tools: only the last one carries cache_control
    let tools_arr = json["tools"].as_array().unwrap();
    assert!(tools_arr[0].get("cache_control").is_none());
    assert_eq!(tools_arr[1]["cache_control"]["type"], "ephemeral");

    // System: array form with cache_control on its single block
    let system_arr = json["system"].as_array().unwrap();
    assert_eq!(system_arr[0]["cache_control"]["type"], "ephemeral");

    // Messages: only the final message's last block carries cache_control
    let msgs = json["messages"].as_array().unwrap();
    assert!(msgs[0]["content"].is_string());
    assert!(msgs[1]["content"].is_string());
    let last_blocks = msgs[2]["content"].as_array().unwrap();
    assert_eq!(
        last_blocks.last().unwrap()["cache_control"]["type"],
        "ephemeral"
    );
}

#[test]
fn process_sse_captures_redacted_thinking_data_field() {
    // redacted_thinking blocks carry their (encrypted) payload in
    // `data`, not `thinking`. Reading `thinking` produced 0 chars even
    // though the model spent output tokens on the block — the engine
    // then surfaced "no response" with a misleading hint. Capturing the
    // data length keeps thinking_chars non-zero so the empty-completion
    // diagnostic can distinguish encrypted reasoning from true silence.
    let mut blocks = Vec::new();
    let mut meta = TurnMeta::default();
    let event = r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"AAAAAA=="}}"#;

    VertexProvider::process_sse_data(event, &mut blocks, &mut meta).unwrap();

    match &blocks[0] {
        AccumulatedBlock::Thinking(s) => assert_eq!(s, "AAAAAA=="),
        _ => panic!("redacted_thinking must bucket as Thinking"),
    }
}

#[test]
fn process_sse_increments_unknown_for_new_block_type() {
    // When Anthropic emits a block type the parser doesn't recognize,
    // every delta for that block falls through silently and the model's
    // output tokens disappear from the LlmResponse. Tracking the count
    // lets the empty-completion diagnostic say "engine dropped unknown
    // SSE shapes" instead of "model decided no action was needed".
    let mut blocks = Vec::new();
    let mut meta = TurnMeta::default();
    let event = r#"{"type":"content_block_start","index":0,"content_block":{"type":"some_new_block_type"}}"#;

    VertexProvider::process_sse_data(event, &mut blocks, &mut meta).unwrap();

    assert_eq!(meta.unknown_sse_dropped, 1);
}

#[test]
fn process_sse_increments_unknown_for_new_delta_type() {
    let mut blocks = Vec::new();
    let mut meta = TurnMeta::default();
    // Start a text block so the index is populated, then send a delta
    // type the parser doesn't recognize.
    VertexProvider::process_sse_data(
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        &mut blocks,
        &mut meta,
    )
    .unwrap();
    VertexProvider::process_sse_data(
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"future_delta_type","value":"ignored"}}"#,
        &mut blocks,
        &mut meta,
    )
    .unwrap();

    assert_eq!(meta.unknown_sse_dropped, 1);
}

#[test]
fn process_sse_signature_delta_is_known_quiet() {
    // signature_delta arrives on every thinking block to sign it. It
    // carries no user-visible content and the parser intentionally
    // ignores it — must NOT count as a dropped unknown shape.
    let mut blocks = Vec::new();
    let mut meta = TurnMeta::default();
    VertexProvider::process_sse_data(
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        &mut blocks,
        &mut meta,
    )
    .unwrap();
    VertexProvider::process_sse_data(
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"abc"}}"#,
        &mut blocks,
        &mut meta,
    )
    .unwrap();

    assert_eq!(meta.unknown_sse_dropped, 0);
}
