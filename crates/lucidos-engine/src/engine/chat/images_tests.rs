use super::*;
use crate::core::blobs::write_blob;
use crate::core::store::SessionMessage;
use std::path::Path;

fn make_chat_img(data: &str, mime: &str) -> crate::api::ChatImage {
    crate::api::ChatImage {
        base64: data.to_string(),
        mime_type: mime.to_string(),
    }
}

/// Minimal valid PNG (1×1 pixel) — passes the magic-byte sniff in
/// `core::blobs::sniff_image_mime`. Tests that need history images write
/// distinct PNGs so the resulting hashes differ; the helper appends a
/// per-test discriminator byte to make each blob unique.
fn png_with_marker(marker: u8) -> Vec<u8> {
    vec![
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, // signature
        0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D', b'R', // IHDR start
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, marker,
    ]
}

/// Write `n` distinct image blobs into `workspace` and return their hashes
/// in the order written. Used by build_user_content_with_images tests that
/// need history hashes which actually resolve.
fn write_blobs(workspace: &Path, n: u8) -> Vec<String> {
    (0..n)
        .map(|i| write_blob(workspace, &png_with_marker(i)).unwrap().hash)
        .collect()
}

// --- build_user_content_with_images tests ---

#[test]
fn no_images_returns_text_only() {
    let tmp = tempfile::TempDir::new().unwrap();
    let result = build_user_content_with_images("hello".into(), tmp.path(), &[], None);
    match result {
        MessageContent::Text(t) => assert_eq!(t, "hello"),
        _ => panic!("expected Text, got Blocks"),
    }
}

#[test]
fn current_image_only() {
    let tmp = tempfile::TempDir::new().unwrap();
    let imgs = vec![make_chat_img("AAAA", "image/jpeg")];
    let result =
        build_user_content_with_images("check this".into(), tmp.path(), &[], Some(&imgs));
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
    let tmp = tempfile::TempDir::new().unwrap();
    let hashes = write_blobs(tmp.path(), 1);
    let history = vec![hashes];
    let result = build_user_content_with_images(
        "what was in the image?".into(),
        tmp.path(),
        &history,
        None,
    );
    match result {
        MessageContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 2);
            assert!(
                matches!(&blocks[0], ContentBlock::Text { text } if text.contains("what was in the image?"))
            );
            assert!(matches!(&blocks[0], ContentBlock::Text { text } if text.contains("earlier")));
            // History image is one ContentBlock::Image with the resolved bytes
            assert!(matches!(&blocks[1], ContentBlock::Image { .. }));
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn mixed_history_and_current_images_separated() {
    let tmp = tempfile::TempDir::new().unwrap();
    let hashes = write_blobs(tmp.path(), 3);
    let history = vec![vec![hashes[0].clone()], vec![hashes[1].clone(), hashes[2].clone()]];
    let current = vec![make_chat_img("IMG4", "image/png")];
    let result = build_user_content_with_images(
        "summarize all".into(),
        tmp.path(),
        &history,
        Some(&current),
    );
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
            // Last image is the current one — its data is the literal "IMG4" string
            // because current_images already carries inline base64.
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
    let tmp = tempfile::TempDir::new().unwrap();
    let hashes = write_blobs(tmp.path(), 2);
    let history = vec![vec![hashes[0].clone()], vec![hashes[1].clone()]];
    let current = vec![make_chat_img("NEW1", "image/png")];
    let result = build_user_content_with_images(
        "here is a photo".into(),
        tmp.path(),
        &history,
        Some(&current),
    );
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

/// Generate a noisy PNG whose base64 payload exceeds the per-image LLM target.
/// Noise resists PNG compression so the encoded image stays large; a smooth
/// gradient would shrink under target and never exercise the fit/compress path.
fn oversized_noisy_png_chat_image() -> crate::api::ChatImage {
    use base64::Engine as _;
    let img_buf: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(1400, 1400, |x, y| {
            let mut h = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(2_246_822_519);
            h ^= h >> 15;
            h = h.wrapping_mul(0x85EB_CA6B);
            h ^= h >> 13;
            let b = h.to_le_bytes();
            image::Rgba([b[0], b[1], b[2], 255])
        });
    let mut png = std::io::Cursor::new(Vec::new());
    img_buf
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    crate::api::ChatImage {
        base64: base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
        mime_type: "image/png".to_string(),
    }
}

#[test]
fn oversized_current_image_is_fitted_to_target() {
    // Regression: an uploaded photo larger than the provider's per-image limit
    // must be downsampled at the LLM boundary, not sent raw (which 400s with
    // "image exceeds 5 MB maximum").
    use base64::Engine as _;
    let tmp = tempfile::TempDir::new().unwrap();
    let img = oversized_noisy_png_chat_image();
    assert!(
        img.base64.len() > crate::api::MAX_IMAGE_BASE64_BYTES,
        "fixture must start over the target, got {} bytes",
        img.base64.len()
    );
    let current = vec![img];
    let result = build_user_content_with_images("look".into(), tmp.path(), &[], Some(&current));
    match result {
        MessageContent::Blocks(blocks) => {
            let (data, media_type) = blocks
                .iter()
                .find_map(|b| match b {
                    ContentBlock::Image {
                        data, media_type, ..
                    } => Some((data, media_type)),
                    _ => None,
                })
                .expect("must contain an image block");
            assert!(
                data.len() <= crate::api::MAX_IMAGE_BASE64_BYTES,
                "current image must be downsampled to fit, got {} bytes",
                data.len()
            );
            // fit_for_llm compresses to JPEG when it shrinks the image.
            assert_eq!(media_type, "image/jpeg");
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap();
            assert!(image::load_from_memory(&decoded).is_ok());
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn oversized_history_image_is_fitted_to_target() {
    // Same fit applies to images pulled from history on a follow-up message —
    // otherwise a thread with a large image would 400 on every subsequent turn.
    use base64::Engine as _;
    let tmp = tempfile::TempDir::new().unwrap();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(oversized_noisy_png_chat_image().base64)
        .unwrap();
    let hash = write_blob(tmp.path(), &raw).unwrap().hash;
    let history = vec![vec![hash]];
    let result = build_user_content_with_images("recall".into(), tmp.path(), &history, None);
    match result {
        MessageContent::Blocks(blocks) => {
            let data = blocks
                .iter()
                .find_map(|b| match b {
                    ContentBlock::Image { data, .. } => Some(data),
                    _ => None,
                })
                .expect("history image block present");
            assert!(
                data.len() <= crate::api::MAX_IMAGE_BASE64_BYTES,
                "history image must be downsampled to fit, got {} bytes",
                data.len()
            );
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn oversized_history_images_skipped() {
    // Budget is measured on the fitted (compressed-if-over) size. The big blob
    // is a PNG header followed by megabytes of zero padding — not a decodable
    // image, so `fit_for_llm`/`compress` can't shrink it; it stays over the
    // budget and is skipped. The minimal PNG fixture is ~30 bytes, far under
    // MAX_TOTAL_IMAGE_BASE64, so the small blob always fits.
    let tmp = tempfile::TempDir::new().unwrap();
    let big_bytes = {
        let mut buf = png_with_marker(1);
        buf.extend(std::iter::repeat_n(0u8, MAX_TOTAL_IMAGE_BASE64 + 1));
        buf
    };
    let big_hash = write_blob(tmp.path(), &big_bytes).unwrap().hash;
    let small_hash = write_blob(tmp.path(), &png_with_marker(2)).unwrap().hash;
    let history = vec![vec![big_hash], vec![small_hash]];
    let result = build_user_content_with_images("test".into(), tmp.path(), &history, None);
    match result {
        MessageContent::Blocks(blocks) => {
            let img_count = blocks
                .iter()
                .filter(|b| matches!(b, ContentBlock::Image { .. }))
                .count();
            assert_eq!(img_count, 1, "only the small image fits in the budget");
        }
        _ => panic!("expected Blocks with only small image"),
    }
}

// --- staleness warning in build_user_content_with_images ---

#[test]
fn history_images_include_staleness_warning() {
    let tmp = tempfile::TempDir::new().unwrap();
    let hashes = write_blobs(tmp.path(), 1);
    let history = vec![hashes];
    let result =
        build_user_content_with_images("what changed?".into(), tmp.path(), &history, None);
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
    let tmp = tempfile::TempDir::new().unwrap();
    let imgs = vec![make_chat_img("NEW", "image/jpeg")];
    let result =
        build_user_content_with_images("check this".into(), tmp.path(), &[], Some(&imgs));
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
    let tmp = tempfile::TempDir::new().unwrap();
    let hashes = write_blobs(tmp.path(), 1);
    let history = vec![hashes];
    let current = vec![make_chat_img("NEW", "image/png")];
    let result = build_user_content_with_images(
        "compare".into(),
        tmp.path(),
        &history,
        Some(&current),
    );
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

// --- filter_recent_history_image_hashes tests ---

fn make_user_msg(content: &str, hashes: Vec<String>) -> SessionMessage {
    SessionMessage {
        role: "user".to_string(),
        content: content.to_string(),
        created_at: chrono::Utc::now(),
        channel: None,
        steps: vec![],
        images: vec![],
        user_image_hashes: hashes,
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
        user_image_hashes: vec![],
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
    let result = filter_recent_history_image_hashes(&[], 3);
    assert!(result.is_empty());
}

#[test]
fn filter_images_single_message_with_image() {
    let msgs = vec![make_user_msg("screenshot", vec!["hash1".to_string()])];
    let result = filter_recent_history_image_hashes(&msgs, 3);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], "hash1");
}

#[test]
fn filter_images_within_limit_keeps_all() {
    let msgs = vec![
        make_user_msg("img1", vec!["A".to_string()]),
        make_assistant_msg("response 1"),
        make_user_msg("img2", vec!["B".to_string()]),
        make_assistant_msg("response 2"),
        make_user_msg("img3", vec!["C".to_string()]),
    ];
    let result = filter_recent_history_image_hashes(&msgs, 3);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0][0], "A");
    assert_eq!(result[1][0], "B");
    assert_eq!(result[2][0], "C");
}

#[test]
fn filter_images_drops_oldest_when_over_limit() {
    // 5 user messages with images, limit 3 → only last 3 kept
    let msgs = vec![
        make_user_msg("old screenshot", vec!["OLD1".to_string()]),
        make_assistant_msg("response"),
        make_user_msg("another old one", vec!["OLD2".to_string()]),
        make_assistant_msg("response"),
        make_user_msg("mid thread", vec!["MID".to_string()]),
        make_assistant_msg("response"),
        make_user_msg("recent", vec!["RECENT1".to_string()]),
        make_assistant_msg("response"),
        make_user_msg("latest", vec!["LATEST".to_string()]),
    ];
    let result = filter_recent_history_image_hashes(&msgs, 3);
    assert_eq!(
        result.len(),
        3,
        "should only keep 3 most recent image messages"
    );
    // OLD1 and OLD2 should be dropped
    assert_eq!(result[0][0], "MID");
    assert_eq!(result[1][0], "RECENT1");
    assert_eq!(result[2][0], "LATEST");
}

#[test]
fn filter_images_counts_all_user_messages_for_recency() {
    // 6 user messages: image "A" at position 2, image "B" at position 6.
    // With max_messages=3, only the last 3 user messages are eligible
    // (positions 4, 5, 6). Image "A" at position 2 is stale → dropped.
    let msgs = vec![
        make_user_msg("text only 1", vec![]),
        make_assistant_msg("response"),
        make_user_msg("with image", vec!["A".to_string()]),
        make_assistant_msg("response"),
        make_user_msg("text only 2", vec![]),
        make_assistant_msg("response"),
        make_user_msg("text only 3", vec![]),
        make_assistant_msg("response"),
        make_user_msg("text only 4", vec![]),
        make_assistant_msg("response"),
        make_user_msg("another image", vec!["B".to_string()]),
    ];
    let result = filter_recent_history_image_hashes(&msgs, 3);
    assert_eq!(
        result.len(),
        1,
        "only image B should survive — A is 4 messages stale"
    );
    assert_eq!(result[0][0], "B");
}

#[test]
fn filter_images_ignores_assistant_images() {
    let mut assistant = make_assistant_msg("here is a generated image");
    assistant.images = vec!["generated.png".to_string()];
    assistant.user_image_hashes = vec!["SHOULD_NOT_APPEAR".to_string()];
    // Even if assistant message has user_image_hashes set (shouldn't happen), role filter blocks it
    let msgs = vec![
        make_user_msg("user img", vec!["USER".to_string()]),
        assistant,
    ];
    let result = filter_recent_history_image_hashes(&msgs, 3);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], "USER");
}

#[test]
fn filter_images_multi_image_messages() {
    let msgs = vec![
        make_user_msg("old multi", vec!["O1".to_string(), "O2".to_string()]),
        make_assistant_msg("response"),
        make_user_msg("recent multi", vec!["R1".to_string(), "R2".to_string()]),
    ];
    let result = filter_recent_history_image_hashes(&msgs, 1);
    assert_eq!(
        result.len(),
        1,
        "limit 1 should only keep last image message"
    );
    assert_eq!(result[0].len(), 2, "the message had 2 images");
    assert_eq!(result[0][0], "R1");
    assert_eq!(result[0][1], "R2");
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
            vec!["OLD_SCREENSHOT".to_string()],
        ),
        make_assistant_msg("I can see the issue in your screenshot"),
    ];
    for i in 0..8 {
        msgs.push(make_user_msg(&format!("follow-up {}", i), vec![]));
        msgs.push(make_assistant_msg(&format!("response {}", i)));
    }
    // 9 user messages total, only the 1st has an image.
    let result = filter_recent_history_image_hashes(&msgs, 3);
    assert!(
        result.is_empty(),
        "stale image from MSG 0 should be dropped, got {} image groups",
        result.len()
    );
}
