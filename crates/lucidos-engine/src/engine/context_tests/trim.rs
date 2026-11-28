use super::*;
use crate::llm::model_registry::context_window_from_prefix;
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

fn tool_uses_msg(ids: &[&str]) -> Message {
    Message {
        role: "assistant".to_string(),
        content: MessageContent::Blocks(
            ids.iter()
                .map(|id| ContentBlock::ToolUse {
                    id: (*id).to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": id}),
                    thought_signature: None,
                })
                .collect(),
        ),
    }
}

fn tool_results_msg(results: &[(&str, String)]) -> Message {
    Message {
        role: "user".to_string(),
        content: MessageContent::Blocks(
            results
                .iter()
                .map(|(id, content)| ContentBlock::ToolResult {
                    tool_use_id: (*id).to_string(),
                    content: content.clone(),
                })
                .collect(),
        ),
    }
}

const TEST_ADDRESS: &str = "evt-0123456789abcdef0123456789abcdef";

fn addressed(body: &str) -> String {
    format!("{body}\n[{TEST_ADDRESS}]")
}

fn result_at(messages: &[Message], idx: usize) -> &str {
    match &messages[idx].content {
        MessageContent::Blocks(blocks) => match &blocks[0] {
            ContentBlock::ToolResult { content, .. } => content,
            _ => panic!("expected a ToolResult at {idx}"),
        },
        _ => panic!("expected Blocks at {idx}"),
    }
}

fn results_at(messages: &[Message], idx: usize) -> Vec<&str> {
    match &messages[idx].content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect(),
        _ => panic!("expected Blocks at {idx}"),
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
    let removed =
        trim_context_if_needed(&mut messages, 500_000, None, &[], plain()).messages_removed;
    assert_eq!(removed, 0);
    assert_eq!(messages.len(), original_len);
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
        tool_use_msg("t3", "write_file", serde_json::json!({"path": "out.rs"})),
        tool_result_msg("t3", "ok"),
        text_msg("assistant", "Done"),
        text_msg("user", "Thanks"),
    ];
    let original_len = messages.len();
    let removed = trim_context_if_needed(&mut messages, 5_000, None, &[], plain()).messages_removed;

    assert_eq!(removed, 0, "pass 5 should not be needed");
    assert_eq!(messages.len(), original_len, "no messages removed");

    if let MessageContent::Blocks(blocks) = &messages[2].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert!(
                content.contains(BUDGET_CUT_NOTE),
                "large result should be truncated"
            );
            assert!(content.contains("10000"), "should mention original size");
        } else {
            panic!("expected ToolResult block");
        }
    } else {
        panic!("expected Blocks content");
    }

    if let MessageContent::Blocks(blocks) = &messages[4].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert_eq!(content, "small result");
        }
    }
}

#[test]
fn test_pass5_removes_old_pairs() {
    let mut messages = vec![text_msg("user", "initial message")];
    for i in 0..10 {
        let id = format!("t{}", i);
        messages.push(tool_use_msg(
            &id,
            "some_tool",
            serde_json::json!({"x": "y"}),
        ));
        messages.push(tool_result_msg(&id, "result content here that takes space"));
    }
    messages.push(tool_use_msg("recent1", "tool", serde_json::json!({})));
    messages.push(tool_result_msg("recent1", "recent result"));
    messages.push(text_msg("assistant", "final answer"));
    messages.push(text_msg("user", "ok"));

    let initial_len = messages.len();
    let budget = 200;
    let removed =
        trim_context_if_needed(&mut messages, budget, None, &[], plain()).messages_removed;

    assert!(removed > 0, "should have removed messages in pass 5");
    assert_eq!(
        messages.len(),
        initial_len - removed,
        "returned removed count must match actual length delta"
    );
    assert_eq!(messages[0].content.as_text(), "initial message");
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
        tool_use_msg(
            "new1",
            "write_file",
            serde_json::json!({"path": "out.rs", "content": "fn main() {}"}),
        ),
        tool_result_msg("new1", "File written successfully"),
        text_msg("assistant", "I've written the file for you"),
        text_msg("user", "Great, thanks"),
    ];

    let budget = 1_000;
    trim_context_if_needed(&mut messages, budget, None, &[], plain());

    assert_eq!(messages[0].content.as_text(), "important initial context");

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
        image_msg("user", "old screenshot", 200),
        text_msg("assistant", "I see it"),
        image_msg("user", "another old screenshot", 100),
        text_msg("assistant", "Got it"),
        image_msg("user", "current screenshot", 100),
    ];

    trim_context_if_needed(&mut messages, 500_000, None, &[], plain());

    if let MessageContent::Blocks(blocks) = &messages[0].content {
        assert!(
            !blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "older image at index 0 should be stripped"
        );
        assert!(
            blocks.iter().any(
                |b| matches!(b, ContentBlock::Text { text } if text.contains("image from earlier"))
            ),
            "should have placeholder text at index 0"
        );
    }

    if let MessageContent::Blocks(blocks) = &messages[2].content {
        assert!(
            !blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "older image at index 2 should be stripped"
        );
    }

    let last_idx = messages.len() - 1;
    if let MessageContent::Blocks(blocks) = &messages[last_idx].content {
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "current user message (last) image must be preserved"
        );
    }

    assert_eq!(messages.len(), 5);
}

#[test]
fn test_pass0_preserves_current_message_image_with_resume_blocks() {
    let mut messages = vec![
        tool_use_msg("t1", "read_file", serde_json::json!({"path": "x.rs"})),
        tool_result_msg("t1", "file contents"),
        tool_use_msg("t2", "list_files", serde_json::json!({})),
        tool_result_msg("t2", "files: a, b, c"),
        image_msg("user", "Request: what is in this screenshot?", 100),
    ];

    trim_context_if_needed(&mut messages, 500_000, None, &[], plain());

    let last_idx = messages.len() - 1;
    if let MessageContent::Blocks(blocks) = &messages[last_idx].content {
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "current user message's image must reach the LLM"
        );
        assert!(
            !blocks.iter().any(
                |b| matches!(b, ContentBlock::Text { text } if text.contains("image from earlier"))
            ),
            "current user message's image must NOT be replaced with the placeholder"
        );
    } else {
        panic!("expected Blocks content for current user message");
    }
}

#[test]
fn test_image_block_char_estimate_is_fixed_not_base64_len() {
    let small = estimate_message_chars(&image_msg("user", "shot", 10));
    let huge = estimate_message_chars(&image_msg("user", "shot", 3_000));
    assert_eq!(
        small, huge,
        "image budget cost must not scale with base64 length"
    );
    let img_only = estimate_message_chars(&Message {
        role: "user".to_string(),
        content: MessageContent::Blocks(vec![ContentBlock::Image {
            source_type: "base64".to_string(),
            media_type: "image/png".to_string(),
            data: "x".repeat(3_000 * 1024),
        }]),
    });
    assert!(
        img_only < 10_000,
        "one image must cost far less than its base64 length ({img_only} chars)"
    );
}

#[test]
fn test_pass0_keeps_current_turn_image_when_not_last() {
    let img_idx = 0;
    let mut messages = vec![
        image_msg("user", "Request: why won't this part fit?", 100),
        tool_use_msg("t1", "list_files", serde_json::json!({})),
        tool_result_msg("t1", "files: a, b, c"),
        tool_use_msg("t2", "read_file", serde_json::json!({"path": "notes.md"})),
        tool_result_msg("t2", "notes"),
    ];

    trim_context_if_needed(&mut messages, 500_000, Some(img_idx), &[img_idx], plain());

    if let MessageContent::Blocks(blocks) = &messages[img_idx].content {
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "current-turn image must be kept for the whole turn, not just the first call"
        );
        assert!(
            !blocks.iter().any(
                |b| matches!(b, ContentBlock::Text { text } if text.contains("image from earlier"))
            ),
            "current-turn image must NOT be replaced with the placeholder"
        );
    } else {
        panic!("expected Blocks content at the current-turn user message");
    }
}

#[test]
fn test_pass0_keeps_every_pinned_image_message() {
    let mut messages = vec![
        image_msg("user", "Request: why won't this part fit?", 100),
        tool_use_msg("t1", "capture_app", serde_json::json!({})),
        image_msg("user", "app capture", 100),
        tool_use_msg("t2", "view_image", serde_json::json!({"image": "thread:1"})),
        image_msg("user", "re-loaded thread image", 100),
        tool_use_msg("t3", "grep_files", serde_json::json!({"pattern": "x"})),
        tool_result_msg("t3", "match"),
    ];

    trim_context_if_needed(&mut messages, 500_000, Some(0), &[0, 4], plain());

    for idx in [0usize, 4] {
        let MessageContent::Blocks(blocks) = &messages[idx].content else {
            panic!("expected Blocks content at pinned index {}", idx);
        };
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "pinned message {} must keep its image bytes",
            idx
        );
    }

    let MessageContent::Blocks(blocks) = &messages[2].content else {
        panic!("expected Blocks content at the unpinned capture");
    };
    assert!(
        !blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. })),
        "an unpinned ambient capture must still age out"
    );
}

#[test]
fn test_pass5_floor_ignores_pins_above_the_lowest() {
    let build = || {
        let large = "x".repeat(2_000);
        let mut messages = vec![text_msg("user", "workspace context")];
        for i in 0..10 {
            let id = format!("resume{}", i);
            messages.push(tool_use_msg(
                &id,
                "read_file",
                serde_json::json!({"path": "x.rs"}),
            ));
            messages.push(tool_result_msg(&id, &large));
        }
        messages.push(image_msg("user", "Request: what is this?", 100));
        for i in 0..4 {
            let id = format!("recent{}", i);
            messages.push(tool_use_msg(
                &id,
                "grep",
                serde_json::json!({"pattern": "y"}),
            ));
            messages.push(tool_result_msg(&id, "ok"));
        }
        messages
    };
    let low_pin = 21;
    let high_pin = 25;

    let mut baseline = build();
    let baseline_removed =
        trim_context_if_needed(&mut baseline, 1_000, Some(low_pin), &[low_pin], plain())
            .messages_removed;

    let mut with_high_pin = build();
    let with_high_removed = trim_context_if_needed(
        &mut with_high_pin,
        1_000,
        Some(low_pin),
        &[low_pin, high_pin],
        plain(),
    )
    .messages_removed;

    assert!(
        baseline_removed > 0,
        "the fixture must actually force pass 5 to evict, or this proves nothing"
    );
    assert_eq!(
        with_high_removed, baseline_removed,
        "a pin above the floor must not reduce how much pass 5 can evict"
    );
}

#[test]
fn test_pass0_strips_non_last_image_when_not_kept() {
    let mut messages = vec![
        image_msg("user", "older screenshot", 100),
        tool_use_msg("t1", "list_files", serde_json::json!({})),
        tool_result_msg("t1", "files"),
    ];

    trim_context_if_needed(&mut messages, 500_000, None, &[], plain());

    if let MessageContent::Blocks(blocks) = &messages[0].content {
        assert!(
            !blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "a non-last image with no pin must still be stripped"
        );
    }
}

#[test]
fn test_pass5_keeps_image_message_below_protected_after_injection() {
    let large_resume = "x".repeat(2_000);
    let mut messages = vec![text_msg("user", "workspace context")];
    for i in 0..10 {
        let id = format!("resume{}", i);
        messages.push(tool_use_msg(
            &id,
            "read_file",
            serde_json::json!({"path": "x.rs"}),
        ));
        messages.push(tool_result_msg(&id, &large_resume));
    }
    let img_idx = messages.len();
    messages.push(image_msg("user", "Request: why won't this part fit?", 100));
    messages.push(tool_use_msg(
        "mid",
        "grep",
        serde_json::json!({"pattern": "x"}),
    ));
    messages.push(tool_result_msg("mid", "match"));
    let injected_idx = messages.len();
    messages.push(text_msg("user", "actually also check the manual"));
    for i in 0..3 {
        let id = format!("recent{}", i);
        messages.push(tool_use_msg(
            &id,
            "grep",
            serde_json::json!({"pattern": "y"}),
        ));
        messages.push(tool_result_msg(&id, "ok"));
    }

    assert!(
        injected_idx > img_idx,
        "injected prompt must sit above the image message"
    );

    let removed = trim_context_if_needed(
        &mut messages,
        1_000,
        Some(injected_idx),
        &[img_idx],
        plain(),
    )
    .messages_removed;

    let post_img = img_idx.saturating_sub(removed);
    if let MessageContent::Blocks(blocks) = &messages[post_img].content {
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "the image-bearing message must survive pass 5 even when protected_idx diverges above it"
        );
    } else {
        panic!("expected the image message to survive with its Blocks content");
    }
}

#[test]
fn test_pass5_does_not_remove_protected_user_message() {
    let large_resume = "x".repeat(2_000);
    let mut messages = vec![text_msg("user", "workspace context")];
    for i in 0..10 {
        let id = format!("resume{}", i);
        messages.push(tool_use_msg(
            &id,
            "read_file",
            serde_json::json!({"path": "x.rs"}),
        ));
        messages.push(tool_result_msg(&id, &large_resume));
    }
    let protected = messages.len();
    messages.push(text_msg("user", "Request: what is in this screenshot?"));
    for i in 0..4 {
        let id = format!("recent{}", i);
        messages.push(tool_use_msg(
            &id,
            "grep",
            serde_json::json!({"pattern": "x"}),
        ));
        messages.push(tool_result_msg(&id, "match found"));
    }

    let removed = trim_context_if_needed(&mut messages, 1_000, Some(protected), &[], plain())
        .messages_removed;

    let post_protected = protected.saturating_sub(removed);
    assert_eq!(
        messages[post_protected].content.as_text(),
        "Request: what is in this screenshot?",
        "pinned user message must survive pass 5"
    );
    let len = messages.len();
    if let MessageContent::Blocks(blocks) = &messages[len - 1].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert_eq!(content, "match found");
        }
    }
}

#[test]
fn test_pass5_stops_when_protected_lands_at_index_one() {
    let mut messages = vec![
        text_msg("user", "workspace context"),
        text_msg("user", "Request: original ask"),
        text_msg("assistant", "I'll look into it"),
        text_msg("user", "tool result a"),
        text_msg("assistant", "more"),
        text_msg("user", "tool result b"),
    ];
    let protected = 1;

    let removed =
        trim_context_if_needed(&mut messages, 10, Some(protected), &[], plain()).messages_removed;

    assert_eq!(
        messages[protected].content.as_text(),
        "Request: original ask",
        "protected message at index 1 must not be removed"
    );
    assert_eq!(
        removed, 0,
        "guard at index 1 must short-circuit before any removal"
    );
}

#[test]
fn test_pass3_trims_large_tail_tool_results() {
    let huge = "x".repeat(TAIL_TRUNCATION_THRESHOLD + 5_000);
    let mut messages = vec![
        text_msg("user", "initial request"),
        tool_use_msg("old1", "grep", serde_json::json!({"pattern": "x"})),
        tool_result_msg("old1", "match"),
        tool_use_msg("old2", "grep", serde_json::json!({"pattern": "x"})),
        tool_result_msg("old2", "match"),
        tool_use_msg(
            "recent1",
            "query_events",
            serde_json::json!({"event_type": "X"}),
        ),
        tool_result_msg("recent1", &huge),
        tool_use_msg(
            "recent2",
            "query_events",
            serde_json::json!({"event_type": "Y"}),
        ),
        tool_result_msg("recent2", &huge),
    ];

    let total_before: usize = messages.iter().map(estimate_message_chars).sum();
    let budget = TAIL_TRUNCATION_THRESHOLD;

    assert!(
        total_before > budget,
        "test premise: total must exceed budget"
    );

    let _removed =
        trim_context_if_needed(&mut messages, budget, None, &[], plain()).messages_removed;

    let len = messages.len();
    if let MessageContent::Blocks(blocks) = &messages[len - 3].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert!(
                content.contains(BUDGET_CUT_NOTE),
                "huge tail ToolResult (not the last message) must be truncated by pass 3"
            );
        } else {
            panic!("expected ToolResult block at len-3");
        }
    } else {
        panic!("expected Blocks at len-3");
    }
}

#[test]
fn test_pass3_preserves_very_last_message() {
    let huge = "x".repeat(TAIL_TRUNCATION_THRESHOLD + 5_000);
    let mut messages = vec![
        text_msg("user", "initial"),
        tool_use_msg("old", "grep", serde_json::json!({})),
        tool_result_msg("old", "match"),
        tool_use_msg("r1", "tool", serde_json::json!({})),
        tool_result_msg("r1", &huge),
        tool_use_msg("r2", "tool", serde_json::json!({})),
        tool_result_msg("r2", &huge),
    ];

    let budget = TAIL_TRUNCATION_THRESHOLD + 10_000;
    let outcome = trim_context_if_needed(&mut messages, budget, None, &[], plain());
    assert_eq!(
        outcome.messages_removed, 0,
        "pass 3 alone must reach the budget here"
    );

    let last_idx = messages.len() - 1;
    if let MessageContent::Blocks(blocks) = &messages[last_idx].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert!(
                !content.contains(BUDGET_CUT_NOTE),
                "the very last message's ToolResult must NOT be truncated by pass 3 (it's the data the next turn will reason about)"
            );
            assert_eq!(
                content.len(),
                huge.len(),
                "verbatim length must be preserved"
            );
        }
    }
}

#[test]
fn test_pass3_then_pass5_uses_post_truncation_total() {
    let huge = "x".repeat(TAIL_TRUNCATION_THRESHOLD * 4);
    let small_old_pair = "y".repeat(400);
    let mut messages = vec![text_msg("user", "initial")];
    for i in 0..6 {
        let id = format!("old{}", i);
        messages.push(tool_use_msg(&id, "grep", serde_json::json!({"p": "q"})));
        messages.push(tool_result_msg(&id, &small_old_pair));
    }
    messages.push(tool_use_msg("r1", "query_events", serde_json::json!({})));
    messages.push(tool_result_msg("r1", &huge));
    messages.push(tool_use_msg("r2", "query_events", serde_json::json!({})));
    messages.push(tool_result_msg("r2", "small"));

    let budget = 1_500;
    let removed =
        trim_context_if_needed(&mut messages, budget, None, &[], plain()).messages_removed;

    assert!(
        removed < 12,
        "Pass 5 evicted {} messages (all 6 old pairs). The stale-current_total \
         bug is back: Pass 5 is reading total_after_pass1 (~82K) instead of \
         total_after_truncation (~2.6K), so its while-guard never reaches the \
         budget and only the message-count guard stops it.",
        removed
    );

    let len = messages.len();
    if let MessageContent::Blocks(blocks) = &messages[len - 3].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert!(
                content.contains(BUDGET_CUT_NOTE),
                "expected the tail tool_result at len-3 to carry Pass 3's budget stub"
            );
        }
    }
}

#[test]
fn test_pass1_truncation_alone_counts_as_trimmed() {
    let large_content = "x".repeat(10_000);
    let mut messages = vec![
        text_msg("user", "initial"),
        tool_use_msg("t1", "read_file", serde_json::json!({"path": "foo.rs"})),
        tool_result_msg("t1", &large_content),
        tool_use_msg("t2", "read_file", serde_json::json!({"path": "bar.rs"})),
        tool_result_msg("t2", "small result"),
        tool_use_msg("t3", "write_file", serde_json::json!({"path": "out.rs"})),
        tool_result_msg("t3", "ok"),
        text_msg("assistant", "Done"),
        text_msg("user", "Thanks"),
    ];
    let original_len = messages.len();
    let outcome = trim_context_if_needed(&mut messages, 5_000, None, &[], plain());

    assert_eq!(outcome.messages_removed, 0, "pass 5 should not be needed");
    assert_eq!(messages.len(), original_len);
    assert!(
        outcome.blocks_truncated > 0,
        "pass 1 truncated a tool result, so it must be counted"
    );
    assert!(
        outcome.any(),
        "content WAS dropped — `trimmed` must report true"
    );
}

#[test]
fn test_truncated_tool_arguments_count_as_trimmed() {
    let big_arg = "z".repeat(5_000);
    let mut messages = vec![
        text_msg("user", "initial"),
        tool_use_msg(
            "t1",
            "write_file",
            serde_json::json!({ "content": big_arg }),
        ),
        tool_result_msg("t1", "ok"),
        text_msg("assistant", "a"),
        text_msg("user", "b"),
        text_msg("assistant", "c"),
        text_msg("user", "d"),
    ];
    let outcome = trim_context_if_needed(&mut messages, 2_000, None, &[], plain());

    assert!(
        outcome.blocks_truncated > 0,
        "a truncated tool argument is content loss and must be counted"
    );
    assert!(outcome.any(), "`trimmed` must report true");
}

#[test]
fn test_truncate_large_json_strings_counts_what_it_cut() {
    let big = "q".repeat(TRUNCATION_THRESHOLD + 1);
    let mut nested = serde_json::json!({
        "small": "fine",
        "big": big,
        "nested": { "also_big": big, "arr": [big, "short"] },
    });
    assert_eq!(truncate_large_json_strings(&mut nested), 3);

    let mut untouched = serde_json::json!({ "a": "short", "b": [1, 2, 3] });
    assert_eq!(truncate_large_json_strings(&mut untouched), 0);
}

#[test]
fn test_untouched_context_reports_nothing_trimmed() {
    let mut messages = vec![
        text_msg("user", "Hello"),
        text_msg("assistant", "Hi there"),
        text_msg("user", "How are you?"),
    ];
    let outcome = trim_context_if_needed(&mut messages, 500_000, None, &[], plain());
    assert_eq!(outcome, TrimOutcome::default());
    assert!(!outcome.any());
}

#[test]
fn test_budget_token_round_trip_matches_budget_ratio() {
    for model in ["claude-opus-4-7", "claude-opus-4-7[1m]", "gpt-5"] {
        let window = context_window_from_prefix(model);
        let usable = window - RESPONSE_TOKEN_RESERVE;
        let budget_chars = agent_context_char_budget(window);
        let budget_tokens = budget_tokens_from_chars(budget_chars);
        assert!(
            budget_tokens <= usable && budget_tokens + 1 >= usable,
            "model {}: budget={} chars round-trips to {} tokens, expected ≈ {}",
            model,
            budget_chars,
            budget_tokens,
            usable,
        );
    }
}

#[test]
fn test_pass5_skips_pair_removal_that_would_drop_protected() {
    let large = "y".repeat(2_000);
    let mut messages = vec![
        text_msg("user", "workspace context"),
        tool_use_msg("t1", "read_file", serde_json::json!({"path": "x.rs"})),
        text_msg("user", "Request: original ask"),
        text_msg("assistant", "looking"),
        tool_result_msg("filler", &large),
        text_msg("assistant", "still"),
        text_msg("user", "ok"),
    ];
    let protected = 2;

    let _ = trim_context_if_needed(&mut messages, 100, Some(protected), &[], plain());

    let still_present = messages
        .iter()
        .any(|m| m.content.as_text() == "Request: original ask");
    assert!(
        still_present,
        "pair removal must not drop the protected message"
    );
}

#[test]
fn the_current_rounds_results_survive_a_short_message_array() {
    let docs: Vec<String> = (0..5).map(|_| addressed(&"d".repeat(38_000))).collect();
    let ids = ["t1", "t2", "t3", "t4", "t5"];
    let pairs: Vec<(&str, String)> = ids.iter().copied().zip(docs.iter().cloned()).collect();
    let mut messages = vec![
        text_msg("user", &"the assembled turn body ".repeat(6_250)),
        tool_uses_msg(&ids),
        tool_results_msg(&pairs),
    ];

    let budget = 200_000;
    let outcome = trim_context_if_needed(&mut messages, budget, None, &[], plain());

    assert_eq!(outcome.messages_removed, 0, "nothing may be removed");
    assert_eq!(
        outcome.blocks_truncated, 0,
        "the round that just arrived must reach the model whole"
    );
    for (i, content) in results_at(&messages, 2).iter().enumerate() {
        assert_eq!(*content, docs[i], "document {i} must be byte-identical");
    }
}

#[test]
fn pass_one_stops_as_soon_as_it_is_under_budget() {
    let big = "b".repeat(30_000);
    let mut messages = vec![text_msg("user", "initial")];
    for i in 0..4 {
        let id = format!("t{i}");
        messages.push(tool_use_msg(
            &id,
            "read_file",
            serde_json::json!({"p": &id}),
        ));
        messages.push(tool_result_msg(&id, &big));
    }
    for i in 0..4 {
        messages.push(text_msg("assistant", &format!("tail {i}")));
    }

    let outcome = trim_context_if_needed(&mut messages, 100_000, None, &[], plain());

    assert_eq!(outcome.messages_removed, 0, "stubbing alone must suffice");
    assert_eq!(outcome.blocks_truncated, 1, "one stub, not four");
    assert!(result_at(&messages, 2).contains(BUDGET_CUT_NOTE));
    for idx in [4, 6, 8] {
        assert_eq!(
            result_at(&messages, idx).len(),
            big.len(),
            "result at {idx} must survive verbatim"
        );
    }
}

#[test]
fn a_truncation_stub_names_the_address_that_reads_it_back() {
    let body = addressed(&"z".repeat(30_000));
    let mut messages = vec![text_msg("user", "initial")];
    messages.push(tool_use_msg("t0", "read_file", serde_json::json!({})));
    messages.push(tool_result_msg("t0", &body));
    for i in 0..4 {
        messages.push(text_msg("assistant", &format!("tail {i}")));
    }

    trim_context_if_needed(&mut messages, 1_000, None, &[], plain());

    let stub = result_at(&messages, 2);
    assert!(
        stub.contains(BUDGET_CUT_NOTE),
        "the stub must say it cut: {stub}"
    );
    assert!(
        stub.contains(&body.len().to_string()),
        "it must state the size"
    );
    assert!(
        stub.contains("events(action=\"query\""),
        "it must name the call"
    );
    assert!(
        stub.ends_with(&format!("\n[{TEST_ADDRESS}]")),
        "the bare address trailer must survive: {stub}"
    );
}

#[test]
fn nothing_is_removed_while_anything_is_still_stubbable() {
    let prose = "p".repeat(20_000);
    let mut messages = vec![text_msg("user", "initial")];
    for _ in 0..4 {
        messages.push(text_msg("assistant", &prose));
        messages.push(text_msg("user", "ack"));
    }
    for i in 0..4 {
        messages.push(text_msg("assistant", &format!("tail {i}")));
    }

    let outcome = trim_context_if_needed(&mut messages, 45_000, None, &[], plain());

    assert_eq!(
        outcome.messages_removed, 0,
        "prose was still stubbable, so nothing may be removed"
    );
    assert_eq!(
        outcome.blocks_truncated, 2,
        "two collapses reach the budget"
    );
    for idx in [5, 7] {
        assert_eq!(
            messages[idx].content.as_text().len(),
            prose.len(),
            "prose at {idx} must survive verbatim"
        );
    }
}

#[test]
fn a_user_message_in_the_collapse_range_keeps_its_text() {
    let instruction = format!("Do not touch the frontend. {}", "detail ".repeat(4_000));
    let mut messages = vec![
        text_msg("user", "initial"),
        text_msg("assistant", &"a".repeat(30_000)),
        text_msg("user", &instruction),
        text_msg("assistant", &"b".repeat(30_000)),
    ];
    for i in 0..4 {
        messages.push(text_msg("assistant", &format!("tail {i}")));
    }

    let outcome = trim_context_if_needed(&mut messages, 30_000, None, &[], plain());

    assert_eq!(outcome.messages_removed, 0, "stubbing alone must suffice");
    assert_eq!(
        messages[2].content.as_text(),
        instruction,
        "the injected instruction must survive byte-identical"
    );
    for idx in [1, 3] {
        assert!(
            messages[idx].content.as_text().contains(BUDGET_CUT_NOTE),
            "assistant prose at {idx} was the reclaim, so it must be stubbed"
        );
    }
}

#[test]
fn the_last_message_keeps_its_head_when_its_results_alone_exceed_the_budget() {
    let docs: Vec<String> = (0..5).map(|_| addressed(&"d".repeat(40_000))).collect();
    let ids = ["t1", "t2", "t3", "t4", "t5"];
    let pairs: Vec<(&str, String)> = ids.iter().copied().zip(docs.iter().cloned()).collect();
    let mut messages = vec![
        text_msg("user", "initial ask"),
        tool_uses_msg(&ids),
        tool_results_msg(&pairs),
    ];

    let budget = 170_000;
    let outcome = trim_context_if_needed(&mut messages, budget, None, &[], plain());

    assert_eq!(outcome.messages_removed, 0, "nothing may be removed");
    let after: usize = messages.iter().map(estimate_message_chars).sum();
    assert!(
        after <= budget,
        "the last resort must reach the budget: {after}"
    );

    let results = results_at(&messages, 2);
    let verbatim = results.iter().filter(|c| c.len() == docs[0].len()).count();
    assert!(verbatim >= 3, "only the outliers get cut, not the round");
    for content in results.iter().filter(|c| c.len() != docs[0].len()) {
        assert!(
            content.len() > LAST_RESORT_HEAD_CHARS,
            "a cut result must keep its head, not become a bare stub"
        );
        assert!(content.contains(BUDGET_CUT_NOTE), "the cut must be loud");
        assert!(
            content.ends_with(&format!("\n[{TEST_ADDRESS}]")),
            "the address must survive so the model can read the rest back"
        );
    }
}

#[test]
fn address_trailer_reads_the_line_the_agent_loop_appends() {
    assert_eq!(
        address_trailer(&addressed("body")),
        Some(TEST_ADDRESS),
        "the bare trailer is what with_event_address writes"
    );
    assert_eq!(address_trailer("no trailer at all"), None);
    assert_eq!(address_trailer("body\n[not-an-address]"), None);
    assert_eq!(
        address_trailer("body\n[evt-nothexatall]"),
        None,
        "the hex has to parse, or a stub would name a dead address"
    );
}

#[test]
fn a_budget_stub_falls_back_when_the_content_has_no_address() {
    let stub = budget_stub("plain content with no trailer", RecoveryClause::State);
    assert!(stub.contains(BUDGET_CUT_NOTE));
    assert!(stub.contains("29"), "it still states the size: {stub}");
    assert!(
        !stub.contains("events(action="),
        "no address means no recovery call to promise"
    );
}

#[test]
fn a_stub_that_would_grow_the_result_is_refused() {
    let content = addressed(&"z".repeat(LAST_RESORT_HEAD_CHARS));
    let was = content.len();
    assert!(
        head_stub(&content, LAST_RESORT_HEAD_CHARS, RecoveryClause::State).len() > was,
        "the fixture must be in the range where the head stub is the longer one"
    );

    let mut messages = vec![
        text_msg("user", "initial ask"),
        tool_uses_msg(&["t1"]),
        tool_results_msg(&[("t1", content)]),
    ];
    let outcome = trim_context_if_needed(&mut messages, 1_000, None, &[], plain());

    assert_eq!(
        outcome.blocks_truncated, 1,
        "the refused head stub must not be counted as a cut"
    );
    let after = result_at(&messages, 2);
    assert!(after.len() < was, "the result must end up smaller: {after}");
    assert!(after.contains(BUDGET_CUT_NOTE));
    assert!(after.ends_with(&format!("\n[{TEST_ADDRESS}]")));
}

#[test]
fn a_head_stub_keeps_the_head_and_says_what_it_dropped() {
    let content = addressed(&"h".repeat(1_000));
    let stub = head_stub(&content, 100, RecoveryClause::State);
    assert!(stub.starts_with(&"h".repeat(100)), "the head comes first");
    assert!(stub.contains("100 of "), "it states how much is shown");
    assert!(stub.contains(&content.len().to_string()));
    assert!(stub.ends_with(&format!("\n[{TEST_ADDRESS}]")));
}

mod protected_addresses {
    use super::*;
    use std::collections::HashSet;

    const TRAILER_CHARS: usize = 39;

    fn address(byte: u8) -> String {
        format!("evt-{}", format!("{byte:02x}").repeat(16))
    }

    fn addressed_result(id: &str, byte: u8, size: usize) -> Message {
        tool_result_msg(id, &format!("{}\n[{}]", "x".repeat(size), address(byte)))
    }

    fn thread() -> Vec<Message> {
        let mut messages = vec![text_msg("user", "the request")];
        for (i, byte) in [1u8, 2, 3, 4, 5, 6].into_iter().enumerate() {
            let id = format!("call-{i}");
            messages.push(tool_use_msg(&id, "read_file", serde_json::json!({})));
            messages.push(addressed_result(&id, byte, 60_000));
        }
        messages
    }

    fn surviving(messages: &[Message]) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        for message in messages {
            let MessageContent::Blocks(blocks) = &message.content else {
                continue;
            };
            for block in blocks {
                if let ContentBlock::ToolResult { content, .. } = block {
                    if let Some(addr) = address_trailer(content) {
                        out.push((addr.to_string(), content.len()));
                    }
                }
            }
        }
        out
    }

    fn guards<'a>(
        protected: &'a HashSet<String>,
        held_open: &'a HashSet<String>,
    ) -> TrimGuards<'a> {
        TrimGuards {
            protected,
            held_open,
            recovery: RecoveryClause::State,
        }
    }

    #[test]
    fn an_unprotected_thread_is_cut() {
        let mut messages = thread();
        let none = HashSet::new();
        let outcome =
            trim_context_if_needed(&mut messages, 5_000, Some(0), &[], guards(&none, &none));
        assert!(outcome.blocks_truncated > 0, "the baseline must cut");
    }

    #[test]
    fn a_failed_result_is_never_hollowed_out() {
        let mut messages = thread();
        let failed: HashSet<String> = (1u8..=6).map(address).collect();
        let none = HashSet::new();

        trim_context_if_needed(&mut messages, 5_000, Some(0), &[], guards(&failed, &none));

        for (addr, len) in surviving(&messages) {
            assert_eq!(
                len,
                60_000 + TRAILER_CHARS,
                "{addr} survived removal but was stubbed anyway"
            );
        }
    }

    #[test]
    fn a_failed_action_is_protected() {
        let mut messages = thread();
        let failed: HashSet<String> = [address(6)].into_iter().collect();
        let none = HashSet::new();

        trim_context_if_needed(&mut messages, 5_000, Some(0), &[], guards(&failed, &none));

        let found = surviving(&messages)
            .into_iter()
            .find(|(addr, _)| *addr == address(6));
        assert_eq!(
            found.map(|(_, len)| len),
            Some(60_000 + TRAILER_CHARS),
            "the failed action was cut"
        );
    }

    #[test]
    fn protecting_one_address_does_not_spare_the_rest() {
        let mut messages = thread();
        let failed: HashSet<String> = [address(6)].into_iter().collect();
        let none = HashSet::new();

        let outcome =
            trim_context_if_needed(&mut messages, 5_000, Some(0), &[], guards(&failed, &none));

        assert!(
            outcome.blocks_truncated > 0 || outcome.messages_removed > 0,
            "the unprotected results must still go"
        );
    }

    #[test]
    fn nothing_held_open_can_stop_the_wall() {
        let mut messages = thread();
        let held: HashSet<String> = (1u8..=6).map(address).collect();
        let none = HashSet::new();

        let outcome =
            trim_context_if_needed(&mut messages, 5_000, Some(0), &[], guards(&none, &held));

        assert!(
            outcome.blocks_truncated > 0,
            "every item held open, and the wall still had to fit the request"
        );
        for (addr, len) in surviving(&messages) {
            assert!(len < 60_000, "{addr} was held open AND exempted");
        }
    }

    #[test]
    fn the_wall_takes_held_items_last() {
        let mut messages = thread();
        let held: HashSet<String> = [address(1)].into_iter().collect();
        let none = HashSet::new();

        trim_context_if_needed(&mut messages, 250_000, Some(0), &[], guards(&none, &held));

        let sizes: std::collections::HashMap<String, usize> =
            surviving(&messages).into_iter().collect();
        assert_eq!(
            sizes.get(&address(1)).copied(),
            Some(60_000 + TRAILER_CHARS),
            "the held item went before an unheld one of the same size"
        );
        assert!(
            sizes.values().any(|len| *len < 60_000),
            "something unheld had to go"
        );
    }

    #[test]
    fn protection_never_stops_the_wall_from_removing_messages() {
        let mut messages = thread();
        let failed: HashSet<String> = (1u8..=6).map(address).collect();
        let none = HashSet::new();

        let outcome =
            trim_context_if_needed(&mut messages, 5_000, Some(0), &[], guards(&failed, &none));

        assert!(
            outcome.messages_removed > 0,
            "pass 5 must still reach a thread where every result is protected"
        );
    }

    #[test]
    fn the_last_resort_pass_honours_protection_too() {
        let mut messages = vec![text_msg("user", "the request")];
        messages.push(tool_use_msg("call-0", "read_file", serde_json::json!({})));
        messages.push(Message {
            role: "user".to_string(),
            content: MessageContent::Blocks(vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call-0".to_string(),
                    content: format!("{}\n[{}]", "x".repeat(80_000), address(1)),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "call-1".to_string(),
                    content: format!("{}\n[{}]", "y".repeat(80_000), address(2)),
                },
            ]),
        });
        let failed: HashSet<String> = [address(1)].into_iter().collect();
        let none = HashSet::new();

        trim_context_if_needed(&mut messages, 5_000, Some(0), &[], guards(&failed, &none));

        let MessageContent::Blocks(blocks) = &messages[2].content else {
            panic!("expected blocks");
        };
        let ContentBlock::ToolResult { content: held, .. } = &blocks[0] else {
            panic!("expected a result");
        };
        let ContentBlock::ToolResult { content: cut, .. } = &blocks[1] else {
            panic!("expected a result");
        };
        assert_eq!(
            held.len(),
            80_000 + TRAILER_CHARS,
            "the protected result was cut"
        );
        assert!(cut.len() < 80_000, "the unprotected one should have gone");
    }
}

mod the_live_document_is_protected {
    use super::*;

    fn address(n: u8) -> String {
        format!("evt-{}", format!("{n:02x}").repeat(16))
    }

    fn short_thread() -> Vec<Message> {
        let mut messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text("the request".to_string()),
        }];
        for n in 1u8..=2 {
            messages.push(Message {
                role: "assistant".to_string(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: format!("call-{n}"),
                    name: "read_file".to_string(),
                    input: serde_json::json!({}),
                    thought_signature: None,
                }]),
            });
            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: format!("call-{n}"),
                    content: format!("{}\n[{}]", "x".repeat(60_000), address(n)),
                }]),
            });
        }
        messages
    }

    #[test]
    fn the_tail_rendered_document_survives_a_hard_trim() {
        let pad = "[WORKING UNDERSTANDING]\n".to_string() + &"a note that matters. ".repeat(200);
        let mut messages = short_thread();
        let MessageContent::Blocks(blocks) = &mut messages.last_mut().unwrap().content else {
            panic!("expected blocks");
        };
        blocks.push(ContentBlock::Text { text: pad.clone() });

        trim_context_if_needed(&mut messages, 5_000, Some(0), &[], plain());

        let held = messages.iter().any(|m| match &m.content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if *text == pad)),
            _ => false,
        });
        assert!(held, "the tail-rendered pad was cut by a budget pass");
    }

    #[test]
    fn an_oversized_argument_is_cut_like_any_other() {
        let body = "a line of the file. ".repeat(200);
        let mut messages = vec![
            Message {
                role: "user".to_string(),
                content: MessageContent::Text("the request".to_string()),
            },
            Message {
                role: "assistant".to_string(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "write-call".to_string(),
                    name: "write_file".to_string(),
                    input: serde_json::json!({ "path": "notes.md", "content": body }),
                    thought_signature: None,
                }]),
            },
            Message {
                role: "user".to_string(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "write-call".to_string(),
                    content: "Wrote notes.md.".to_string(),
                }]),
            },
        ];
        messages.extend(short_thread().into_iter().skip(1));

        trim_context_if_needed(&mut messages, 5_000, Some(0), &[], plain());

        let held = messages.iter().any(|m| match &m.content {
            MessageContent::Blocks(blocks) => blocks.iter().any(|b| match b {
                ContentBlock::ToolUse { input, .. } => {
                    input.get("content").and_then(|v| v.as_str()) == Some(body.as_str())
                }
                _ => false,
            }),
            _ => false,
        });
        assert!(!held, "a large argument must be reclaimable");
    }

    #[test]
    fn a_trim_that_did_not_fire_reports_no_pass() {
        let mut messages = vec![text_msg("user", "short")];
        let outcome = trim_context_if_needed(&mut messages, 500_000, None, &[], plain());
        assert_eq!(outcome.passes, 0);
        assert!(!outcome.any());
    }

    #[test]
    fn stubbing_an_old_result_sets_pass_one_and_nothing_below_it() {
        let mut messages = vec![
            text_msg("user", "the request"),
            tool_use_msg("t1", "read_file", serde_json::json!({"path": "a.md"})),
            tool_result_msg("t1", &"x".repeat(60_000)),
        ];
        for i in 0..4 {
            messages.push(text_msg("assistant", &format!("turn {i}")));
            messages.push(text_msg("user", "next"));
        }
        let outcome = trim_context_if_needed(&mut messages, 20_000, None, &[], plain());
        assert!(outcome.any(), "60 KB against a 20 KB budget has to cut");
        assert_eq!(outcome.passes & 1, 0, "pass 0 is not a budget pass");
        assert_ne!(outcome.passes & (1 << 1), 0, "pass 1 did the cutting");
        assert_eq!(outcome.passes & (1 << 5), 0, "nothing had to be removed");
    }

    #[test]
    fn removing_a_message_is_the_only_thing_that_sets_pass_five() {
        let mut messages = vec![text_msg("user", "the request")];
        for i in 0..12 {
            messages.push(text_msg("assistant", &format!("turn {i} ")));
            messages.push(text_msg("user", &"y".repeat(4_000)));
        }
        let outcome = trim_context_if_needed(&mut messages, 6_000, None, &[], plain());
        assert!(outcome.messages_removed > 0, "prose alone cannot fit 6 KB");
        assert_ne!(outcome.passes & (1 << 5), 0, "pass 5 evicted the pairs");
    }

    #[test]
    fn a_head_stub_records_the_true_size() {
        let body = format!("{}\n[{}]", "z".repeat(500_000), address(1));
        let head = head_stub(&body, LAST_RESORT_HEAD_CHARS, RecoveryClause::State);
        assert_eq!(
            stub_original_chars(&head),
            Some(body.len()),
            "the panel would price a 500 KB re-fetch at 20 KB"
        );
    }

    #[test]
    fn the_recovery_clause_is_mode_aware() {
        let body = format!("{}\n[{}]", "z".repeat(9_000), address(1));
        let control = budget_stub(&body, RecoveryClause::State);
        assert!(control.contains("events(action=\"query\""), "{control}");

        let mode = budget_stub(&body, RecoveryClause::Omit);
        assert!(!mode.contains("events(action="), "{mode}");
        assert!(
            mode.contains(&address(1)),
            "the address still reads it back"
        );
        assert_eq!(stub_original_chars(&mode), Some(body.len()));

        let head = head_stub(&body, 100, RecoveryClause::Omit);
        assert!(!head.contains("events(action="), "{head}");
        assert!(head.ends_with(&format!("[{}]", address(1))));
    }
}
