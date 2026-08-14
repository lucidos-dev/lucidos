use super::*;
use crate::llm::{ContentBlock, Message, MessageContent};

fn user_text(text: &str) -> Message {
    Message {
        role: "user".to_string(),
        content: MessageContent::Text(text.to_string()),
    }
}

fn assistant_with_tool_uses(ids: &[&str]) -> Message {
    let mut blocks = Vec::new();
    for id in ids {
        blocks.push(ContentBlock::ToolUse {
            id: id.to_string(),
            name: "test_tool".to_string(),
            input: serde_json::json!({}),
            thought_signature: None,
        });
    }
    Message {
        role: "assistant".to_string(),
        content: MessageContent::Blocks(blocks),
    }
}

fn user_with_tool_results(ids: &[&str]) -> Message {
    let blocks: Vec<ContentBlock> = ids
        .iter()
        .map(|id| ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: "result".to_string(),
        })
        .collect();
    Message {
        role: "user".to_string(),
        content: MessageContent::Blocks(blocks),
    }
}

#[test]
fn valid_pairing_returns_zero() {
    let mut messages = vec![
        user_text("hello"),
        assistant_with_tool_uses(&["t1", "t2"]),
        user_with_tool_results(&["t1", "t2"]),
    ];
    assert_eq!(validate_tool_use_pairing(&mut messages), 0);
}

#[test]
fn missing_tool_result_gets_stub() {
    let mut messages = vec![
        user_text("hello"),
        assistant_with_tool_uses(&["t1", "t2"]),
        // Only t1 has a result, t2 is missing
        user_with_tool_results(&["t1"]),
    ];
    let stubs = validate_tool_use_pairing(&mut messages);
    assert_eq!(stubs, 1, "should inject 1 stub for missing t2");

    // Verify t2 was added
    if let MessageContent::Blocks(blocks) = &messages[2].content {
        let result_ids: Vec<&str> = blocks
            .iter()
            .filter_map(|b| {
                if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                    Some(tool_use_id.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(
            result_ids.contains(&"t2"),
            "t2 should be injected: {:?}",
            result_ids
        );
    }
}

#[test]
fn orphaned_assistant_at_end_gets_user_message() {
    let mut messages = vec![
        user_text("hello"),
        assistant_with_tool_uses(&["t1"]),
        // No following user message!
    ];
    let stubs = validate_tool_use_pairing(&mut messages);
    assert_eq!(stubs, 1);
    assert_eq!(messages.len(), 3, "should have injected a user message");
    assert_eq!(messages[2].role, "user");
}

#[test]
fn interleaved_image_blocks_dont_break_matching() {
    // This was the original bug: Image blocks between ToolResult blocks
    // caused the API to miss subsequent ToolResults.
    // After the fix, ToolResults come first, but validate_tool_use_pairing
    // should still pass even if blocks are interleaved (it checks by ID).
    let mut messages = vec![
        user_text("hello"),
        assistant_with_tool_uses(&["t1", "t2", "t3"]),
        Message {
            role: "user".to_string(),
            content: MessageContent::Blocks(vec![
                ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    content: "ok".to_string(),
                },
                ContentBlock::Text {
                    text: "[image placeholder]".to_string(),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "t2".to_string(),
                    content: "ok".to_string(),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "t3".to_string(),
                    content: "ok".to_string(),
                },
            ]),
        },
    ];
    assert_eq!(
        validate_tool_use_pairing(&mut messages),
        0,
        "all 3 tool_results are present even though interleaved with text"
    );
}

#[test]
fn multiple_pairs_validated() {
    let mut messages = vec![
        user_text("hello"),
        assistant_with_tool_uses(&["t1"]),
        user_with_tool_results(&["t1"]),
        assistant_with_tool_uses(&["t2", "t3"]),
        user_with_tool_results(&["t2", "t3"]),
    ];
    assert_eq!(validate_tool_use_pairing(&mut messages), 0);
}

#[test]
fn text_only_assistant_skipped() {
    let mut messages = vec![
        user_text("hello"),
        Message {
            role: "assistant".to_string(),
            content: MessageContent::Text("just text, no tools".to_string()),
        },
        user_text("thanks"),
    ];
    assert_eq!(validate_tool_use_pairing(&mut messages), 0);
}

#[test]
fn consecutive_orphaned_assistants_all_get_stubs() {
    // Two consecutive assistant messages with no user messages between them.
    // Both must get stub user messages inserted.
    let mut messages = vec![
        user_text("hello"),
        assistant_with_tool_uses(&["t1"]),
        assistant_with_tool_uses(&["t2"]),
    ];
    let stubs = validate_tool_use_pairing(&mut messages);
    assert_eq!(stubs, 2, "both orphaned assistants should get stubs");
    assert_eq!(messages.len(), 5, "should have 2 injected user messages");
    // Verify structure: user, assistant(t1), user(stub), assistant(t2), user(stub)
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[2].role, "user");
    assert_eq!(messages[3].role, "assistant");
    assert_eq!(messages[4].role, "user");
}

/// The Anthropic 400 reproduced in thread b101c3d7: an assistant `tool_use`
/// is followed by a user message whose content is `MessageContent::Text`
/// (e.g. the user's prompt after a trim that removed the matching
/// `tool_result`). The validator must inject the missing `tool_result` block
/// even when the next user message has plain `Text` content — converting it
/// to `Blocks([stubs..., Text])` so the original prompt is preserved.
#[test]
fn missing_tool_result_when_next_user_is_text_gets_stub() {
    let mut messages = vec![
        user_text("hello"),
        assistant_with_tool_uses(&["evt-090b688bf0dd4fed90470ad54d76b2a4"]),
        user_text("the next user prompt"),
    ];
    let stubs = validate_tool_use_pairing(&mut messages);
    assert_eq!(
        stubs, 1,
        "must inject 1 stub even when next user message is Text"
    );

    let blocks = match &messages[2].content {
        MessageContent::Blocks(b) => b,
        _ => panic!(
            "validator must promote Text content to Blocks when injecting stubs, got: {:?}",
            messages[2].content
        ),
    };

    let injected_id = blocks.iter().find_map(|b| {
        if let ContentBlock::ToolResult {
            tool_use_id,
            content,
        } = b
        {
            assert!(
                content.contains("orphan") || content.contains("unavailable"),
                "stub content should mark itself as unavailable: {:?}",
                content
            );
            Some(tool_use_id.clone())
        } else {
            None
        }
    });
    assert_eq!(
        injected_id.as_deref(),
        Some("evt-090b688bf0dd4fed90470ad54d76b2a4"),
        "stub must use the orphaned tool_use_id"
    );

    let preserved_text = blocks.iter().find_map(|b| {
        if let ContentBlock::Text { text } = b {
            Some(text.as_str())
        } else {
            None
        }
    });
    assert_eq!(
        preserved_text,
        Some("the next user prompt"),
        "original Text content must survive the conversion"
    );
}

/// Mid-flight `ReentryFromEngine` injection (or any other `MessageContent::Text`
/// user message) lands AFTER tool_use+tool_result, then on the next loop the
/// LLM emits a new tool_use whose result the next iteration provides. But if
/// trim later removes that user(tool_result), the orphan tool_use ends up
/// followed by the ReentryFromEngine Text message, which is exactly the bug shape.
#[test]
fn missing_tool_result_when_next_user_text_preserves_other_blocks() {
    // assistant has TWO tool_use blocks; next user has Text only — both stubs
    // need to land, plus the original Text must survive.
    let mut messages = vec![
        user_text("hello"),
        assistant_with_tool_uses(&["t1", "t2"]),
        user_text("[CHILD THREAD COMPLETED] some text"),
    ];
    let stubs = validate_tool_use_pairing(&mut messages);
    assert_eq!(stubs, 2);
    let blocks = match &messages[2].content {
        MessageContent::Blocks(b) => b,
        _ => panic!("expected Blocks after promotion"),
    };
    let result_ids: Vec<&str> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    assert!(result_ids.contains(&"t1") && result_ids.contains(&"t2"));
    let texts: Vec<&str> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["[CHILD THREAD COMPLETED] some text"]);
}

/// Layer-inversion guard: `llm/*.rs` must NOT import or reference
/// `crate::engine::` (or `super::engine::` etc.). Pre-Wave-4, `llm::vertex`
/// reached into `engine::context::validate_tool_use_pairing` — moving the
/// function into `llm::validate` (this module) made the import unnecessary,
/// and this test ensures nobody adds it back.
///
/// Scope: all `.rs` files directly under `crates/lucidos-engine/src/llm/`
/// (the LLM provider layer). Test files (`*_tests.rs`) are excluded because
/// they're allowed to wire integration tests with engine helpers — only
/// production code is gated.
#[test]
fn llm_does_not_depend_on_engine() {
    let llm_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("llm");
    let mut violations: Vec<(String, usize, String)> = Vec::new();
    for entry in std::fs::read_dir(&llm_dir).expect("read llm dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        // Skip the test file enforcing this rule (its module path includes
        // "engine" in the string literal) and any *_tests.rs files.
        if name.ends_with("_tests.rs") || name == "tests.rs" {
            continue;
        }
        let content = std::fs::read_to_string(&path).expect("read source");
        for (i, line) in content.lines().enumerate() {
            // Skip comment lines so doc-strings explaining the rule itself
            // don't trip the check.
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if line.contains("crate::engine::") || line.contains("crate :: engine ::") {
                violations.push((name.clone(), i + 1, line.to_string()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "llm/*.rs files must not depend on crate::engine — layer inversion. \
         Move the shared helper into llm/ (or core/) instead. Violations:\n{}",
        violations
            .iter()
            .map(|(f, line, src)| format!("  {}:{}: {}", f, line, src.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
