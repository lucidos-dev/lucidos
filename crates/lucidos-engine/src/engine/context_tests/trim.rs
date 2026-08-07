use super::*;
use crate::llm::model_registry::context_window_from_prefix;
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

#[test]
fn test_no_trim_under_budget() {
    let mut messages = vec![
        text_msg("user", "Hello"),
        text_msg("assistant", "Hi there"),
        text_msg("user", "How are you?"),
    ];
    let original_len = messages.len();
    let removed = trim_context_if_needed(&mut messages, 500_000, None, &[]).messages_removed;
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
    let removed = trim_context_if_needed(&mut messages, 5_000, None, &[]).messages_removed;

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

    let initial_len = messages.len();
    // Set a very small budget to force pass 2
    let budget = 200;
    let removed = trim_context_if_needed(&mut messages, budget, None, &[]).messages_removed;

    assert!(removed > 0, "should have removed messages in pass 2");
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
    trim_context_if_needed(&mut messages, budget, None, &[]);

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
    trim_context_if_needed(&mut messages, 500_000, None, &[]);

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

    trim_context_if_needed(&mut messages, 500_000, None, &[]);

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
/// forcing Pass 2 to evict real conversation/tool context.
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

    trim_context_if_needed(&mut messages, 500_000, Some(img_idx), &[img_idx]);

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

    trim_context_if_needed(&mut messages, 500_000, Some(0), &[0, 4]);

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

    trim_context_if_needed(&mut messages, 500_000, Some(0), &[0, view_idx]);

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
    trim_context_if_needed(&mut messages, 500_000, Some(0), &[0]);

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

/// Pass 2's eviction floor derives from the LOWEST pin only. Pinning a later
/// tool-result image (what `view_image` now does) must not make trimming any
/// weaker — otherwise every re-viewed image would ratchet the floor upward and
/// a long turn could no longer evict enough to fit the budget.
#[test]
fn test_pass2_floor_ignores_pins_above_the_lowest() {
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
        trim_context_if_needed(&mut baseline, 1_000, Some(low_pin), &[low_pin]).messages_removed;

    let mut with_high_pin = build();
    let with_high_removed = trim_context_if_needed(
        &mut with_high_pin,
        1_000,
        Some(low_pin),
        &[low_pin, high_pin],
    )
    .messages_removed;

    assert!(
        baseline_removed > 0,
        "the fixture must actually force pass 2 to evict, or this proves nothing"
    );
    assert_eq!(
        with_high_removed, baseline_removed,
        "a pin above the floor must not reduce how much pass 2 can evict"
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

    trim_context_if_needed(&mut messages, 500_000, None, &[]);

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
/// Pass 0 keeps the image bytes, but Pass 2 must ALSO refuse to remove that
/// whole message — it protects down to the lower of the two indices — or the
/// image (and the original request) is lost despite Pass 0 preserving its bytes.
#[test]
fn test_pass2_keeps_image_message_below_protected_after_injection() {
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

    // Tight budget forces pass 2 to remove many old pairs; protected_idx points
    // at the injected prompt, the image pin at the (lower) image message.
    let removed = trim_context_if_needed(&mut messages, 1_000, Some(injected_idx), &[img_idx])
        .messages_removed;

    // The image message must survive at its post-removal position, image intact.
    let post_img = img_idx.saturating_sub(removed);
    if let MessageContent::Blocks(blocks) = &messages[post_img].content {
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
            "the image-bearing message must survive pass 2 even when protected_idx diverges above it"
        );
    } else {
        panic!("expected the image message to survive with its Blocks content");
    }
}

/// Regression for a real long-running thread (2026-05-24). A long turn 1
/// left ~13 resume tool pairs in
/// `messages` for turn 2. As turn 2's tool loop appended assistant/user pairs
/// the current user message slid out of the last `PRESERVE_RECENT_MESSAGES`
/// slots; with no `protected_idx` guard pass 2 eventually removed it, leaving
/// the model with no record of the request line. The captured user_message_idx
/// — which is supposed to follow the message via `saturating_sub(removed_count)`
/// — silently pointed at whatever message ended up at the old slot, so even
/// the bookkeeping looked correct. Pinning the index is what makes the
/// promise true: with `protected_idx = Some(user_message_idx)`, pass 2
/// stops before removing the user message even at the cost of staying over
/// budget. (The attached image on that message is preserved separately by
/// its image pin — see `test_pass0_keeps_current_turn_image_when_not_last`.)
#[test]
fn test_pass2_does_not_remove_protected_user_message() {
    // Layout: m0 = workspace context, then many resume tool pairs (turn 1's
    // history), then the current user message, then a few recent tool pairs
    // from turn 2. Without the guard, pass 2 would chew through the resume
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

    // Tight budget forces pass 2 to remove many messages.
    let removed =
        trim_context_if_needed(&mut messages, 1_000, Some(protected), &[]).messages_removed;

    // The user message must survive at its post-removal position.
    let post_protected = protected.saturating_sub(removed);
    assert_eq!(
        messages[post_protected].content.as_text(),
        "Request: what is in this screenshot?",
        "pinned user message must survive pass 2"
    );
    // Recent tail is still preserved alongside the pinned message.
    let len = messages.len();
    if let MessageContent::Blocks(blocks) = &messages[len - 1].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert_eq!(content, "match found");
        }
    }
}

/// The guard must stop pass 2 cleanly — never panic, never produce a
/// negative index, never remove the protected message — even when the
/// budget is small enough that pass 2 would otherwise want to eat the
/// pinned slot itself.
#[test]
fn test_pass2_stops_when_protected_lands_at_index_one() {
    let mut messages = vec![
        text_msg("user", "workspace context"),
        text_msg("user", "Request: original ask"), // protected
        text_msg("assistant", "I'll look into it"),
        text_msg("user", "tool result a"),
        text_msg("assistant", "more"),
        text_msg("user", "tool result b"),
    ];
    let protected = 1;

    // Tiny budget — pass 2 would remove everything between m0 and the
    // protected entry, then would want to remove the protected entry itself
    // to keep going. The guard must stop instead.
    let removed = trim_context_if_needed(&mut messages, 10, Some(protected), &[]).messages_removed;

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
/// rule), pass 2 can't drop messages it preserves, so the trim returned
/// "done" while leaving 2.6 MB of tool-result content in the context.
/// The next LLM call sent 1.54 M tokens to a 1 M-cap API and the request
/// 400'd. Pass 1.5 must rescue this by truncating large `ToolResult`
/// blocks in the preserved tail (except the very last message).
#[test]
fn test_pass1_5_trims_large_tail_tool_results() {
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
    // but pass 1.5 can.
    let budget = TAIL_TRUNCATION_THRESHOLD; // tiny relative to total

    assert!(
        total_before > budget,
        "test premise: total must exceed budget"
    );

    let _removed = trim_context_if_needed(&mut messages, budget, None, &[]).messages_removed;

    // The second-to-last preserved tool result (recent1's, at index len-3)
    // sits inside the tail BUT NOT in the very last message — pass 1.5
    // must have truncated it.
    let len = messages.len();
    if let MessageContent::Blocks(blocks) = &messages[len - 3].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert!(
                content.contains("truncated"),
                "huge tail ToolResult (not the last message) must be truncated by pass 1.5"
            );
        } else {
            panic!("expected ToolResult block at len-3");
        }
    } else {
        panic!("expected Blocks at len-3");
    }
}

/// Pass 1.5 must NEVER touch the very last message — it carries the
/// most recent tool result the LLM hasn't yet reasoned about. Trimming
/// it strips the data the next turn was about to consume.
#[test]
fn test_pass1_5_preserves_very_last_message() {
    let huge = "x".repeat(TAIL_TRUNCATION_THRESHOLD + 5_000);
    let mut messages = vec![
        text_msg("user", "initial"),
        tool_use_msg("old", "grep", serde_json::json!({})),
        tool_result_msg("old", "match"),
        // Preserved tail
        tool_use_msg("r1", "tool", serde_json::json!({})),
        tool_result_msg("r1", &huge), // tail, not last → trimmed
        tool_use_msg("r2", "tool", serde_json::json!({})),
        tool_result_msg("r2", &huge), // LAST message → preserved verbatim
    ];

    let budget = TAIL_TRUNCATION_THRESHOLD;
    let _ = trim_context_if_needed(&mut messages, budget, None, &[]);

    let last_idx = messages.len() - 1;
    if let MessageContent::Blocks(blocks) = &messages[last_idx].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert!(
                !content.contains("truncated"),
                "the very last message's ToolResult must NOT be truncated by pass 1.5 (it's the data the next turn will reason about)"
            );
            assert_eq!(
                content.len(),
                huge.len(),
                "verbatim length must be preserved"
            );
        }
    }
}

/// Regression for the harden-discovered bug in the original Pass 1.5
/// commit: Pass 2 used `current_total = total_after_pass1`, ignoring
/// however many bytes Pass 1.5 freed. Result: Pass 2 evicted more old
/// pairs than necessary because it ran against an inflated total.
///
/// The fix is to start Pass 2 from the post-truncation total. This test
/// exercises the chain — Pass 1.5 truncates a huge tail block, the
/// remainder is *still* over budget, and Pass 2 walks old pairs. The
/// assertion distinguishes "Pass 2 read the post-1.5 total and removed
/// just enough pairs" (fix) from "Pass 2 read the pre-1.5 total and
/// removed every pair the message-count guard allowed" (bug).
///
/// Math (with `TRUNCATION_THRESHOLD = 500`, `TAIL_TRUNCATION_THRESHOLD
/// = 20_000`, `PRESERVE_RECENT_MESSAGES = 4`):
///   - 6 old pairs at 400-char `tool_result` each (under Pass-1's 500
///     threshold so they SURVIVE Pass 1 verbatim and contribute real
///     bytes to the post-1.5 total): ~2_574 chars total.
///   - Initial user msg: ~7 chars.
///   - Preserved tail: 80_000-char `tool_result` at index 14 +
///     small tool_use msgs + 5-char `tool_result` at the last index.
///   - Total before Pass 1.5: ~82_622 chars.
///   - After Pass 1.5: tail's 80K block → ~39-char truncation note.
///     Total ≈ 2_659 chars — still over `budget = 1_500` by ~1_159.
///   - Each old pair = ~429 chars. Pass 2 needs to drop ⌈1_159/429⌉ = 3
///     pairs (6 messages) to fit, then `current_total ≤ budget` exits.
///
/// Bug behavior (current_total = total_after_pass1 ≈ 82_622): Pass 2's
/// while-guard `current_total > budget` stays true through every pair
/// removal until the message-count guard `messages.len() > 5` fires
/// — i.e. removes all 6 old pairs (12 messages). The strict
/// `removed < 12` assert distinguishes the two paths.
#[test]
fn test_pass1_5_then_pass2_uses_post_truncation_total() {
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
    messages.push(tool_result_msg("r1", &huge)); // truncated by Pass 1.5
    messages.push(tool_use_msg("r2", "query_events", serde_json::json!({})));
    messages.push(tool_result_msg("r2", "small")); // last, preserved verbatim

    let budget = 1_500;
    let removed = trim_context_if_needed(&mut messages, budget, None, &[]).messages_removed;

    // With the bug (`current_total = total_after_pass1`) Pass 2 would
    // remove every old pair until the message-count guard fires
    // (`removed == 12`). With the fix (`current_total =
    // total_after_truncation`) Pass 2 stops after a handful (≈ 6). The
    // strict-less-than assert distinguishes the two paths and stays
    // robust to small future shifts in old-pair size.
    assert!(
        removed < 12,
        "Pass 2 evicted {} messages (all 6 old pairs). The stale-current_total \
         bug is back: Pass 2 is reading total_after_pass1 (~82K) instead of \
         total_after_truncation (~2.6K), so its while-guard never reaches the \
         budget and only the message-count guard stops it.",
        removed
    );

    // Sanity: the truncated tail block at the post-removal `len - 3`
    // slot still carries the truncation note so the test isn't passing
    // because Pass 1.5 silently regressed.
    let len = messages.len();
    if let MessageContent::Blocks(blocks) = &messages[len - 3].content {
        if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
            assert!(
                content.contains("truncated"),
                "expected the tail tool_result at len-3 to carry Pass 1.5's truncation note"
            );
        }
    }
}

/// `ContextCaptured.trimmed` is derived from `TrimOutcome::any()`, so a turn
/// where pass 1 gutted a tool result but pass 2 evicted nothing must still
/// report as trimmed. Before this it reported `trimmed: false` — the UI showed
/// a clean turn while the LLM had been handed
/// "[content truncated — was N chars]" in place of the real data.
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
    let outcome = trim_context_if_needed(&mut messages, 5_000, None, &[]);

    assert_eq!(outcome.messages_removed, 0, "pass 2 should not be needed");
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
    let outcome = trim_context_if_needed(&mut messages, 2_000, None, &[]);

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
    let outcome = trim_context_if_needed(&mut messages, 500_000, None, &[]);
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
fn test_pass2_skips_pair_removal_that_would_drop_protected() {
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

    let _ = trim_context_if_needed(&mut messages, 100, Some(protected), &[]);

    // Find the protected text — it must still be present somewhere.
    let still_present = messages
        .iter()
        .any(|m| m.content.as_text() == "Request: original ask");
    assert!(
        still_present,
        "pair removal must not drop the protected message"
    );
}
