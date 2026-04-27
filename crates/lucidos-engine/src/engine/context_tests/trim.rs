use super::*;
use crate::llm::{ContentBlock, Message, MessageContent};

fn text_msg(role: &str, text: &str) -> Message {
    Message {
        role: role.to_string(),
        content: MessageContent::Text(text.to_string()),
    }
}

fn tool_use_msg(id: &str, name: &str, input: serde_json::Value) -> Message {
    Message {
        role: "assistant".to_string(),
        content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input,
        }]),
    }
}

fn tool_result_msg(tool_use_id: &str, content: &str) -> Message {
    Message {
        role: "user".to_string(),
        content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: content.to_string(),
        }]),
    }
}

#[test]
fn test_no_trim_under_budget() {
    let mut messages = vec![
        text_msg("user", "Hello"),
        text_msg("assistant", "Hi there"),
        text_msg("user", "How are you?"),
    ];
    let original_len = messages.len();
    let removed = trim_context_if_needed(&mut messages, 500_000);
    assert_eq!(removed, 0);
    assert_eq!(messages.len(), original_len);
    // Content should be unchanged
    assert_eq!(messages[0].content.as_text(), "Hello");
}

#[test]
fn test_pass1_truncates_large_tool_results() {
    let large_content = "x".repeat(10_000);
    let mut messages = vec![
        text_msg("user", "initial"),
        tool_use_msg("t1", "read_file", serde_json::json!({"path": "foo.rs"})),
        tool_result_msg("t1", &large_content),
        tool_use_msg("t2", "read_file", serde_json::json!({"path": "bar.rs"})),
        tool_result_msg("t2", "small result"),
        // Recent 4 messages (preserved)
        tool_use_msg("t3", "write_file", serde_json::json!({"path": "out.rs"})),
        tool_result_msg("t3", "ok"),
        text_msg("assistant", "Done"),
        text_msg("user", "Thanks"),
    ];
    let original_len = messages.len();
    // Budget smaller than total (~10K) but large enough that pass 1 truncation suffices
    let removed = trim_context_if_needed(&mut messages, 5_000);

    assert_eq!(removed, 0, "pass 2 should not be needed");
    assert_eq!(messages.len(), original_len, "no messages removed");

    // The large tool result at index 2 should be truncated
    if let MessageContent::Blocks(blocks) = &messages[2].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert!(
                content.contains("truncated"),
                "large result should be truncated"
            );
            assert!(content.contains("10000"), "should mention original size");
        } else {
            panic!("expected ToolResult block");
        }
    } else {
        panic!("expected Blocks content");
    }

    // The small tool result at index 4 should be unchanged
    if let MessageContent::Blocks(blocks) = &messages[4].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert_eq!(content, "small result");
        }
    }
}

#[test]
fn test_pass2_removes_old_pairs() {
    // Create messages where even after truncation, we're over a tiny budget
    let mut messages = vec![text_msg("user", "initial message")];
    // Add 10 tool call pairs (20 messages)
    for i in 0..10 {
        let id = format!("t{}", i);
        messages.push(tool_use_msg(
            &id,
            "some_tool",
            serde_json::json!({"x": "y"}),
        ));
        messages.push(tool_result_msg(&id, "result content here that takes space"));
    }
    // Add 4 recent messages to preserve
    messages.push(tool_use_msg("recent1", "tool", serde_json::json!({})));
    messages.push(tool_result_msg("recent1", "recent result"));
    messages.push(text_msg("assistant", "final answer"));
    messages.push(text_msg("user", "ok"));

    // Set a very small budget to force pass 2
    let budget = 200;
    let removed = trim_context_if_needed(&mut messages, budget);

    assert!(removed > 0, "should have removed messages in pass 2");
    // message[0] should still be the initial user message
    assert_eq!(messages[0].content.as_text(), "initial message");
    // Last 4 should be preserved
    let len = messages.len();
    assert_eq!(messages[len - 1].content.as_text(), "ok");
    assert_eq!(messages[len - 2].content.as_text(), "final answer");
}

#[test]
fn test_preserves_initial_and_recent() {
    let large = "y".repeat(5_000);
    let mut messages = vec![
        text_msg("user", "important initial context"),
        tool_use_msg(
            "old1",
            "read_file",
            serde_json::json!({"content": large.clone()}),
        ),
        tool_result_msg("old1", &large),
        tool_use_msg(
            "old2",
            "read_file",
            serde_json::json!({"content": large.clone()}),
        ),
        tool_result_msg("old2", &large),
        // These 4 are the recent messages that must be preserved
        tool_use_msg(
            "new1",
            "write_file",
            serde_json::json!({"path": "out.rs", "content": "fn main() {}"}),
        ),
        tool_result_msg("new1", "File written successfully"),
        text_msg("assistant", "I've written the file for you"),
        text_msg("user", "Great, thanks"),
    ];

    // Budget that requires trimming the old pairs
    let budget = 1_000;
    trim_context_if_needed(&mut messages, budget);

    // message[0] must always be preserved
    assert_eq!(messages[0].content.as_text(), "important initial context");

    // Last 4 messages must be preserved
    let len = messages.len();
    assert!(len >= 5, "should have at least initial + 4 recent");
    assert_eq!(messages[len - 1].content.as_text(), "Great, thanks");
    assert_eq!(
        messages[len - 2].content.as_text(),
        "I've written the file for you"
    );

    if let MessageContent::Blocks(blocks) = &messages[len - 3].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert_eq!(content, "File written successfully");
        }
    }
}

fn image_msg(role: &str, text: &str, image_kb: usize) -> Message {
    Message {
        role: role.to_string(),
        content: MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: text.to_string(),
            },
            ContentBlock::Image {
                source_type: "base64".to_string(),
                media_type: "image/png".to_string(),
                data: "x".repeat(image_kb * 1024),
            },
        ]),
    }
}

#[test]
fn test_pass0_strips_images_from_older_messages() {
    let mut messages = vec![
        image_msg("user", "current screenshot", 100), // message[0] — keep
        text_msg("assistant", "I see the screenshot"),
        image_msg("user", "old screenshot", 200), // older — strip
        text_msg("assistant", "Got it"),
        text_msg("user", "final question"),
    ];

    // Budget large enough that no other trimming is needed after stripping
    trim_context_if_needed(&mut messages, 500_000);

    // message[0] should still have its image
    if let MessageContent::Blocks(blocks) = &messages[0].content {
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "message[0] image should be preserved"
        );
    }

    // message[2] (old screenshot) should have image replaced with placeholder text
    if let MessageContent::Blocks(blocks) = &messages[2].content {
        assert!(
            !blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "older image should be stripped"
        );
        assert!(
            blocks.iter().any(
                |b| matches!(b, ContentBlock::Text { text } if text.contains("image from earlier"))
            ),
            "should have placeholder text"
        );
    }

    // All 5 messages should still be present
    assert_eq!(messages.len(), 5);
}

#[test]
fn test_pass0_image_stripping_brings_under_budget() {
    // A conversation where images alone push it over budget
    let mut messages = vec![
        text_msg("user", "hello"),
        image_msg("user", "big screenshot", 300), // 300KB image in older msg
        text_msg("assistant", "I see it"),
        text_msg("user", "thanks"),
    ];

    let total_before: usize = messages.iter().map(estimate_message_chars).sum();
    assert!(total_before > 200_000, "should be over budget before trim");

    // Budget that's achievable only by stripping the image
    trim_context_if_needed(&mut messages, 200_000);

    let total_after: usize = messages.iter().map(estimate_message_chars).sum();
    assert!(
        total_after < 200_000,
        "should be under budget after stripping images"
    );
    // No messages removed — just image stripped
    assert_eq!(messages.len(), 4);
}
