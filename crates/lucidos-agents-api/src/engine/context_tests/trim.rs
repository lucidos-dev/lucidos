use super::*;
use crate::llm::{ContentBlock, Message, MessageContent};

static NOTHING: std::sync::LazyLock<ProtectedAddresses> =
    std::sync::LazyLock::new(ProtectedAddresses::new);

fn plain() -> TrimGuards<'static> {
    TrimGuards {
        protected: &NOTHING,
        held_open: &NOTHING,
        recovery: RecoveryClause::State,
    }
}

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
            thought_signature: None,
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

fn view_image_result_msg(tool_use_id: &str, image_kb: usize) -> Message {
    Message {
        role: "user".to_string(),
        content: MessageContent::Blocks(vec![
            ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: crate::engine::tools::files::EXPLICIT_IMAGE_RESULT_TEXT.to_string(),
            },
            ContentBlock::Image {
                source_type: "base64".to_string(),
                media_type: "image/png".to_string(),
                data: "x".repeat(image_kb * 1024),
            },
            ContentBlock::Text {
                text: "Results above.".to_string(),
            },
        ]),
    }
}

fn capture_app_result_msg(tool_use_id: &str, image_kb: usize) -> Message {
    Message {
        role: "user".to_string(),
        content: MessageContent::Blocks(vec![
            ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: "Habit Tracker\nStreak: 4 days".to_string(),
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
fn test_view_image_result_stays_visible_across_later_tool_calls() {
    use crate::engine::agentic_loop::holds_explicitly_requested_image;

    let mut messages = vec![
        text_msg("user", "Request: what is wrong in this screenshot?"),
        tool_use_msg("v1", "view_image", serde_json::json!({"image": "thread:3"})),
        view_image_result_msg("v1", 100),
    ];
    let view_idx = messages.len() - 1;

    let MessageContent::Blocks(blocks) = &messages[view_idx].content else {
        panic!("expected Blocks content for the view_image result");
    };
    assert!(
        holds_explicitly_requested_image(blocks),
        "the loop must recognise a view_image result as an explicitly-requested image"
    );

    for i in 0..4 {
        let id = format!("later{}", i);
        messages.push(tool_use_msg(
            &id,
            "grep_files",
            serde_json::json!({"pattern": "x"}),
        ));
        messages.push(tool_result_msg(&id, "match"));
    }

    trim_context_if_needed(&mut messages, 500_000, Some(0), &[0, view_idx], plain());

    let MessageContent::Blocks(blocks) = &messages[view_idx].content else {
        panic!("expected the view_image result to survive with its Blocks content");
    };
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. })),
        "a re-loaded thread image must stay visible for the rest of the turn, \
         not just until the model's next tool call"
    );
}

#[test]
fn test_capture_app_result_is_not_pinned() {
    use crate::engine::agentic_loop::holds_explicitly_requested_image;

    let mut messages = vec![
        text_msg("user", "Request: check the app"),
        tool_use_msg(
            "c1",
            "capture_app",
            serde_json::json!({"app_id": "habit-tracker"}),
        ),
        capture_app_result_msg("c1", 100),
    ];
    let capture_idx = messages.len() - 1;

    let MessageContent::Blocks(blocks) = &messages[capture_idx].content else {
        panic!("expected Blocks content for the capture_app result");
    };
    assert!(
        !holds_explicitly_requested_image(blocks),
        "an ambient capture must not be treated as an explicitly-requested image"
    );

    messages.push(tool_use_msg(
        "c2",
        "grep_files",
        serde_json::json!({"pattern": "x"}),
    ));
    messages.push(tool_result_msg("c2", "match"));

    trim_context_if_needed(&mut messages, 500_000, Some(0), &[0], plain());

    let MessageContent::Blocks(blocks) = &messages[capture_idx].content else {
        panic!("expected Blocks content at the capture message");
    };
    assert!(
        !blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. })),
        "an ambient capture must age out once it is no longer the last message"
    );
}
