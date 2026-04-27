use super::*;
use crate::core::store::{SessionMessage, UserImagePayload};

fn make_img(data: &str, mime: &str) -> UserImagePayload {
    UserImagePayload {
        base64: data.to_string(),
        mime_type: mime.to_string(),
    }
}

fn make_chat_img(data: &str, mime: &str) -> crate::api::ChatImage {
    crate::api::ChatImage {
        base64: data.to_string(),
        mime_type: mime.to_string(),
    }
}

// --- build_user_content_with_images tests ---

#[test]
fn no_images_returns_text_only() {
    let result = build_user_content_with_images("hello".into(), &[], None);
    match result {
        MessageContent::Text(t) => assert_eq!(t, "hello"),
        _ => panic!("expected Text, got Blocks"),
    }
}

#[test]
fn current_image_only() {
    let imgs = vec![make_chat_img("AAAA", "image/jpeg")];
    let result = build_user_content_with_images("check this".into(), &[], Some(&imgs));
    match result {
        MessageContent::Blocks(blocks) => {
            // Text with hint + 1 image = 2 blocks
            assert_eq!(blocks.len(), 2);
            assert!(
                matches!(&blocks[0], ContentBlock::Text { text } if text.contains("check this"))
            );
            assert!(
                matches!(&blocks[0], ContentBlock::Text { text } if text.contains("attached to this message"))
            );
            assert!(matches!(&blocks[1], ContentBlock::Image { data, .. } if data == "AAAA"));
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn history_images_labeled_as_earlier() {
    let history = vec![vec![make_img("IMG1", "image/jpeg")]];
    let result = build_user_content_with_images("what was in the image?".into(), &history, None);
    match result {
        MessageContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 2);
            assert!(
                matches!(&blocks[0], ContentBlock::Text { text } if text.contains("what was in the image?"))
            );
            assert!(matches!(&blocks[0], ContentBlock::Text { text } if text.contains("earlier")));
            assert!(matches!(&blocks[1], ContentBlock::Image { data, .. } if data == "IMG1"));
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn mixed_history_and_current_images_separated() {
    let history = vec![
        vec![make_img("IMG1", "image/jpeg")],
        vec![
            make_img("IMG2", "image/jpeg"),
            make_img("IMG3", "image/jpeg"),
        ],
    ];
    let current = vec![make_chat_img("IMG4", "image/png")];
    let result = build_user_content_with_images("summarize all".into(), &history, Some(&current));
    match result {
        MessageContent::Blocks(blocks) => {
            // text_hint + IMG1 + IMG2 + IMG3 + separator_text + IMG4 = 6 blocks
            assert_eq!(blocks.len(), 6);
            let img_count = blocks
                .iter()
                .filter(|b| matches!(b, ContentBlock::Image { .. }))
                .count();
            assert_eq!(img_count, 4);
            // Hint distinguishes earlier vs current
            assert!(matches!(&blocks[0], ContentBlock::Text { text } if text.contains("earlier")));
            assert!(
                matches!(&blocks[0], ContentBlock::Text { text } if text.contains("current message"))
            );
            // Separator text between history and current images
            let text_count = blocks
                .iter()
                .filter(|b| matches!(b, ContentBlock::Text { .. }))
                .count();
            assert_eq!(text_count, 2);
            // Last image is the current one
            assert!(matches!(&blocks[5], ContentBlock::Image { data, .. } if data == "IMG4"));
            // Separator is before the last image
            assert!(
                matches!(&blocks[4], ContentBlock::Text { text } if text.contains("current message"))
            );
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn current_vs_history_images_distinguished() {
    // Bug: when both history and current images exist, the LLM can't tell
    // which images are from the current message vs earlier in conversation.
    let history = vec![
        vec![make_img("OLD1", "image/jpeg")],
        vec![make_img("OLD2", "image/jpeg")],
    ];
    let current = vec![make_chat_img("NEW1", "image/png")];
    let result = build_user_content_with_images("here is a photo".into(), &history, Some(&current));
    match result {
        MessageContent::Blocks(blocks) => {
            // Must contain a text separator distinguishing current images from history
            let text_blocks: Vec<&str> = blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();

            // There should be a separator between history and current images
            let has_current_marker = text_blocks.iter().any(|t| t.contains("current message"));
            assert!(
                has_current_marker,
                "must have a marker identifying current-message images, got: {:?}",
                text_blocks
            );

            // The hint should mention earlier images exist
            let has_earlier_marker = text_blocks.iter().any(|t| t.contains("earlier"));
            assert!(
                has_earlier_marker,
                "must indicate some images are from earlier, got: {:?}",
                text_blocks
            );

            // All 3 images still present
            let img_count = blocks
                .iter()
                .filter(|b| matches!(b, ContentBlock::Image { .. }))
                .count();
            assert_eq!(img_count, 3);
        }
        _ => panic!("expected Blocks"),
    }
}

// --- save_images_to_tmp tests ---

#[test]
fn save_images_to_tmp_writes_files() {
    use base64::Engine as _;
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let images = vec![crate::api::ChatImage {
        base64: base64::engine::general_purpose::STANDARD.encode(b"fake-image-data"),
        mime_type: "image/jpeg".to_string(),
    }];
    let paths = save_images_to_tmp(&workspace, &images);
    assert_eq!(paths.len(), 1);
    assert!(paths[0].starts_with(".lucidos/tmp/images/"));
    assert!(paths[0].ends_with(".jpg"));
    let full_path = workspace.join(&paths[0]);
    assert!(full_path.exists());
    let contents = std::fs::read(&full_path).unwrap();
    assert_eq!(contents, b"fake-image-data");
}

#[test]
fn save_images_to_tmp_handles_multiple() {
    use base64::Engine as _;
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let images = vec![
        crate::api::ChatImage {
            base64: base64::engine::general_purpose::STANDARD.encode(b"img1"),
            mime_type: "image/png".to_string(),
        },
        crate::api::ChatImage {
            base64: base64::engine::general_purpose::STANDARD.encode(b"img2"),
            mime_type: "image/jpeg".to_string(),
        },
    ];
    let paths = save_images_to_tmp(&workspace, &images);
    assert_eq!(paths.len(), 2);
    assert!(paths[0].ends_with(".png"));
    assert!(paths[1].ends_with(".jpg"));
    assert_eq!(std::fs::read(workspace.join(&paths[0])).unwrap(), b"img1");
    assert_eq!(std::fs::read(workspace.join(&paths[1])).unwrap(), b"img2");
}

#[test]
fn save_images_to_tmp_empty_returns_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = save_images_to_tmp(tmp.path(), &[]);
    assert!(paths.is_empty());
}

#[test]
fn oversized_history_images_skipped() {
    let big_data = "X".repeat(MAX_TOTAL_IMAGE_BASE64 + 1);
    let history = vec![
        vec![make_img(&big_data, "image/jpeg")],
        vec![make_img("small", "image/jpeg")],
    ];
    let result = build_user_content_with_images("test".into(), &history, None);
    match result {
        MessageContent::Blocks(blocks) => {
            let img_count = blocks
                .iter()
                .filter(|b| matches!(b, ContentBlock::Image { .. }))
                .count();
            assert_eq!(img_count, 1);
            assert!(matches!(&blocks[1], ContentBlock::Image { data, .. } if data == "small"));
        }
        _ => panic!("expected Blocks with only small image"),
    }
}

// --- staleness warning in build_user_content_with_images ---

#[test]
fn history_images_include_staleness_warning() {
    let history = vec![vec![make_img("IMG1", "image/jpeg")]];
    let result = build_user_content_with_images("what changed?".into(), &history, None);
    match result {
        MessageContent::Blocks(blocks) => {
            let text = match &blocks[0] {
                ContentBlock::Text { text } => text,
                _ => panic!("expected text"),
            };
            assert!(
                text.contains("may not reflect current state"),
                "history-only hint should warn about staleness, got: {}",
                text
            );
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn current_images_no_staleness_warning() {
    let imgs = vec![make_chat_img("NEW", "image/jpeg")];
    let result = build_user_content_with_images("check this".into(), &[], Some(&imgs));
    match result {
        MessageContent::Blocks(blocks) => {
            let text = match &blocks[0] {
                ContentBlock::Text { text } => text,
                _ => panic!("expected text"),
            };
            assert!(
                !text.contains("may not reflect current state"),
                "current-only hint should NOT warn about staleness, got: {}",
                text
            );
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn mixed_images_include_staleness_warning() {
    let history = vec![vec![make_img("OLD", "image/jpeg")]];
    let current = vec![make_chat_img("NEW", "image/png")];
    let result = build_user_content_with_images("compare".into(), &history, Some(&current));
    match result {
        MessageContent::Blocks(blocks) => {
            let text = match &blocks[0] {
                ContentBlock::Text { text } => text,
                _ => panic!("expected text"),
            };
            assert!(
                text.contains("may not reflect current state"),
                "mixed hint should warn about staleness for history images, got: {}",
                text
            );
        }
        _ => panic!("expected Blocks"),
    }
}

// --- filter_recent_history_images tests ---

fn make_user_msg(content: &str, images: Vec<UserImagePayload>) -> SessionMessage {
    SessionMessage {
        role: "user".to_string(),
        content: content.to_string(),
        created_at: chrono::Utc::now(),
        channel: None,
        steps: vec![],
        images: vec![],
        user_images: images,
        image_description: None,
        completed: None,
        canceled: false,
        aborted: false,
        text_chunks: vec![],
        events: vec![],
        request_event_id: None,
        event_id: None,
        thread_id: None,
    }
}

fn make_assistant_msg(content: &str) -> SessionMessage {
    SessionMessage {
        role: "assistant".to_string(),
        content: content.to_string(),
        created_at: chrono::Utc::now(),
        channel: None,
        steps: vec![],
        images: vec![],
        user_images: vec![],
        image_description: None,
        completed: Some(true),
        canceled: false,
        aborted: false,
        text_chunks: vec![],
        events: vec![],
        request_event_id: None,
        event_id: None,
        thread_id: None,
    }
}

#[test]
fn filter_images_empty_thread() {
    let result = filter_recent_history_images(&[], 3);
    assert!(result.is_empty());
}

#[test]
fn filter_images_single_message_with_image() {
    let msgs = vec![make_user_msg(
        "screenshot",
        vec![make_img("IMG1", "image/png")],
    )];
    let result = filter_recent_history_images(&msgs, 3);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0].base64, "IMG1");
}

#[test]
fn filter_images_within_limit_keeps_all() {
    let msgs = vec![
        make_user_msg("img1", vec![make_img("A", "image/png")]),
        make_assistant_msg("response 1"),
        make_user_msg("img2", vec![make_img("B", "image/png")]),
        make_assistant_msg("response 2"),
        make_user_msg("img3", vec![make_img("C", "image/png")]),
    ];
    let result = filter_recent_history_images(&msgs, 3);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0][0].base64, "A");
    assert_eq!(result[1][0].base64, "B");
    assert_eq!(result[2][0].base64, "C");
}

#[test]
fn filter_images_drops_oldest_when_over_limit() {
    // 5 user messages with images, limit 3 → only last 3 kept
    let msgs = vec![
        make_user_msg("old screenshot", vec![make_img("OLD1", "image/png")]),
        make_assistant_msg("response"),
        make_user_msg("another old one", vec![make_img("OLD2", "image/png")]),
        make_assistant_msg("response"),
        make_user_msg("mid thread", vec![make_img("MID", "image/png")]),
        make_assistant_msg("response"),
        make_user_msg("recent", vec![make_img("RECENT1", "image/png")]),
        make_assistant_msg("response"),
        make_user_msg("latest", vec![make_img("LATEST", "image/png")]),
    ];
    let result = filter_recent_history_images(&msgs, 3);
    assert_eq!(
        result.len(),
        3,
        "should only keep 3 most recent image messages"
    );
    // OLD1 and OLD2 should be dropped
    assert_eq!(result[0][0].base64, "MID");
    assert_eq!(result[1][0].base64, "RECENT1");
    assert_eq!(result[2][0].base64, "LATEST");
}

#[test]
fn filter_images_counts_all_user_messages_for_recency() {
    // 6 user messages: image "A" at position 2, image "B" at position 6.
    // With max_messages=3, only the last 3 user messages are eligible
    // (positions 4, 5, 6). Image "A" at position 2 is stale → dropped.
    let msgs = vec![
        make_user_msg("text only 1", vec![]),
        make_assistant_msg("response"),
        make_user_msg("with image", vec![make_img("A", "image/png")]),
        make_assistant_msg("response"),
        make_user_msg("text only 2", vec![]),
        make_assistant_msg("response"),
        make_user_msg("text only 3", vec![]),
        make_assistant_msg("response"),
        make_user_msg("text only 4", vec![]),
        make_assistant_msg("response"),
        make_user_msg("another image", vec![make_img("B", "image/png")]),
    ];
    let result = filter_recent_history_images(&msgs, 3);
    assert_eq!(
        result.len(),
        1,
        "only image B should survive — A is 4 messages stale"
    );
    assert_eq!(result[0][0].base64, "B");
}

#[test]
fn filter_images_ignores_assistant_images() {
    let mut assistant = make_assistant_msg("here is a generated image");
    assistant.images = vec!["generated.png".to_string()];
    assistant.user_images = vec![make_img("SHOULD_NOT_APPEAR", "image/png")];
    // Even if assistant message has user_images set (shouldn't happen), role filter blocks it
    let msgs = vec![
        make_user_msg("user img", vec![make_img("USER", "image/png")]),
        assistant,
    ];
    let result = filter_recent_history_images(&msgs, 3);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0].base64, "USER");
}

#[test]
fn filter_images_multi_image_messages() {
    let msgs = vec![
        make_user_msg(
            "old multi",
            vec![make_img("O1", "image/png"), make_img("O2", "image/png")],
        ),
        make_assistant_msg("response"),
        make_user_msg(
            "recent multi",
            vec![make_img("R1", "image/png"), make_img("R2", "image/png")],
        ),
    ];
    let result = filter_recent_history_images(&msgs, 1);
    assert_eq!(
        result.len(),
        1,
        "limit 1 should only keep last image message"
    );
    assert_eq!(result[0].len(), 2, "the message had 2 images");
    assert_eq!(result[0][0].base64, "R1");
    assert_eq!(result[0][1].base64, "R2");
}

#[test]
fn filter_images_drops_stale_image_in_long_thread() {
    // Bug scenario: screenshot sent at MSG 0, followed by 8 more user
    // messages with no images. The screenshot is stale — 8 user messages
    // old — and should NOT be included in context (max_messages=3).
    // The function must count ALL user messages for recency, not just
    // image-bearing ones, otherwise a single old screenshot survives forever.
    let mut msgs = vec![
        make_user_msg(
            "here is a screenshot of the bug",
            vec![make_img("OLD_SCREENSHOT", "image/png")],
        ),
        make_assistant_msg("I can see the issue in your screenshot"),
    ];
    for i in 0..8 {
        msgs.push(make_user_msg(&format!("follow-up {}", i), vec![]));
        msgs.push(make_assistant_msg(&format!("response {}", i)));
    }
    // 9 user messages total, only the 1st has an image.
    let result = filter_recent_history_images(&msgs, 3);
    assert!(
        result.is_empty(),
        "stale image from MSG 0 should be dropped, got {} image groups",
        result.len()
    );
}
