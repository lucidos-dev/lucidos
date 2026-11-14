use super::*;
use crate::llm::provider::LlmResponse;

fn reply_calling(tool: &str) -> LlmResponse {
    serde_json::from_value(serde_json::json!({
        "content": "Let me check.",
        "tool_calls": [{ "id": "call_1", "name": tool, "arguments": { "path": "notes.md" } }],
    }))
    .unwrap()
}

/// A side question never runs a tool. The call is echoed back with a refusal,
/// so the next round has to answer from what it knows.
#[test]
fn a_tool_call_is_answered_with_a_refusal_and_never_run() {
    let mut messages = Vec::new();
    refuse_tool_calls(&mut messages, &reply_calling("read_file"));

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "assistant");
    let MessageContent::Blocks(asked) = &messages[0].content else {
        panic!("the assistant turn must carry blocks");
    };
    assert!(asked.iter().any(|block| matches!(
        block,
        ContentBlock::ToolUse { id, name, .. } if id == "call_1" && name == "read_file"
    )));
    assert_eq!(messages[1].role, "user");
    let MessageContent::Blocks(results) = &messages[1].content else {
        panic!("the refusal must be a tool result");
    };
    assert!(matches!(
        results.as_slice(),
        [ContentBlock::ToolResult { tool_use_id, content }]
            if tool_use_id == "call_1" && content == SIDE_QUESTION_TOOL_REFUSAL
    ));
}
