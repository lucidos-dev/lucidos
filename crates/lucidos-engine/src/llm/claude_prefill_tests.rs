use super::*;
use crate::llm::anthropic_wire::{build_claude_request, WireTarget};
use crate::llm::provider::{ContentBlock, Message, MessageContent, ToolDefinition};

const MODEL: &str = "claude-opus-5-5";
const URL: &str = "https://example.invalid/v1/messages";

fn tools() -> Vec<ToolDefinition> {
    ["read_file", "write_file"]
        .iter()
        .map(|name| ToolDefinition {
            name: name.to_string(),
            description: format!("{name} a file"),
            parameters: serde_json::json!({"type": "object"}),
        })
        .collect()
}

fn text(role: &str, text: &str) -> Message {
    Message {
        role: role.into(),
        content: MessageContent::Text(text.into()),
    }
}

/// A turn's user message. A Tree turn leads with the snapshot's view block
/// and two thread view rungs.
fn user_message(tree: bool) -> Message {
    let mut blocks = Vec::new();
    if tree {
        for view in [
            "[WORKSPACE MEMORY VIEW]\n[w/0+4] shared\n",
            "[THREAD MEMORY VIEW]\n[0+8] old\n",
            "[8+4] newer\n",
        ] {
            blocks.push(ContentBlock::MemoryView { text: view.into() });
        }
    }
    blocks.push(ContentBlock::StableContext {
        text: "[PROFILE] the user".into(),
    });
    blocks.push(ContentBlock::Text {
        text: "Request: hello".into(),
    });
    Message {
        role: "user".into(),
        content: MessageContent::Blocks(blocks),
    }
}

/// The prior turn's tool pairs, which lead the messages of a resumed turn.
fn resume_blocks() -> Vec<Message> {
    vec![
        text("assistant", "reading it"),
        text("user", "[tool result]"),
        text("assistant", "reading the next"),
        text("user", "[tool result]"),
    ]
}

fn request(messages: Vec<Message>, model: &str, effort: &str) -> ClaudeRequest {
    build_claude_request(
        messages,
        tools(),
        model,
        Some("system prompt body"),
        Some(effort),
        WireTarget::Direct { url: URL },
        "Anthropic",
    )
    .0
}

fn route() -> TurnRoute<'static> {
    TurnRoute {
        wire_model: MODEL,
        reasoning_effort: Some("high"),
        betas: &["a-beta"],
    }
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("a body is JSON")
}

/// What the turn itself sent, as JSON.
fn sent(request: &ClaudeRequest) -> Value {
    serde_json::to_value(request).unwrap()
}

/// The block a beat appends to the last message.
fn beat_block() -> Value {
    serde_json::json!({"type": "text", "text": BEAT_PROMPT})
}

/// `message`'s content with the beat block after it.
fn with_beat_block(message: &mut Value) {
    let blocks = message["content"].as_array_mut().expect("block content");
    blocks.push(beat_block());
}

fn markers(value: &Value) -> usize {
    match value {
        Value::Object(fields) => {
            usize::from(fields.contains_key("cache_control"))
                + fields.values().map(markers).sum::<usize>()
        }
        Value::Array(items) => items.iter().map(markers).sum(),
        _ => 0,
    }
}

#[test]
fn the_whole_cut_is_the_turn_with_one_block_after_its_last_marker() {
    let turn = request(vec![user_message(true)], MODEL, "high");
    let pair = pair(&turn, route()).expect("adaptive thinking beats");
    let whole = json(&pair.whole.body);
    let mut expected = sent(&turn);
    with_beat_block(
        expected["messages"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap(),
    );
    assert_eq!(whole, expected);
    assert_eq!(whole["max_tokens"], sent(&turn)["max_tokens"]);
    assert_eq!(whole["stream"], sent(&turn)["stream"]);
    assert_eq!(whole["thinking"]["type"], "adaptive");
    assert_eq!(whole["output_config"]["effort"], "high");
    assert_eq!(markers(&whole), markers(&sent(&turn)), "markers kept");
}

#[test]
fn both_bodies_carry_the_turn_route_and_read_to_message_start() {
    let turn = request(vec![user_message(true)], MODEL, "high");
    let pair = pair(&turn, route()).unwrap();
    for body in [&pair.stable, &pair.whole] {
        assert_eq!(body.wire_model, MODEL);
        assert_eq!(body.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(body.betas, vec!["a-beta"]);
        assert_eq!(
            body.output_cap, None,
            "a beat that hangs up has no output to cap"
        );
        assert_eq!(body.reply, ReplyShape::MessageStart);
        assert_eq!(body.provider, None, "only the router knows the backend");
    }
}

#[test]
fn a_forced_tool_keeps_its_tool_choice() {
    let mut turn = request(vec![user_message(false)], "claude-opus-5", "high");
    turn.force_tool("read_file", "claude-opus-5").unwrap();
    let pair = pair(&turn, route()).unwrap();
    for body in [&pair.stable, &pair.whole] {
        assert_eq!(json(&body.body)["tool_choice"]["name"], "read_file");
    }
}

/// A first round marks every view block, so the stable cut ends at the
/// stable-prefix end, and holds exactly the bytes the turn sent there.
#[test]
fn the_stable_cut_ends_at_the_stable_prefix_end() {
    let turn = request(vec![user_message(true)], MODEL, "high");
    let (message, block) = turn.stable_prefix_end.expect("a Tree turn has views");
    let stable = json(&pair(&turn, route()).unwrap().stable.body);

    let mut expected = sent(&turn);
    expected["messages"]
        .as_array_mut()
        .unwrap()
        .truncate(message + 1);
    expected["messages"][message]["content"]
        .as_array_mut()
        .unwrap()
        .truncate(block + 1);
    with_beat_block(&mut expected["messages"][message]);
    assert_eq!(stable, expected);
    assert!(stable["messages"][message]["content"][block]
        .get("cache_control")
        .is_some());
}

/// The stable cut fingerprints like the turn: the same content, through the
/// same block, whatever the markers. The beat block after it is no part of
/// the stable prefix.
#[test]
fn the_stable_cut_matches_the_turn_fingerprint() {
    let turn = request(vec![user_message(true)], MODEL, "high");
    let mut stable = json(&pair(&turn, route()).unwrap().stable.body);
    let (message, block) = turn.stable_prefix_end.unwrap();
    let blocks = stable["messages"][message]["content"]
        .as_array_mut()
        .unwrap();
    assert_eq!(blocks.pop(), Some(beat_block()));
    let mut rebuilt = request(vec![user_message(true)], MODEL, "high");
    rebuilt.messages.truncate(message + 1);
    let Value::Array(blocks) = &mut rebuilt.messages[message].content else {
        panic!("a Tree message is blocks");
    };
    blocks.truncate(block + 1);
    assert_eq!(stable["messages"], sent(&rebuilt)["messages"]);
    assert_eq!(
        crate::llm::stable_prefix::fingerprint(&turn, MODEL),
        crate::llm::stable_prefix::fingerprint(&rebuilt, MODEL),
    );
}

/// A resumed turn spends two markers on its newest messages, so its last
/// view block goes unmarked. The cut stops at the last marked one, where
/// the turn wrote an entry, rather than write one of its own.
#[test]
fn the_stable_cut_stops_at_the_last_marked_block() {
    let mut messages = resume_blocks();
    messages.push(user_message(true));
    let turn = request(messages, MODEL, "high");
    let (message, block) = turn.stable_prefix_end.unwrap();
    let sent = sent(&turn);
    assert!(
        sent["messages"][message]["content"][block]
            .get("cache_control")
            .is_none(),
        "the newest view block is unmarked"
    );

    let stable = json(&pair(&turn, route()).unwrap().stable.body);
    let kept = stable["messages"].as_array().unwrap();
    assert_eq!(kept.len(), message + 1);
    let last = kept[message]["content"].as_array().unwrap();
    assert_eq!(last.len(), block + 1, "one block short of the prefix end");
    assert!(last[block - 1].get("cache_control").is_some());
    assert_eq!(last[block], beat_block());
}

/// A Classic turn has no view blocks, so its stable prefix ends at the
/// system block. The cut sends that, and one plain user message that ends
/// with the beat block.
#[test]
fn a_classic_cut_ends_at_the_system_block() {
    let turn = request(vec![user_message(false)], MODEL, "high");
    assert_eq!(turn.stable_prefix_end, None);
    let stable = json(&pair(&turn, route()).unwrap().stable.body);
    let sent = sent(&turn);
    assert_eq!(stable["system"], sent["system"]);
    assert!(stable["system"][0].get("cache_control").is_some());
    assert_eq!(stable["tools"], sent["tools"]);
    assert_eq!(
        stable["messages"],
        serde_json::json!([{
            "role": "user",
            "content": [{"type": "text", "text": "."}, beat_block()]
        }])
    );
    assert_eq!(stable["max_tokens"], sent["max_tokens"]);
}

/// Every parameter of the turn stays, so the beat shares its cache pool.
/// With the budget under `max_tokens`, budget thinking beats too.
#[test]
fn budget_thinking_beats_with_its_budget_kept() {
    let turn = request(vec![user_message(true)], "claude-sonnet-4-5", "high");
    assert_eq!(
        turn.thinking.as_ref().map(|t| t.thinking_type.as_str()),
        Some("enabled")
    );
    let whole = json(
        &pair(&turn, route())
            .expect("budget thinking beats")
            .whole
            .body,
    );
    assert_eq!(whole["thinking"], sent(&turn)["thinking"]);
    assert_eq!(whole["max_tokens"], sent(&turn)["max_tokens"]);
}

/// The beat block goes in the user's last message, so a request that ends
/// on the assistant's has nowhere to put it.
#[test]
fn a_request_ending_on_the_assistant_never_beats() {
    let turn = request(
        vec![user_message(true), text("assistant", "done")],
        MODEL,
        "high",
    );
    assert!(pair(&turn, route()).is_none());
}

#[test]
fn thinking_turned_off_still_beats() {
    let turn = request(vec![user_message(true)], "claude-opus-5", "none");
    assert_eq!(
        turn.thinking.as_ref().map(|t| t.thinking_type.as_str()),
        Some("disabled")
    );
    assert!(pair(&turn, route()).is_some());
}
