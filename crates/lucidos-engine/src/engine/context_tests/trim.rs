use super::*;
use crate::llm::model_registry::context_window_from_prefix;
use crate::llm::{ContentBlock, Message, MessageContent};

/// Nothing protected and nothing held open, which is every workspace with the
/// self-curated context mode off. That arm's stub names its own way back, so
/// `RecoveryClause::State` is the control-arm shape these tests measure.
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

/// A whole round's worth of parallel calls, batched into one message.
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

/// The matching results, all in the one user message the provider expects.
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

/// A valid `evt-<32 hex>` address, the form `synthesize_tool_use_id` renders.
const TEST_ADDRESS: &str = "evt-0123456789abcdef0123456789abcdef";

/// A tool result as the agent loop really ships it, address trailer and all.
fn addressed(body: &str) -> String {
    format!("{body}\n[{TEST_ADDRESS}]")
}

/// The `ToolResult` content at `idx`, block 0.
fn result_at(messages: &[Message], idx: usize) -> &str {
    match &messages[idx].content {
        MessageContent::Blocks(blocks) => match &blocks[0] {
            ContentBlock::ToolResult { content, .. } => content,
            _ => panic!("expected a ToolResult at {idx}"),
        },
        _ => panic!("expected Blocks at {idx}"),
    }
}

/// Every `ToolResult` content in the message at `idx`.
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
    let removed = trim_context_if_needed(&mut messages, 5_000, None, &[], plain()).messages_removed;

    assert_eq!(removed, 0, "pass 5 should not be needed");
    assert_eq!(messages.len(), original_len, "no messages removed");

    // The large tool result at index 2 should be truncated
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

    // The small tool result at index 4 should be unchanged
    if let MessageContent::Blocks(blocks) = &messages[4].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert_eq!(content, "small result");
        }
    }
}

#[test]
fn test_pass5_removes_old_pairs() {
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

    let initial_len = messages.len();
    // Set a very small budget to force pass 5
    let budget = 200;
    let removed =
        trim_context_if_needed(&mut messages, budget, None, &[], plain()).messages_removed;

    assert!(removed > 0, "should have removed messages in pass 5");
    // The returned count must match the actual length delta — `agentic_loop`
    // shifts its captured `user_message_idx` down by this number on every
    // iteration, so any over- or under-report would point the iteration-1
    // image-replacement hook at the wrong message.
    assert_eq!(
        messages.len(),
        initial_len - removed,
        "returned removed count must match actual length delta"
    );
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
    trim_context_if_needed(&mut messages, budget, None, &[], plain());

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
        image_msg("user", "old screenshot", 200), // older — strip
        text_msg("assistant", "I see it"),
        image_msg("user", "another old screenshot", 100), // older — strip
        text_msg("assistant", "Got it"),
        image_msg("user", "current screenshot", 100), // last (current) — keep
    ];

    // Budget large enough that no other trimming is needed after stripping
    trim_context_if_needed(&mut messages, 500_000, None, &[], plain());

    // Older image at index 0 should be replaced with placeholder
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

    // Older image at index 2 should also be stripped
    if let MessageContent::Blocks(blocks) = &messages[2].content {
        assert!(
            !blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "older image at index 2 should be stripped"
        );
    }

    // The LAST message — the current user message — must keep its image
    let last_idx = messages.len() - 1;
    if let MessageContent::Blocks(blocks) = &messages[last_idx].content {
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "current user message (last) image must be preserved"
        );
    }

    // All 5 messages should still be present
    assert_eq!(messages.len(), 5);
}

/// Regression for the layout used by `chat::process`:
///   `messages = resume_tool_blocks; messages.push(current_user_message);`
/// so the current user message — with the image bytes just resolved from the
/// blob store — is at the LAST index, not [0]. Pass 0 must preserve the LAST
/// message's images, not the first; otherwise the LLM sees the placeholder
/// `[image from earlier in conversation]` instead of the actual image bytes.
#[test]
fn test_pass0_preserves_current_message_image_with_resume_blocks() {
    let mut messages = vec![
        // Prior resume_tool_blocks — assistant tool_use + user tool_result pairs
        tool_use_msg("t1", "read_file", serde_json::json!({"path": "x.rs"})),
        tool_result_msg("t1", "file contents"),
        tool_use_msg("t2", "list_files", serde_json::json!({})),
        tool_result_msg("t2", "files: a, b, c"),
        // Current user message with an image attached (production layout)
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

/// An image block's budget cost is a small FIXED estimate (real provider token
/// cost), NOT its base64 byte length. This is what lets the current-turn image
/// stay in context for the whole turn without one photo dwarfing the budget and
/// forcing Pass 5 to evict real conversation/tool context.
#[test]
fn test_image_block_char_estimate_is_fixed_not_base64_len() {
    let small = estimate_message_chars(&image_msg("user", "shot", 10)); // 10 KB base64
    let huge = estimate_message_chars(&image_msg("user", "shot", 3_000)); // ~3 MB base64
    assert_eq!(
        small, huge,
        "image budget cost must not scale with base64 length"
    );
    // The fixed per-image cost is tiny relative to a real megabyte photo, so a
    // single image can never blow a normal char budget on its own.
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

/// The core regression for "the bot can't see my attached image": once the
/// model makes a tool call, the current-turn user message (with the image) is
/// no longer the LAST entry. Pass 0 must still keep its image bytes — driven by
/// an image pin — so the model can reason about the image after gathering
/// context. Without the fix the image was replaced with a text placeholder and
/// the model went blind mid-turn.
#[test]
fn test_pass0_keeps_current_turn_image_when_not_last() {
    let img_idx = 0;
    let mut messages = vec![
        // Current-turn user message with the attached image at index 0...
        image_msg("user", "Request: why won't this part fit?", 100),
        // ...followed by tool pairs the loop appended, so it is no longer last.
        tool_use_msg("t1", "list_files", serde_json::json!({})),
        tool_result_msg("t1", "files: a, b, c"),
        tool_use_msg("t2", "read_file", serde_json::json!({"path": "notes.md"})),
        tool_result_msg("t2", "notes"),
    ];

    trim_context_if_needed(&mut messages, 500_000, Some(img_idx), &[img_idx], plain());

    // The image bytes must survive on the (now non-last) current-turn message.
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

/// Pass 0 honours EVERY pinned index, not just one. Three image-bearing
/// messages, two pinned and one not — only the unpinned one loses its bytes.
///
/// One pin was the whole bug behind "the agent can't see the image": with a
/// single slot, an image the model re-loaded via `view_image` (or one the user
/// attached to a mid-turn injected message) had nowhere to be recorded, so pass
/// 0 stripped it on the model's very next tool call.
#[test]
fn test_pass0_keeps_every_pinned_image_message() {
    let mut messages = vec![
        image_msg("user", "Request: why won't this part fit?", 100), // 0 — pinned
        tool_use_msg("t1", "capture_app", serde_json::json!({})),
        image_msg("user", "app capture", 100), // 2 — ambient, NOT pinned
        tool_use_msg("t2", "view_image", serde_json::json!({"image": "thread:1"})),
        image_msg("user", "re-loaded thread image", 100), // 4 — pinned
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

/// The exact message the agentic loop builds for a `view_image` result: the
/// explicit-view `ToolResult` text, the lifted image block, and the trailing
/// instruction. `parse_app_capture_marker`'s branch writes the page's DOM text
/// as its `ToolResult` content instead, which is what `capture_app_result_msg`
/// mirrors.
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

/// Regression for the reported bug: the agent called `view_image` on a thread
/// image, made a few more tool calls, called `view_image` on the SAME reference
/// again, and still ended the turn telling the user it could not see the image.
///
/// `view_image` exists solely to bring an aged-out image back into vision, but
/// its result was pinned by nothing — so it survived only while it happened to
/// be the last message, and the model's very next tool call replaced it with
/// `[image from earlier in conversation]`. The tool could never do its one job.
///
/// Asserts the whole chain the loop relies on: the built message is recognised
/// as holding an explicitly-requested image, and once pinned its bytes survive
/// several later tool pairs.
#[test]
fn test_view_image_result_stays_visible_across_later_tool_calls() {
    use crate::engine::agentic_loop::holds_explicitly_requested_image;

    let mut messages = vec![
        text_msg("user", "Request: what is wrong in this screenshot?"),
        tool_use_msg("v1", "view_image", serde_json::json!({"image": "thread:3"})),
        view_image_result_msg("v1", 100),
    ];
    let view_idx = messages.len() - 1;

    // The loop derives the pin from the built blocks — assert that link holds,
    // otherwise the pin below is testing a fiction.
    let MessageContent::Blocks(blocks) = &messages[view_idx].content else {
        panic!("expected Blocks content for the view_image result");
    };
    assert!(
        holds_explicitly_requested_image(blocks),
        "the loop must recognise a view_image result as an explicitly-requested image"
    );

    // Several more tool calls, so the view_image result is far from last.
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

/// The other side of the boundary: an ambient `capture_app` screenshot is NOT
/// recognised as explicitly requested, so it stays unpinned and ages out after
/// one call. Screenshots snapshot state that changes under the model — keeping
/// them would let it reason about a UI that no longer exists, which is the
/// reason pass 0 strips images at all.
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

    // Pinning only the user message — exactly what the loop does for a capture.
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

/// Pass 5's eviction floor derives from the LOWEST pin only. Pinning a later
/// tool-result image (what `view_image` now does) must not make trimming any
/// weaker — otherwise every re-viewed image would ratchet the floor upward and
/// a long turn could no longer evict enough to fit the budget.
#[test]
fn test_pass5_floor_ignores_pins_above_the_lowest() {
    // Build the same over-budget history twice and trim with different pin sets.
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
        messages.push(image_msg("user", "Request: what is this?", 100)); // the low pin
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

/// With no pins, a non-last image-bearing message is still stripped (older
/// images the model already saw). This pins the "only pinned images are exempt"
/// boundary.
#[test]
fn test_pass0_strips_non_last_image_when_not_kept() {
    let mut messages = vec![
        image_msg("user", "older screenshot", 100), // index 0 — not kept, not last
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

/// After a mid-turn prompt injection the protected index (latest user input)
/// moves ABOVE the image-bearing message, so the image pin sits below `protected_idx`.
/// Pass 0 keeps the image bytes, but Pass 5 must ALSO refuse to remove that
/// whole message — it protects down to the lower of the two indices — or the
/// image (and the original request) is lost despite Pass 0 preserving its bytes.
#[test]
fn test_pass5_keeps_image_message_below_protected_after_injection() {
    let large_resume = "x".repeat(2_000);
    let mut messages = vec![text_msg("user", "workspace context")];
    // 10 resume tool pairs from turn 1 (20 messages).
    for i in 0..10 {
        let id = format!("resume{}", i);
        messages.push(tool_use_msg(
            &id,
            "read_file",
            serde_json::json!({"path": "x.rs"}),
        ));
        messages.push(tool_result_msg(&id, &large_resume));
    }
    // The current-turn image message — must survive with its image.
    let img_idx = messages.len();
    messages.push(image_msg("user", "Request: why won't this part fit?", 100));
    // A tool pair, then a mid-turn injected prompt (the new latest user input).
    messages.push(tool_use_msg(
        "mid",
        "grep",
        serde_json::json!({"pattern": "x"}),
    ));
    messages.push(tool_result_msg("mid", "match"));
    let injected_idx = messages.len();
    messages.push(text_msg("user", "actually also check the manual"));
    // Recent tail.
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

    // Tight budget forces pass 5 to remove many old pairs; protected_idx points
    // at the injected prompt, the image pin at the (lower) image message.
    let removed = trim_context_if_needed(
        &mut messages,
        1_000,
        Some(injected_idx),
        &[img_idx],
        plain(),
    )
    .messages_removed;

    // The image message must survive at its post-removal position, image intact.
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

/// Regression for a real long-running thread (2026-05-24). A long turn 1
/// left ~13 resume tool pairs in
/// `messages` for turn 2. As turn 2's tool loop appended assistant/user pairs
/// the current user message slid out of the last `PRESERVE_RECENT_MESSAGES`
/// slots; with no `protected_idx` guard pass 5 eventually removed it, leaving
/// the model with no record of the request line. The captured user_message_idx
/// — which is supposed to follow the message via `saturating_sub(removed_count)`
/// — silently pointed at whatever message ended up at the old slot, so even
/// the bookkeeping looked correct. Pinning the index is what makes the
/// promise true: with `protected_idx = Some(user_message_idx)`, pass 5
/// stops before removing the user message even at the cost of staying over
/// budget. (The attached image on that message is preserved separately by
/// its image pin — see `test_pass0_keeps_current_turn_image_when_not_last`.)
#[test]
fn test_pass5_does_not_remove_protected_user_message() {
    // Layout: m0 = workspace context, then many resume tool pairs (turn 1's
    // history), then the current user message, then a few recent tool pairs
    // from turn 2. Without the guard, pass 5 would chew through the resume
    // pairs and then start eating the current-turn messages, eventually
    // removing the protected user message itself.
    let large_resume = "x".repeat(2_000);
    let mut messages = vec![text_msg("user", "workspace context")];
    // 10 resume tool pairs from turn 1 (20 messages)
    for i in 0..10 {
        let id = format!("resume{}", i);
        messages.push(tool_use_msg(
            &id,
            "read_file",
            serde_json::json!({"path": "x.rs"}),
        ));
        messages.push(tool_result_msg(&id, &large_resume));
    }
    // Current user message — the one we must protect
    let protected = messages.len();
    messages.push(text_msg("user", "Request: what is in this screenshot?"));
    // 4 recent tool pairs from turn 2 (8 messages) — the recent-tail rule
    // alone protects these, but it does NOT cover the user message above
    // because it's no longer in the last 4.
    for i in 0..4 {
        let id = format!("recent{}", i);
        messages.push(tool_use_msg(
            &id,
            "grep",
            serde_json::json!({"pattern": "x"}),
        ));
        messages.push(tool_result_msg(&id, "match found"));
    }

    // Tight budget forces pass 5 to remove many messages.
    let removed = trim_context_if_needed(&mut messages, 1_000, Some(protected), &[], plain())
        .messages_removed;

    // The user message must survive at its post-removal position.
    let post_protected = protected.saturating_sub(removed);
    assert_eq!(
        messages[post_protected].content.as_text(),
        "Request: what is in this screenshot?",
        "pinned user message must survive pass 5"
    );
    // Recent tail is still preserved alongside the pinned message.
    let len = messages.len();
    if let MessageContent::Blocks(blocks) = &messages[len - 1].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert_eq!(content, "match found");
        }
    }
}

/// The guard must stop pass 5 cleanly: never panic, never produce a negative
/// index, never remove the protected message. That holds even when the budget
/// is small enough that pass 5 would otherwise want to eat the pinned slot
/// itself.
#[test]
fn test_pass5_stops_when_protected_lands_at_index_one() {
    let mut messages = vec![
        text_msg("user", "workspace context"),
        text_msg("user", "Request: original ask"), // protected
        text_msg("assistant", "I'll look into it"),
        text_msg("user", "tool result a"),
        text_msg("assistant", "more"),
        text_msg("user", "tool result b"),
    ];
    let protected = 1;

    // A tiny budget makes pass 5 remove everything between m0 and the protected
    // entry, then want to remove the protected entry itself to keep going. The
    // guard must stop instead.
    let removed =
        trim_context_if_needed(&mut messages, 10, Some(protected), &[], plain()).messages_removed;

    // Protected index hasn't shifted (nothing between m0 and it was removed
    // since it was already at index 1).
    assert_eq!(
        messages[protected].content.as_text(),
        "Request: original ask",
        "protected message at index 1 must not be removed"
    );
    // Guard at index 1 fires before any removal, so removed must be exactly 0.
    assert_eq!(
        removed, 0,
        "guard at index 1 must short-circuit before any removal"
    );
}

/// Regression for the May 25 `workspace-learning` trigger: a single turn
/// chained 8 `query_events` calls whose `ToolResult` payloads ended up in
/// the preserved tail. Pass 1 skips that tail (recent-message preservation
/// rule), pass 5 can't drop messages it preserves, so the trim returned
/// "done" while leaving 2.6 MB of tool-result content in the context.
/// The next LLM call sent 1.54 M tokens to a 1 M-cap API and the request
/// 400'd. Pass 3 must rescue this by truncating large `ToolResult`
/// blocks in the preserved tail (except the very last message).
#[test]
fn test_pass3_trims_large_tail_tool_results() {
    let huge = "x".repeat(TAIL_TRUNCATION_THRESHOLD + 5_000); // ≥ threshold
    let mut messages = vec![
        text_msg("user", "initial request"),
        // 5 old pairs that Pass 1 already truncated last iter
        tool_use_msg("old1", "grep", serde_json::json!({"pattern": "x"})),
        tool_result_msg("old1", "match"),
        tool_use_msg("old2", "grep", serde_json::json!({"pattern": "x"})),
        tool_result_msg("old2", "match"),
        // Preserved tail (last PRESERVE_RECENT_MESSAGES=4): two pairs of
        // huge tool-result dumps from query_events
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
    // Budget that pass 1 alone cannot meet (it skips the huge tail blocks)
    // but pass 3 can.
    let budget = TAIL_TRUNCATION_THRESHOLD; // tiny relative to total

    assert!(
        total_before > budget,
        "test premise: total must exceed budget"
    );

    let _removed =
        trim_context_if_needed(&mut messages, budget, None, &[], plain()).messages_removed;

    // The second-to-last preserved tool result (recent1's, at index len-3)
    // sits inside the tail but NOT in the very last message, so pass 3
    // must have truncated it.
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

/// Pass 3 must NEVER touch the very last message. It carries the tool result
/// the LLM has not read yet. Trimming it strips the data the next turn was
/// about to consume.
///
/// The budget here sits above the last message's own size on purpose. That
/// keeps pass 4, the last resort, out of the picture, so this test measures
/// pass 3 alone. `the_last_message_keeps_its_head_when_its_results_alone_exceed_the_budget`
/// covers the case where pass 4 does fire.
#[test]
fn test_pass3_preserves_very_last_message() {
    let huge = "x".repeat(TAIL_TRUNCATION_THRESHOLD + 5_000);
    let mut messages = vec![
        text_msg("user", "initial"),
        tool_use_msg("old", "grep", serde_json::json!({})),
        tool_result_msg("old", "match"),
        // Preserved tail
        tool_use_msg("r1", "tool", serde_json::json!({})),
        tool_result_msg("r1", &huge), // tail, not last, so trimmed
        tool_use_msg("r2", "tool", serde_json::json!({})),
        tool_result_msg("r2", &huge), // LAST message, preserved verbatim
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

/// Regression for the harden-discovered bug in the original Pass 3
/// commit: Pass 5 used `current_total = total_after_pass1`, ignoring
/// however many bytes Pass 3 freed. Result: Pass 5 evicted more old
/// pairs than necessary because it ran against an inflated total.
///
/// The fix is to start Pass 5 from the post-truncation total. This test
/// exercises the chain. Pass 3 truncates a huge tail block, the
/// remainder is *still* over budget, and Pass 5 walks old pairs. The
/// assertion distinguishes "Pass 5 read the post-pass-3 total and removed
/// just enough pairs" (fix) from "Pass 5 read the pre-pass-3 total and
/// removed every pair the message-count guard allowed" (bug).
///
/// Math (with `TRUNCATION_THRESHOLD = 500`, `TAIL_TRUNCATION_THRESHOLD
/// = 20_000`, `PRESERVE_RECENT_MESSAGES = 4`):
///   - 6 old pairs at 400-char `tool_result` each (under Pass-1's 500
///     threshold so they SURVIVE Pass 1 verbatim and contribute real
///     bytes to the post-pass-3 total): ~2_574 chars total.
///   - Initial user msg: ~7 chars.
///   - Preserved tail: 80_000-char `tool_result` at index 14 +
///     small tool_use msgs + 5-char `tool_result` at the last index.
///   - Total before Pass 3: ~82_622 chars.
///   - After Pass 3: tail's 80K block → a ~48-char budget stub.
///     Total ≈ 2_670 chars, still over `budget = 1_500` by ~1_170.
///   - Each old pair = ~429 chars. Pass 5 needs to drop ⌈1_170/429⌉ = 3
///     pairs (6 messages) to fit, then `current_total ≤ budget` exits.
///
/// Bug behavior (current_total = total_after_pass1 ≈ 82_622): Pass 5's
/// while-guard `current_total > budget` stays true through every pair
/// removal until the message-count guard `messages.len() > 5` fires
/// — i.e. removes all 6 old pairs (12 messages). The strict
/// `removed < 12` assert distinguishes the two paths.
#[test]
fn test_pass3_then_pass5_uses_post_truncation_total() {
    let huge = "x".repeat(TAIL_TRUNCATION_THRESHOLD * 4); // 80K
    let small_old_pair = "y".repeat(400); // < TRUNCATION_THRESHOLD (500)
    let mut messages = vec![text_msg("user", "initial")];
    for i in 0..6 {
        let id = format!("old{}", i);
        messages.push(tool_use_msg(&id, "grep", serde_json::json!({"p": "q"})));
        messages.push(tool_result_msg(&id, &small_old_pair));
    }
    // Preserved tail (last 4)
    messages.push(tool_use_msg("r1", "query_events", serde_json::json!({})));
    messages.push(tool_result_msg("r1", &huge)); // truncated by Pass 3
    messages.push(tool_use_msg("r2", "query_events", serde_json::json!({})));
    messages.push(tool_result_msg("r2", "small")); // last, preserved verbatim

    let budget = 1_500;
    let removed =
        trim_context_if_needed(&mut messages, budget, None, &[], plain()).messages_removed;

    // With the bug (`current_total = total_after_pass1`) Pass 5 would
    // remove every old pair until the message-count guard fires
    // (`removed == 12`). With the fix (`current_total =
    // total_after_truncation`) Pass 5 stops after a handful (≈ 6). The
    // strict-less-than assert distinguishes the two paths and stays
    // robust to small future shifts in old-pair size.
    assert!(
        removed < 12,
        "Pass 5 evicted {} messages (all 6 old pairs). The stale-current_total \
         bug is back: Pass 5 is reading total_after_pass1 (~82K) instead of \
         total_after_truncation (~2.6K), so its while-guard never reaches the \
         budget and only the message-count guard stops it.",
        removed
    );

    // Sanity: the truncated tail block at the post-removal `len - 3`
    // slot still carries the budget stub, so the test isn't passing
    // because Pass 3 silently regressed.
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

/// `ContextCaptured.trimmed` is derived from `TrimOutcome::any()`, so a turn
/// where pass 1 gutted a tool result but pass 5 evicted nothing must still
/// report as trimmed. It once reported `trimmed: false` here. The UI then
/// showed a clean turn while the LLM held a budget stub, not the real data.
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
    // Same budget as the pass-1 test above: enough that truncation alone fits.
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

/// Pass 1 also truncates oversized `ToolUse` ARGUMENT strings, not just tool
/// results. That is content the LLM can no longer see, so it must count too —
/// otherwise a turn whose only loss was a gutted tool argument still reports
/// `trimmed: false`.
#[test]
fn test_truncated_tool_arguments_count_as_trimmed() {
    let big_arg = "z".repeat(5_000);
    let mut messages = vec![
        text_msg("user", "initial"),
        // Old message with a huge tool ARGUMENT but a small result — only the
        // ToolUse branch of pass 1 can fire here.
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

/// `truncate_large_json_strings` counts every string it cuts, at any depth, and
/// reports zero when nothing needed cutting.
#[test]
fn test_truncate_large_json_strings_counts_what_it_cut() {
    let big = "q".repeat(TRUNCATION_THRESHOLD + 1);
    let mut nested = serde_json::json!({
        "small": "fine",
        "big": big,
        "nested": { "also_big": big, "arr": [big, "short"] },
    });
    // Three oversized strings: `big`, `nested.also_big`, `nested.arr[0]`.
    // `small` and `arr[1]` are under the threshold and left alone.
    assert_eq!(truncate_large_json_strings(&mut nested), 3);

    let mut untouched = serde_json::json!({ "a": "short", "b": [1, 2, 3] });
    assert_eq!(truncate_large_json_strings(&mut untouched), 0);
}

/// The complement: a turn that fits keeps every field zero, so an untouched
/// context never reports as trimmed.
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

/// `budget_tokens_from_chars` must be the exact inverse of the budget's
/// chars/token assumption. The trim log lines print the content and the
/// budget side by side, so a drift here makes "over budget" unreadable: the
/// same char budget would report as a token count that isn't the window. If
/// `agent_context_char_budget(model)` yields B chars on a window of W
/// tokens, then `budget_tokens_from_chars(B)` must yield ≤ W (saturating
/// down by at most 1 from integer division).
///
/// Note this is deliberately NOT `estimate_tokens_from_chars`, which answers
/// "how many tokens is this really" at a measured 2.5 chars/token and does
/// not round-trip the budget. See both doc comments in `context.rs`.
#[test]
fn test_budget_token_round_trip_matches_budget_ratio() {
    for model in ["claude-opus-4-7", "claude-opus-4-7[1m]", "gpt-5"] {
        let window = context_window_from_prefix(model);
        let usable = window - RESPONSE_TOKEN_RESERVE;
        let budget_chars = agent_context_char_budget(window);
        let budget_tokens = budget_tokens_from_chars(budget_chars);
        // Round-trip should land within 1 of usable (integer-division loss).
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

/// Pair-removal would also drop messages[2]; the guard must refuse if that
/// pair-mate is the protected message, rather than silently eating it.
#[test]
fn test_pass5_skips_pair_removal_that_would_drop_protected() {
    let large = "y".repeat(2_000);
    let mut messages = vec![
        text_msg("user", "workspace context"),
        tool_use_msg("t1", "read_file", serde_json::json!({"path": "x.rs"})),
        text_msg("user", "Request: original ask"), // protected — would be removed by pair logic
        text_msg("assistant", "looking"),
        tool_result_msg("filler", &large),
        text_msg("assistant", "still"),
        text_msg("user", "ok"),
    ];
    let protected = 2;

    let _ = trim_context_if_needed(&mut messages, 100, Some(protected), &[], plain());

    // Find the protected text — it must still be present somewhere.
    let still_present = messages
        .iter()
        .any(|m| m.content.as_text() == "Request: original ask");
    assert!(
        still_present,
        "pair removal must not drop the protected message"
    );
}

/// The T14 regression, and the reason this audit happened.
///
/// One round issued five parallel `read_file` calls. The next round opened
/// with three messages in the array. The old `preserve_start` collapsed onto
/// `len` at that count, so pass 1 swept the round that had just arrived. Every
/// document became a stub before the model read a word, and the thread spent
/// 70 more rounds re-reading the same five paths.
///
/// Nothing here is safe to cut. Message 0 is pinned, the tail is the round
/// itself, and the array is too short to remove a pair from. Shipping over
/// budget is the correct answer, and it is what the other arm did.
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

    // Above the round's own size, so the last-resort pass stays out. Below the
    // total, so the trimmer runs and has to decide it can do nothing.
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

/// A pass cuts what the budget needs and then stops.
///
/// The old pass 1 cut everything in range whatever the shortfall. On T14 that
/// meant destroying 193 KB to reclaim 34 KB.
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

    // One stub reclaims ~30 KB, which is more than the ~20 KB shortfall.
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

/// A stub is loud and recoverable, per ADR 0085's loss contract.
///
/// It states the size it replaced, names the call that reads the content back,
/// and keeps the bare address trailer that `stub_live_result` matches on.
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

/// Removal is the only silent loss, so it goes last.
///
/// Here the whole shortfall sits in old assistant prose. Pass 2 collapses two
/// of those messages and reaches the budget, so pass 5 never runs.
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

/// Pass 2 is role-gated, and this is why.
///
/// A mid-turn user injection carries a real instruction, said once. Collapsing
/// it loses the instruction with no way back, because a user message has no
/// event address to fetch.
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

/// One round's own results exceed the whole budget, so no eviction elsewhere
/// can help. Cut them largest first, and keep a head each time.
///
/// Erasing them outright guarantees the model re-fetches what it just asked
/// for. That is a livelock, not a saving. A head plus the address lets the
/// round progress.
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

/// A stub that is longer than what it replaces must not be applied.
///
/// A head stub carries a note and an address, so on a result barely over the
/// head threshold it is the bigger of the two. Applying it grows the request
/// while the running total stands still. Every later pass then works from a
/// number that no longer matches the messages.
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

/// A failed action is the one thing no STUBBING pass may cut. Pass 5 is
/// deliberately not bound by it: removal at the wall is what makes a request
/// fit at all.
///
/// A KEEP is not in here. It moves the item's clock and exempts nothing, so the
/// wall can always cut. What a keep buys at the wall is ordering.
mod protected_addresses {
    use super::*;
    use std::collections::HashSet;

    /// `\n[` + a 36-char address + `]`, which is what `address_trailer` reads.
    const TRAILER_CHARS: usize = 39;

    fn address(byte: u8) -> String {
        format!("evt-{}", format!("{byte:02x}").repeat(16))
    }

    fn addressed_result(id: &str, byte: u8, size: usize) -> Message {
        tool_result_msg(id, &format!("{}\n[{}]", "x".repeat(size), address(byte)))
    }

    /// Six pairs, each holding one large addressed result.
    fn thread() -> Vec<Message> {
        let mut messages = vec![text_msg("user", "the request")];
        for (i, byte) in [1u8, 2, 3, 4, 5, 6].into_iter().enumerate() {
            let id = format!("call-{i}");
            messages.push(tool_use_msg(&id, "read_file", serde_json::json!({})));
            messages.push(addressed_result(&id, byte, 60_000));
        }
        messages
    }

    /// Every tool result still in the array, by address.
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

    /// The baseline: with nothing protected, a tight budget cuts results.
    #[test]
    fn an_unprotected_thread_is_cut() {
        let mut messages = thread();
        let none = HashSet::new();
        let outcome =
            trim_context_if_needed(&mut messages, 5_000, Some(0), &[], guards(&none, &none));
        assert!(outcome.blocks_truncated > 0, "the baseline must cut");
    }

    /// An opt-out that yields under pressure is not an opt-out. What survives
    /// removal must survive whole, never as a stub.
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

    /// Manus's finding: leaving mistakes in context is what stops the model
    /// repeating them. It is the one exemption the trimmer honours.
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

    /// Protection is per address. Everything else still goes, or it would be a
    /// way to defeat the backstop entirely.
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

    /// Invariant 36. Nothing the model does can stop the trimmer cutting: a
    /// whole-cycle keep would otherwise be a way to wedge its own turn.
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

    /// Invariant 37. The trimmer walks oldest-first by position, and a held
    /// item sits at an old position. A naive pass destroys exactly what the
    /// model asked to hold.
    #[test]
    fn the_wall_takes_held_items_last() {
        let mut messages = thread();
        // The OLDEST result is the one held open, so position alone would take
        // it first. Only ordering can save it.
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

    /// Removal at the wall is the backstop that makes a request fit at all.
    /// A protected address early in the array must not freeze it.
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

    /// The last-resort pass reaches into the newest message and honours the
    /// same set. Without this an exemption would hold everywhere except the one
    /// place the model just asked for.
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

/// The live working understanding is the one thing no pass may cut.
///
/// It needs no key. The loop renders it at the tail of the NEWEST message
/// every round. Pass 1 does not reach there, pass 2 skips it as non-assistant,
/// and pass 5 cannot remove it. These pin that reasoning, because it is what
/// holds the model's compressed memory up.
mod the_live_document_is_protected {
    use super::*;

    fn address(n: u8) -> String {
        format!("evt-{}", format!("{n:02x}").repeat(16))
    }

    /// Four messages, so pass 5's `PRESERVE_RECENT_MESSAGES` floor puts every
    /// one of them out of its reach.
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

    /// The document rides at the tail as a `Text` block on the newest message,
    /// and a hard trim must leave it whole.
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

    /// An oversized `tool_use` ARGUMENT is not protected, and that is the
    /// contrast the tail block above earns its protection against. Sparing
    /// every large argument would forbid reclaiming thousands of chars under
    /// pressure, and pass 1 is where they go.
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

    /// A round that fits sets no bit, and the mask has room for six passes.
    ///
    /// The eval's utilisation axis reads this to say WHERE a round lost content.
    /// Only pass 5 removes a message, so a mask without bit 5 means every loss
    /// on that round left an addressed stub behind.
    #[test]
    fn a_trim_that_did_not_fire_reports_no_pass() {
        let mut messages = vec![text_msg("user", "short")];
        let outcome = trim_context_if_needed(&mut messages, 500_000, None, &[], plain());
        assert_eq!(outcome.passes, 0);
        assert!(!outcome.any());
    }

    /// Pass 1 stubs oversized results in OLD messages, so the big result has to
    /// sit outside the preserved tail for it to fire. Pass 0 strips image bytes
    /// before the budget gate and is not a budget pass, so it never sets a bit.
    #[test]
    fn stubbing_an_old_result_sets_pass_one_and_nothing_below_it() {
        let mut messages = vec![
            text_msg("user", "the request"),
            tool_use_msg("t1", "read_file", serde_json::json!({"path": "a.md"})),
            tool_result_msg("t1", &"x".repeat(60_000)),
        ];
        // Four short pairs of tail, so `preserve_start` clears the result.
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

    /// Pass 5 is the only one that removes a message, and the only one whose
    /// loss leaves no stub. So bit 5 is the one a reader looks at, and it must
    /// be set exactly when a message was evicted.
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

    /// A head stub keeps 20,000 chars of the body. The re-fetch price the panel
    /// shows must stay the ORIGINAL size rather than the head's.
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

    /// Invariant 35 and decision 8. The mode drops the fetch-back command from
    /// a placeholder, and the control arm keeps it: `with_event_address` stamps
    /// both arms, so a shared change would move ADR 0087's baseline.
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
