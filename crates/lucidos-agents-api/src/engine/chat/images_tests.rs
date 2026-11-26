use super::*;
use crate::core::blobs::write_blob;
use crate::core::store::SessionMessage;
use std::path::Path;

fn make_chat_img(data: &str, mime: &str) -> crate::engine::types::ChatImage {
    crate::engine::types::ChatImage {
        base64: data.to_string(),
        mime_type: mime.to_string(),
    }
}

fn png_with_marker(marker: u8) -> Vec<u8> {
    vec![
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, marker,
    ]
}

fn write_blobs(workspace: &Path, n: u8) -> Vec<String> {
    (0..n)
        .map(|i| write_blob(workspace, &png_with_marker(i)).unwrap().hash)
        .collect()
}

#[test]
fn no_images_returns_text_only() {
    let tmp = tempfile::TempDir::new().unwrap();
    let result = build_user_content_with_images("hello".into(), tmp.path(), &[], None, &[]);
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
        build_user_content_with_images("check this".into(), tmp.path(), &[], Some(&imgs), &[]);
    match result {
        MessageContent::Blocks(blocks) => {
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
        &[],
    );
    match result {
        MessageContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 2);
            assert!(
                matches!(&blocks[0], ContentBlock::Text { text } if text.contains("what was in the image?"))
            );
            assert!(matches!(&blocks[0], ContentBlock::Text { text } if text.contains("earlier")));
            assert!(matches!(&blocks[1], ContentBlock::Image { .. }));
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn mixed_history_and_current_images_separated() {
    let tmp = tempfile::TempDir::new().unwrap();
    let hashes = write_blobs(tmp.path(), 3);
    let history = vec![
        vec![hashes[0].clone()],
        vec![hashes[1].clone(), hashes[2].clone()],
    ];
    let current = vec![make_chat_img("IMG4", "image/png")];
    let result = build_user_content_with_images(
        "summarize all".into(),
        tmp.path(),
        &history,
        Some(&current),
        &[],
    );
    match result {
        MessageContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 6);
            let img_count = blocks
                .iter()
                .filter(|b| matches!(b, ContentBlock::Image { .. }))
                .count();
            assert_eq!(img_count, 4);
            assert!(matches!(&blocks[0], ContentBlock::Text { text } if text.contains("earlier")));
            assert!(
                matches!(&blocks[0], ContentBlock::Text { text } if text.contains("current message"))
            );
            let text_count = blocks
                .iter()
                .filter(|b| matches!(b, ContentBlock::Text { .. }))
                .count();
            assert_eq!(text_count, 2);
            assert!(matches!(&blocks[5], ContentBlock::Image { data, .. } if data == "IMG4"));
            assert!(
                matches!(&blocks[4], ContentBlock::Text { text } if text.contains("current message"))
            );
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn current_vs_history_images_distinguished() {
    let tmp = tempfile::TempDir::new().unwrap();
    let hashes = write_blobs(tmp.path(), 2);
    let history = vec![vec![hashes[0].clone()], vec![hashes[1].clone()]];
    let current = vec![make_chat_img("NEW1", "image/png")];
    let result = build_user_content_with_images(
        "here is a photo".into(),
        tmp.path(),
        &history,
        Some(&current),
        &[],
    );
    match result {
        MessageContent::Blocks(blocks) => {
            let text_blocks: Vec<&str> = blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();

            let has_current_marker = text_blocks.iter().any(|t| t.contains("current message"));
            assert!(
                has_current_marker,
                "must have a marker identifying current-message images, got: {:?}",
                text_blocks
            );

            let has_earlier_marker = text_blocks.iter().any(|t| t.contains("earlier"));
            assert!(
                has_earlier_marker,
                "must indicate some images are from earlier, got: {:?}",
                text_blocks
            );

            let img_count = blocks
                .iter()
                .filter(|b| matches!(b, ContentBlock::Image { .. }))
                .count();
            assert_eq!(img_count, 3);
        }
        _ => panic!("expected Blocks"),
    }
}

fn oversized_noisy_png_chat_image() -> crate::engine::types::ChatImage {
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
    img_buf.write_to(&mut png, image::ImageFormat::Png).unwrap();
    crate::engine::types::ChatImage {
        base64: base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
        mime_type: "image/png".to_string(),
    }
}

#[test]
fn oversized_current_image_is_fitted_to_target() {
    use base64::Engine as _;
    let tmp = tempfile::TempDir::new().unwrap();
    let img = oversized_noisy_png_chat_image();
    assert!(
        img.base64.len() > crate::engine::types::MAX_IMAGE_BASE64_BYTES,
        "fixture must start over the target, got {} bytes",
        img.base64.len()
    );
    let current = vec![img];
    let result =
        build_user_content_with_images("look".into(), tmp.path(), &[], Some(&current), &[]);
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
                data.len() <= crate::engine::types::MAX_IMAGE_BASE64_BYTES,
                "current image must be downsampled to fit, got {} bytes",
                data.len()
            );
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
    use base64::Engine as _;
    let tmp = tempfile::TempDir::new().unwrap();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(oversized_noisy_png_chat_image().base64)
        .unwrap();
    let hash = write_blob(tmp.path(), &raw).unwrap().hash;
    let history = vec![vec![hash]];
    let result = build_user_content_with_images("recall".into(), tmp.path(), &history, None, &[]);
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
                data.len() <= crate::engine::types::MAX_IMAGE_BASE64_BYTES,
                "history image must be downsampled to fit, got {} bytes",
                data.len()
            );
        }
        _ => panic!("expected Blocks"),
    }
}

#[test]
fn oversized_history_images_skipped() {
    let tmp = tempfile::TempDir::new().unwrap();
    let big_bytes = {
        let mut buf = png_with_marker(1);
        buf.extend(std::iter::repeat_n(0u8, MAX_TOTAL_IMAGE_BASE64 + 1));
        buf
    };
    let big_hash = write_blob(tmp.path(), &big_bytes).unwrap().hash;
    let small_hash = write_blob(tmp.path(), &png_with_marker(2)).unwrap().hash;
    let history = vec![vec![big_hash], vec![small_hash]];
    let result = build_user_content_with_images("test".into(), tmp.path(), &history, None, &[]);
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

#[test]
fn history_images_include_staleness_warning() {
    let tmp = tempfile::TempDir::new().unwrap();
    let hashes = write_blobs(tmp.path(), 1);
    let history = vec![hashes];
    let result =
        build_user_content_with_images("what changed?".into(), tmp.path(), &history, None, &[]);
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
        build_user_content_with_images("check this".into(), tmp.path(), &[], Some(&imgs), &[]);
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
    let result =
        build_user_content_with_images("compare".into(), tmp.path(), &history, Some(&current), &[]);
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

fn png_chat_img(marker: u8) -> crate::engine::types::ChatImage {
    use base64::Engine as _;
    make_chat_img(
        &base64::engine::general_purpose::STANDARD.encode(png_with_marker(marker)),
        "image/png",
    )
}

fn stored_handles(
    workspace: &Path,
    imgs: &[crate::engine::types::ChatImage],
) -> Vec<Option<String>> {
    crate::engine::message_events::images_to_hashes(workspace, Some(imgs));
    current_image_handles(workspace, imgs)
}

#[test]
fn a_current_image_is_labeled_with_its_handle() {
    let tmp = tempfile::TempDir::new().unwrap();
    let imgs = vec![png_chat_img(7)];
    let handles = stored_handles(tmp.path(), &imgs);
    let handle = handles[0].clone().expect("a stored PNG has a handle");
    let result = build_user_content_with_images(
        "heres the process".into(),
        tmp.path(),
        &[],
        Some(&imgs),
        &handles,
    );
    let MessageContent::Blocks(blocks) = result else {
        panic!("expected Blocks");
    };
    assert!(
        matches!(&blocks[0], ContentBlock::Text { text } if text.contains(&handle)),
        "the hint must name the image's handle, got: {:?}",
        blocks[0]
    );
}

#[test]
fn the_current_message_separator_names_the_handles_too() {
    let tmp = tempfile::TempDir::new().unwrap();
    let history = vec![write_blobs(tmp.path(), 1)];
    let imgs = vec![png_chat_img(8), png_chat_img(9)];
    let handles = stored_handles(tmp.path(), &imgs);
    let result = build_user_content_with_images(
        "compare".into(),
        tmp.path(),
        &history,
        Some(&imgs),
        &handles,
    );
    let MessageContent::Blocks(blocks) = result else {
        panic!("expected Blocks");
    };
    let separator = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } if text.starts_with("[Below:") => Some(text),
            _ => None,
        })
        .next()
        .expect("mixed content has a separator");
    for handle in handles.iter().flatten() {
        assert!(separator.contains(handle), "{separator}");
    }
}

#[test]
fn an_image_the_budget_skips_is_not_named() {
    use base64::Engine as _;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut big = png_with_marker(20);
    big.extend(std::iter::repeat_n(0u8, MAX_TOTAL_IMAGE_BASE64));
    let imgs = vec![
        png_chat_img(21),
        make_chat_img(
            &base64::engine::general_purpose::STANDARD.encode(&big),
            "image/png",
        ),
    ];
    let handles = stored_handles(tmp.path(), &imgs);
    let (kept, skipped) = (handles[0].clone().unwrap(), handles[1].clone().unwrap());
    let result =
        build_user_content_with_images("two".into(), tmp.path(), &[], Some(&imgs), &handles);
    let MessageContent::Blocks(blocks) = result else {
        panic!("expected Blocks");
    };
    let ContentBlock::Text { text } = &blocks[0] else {
        panic!("expected the hint first");
    };
    assert!(text.contains("1 image attached"), "{text}");
    assert!(text.contains(&kept), "{text}");
    assert!(!text.contains(&skipped), "{text}");
}

#[test]
fn a_current_images_handle_resolves_once_its_message_is_stored() {
    let tmp = tempfile::TempDir::new().unwrap();
    let older = write_blobs(tmp.path(), 1);
    let imgs = vec![png_chat_img(11)];
    let handles: Vec<String> = stored_handles(tmp.path(), &imgs)
        .into_iter()
        .flatten()
        .collect();

    let stored = crate::engine::message_events::images_to_hashes(tmp.path(), Some(&imgs));
    let events: Vec<crate::core::events::EventRow> = [older, stored]
        .into_iter()
        .map(|hashes| {
            crate::core::events::EventRow::new(
                "MessageReceived",
                serde_json::json!({ "text": "hi", "user_image_hashes": hashes }),
            )
        })
        .collect();

    let by_handle =
        crate::engine::tools::image::resolve_thread_image_refs(tmp.path(), &events, &handles)
            .expect("the printed handle must resolve");
    let by_index = crate::engine::tools::image::resolve_thread_image_refs(
        tmp.path(),
        &events,
        &["thread:2".to_string()],
    )
    .unwrap();
    assert_eq!(by_handle[0].base64, by_index[0].base64);
}

#[test]
fn an_image_with_no_stored_blob_gets_no_handle() {
    let tmp = tempfile::TempDir::new().unwrap();
    let imgs = vec![png_chat_img(30), make_chat_img("not base64!", "image/png")];
    assert_eq!(current_image_handles(tmp.path(), &imgs), vec![None, None]);
}

fn make_user_msg(content: &str, hashes: Vec<String>) -> SessionMessage {
    SessionMessage {
        role: "user".to_string(),
        content: content.to_string(),
        created_at: chrono::Utc::now(),
        channel: None,
        steps: vec![],
        image_handles: vec![],
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
        agent: None,
        sender_device_id: None,
    }
}

fn make_assistant_msg(content: &str) -> SessionMessage {
    SessionMessage {
        role: "assistant".to_string(),
        content: content.to_string(),
        created_at: chrono::Utc::now(),
        channel: None,
        steps: vec![],
        image_handles: vec![],
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
        agent: None,
        sender_device_id: None,
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
    assert_eq!(result[0][0], "MID");
    assert_eq!(result[1][0], "RECENT1");
    assert_eq!(result[2][0], "LATEST");
}

#[test]
fn filter_images_counts_all_user_messages_for_recency() {
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
    assistant.image_handles = vec!["img-0123456789abcdef".to_string()];
    assistant.user_image_hashes = vec!["SHOULD_NOT_APPEAR".to_string()];
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
    let result = filter_recent_history_image_hashes(&msgs, 3);
    assert!(
        result.is_empty(),
        "stale image from MSG 0 should be dropped, got {} image groups",
        result.len()
    );
}
