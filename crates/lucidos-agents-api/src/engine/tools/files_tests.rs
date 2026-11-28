use super::*;

fn no_apps() -> crate::core::AppManager {
    let ws = tempfile::tempdir().unwrap();
    crate::core::AppManager::new(ws.path()).unwrap()
}

#[test]
fn read_only_reason_blocks_a_built_in_widget_and_nothing_else() {
    let ws = tempfile::tempdir().unwrap();
    let shipped = tempfile::tempdir().unwrap();
    let player = shipped.path().join("lucidos-sound-player");
    std::fs::create_dir_all(&player).unwrap();
    std::fs::write(player.join("manifest.json"), r#"{"name":"Sound player"}"#).unwrap();
    let apps = crate::core::AppManager::new(ws.path())
        .unwrap()
        .with_system_widgets(Some(shipped.path().to_path_buf()));

    for p in [
        "apps/lucidos-sound-player/index.html",
        "apps/lucidos-sound-player/manifest.json",
    ] {
        let reason = read_only_reason(p, &apps).expect(p);
        assert!(reason.contains("built-in"), "got: {reason}");
    }
    let reason = read_only_reason("apps/lucidos-new/manifest.json", &apps).unwrap();
    assert!(reason.contains("reserved"), "got: {reason}");
    let mine = ws.path().join("data/apps/lucidos-mine");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join("manifest.json"), r#"{"name":"Mine"}"#).unwrap();
    assert!(read_only_reason("apps/lucidos-mine/index.html", &apps).is_none());
}

#[test]
fn read_only_reason_blocks_system_knowhow() {
    assert!(read_only_reason("system-knowhow/best-practices.md", &no_apps()).is_some());
    assert!(read_only_reason("system-knowhow/scripts/list.sh", &no_apps()).is_some());
}

#[test]
fn read_only_reason_blocks_ephemeral_scratch() {
    for p in [
        ".lucidos/tmp/scratch.py",
        ".lucidos/tmp/changelog-v0.15.0.md",
        ".lucidos/tmp/some-repo/build.sh",
    ] {
        assert!(
            read_only_reason(p, &no_apps()).is_some(),
            "{p} should be refused"
        );
    }
}

#[test]
fn scratch_refusal_names_run_python() {
    let reason = read_only_reason(".lucidos/tmp/scratch.py", &no_apps()).unwrap();
    assert!(reason.contains("run_python"), "got: {reason}");
    assert!(reason.contains("workspace root"), "got: {reason}");
}

#[test]
fn data_path_app_id_extracts_id_under_apps() {
    assert_eq!(
        data_path_app_id("apps/demo-director/index.html"),
        Some("demo-director")
    );
    assert_eq!(data_path_app_id("apps/foo/scripts/x.py"), Some("foo"));
    assert_eq!(data_path_app_id("artifacts/notes.md"), None);
    assert_eq!(data_path_app_id("apps/"), None);
    assert_eq!(data_path_app_id("apps"), None);
}

#[test]
fn data_path_app_manifest_id_only_matches_the_manifest() {
    assert_eq!(
        data_path_app_manifest_id("apps/demo-director/manifest.json"),
        Some("demo-director")
    );
    assert_eq!(
        data_path_app_manifest_id("apps/demo-director/index.html"),
        None
    );
    assert_eq!(data_path_app_manifest_id("apps/foo/scripts/x.py"), None);
    assert_eq!(
        data_path_app_manifest_id("apps/foo/sub/manifest.json"),
        None
    );
    assert_eq!(data_path_app_manifest_id("artifacts/manifest.json"), None);
    assert_eq!(data_path_app_manifest_id("apps/manifest.json"), None);
    assert_eq!(data_path_app_manifest_id("apps/foo"), None);
}

#[test]
fn app_lifecycle_event_is_birth_and_death_only() {
    let name = || Some("Demo".to_string());
    let delete = ManifestOp::Delete {
        kind_before: Some(AppKind::Widget),
    };
    assert!(matches!(
        app_lifecycle_event("a", name, true, false, ManifestOp::Write),
        Some(SystemEvent::AppCreated { .. })
    ));
    assert!(app_lifecycle_event("a", name, true, true, ManifestOp::Write).is_none());
    assert!(matches!(
        app_lifecycle_event("a", name, false, true, delete),
        Some(SystemEvent::AppDeleted {
            kind: Some(AppKind::Widget),
            ..
        })
    ));
    assert!(app_lifecycle_event("a", name, false, false, delete).is_none());
}

#[test]
fn a_file_tool_announces_every_data_change_outside_artifacts() {
    for path in [
        "knowhow/cooking/bread.md",
        "apps/habit-tracker/index.html",
        "config/apis.json",
        "themes/dusk.json",
    ] {
        assert!(
            matches!(
                data_file_event(path, DataFileChange::Written { commit: "abc" }),
                Some(SystemEvent::DataFileWritten { path: p, commit: Some(c), actor: None })
                    if p == path && c == "abc"
            ),
            "{path} written"
        );
        assert!(
            matches!(
                data_file_event(path, DataFileChange::Edited),
                Some(SystemEvent::DataFileEdited { path: p, operations_count: 1, actor: None })
                    if p == path
            ),
            "{path} edited"
        );
        assert!(
            matches!(
                data_file_event(path, DataFileChange::Deleted { commit: "abc" }),
                Some(SystemEvent::DataFileDeleted { path: p, .. }) if p == path
            ),
            "{path} deleted"
        );
    }
}

#[test]
fn an_artifact_change_is_left_to_its_own_artifact_event() {
    for change in [
        DataFileChange::Written { commit: "abc" },
        DataFileChange::Edited,
        DataFileChange::Deleted { commit: "abc" },
    ] {
        assert!(data_file_event("artifacts/notes.md", change).is_none());
    }
}

#[test]
fn read_only_reason_allows_user_paths() {
    assert!(read_only_reason("artifacts/notes.md", &no_apps()).is_none());
    assert!(read_only_reason("knowhow/lucidos/best-practices.md", &no_apps()).is_none());
    assert!(read_only_reason("apps/foo/index.html", &no_apps()).is_none());
    assert!(read_only_reason("triggers/daily/check.md", &no_apps()).is_none());
    assert!(read_only_reason("artifacts/.lucidosrc", &no_apps()).is_none());
}

#[test]
fn test_image_media_type_mapping() {
    assert_eq!(image_media_type("png"), Some("image/png"));
    assert_eq!(image_media_type("jpg"), Some("image/jpeg"));
    assert_eq!(image_media_type("jpeg"), Some("image/jpeg"));
    assert_eq!(image_media_type("gif"), Some("image/gif"));
    assert_eq!(image_media_type("webp"), Some("image/webp"));
    assert_eq!(image_media_type("svg"), None);
    assert_eq!(image_media_type("pdf"), None);
    assert_eq!(image_media_type("txt"), None);
}

#[test]
fn test_image_content_marker_format() {
    let media_type = "image/png";
    let b64_data = "iVBORw0KGgo=";
    let marker = format!("[IMAGE_CONTENT:{}]\n{}", media_type, b64_data);

    let rest = marker.strip_prefix("[IMAGE_CONTENT:").unwrap();
    let end_bracket = rest.find("]\n").unwrap();
    let parsed_media = &rest[..end_bracket];
    let parsed_data = rest[end_bracket + 2..].trim();

    assert_eq!(parsed_media, "image/png");
    assert_eq!(parsed_data, "iVBORw0KGgo=");
}

#[test]
fn test_image_size_guard() {
    assert_eq!(IMAGE_MAX_BYTES, 25 * 1024 * 1024);
    const { assert!(12 * 1024 * 1024 < IMAGE_MAX_BYTES) };
}

#[test]
fn test_read_image_file_returns_base64_marker() {
    let dir = tempfile::tempdir().unwrap();
    let img_path = dir.path().join("test.png");
    let png_bytes: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x62, 0x00,
        0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, 0x27, 0xDE, 0xFC, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
        0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    std::fs::write(&img_path, &png_bytes).unwrap();

    let extension = "png";
    let media_type = image_media_type(extension).unwrap();
    let bytes = std::fs::read(&img_path).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let result = format!("[IMAGE_CONTENT:{}]\n{}", media_type, b64);

    assert!(result.starts_with("[IMAGE_CONTENT:image/png]\n"));
    let rest = result.strip_prefix("[IMAGE_CONTENT:image/png]\n").unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(rest.trim())
        .unwrap();
    assert_eq!(decoded, png_bytes);
}

#[test]
fn strip_image_content_marker_png() {
    let input = "[IMAGE_CONTENT:image/png]\niVBORw0KGgo=";
    let stub = strip_image_content_marker(input).expect("should match marker");
    assert!(
        stub.contains("image/png"),
        "stub mentions media type: {}",
        stub
    );
    assert!(stub.contains("omitted"), "stub flags omission: {}", stub);
    assert!(stub.len() < 100, "stub is small: {} chars", stub.len());
}

#[test]
fn strip_image_content_marker_jpeg() {
    let input = "[IMAGE_CONTENT:image/jpeg]\nABCDEFGH";
    let stub = strip_image_content_marker(input).unwrap();
    assert!(stub.contains("image/jpeg"));
}

#[test]
fn strip_image_content_marker_includes_size_label() {
    let stub_small = strip_image_content_marker("[IMAGE_CONTENT:image/png]\nABCD").unwrap();
    assert!(stub_small.contains("bytes"), "{}", stub_small);

    let big_b64 = "A".repeat(1_400_000);
    let stub_big =
        strip_image_content_marker(&format!("[IMAGE_CONTENT:image/png]\n{}", big_b64)).unwrap();
    assert!(stub_big.contains("MB"), "{}", stub_big);
}

#[test]
fn strip_image_content_marker_returns_none_for_plain_text() {
    assert!(strip_image_content_marker("Hello, world").is_none());
    assert!(strip_image_content_marker("File contents:\nline1\nline2").is_none());
}

#[test]
fn strip_image_content_marker_returns_none_for_malformed() {
    assert!(strip_image_content_marker("[IMAGE_CONTENT:image/png ABCDEFGH").is_none());
    assert!(strip_image_content_marker("[IMAGE_CONTENT:image/png]ABCDEFGH").is_none());
}

#[test]
fn strip_image_content_marker_strips_real_size() {
    let huge = "A".repeat(2 * 1024 * 1024);
    let input = format!("[IMAGE_CONTENT:image/png]\n{}", huge);
    let stub = strip_image_content_marker(&input).unwrap();
    assert!(stub.len() < 100, "stub size {} should be < 100", stub.len());
    assert!(input.len() > 1_000_000);
}

#[test]
fn test_large_image_error_message() {
    let size: u64 = 30 * 1024 * 1024;
    assert!(size > IMAGE_MAX_BYTES);
    let mb = size as f64 / (1024.0 * 1024.0);
    let msg = format!(
        "Image too large to read directly ({:.1} MB). Max 25MB; \
         smaller images are automatically resized to fit.",
        mb
    );
    assert_eq!(
        msg,
        "Image too large to read directly (30.0 MB). Max 25MB; \
         smaller images are automatically resized to fit."
    );
}

#[test]
fn read_oversized_image_is_downsampled_not_rejected() {
    let (w, h) = (1600u32, 1600u32);
    let mut buf = image::RgbImage::new(w, h);
    let mut state: u32 = 0x9E3779B9;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state & 0xFF) as u8
    };
    for px in buf.pixels_mut() {
        *px = image::Rgb([next(), next(), next()]);
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(buf)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let png_bytes = png.into_inner();

    let raw_b64_len = base64::engine::general_purpose::STANDARD
        .encode(&png_bytes)
        .len();
    assert!(
        raw_b64_len > crate::engine::types::MAX_IMAGE_BASE64_BYTES,
        "fixture must exceed the LLM target to exercise downsampling (got {raw_b64_len} base64 bytes)"
    );
    assert!((png_bytes.len() as u64) < IMAGE_MAX_BYTES);

    let result = encode_image_for_read(png_bytes, "image/png");
    assert!(
        result.starts_with("[IMAGE_CONTENT:"),
        "oversized image is downsampled and returned, not rejected"
    );
    let (media_type, b64) =
        parse_image_content_marker(&result).expect("result is a valid IMAGE_CONTENT sentinel");
    assert_eq!(media_type, "image/jpeg");
    assert!(
        b64.len() <= crate::engine::types::MAX_IMAGE_BASE64_BYTES,
        "downsampled payload must fit the LLM target, got {} base64 bytes",
        b64.len()
    );
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .expect("returned base64 decodes");
    assert!(image::load_from_memory(&decoded).is_ok());
}

#[test]
fn read_undecodable_oversized_image_reports_clear_error() {
    let bytes: Vec<u8> = (0..4 * 1024 * 1024usize).map(|i| (i % 251) as u8).collect();
    assert!(
        base64::engine::general_purpose::STANDARD
            .encode(&bytes)
            .len()
            > crate::engine::types::MAX_IMAGE_BASE64_BYTES
    );
    let result = encode_image_for_read(bytes, "image/png");
    assert!(
        !result.starts_with("[IMAGE_CONTENT:"),
        "undecodable oversized image must not produce an IMAGE_CONTENT payload, got: {}",
        &result[..result.floor_char_boundary(80)]
    );
    assert!(
        result.contains("could not be"),
        "result names the failure: {result}"
    );
}

#[test]
fn read_small_image_is_not_recompressed() {
    let png_bytes: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x62, 0x00,
        0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, 0x27, 0xDE, 0xFC, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
        0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let result = encode_image_for_read(png_bytes.clone(), "image/png");
    let (media_type, b64) = parse_image_content_marker(&result).unwrap();
    assert_eq!(media_type, "image/png");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    assert_eq!(decoded, png_bytes);
}

#[test]
fn a_jpeg_saved_with_a_png_extension_is_sent_as_jpeg() {
    let jpeg_bytes: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F'];
    let result = encode_image_for_read(jpeg_bytes, "image/png");
    assert!(
        result.starts_with("[IMAGE_CONTENT:image/jpeg]\n"),
        "got: {}",
        &result[..result.floor_char_boundary(60)]
    );
}

#[test]
fn a_real_png_still_reports_as_png() {
    let png_bytes: Vec<u8> = vec![
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00,
    ];
    let result = encode_image_for_read(png_bytes, "image/png");
    assert!(
        result.starts_with("[IMAGE_CONTENT:image/png]\n"),
        "got: {}",
        &result[..result.floor_char_boundary(60)]
    );
}

#[test]
fn an_unrecognized_blob_falls_back_to_the_extension() {
    let unknown_bytes: Vec<u8> = vec![0x00, 0x01, 0x02, 0x03, 0x04];
    let result = encode_image_for_read(unknown_bytes, "image/webp");
    assert!(
        result.starts_with("[IMAGE_CONTENT:image/webp]\n"),
        "got: {}",
        &result[..result.floor_char_boundary(60)]
    );
}

#[test]
fn test_svg_not_treated_as_image() {
    assert_eq!(image_media_type("svg"), None);
    assert!(!crate::core::is_binary_extension("svg"));
}

#[test]
fn test_dot_path_to_pointer_simple_key() {
    assert_eq!(dot_path_to_pointer("title"), "/title");
}

#[test]
fn test_dot_path_to_pointer_nested() {
    assert_eq!(
        dot_path_to_pointer("metadata.author.name"),
        "/metadata/author/name"
    );
}

#[test]
fn test_dot_path_to_pointer_array_index() {
    assert_eq!(dot_path_to_pointer("sections[1]"), "/sections/1");
}

#[test]
fn test_dot_path_to_pointer_mixed() {
    assert_eq!(
        dot_path_to_pointer("sections[1].slides[0].content[2].text"),
        "/sections/1/slides/0/content/2/text"
    );
}

#[test]
fn test_dot_path_to_pointer_empty() {
    assert_eq!(dot_path_to_pointer(""), "");
}

#[test]
fn test_dot_path_to_pointer_already_pointer() {
    assert_eq!(
        dot_path_to_pointer("/sections/1/title"),
        "/sections/1/title"
    );
}

#[test]
fn test_json_set_value_simple_key() {
    let mut doc: serde_json::Value = serde_json::from_str(r#"{"title": "Old"}"#).unwrap();
    let new_val = serde_json::Value::String("New".to_string());
    let result = json_set_value(&mut doc, "/title", new_val);
    assert!(result.is_ok());
    assert_eq!(doc["title"], "New");
}

#[test]
fn test_json_set_value_nested() {
    let mut doc: serde_json::Value =
        serde_json::from_str(r#"{"sections": [{"title": "A", "slides": [{"title": "S1"}]}]}"#)
            .unwrap();
    let new_val = serde_json::Value::String("Updated".to_string());
    let result = json_set_value(&mut doc, "/sections/0/slides/0/title", new_val);
    assert!(result.is_ok());
    assert_eq!(doc["sections"][0]["slides"][0]["title"], "Updated");
}

#[test]
fn test_json_set_value_replace_object() {
    let mut doc: serde_json::Value = serde_json::from_str(r#"{"meta": {"version": 1}}"#).unwrap();
    let new_val = serde_json::json!({"version": 2, "author": "test"});
    let result = json_set_value(&mut doc, "/meta", new_val.clone());
    assert!(result.is_ok());
    assert_eq!(doc["meta"], new_val);
}

#[test]
fn test_json_set_value_replace_array_element() {
    let mut doc: serde_json::Value = serde_json::from_str(r#"{"items": ["a", "b", "c"]}"#).unwrap();
    let new_val = serde_json::Value::String("B".to_string());
    let result = json_set_value(&mut doc, "/items/1", new_val);
    assert!(result.is_ok());
    assert_eq!(doc["items"][1], "B");
}

#[test]
fn test_json_set_value_invalid_path() {
    let mut doc: serde_json::Value = serde_json::from_str(r#"{"title": "X"}"#).unwrap();
    let new_val = serde_json::Value::String("Y".to_string());
    let result = json_set_value(&mut doc, "/nonexistent/deep/path", new_val);
    assert!(result.unwrap_err().contains("/nonexistent/deep/path"));
}

#[test]
fn test_dot_path_to_pointer_consecutive_brackets() {
    assert_eq!(dot_path_to_pointer("matrix[1][2]"), "/matrix/1/2");
}

#[test]
fn test_dot_path_to_pointer_double_quoted_key() {
    assert_eq!(
        dot_path_to_pointer(r#"dailyLog["2026-05-04"]"#),
        "/dailyLog/2026-05-04"
    );
}

#[test]
fn test_dot_path_to_pointer_single_quoted_key() {
    assert_eq!(
        dot_path_to_pointer("dailyLog['2026-05-04']"),
        "/dailyLog/2026-05-04"
    );
}

#[test]
fn test_dot_path_to_pointer_quoted_key_in_chain() {
    assert_eq!(
        dot_path_to_pointer(r#"habits[0].dailyLog["2026-05-04"]"#),
        "/habits/0/dailyLog/2026-05-04"
    );
}

#[test]
fn test_dot_path_to_pointer_quoted_key_with_dot() {
    assert_eq!(dot_path_to_pointer(r#"data["foo.bar"]"#), "/data/foo.bar");
}

#[test]
fn test_dot_path_to_pointer_quoted_key_pointer_escapes() {
    assert_eq!(dot_path_to_pointer(r#"data["a/b"]"#), "/data/a~1b");
    assert_eq!(dot_path_to_pointer(r#"data["a~b"]"#), "/data/a~0b");
}

#[test]
fn test_dot_path_to_pointer_jsonpath_root() {
    assert_eq!(dot_path_to_pointer("$.streak"), "/streak");
    assert_eq!(dot_path_to_pointer("$"), "");
    assert_eq!(
        dot_path_to_pointer(r#"$.habits[0].dailyLog["2026-05-04"]"#),
        "/habits/0/dailyLog/2026-05-04"
    );
}

#[test]
fn test_dot_path_to_pointer_leading_bracket() {
    assert_eq!(dot_path_to_pointer("[0].name"), "/0/name");
    assert_eq!(dot_path_to_pointer(r#"["key"]"#), "/key");
}

#[test]
fn test_json_set_value_with_quoted_key_path() {
    let mut doc: serde_json::Value =
        serde_json::from_str(r#"{"habits":[{"dailyLog":{"2026-05-04":2}}]}"#).unwrap();
    let pointer = dot_path_to_pointer(r#"habits[0].dailyLog["2026-05-04"]"#);
    let result = json_set_value(&mut doc, &pointer, serde_json::json!(3));
    assert!(
        result.is_ok(),
        "Failed to set value at {}: {:?}",
        pointer,
        result
    );
    assert_eq!(doc["habits"][0]["dailyLog"]["2026-05-04"], 3);
}

#[test]
fn test_json_set_value_root() {
    let mut doc: serde_json::Value = serde_json::from_str(r#"{"old": true}"#).unwrap();
    let new_val = serde_json::json!({"new": true});
    let result = json_set_value(&mut doc, "", new_val.clone());
    assert!(result.is_ok());
    assert_eq!(doc, new_val);
}

#[test]
fn slice_lines_full_content_when_unbounded() {
    assert_eq!(slice_lines("a\nb\nc", 1, None), "a\nb\nc");
}

#[test]
fn slice_lines_inclusive_range() {
    assert_eq!(slice_lines("a\nb\nc\nd\ne", 2, Some(2)), "b\nc");
}

#[test]
fn slice_lines_clamps_at_eof() {
    assert_eq!(slice_lines("a\nb", 1, Some(100)), "a\nb");
}

#[test]
fn slice_lines_past_eof_returns_empty() {
    assert_eq!(slice_lines("a\nb", 100, Some(1)), "");
}

#[test]
fn slice_lines_zero_count_returns_empty() {
    assert_eq!(slice_lines("a\nb\nc", 1, Some(0)), "");
}

#[test]
fn slice_lines_treats_zero_start_as_one() {
    assert_eq!(slice_lines("a\nb\nc", 0, Some(2)), "a\nb");
}

#[test]
fn slice_lines_single_line_no_trailing_newline() {
    assert_eq!(slice_lines("only", 1, None), "only");
    assert_eq!(slice_lines("only", 1, Some(1)), "only");
}

#[test]
fn split_archive_path_lucidos_plugin() {
    assert_eq!(
        split_archive_path("artifacts/plugins/foo-0.1.0.lucidos-plugin/apps/x/index.html"),
        Some((
            "artifacts/plugins/foo-0.1.0.lucidos-plugin".to_string(),
            "apps/x/index.html".to_string(),
        ))
    );
}

#[test]
fn split_archive_path_zip() {
    assert_eq!(
        split_archive_path("artifacts/imports/bundle.zip/data.json"),
        Some((
            "artifacts/imports/bundle.zip".to_string(),
            "data.json".to_string(),
        ))
    );
}

#[test]
fn split_archive_path_no_inner_returns_none() {
    assert_eq!(
        split_archive_path("artifacts/plugins/foo.lucidos-plugin"),
        None
    );
    assert_eq!(split_archive_path("artifacts/foo.zip"), None);
}

#[test]
fn split_archive_path_regular_path_returns_none() {
    assert_eq!(split_archive_path("artifacts/notes.md"), None);
    assert_eq!(split_archive_path("apps/foo/index.html"), None);
}

#[test]
fn split_archive_path_first_archive_wins_for_nested() {
    assert_eq!(
        split_archive_path("artifacts/a.zip/b.zip/c.txt"),
        Some(("artifacts/a.zip".to_string(), "b.zip/c.txt".to_string()))
    );
}

#[test]
fn read_text_from_zip_returns_entry_content() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("test.zip");
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("inner/hello.txt", opts).unwrap();
    zw.write_all(b"hello world").unwrap();
    zw.finish().unwrap();

    let got = read_text_from_zip(
        &zip_path,
        "inner/hello.txt",
        READ_FILE_FROM_ARCHIVE_MAX_BYTES,
    )
    .unwrap();
    assert_eq!(got, "hello world");
}

#[test]
fn read_text_from_zip_rejects_oversized_entry() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("big.zip");
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("big.txt", opts).unwrap();
    zw.write_all(&[b'x'; 200]).unwrap();
    zw.finish().unwrap();

    let err = read_text_from_zip(&zip_path, "big.txt", 100).unwrap_err();
    assert!(
        err.contains("too large") || err.contains("exceeds"),
        "oversized entry must be rejected with a size hint, got: {}",
        err
    );
    assert!(
        err.contains("100"),
        "error should mention the cap, got: {}",
        err
    );
}

#[test]
fn read_text_from_zip_refuses_an_entry_whose_header_understates_its_size() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("bomb.zip");
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zw.start_file("bomb.txt", opts).unwrap();
    zw.write_all(&[b'a'; 1000]).unwrap();
    zw.finish().unwrap();

    let mut raw = std::fs::read(&zip_path).unwrap();
    let mut patch = |signature: &[u8; 4], offset: usize| {
        let at = raw
            .windows(4)
            .position(|w| w == signature)
            .expect("zip header present");
        raw[at + offset..at + offset + 4].copy_from_slice(&10u32.to_le_bytes());
    };
    patch(b"PK\x03\x04", 22);
    patch(b"PK\x01\x02", 24);
    std::fs::write(&zip_path, &raw).unwrap();

    let err = read_text_from_zip(&zip_path, "bomb.txt", 100).unwrap_err();
    assert!(
        err.contains("larger than its declared uncompressed size"),
        "an entry exceeding its declared size must be refused, got: {err}"
    );
}

#[test]
fn line_window_none_when_no_args() {
    assert_eq!(line_window_from_args(&serde_json::json!({})), None);
    assert_eq!(
        line_window_from_args(&serde_json::json!({"path": "x"})),
        None
    );
}

#[test]
fn line_window_start_only_defaults_count_to_open() {
    assert_eq!(
        line_window_from_args(&serde_json::json!({"start_line": 5})),
        Some((5, None))
    );
}

#[test]
fn line_window_count_only_defaults_start_to_one() {
    assert_eq!(
        line_window_from_args(&serde_json::json!({"line_count": 3})),
        Some((1, Some(3)))
    );
}

#[test]
fn line_window_both_args() {
    assert_eq!(
        line_window_from_args(&serde_json::json!({"start_line": 10, "line_count": 4})),
        Some((10, Some(4)))
    );
}

#[test]
fn archive_entry_unsupported_text_extensions_return_none() {
    for inner in [
        "apps/x/index.html",
        "knowhow/notes.md",
        "manifest.toml",
        "scripts/run.sh",
        "no-extension-file",
    ] {
        assert_eq!(
            archive_entry_unsupported_message(inner),
            None,
            "text-shaped inner path '{}' should not short-circuit",
            inner
        );
    }
}

#[test]
fn archive_entry_unsupported_binary_extensions_return_message() {
    for (inner, hint) in [
        ("icon.png", "binary"),
        ("doc.pdf", "binary"),
        ("apps/foo/screenshot.jpg", "binary"),
        ("nested.zip", "binary"),
    ] {
        let msg = archive_entry_unsupported_message(inner)
            .unwrap_or_else(|| panic!("'{}' should short-circuit with a message", inner));
        assert!(
            msg.contains(hint),
            "message for '{}' should mention '{}', got: {}",
            inner,
            hint,
            msg
        );
        assert!(
            msg.contains(inner),
            "message for '{}' should name the entry, got: {}",
            inner,
            msg
        );
    }
}

#[test]
fn read_text_from_zip_rejects_empty_inner_path() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("test.zip");
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("a.txt", opts).unwrap();
    zw.write_all(b"a").unwrap();
    zw.finish().unwrap();

    let err = read_text_from_zip(&zip_path, "", READ_FILE_FROM_ARCHIVE_MAX_BYTES).unwrap_err();
    assert!(
        err.contains("rejected") && err.contains("unsafe"),
        "empty inner must surface the validation error, got: {}",
        err
    );
}

#[test]
fn read_text_from_zip_rejects_zip_slip_inner_path() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("test.zip");
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("a.txt", opts).unwrap();
    zw.write_all(b"a").unwrap();
    zw.finish().unwrap();

    for bad in [
        "../a.txt",
        "/etc/passwd",
        "foo/../../etc/passwd",
        "\\windows",
    ] {
        let err = read_text_from_zip(&zip_path, bad, READ_FILE_FROM_ARCHIVE_MAX_BYTES).unwrap_err();
        assert!(
            err.contains("unsafe"),
            "rejected `{}` must surface the validation error (not a generic \
             'not found'), got: {}",
            bad,
            err
        );
    }
}

#[test]
fn read_text_from_zip_missing_entry_errors() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("test.zip");
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("a.txt", opts).unwrap();
    zw.write_all(b"a").unwrap();
    zw.finish().unwrap();

    let err =
        read_text_from_zip(&zip_path, "missing.txt", READ_FILE_FROM_ARCHIVE_MAX_BYTES).unwrap_err();
    assert!(
        err.contains("missing.txt"),
        "error should name the missing entry, got: {}",
        err
    );
}

#[test]
fn a_partial_text_edit_reports_both_counts_and_names_replace_all() {
    let content = "a\nfoo\nb\nfoo\nc\nfoo\nd\nfoo\n";
    let (out, outcome) = apply_text_edit(content, "foo", "bar", false, "x.md").unwrap();

    assert_eq!(out, "a\nbar\nb\nfoo\nc\nfoo\nd\nfoo\n");
    assert_eq!(out.matches("bar").count(), 1);
    assert_eq!(out.matches("foo").count(), 3);

    let msg = FileEditResult {
        path: "x.md".to_string(),
        outcome,
        target: EditTarget::Workspace {
            commit: "abc1234".to_string(),
        },
    }
    .tool_message();
    assert!(msg.contains("1 of 4 occurrences replaced"), "got: {msg}");
    assert!(msg.contains("replace_all"), "got: {msg}");
}

#[test]
fn replace_all_reports_every_occurrence_and_never_mentions_the_flag() {
    let content = "foo foo foo foo";
    let (out, outcome) = apply_text_edit(content, "foo", "bar", true, "x.md").unwrap();

    assert_eq!(out, "bar bar bar bar");
    let msg = FileEditResult {
        path: "x.md".to_string(),
        outcome,
        target: EditTarget::Workspace {
            commit: "abc1234".to_string(),
        },
    }
    .tool_message();
    assert!(msg.contains("4 of 4 occurrences replaced"), "got: {msg}");
    assert!(
        !msg.contains("replace_all"),
        "nothing was left behind, so the hint is noise: {msg}"
    );
}

#[test]
fn a_single_occurrence_reports_one_of_one() {
    let (_, outcome) = apply_text_edit("only once here", "once", "twice", false, "x.md").unwrap();
    let msg = FileEditResult {
        path: "x.md".to_string(),
        outcome,
        target: EditTarget::Workspace {
            commit: "abc1234".to_string(),
        },
    }
    .tool_message();
    assert!(msg.contains("1 of 1 occurrences replaced"), "got: {msg}");
}

#[test]
fn the_reported_counts_match_the_written_content() {
    for (content, replace_all, expect_replaced, expect_found) in [
        ("x x x", false, 1, 3),
        ("x x x", true, 3, 3),
        ("x", false, 1, 1),
    ] {
        let (out, outcome) = apply_text_edit(content, "x", "y", replace_all, "f").unwrap();
        let EditOutcome::Text {
            replaced,
            occurrences,
        } = outcome
        else {
            panic!("text mode must produce a text outcome");
        };
        assert_eq!(replaced, expect_replaced, "{content:?}");
        assert_eq!(occurrences, expect_found, "{content:?}");
        assert_eq!(out.matches('y').count(), replaced, "{content:?}");
        assert_eq!(out.matches('x').count(), occurrences - replaced);
    }
}

#[test]
fn json_mode_reports_no_occurrence_count() {
    let msg = FileEditResult {
        path: "artifacts/x.json".to_string(),
        outcome: EditOutcome::JsonValueSet,
        target: EditTarget::Workspace {
            commit: "abc1234".to_string(),
        },
    }
    .tool_message();
    assert_eq!(
        msg,
        "[ACTION COMPLETED] UPDATED: artifacts/x.json (commit: abc1234)"
    );
}

#[test]
fn a_missing_old_string_still_errors_by_its_recognised_phrase() {
    let err = apply_text_edit("nothing here", "absent", "x", false, "x.md").unwrap_err();
    assert!(err.contains("old_string not found"), "got: {err}");
}

#[test]
fn a_repo_edit_result_says_uncommitted_and_carries_no_sha() {
    let msg = FileEditResult {
        path: "Lucidos/src/main.rs".to_string(),
        outcome: EditOutcome::Text {
            replaced: 2,
            occurrences: 2,
        },
        target: EditTarget::RepoWorkingTree,
    }
    .tool_message();

    assert!(msg.contains("Lucidos/src/main.rs"), "got: {msg}");
    assert!(msg.contains("2 of 2 occurrences replaced"), "got: {msg}");
    assert!(msg.contains("NOT committed"), "got: {msg}");
    assert!(msg.contains("git diff"), "got: {msg}");
    assert!(!msg.contains("commit:"), "no sha may appear: {msg}");
}

#[test]
fn commit_pairing_accepts_only_the_two_honest_combinations() {
    assert!(check_commit_pairing(false, None).is_ok());
    assert!(check_commit_pairing(true, Some(false)).is_ok());
}

#[test]
fn commit_pairing_refuses_a_repo_edit_that_would_commit() {
    let err = check_commit_pairing(true, Some(true)).unwrap_err();
    assert!(err.contains("never committed"), "got: {err}");
    assert!(err.contains("run_coding_agent"), "got: {err}");
}

#[test]
fn commit_pairing_refuses_a_repo_edit_that_stays_silent() {
    let err = check_commit_pairing(true, None).unwrap_err();
    assert!(err.contains("commit: false"), "got: {err}");
}

#[test]
fn commit_pairing_refuses_an_uncommitted_workspace_write() {
    let err = check_commit_pairing(false, Some(false)).unwrap_err();
    assert!(err.contains("needs `repo`"), "got: {err}");
    assert!(err.contains("run_python"), "got: {err}");
}

#[test]
fn an_empty_old_string_is_refused_rather_than_matching_everywhere() {
    for (new, replace_all) in [("x", true), ("x", false), ("", false)] {
        let err = apply_text_edit("ab", "", new, replace_all, "x.md").unwrap_err();
        assert!(err.contains("old_string is empty"), "got: {err}");
        assert!(
            err.contains("write_file"),
            "the refusal names a route: {err}"
        );
    }
}

#[test]
fn only_an_in_process_write_under_data_scripts_records_authorship() {
    assert!(records_script_authorship(
        WriteAuthorship::InProcessTool,
        "data/scripts/auth/comfort-cloud.py"
    ));
    assert!(!records_script_authorship(
        WriteAuthorship::ApiCaller,
        "data/scripts/auth/comfort-cloud.py"
    ));
    for elsewhere in [
        "data/artifacts/notes.md",
        "data/config/apis.json",
        "data/apps/habit-tracker/scripts/x.py",
        "scripts/auth/x.py",
    ] {
        assert!(
            !records_script_authorship(WriteAuthorship::InProcessTool, elsewhere),
            "{elsewhere} is not a handshake script"
        );
    }
}

#[test]
fn a_file_resolved_into_the_shared_user_dir_is_owned_by_it() {
    let user_dir = Path::new("/home/user/.lucidos");
    let shared = user_dir.join("knowhow/cooking.md");
    assert_eq!(
        change_owner(Some(user_dir), "knowhow/cooking.md", &shared),
        ChangeOwner::SharedUserDir {
            root: user_dir,
            relative: "knowhow/cooking.md".to_string(),
        }
    );
}

#[test]
fn a_workspace_file_is_owned_by_its_app_or_the_workspace() {
    let user_dir = Path::new("/home/user/.lucidos");
    let data = Path::new("/home/user/workspaces/myws/data");
    assert_eq!(
        change_owner(
            Some(user_dir),
            "knowhow/cooking.md",
            &data.join("knowhow/cooking.md")
        ),
        ChangeOwner::Workspace
    );
    assert_eq!(
        change_owner(
            Some(user_dir),
            "apps/habit-tracker/index.html",
            &data.join("apps/habit-tracker/index.html")
        ),
        ChangeOwner::App("habit-tracker/index.html")
    );
    assert_eq!(
        change_owner(None, "artifacts/notes.md", &data.join("artifacts/notes.md")),
        ChangeOwner::Workspace
    );
}

#[test]
fn deleting_a_shared_file_removes_it_from_the_shared_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let user_dir = tmp.path().join(".lucidos");
    std::fs::create_dir_all(user_dir.join("knowhow")).unwrap();
    crate::core::user_dir::ensure_git_init(&user_dir);
    let shared = user_dir.join("knowhow/cooking.md");
    std::fs::write(&shared, "Content.").unwrap();
    crate::core::user_dir::auto_commit(&user_dir, "knowhow/cooking.md", "add");

    let ChangeOwner::SharedUserDir { root, relative } =
        change_owner(Some(&user_dir), "knowhow/cooking.md", &shared)
    else {
        panic!("a shared file must be owned by the shared user dir");
    };
    crate::core::user_dir::remove_and_commit(root, &relative, "delete").unwrap();

    assert!(!shared.exists(), "the shared file must be gone");
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&user_dir)
        .output()
        .unwrap();
    assert!(
        status.stdout.is_empty(),
        "the removal must be committed: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}

#[test]
fn a_copy_onto_its_own_source_is_recognised() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.md");
    std::fs::write(&a, "x").unwrap();
    assert!(is_same_file(&a, &a));
    assert!(is_same_file(&a, &dir.path().join("./a.md")));
    let link = dir.path().join("link.md");
    std::fs::hard_link(&a, &link).unwrap();
    assert!(is_same_file(&a, &link));

    let b = dir.path().join("b.md");
    std::fs::write(&b, "x").unwrap();
    assert!(!is_same_file(&a, &b));
    assert!(!is_same_file(&a, &dir.path().join("new.md")));
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "x");
}

async fn engine_on(db: &str, workspace: &Path) -> crate::engine::LucidosEngine {
    crate::engine::LucidosEngine::new(
        workspace.to_path_buf(),
        &crate::test_support::test_db_url(db),
        crate::llm::ProviderBuildContext {
            default_model: "mock".into(),
            model_is_mock: true,
            vertex_project_id: String::new(),
            vertex_location: crate::llm::vertex::location_handle(String::new()),
            vertex_token_cache: None,
            model_registry: Default::default(),
            not_served: Default::default(),
            claude_gateway_access: None,
            chatgpt_plan_access: None,
        },
        std::sync::Arc::new(crate::engine::agent_recovery::EngineAgentHost),
        Box::new(crate::engine::provider_adapters::ProviderAdapters::default()),
    )
    .await
    .unwrap()
}

struct BroadcastLog {
    rx: tokio::sync::broadcast::Receiver<crate::engine::event_bus::EmittedEvent>,
    seen: Vec<&'static str>,
}

impl BroadcastLog {
    fn count(&mut self, event_type: &str) -> usize {
        while let Ok(emitted) = self.rx.try_recv() {
            if let crate::engine::event_bus::BusEvent::System(event) = emitted.typed {
                self.seen.push(event.event_type());
            }
        }
        self.seen.iter().filter(|seen| **seen == event_type).count()
    }
}

#[tokio::test]
async fn a_copy_onto_its_own_source_leaves_the_file_intact() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let engine = engine_on(&db, workspace.path()).await;
    let thread = uuid::Uuid::new_v4();

    let written = engine
        .execute_file_tool(
            "write_file",
            &serde_json::json!({ "path": "knowhow/notes.md", "content": "keep me" }),
            thread,
            None,
        )
        .await
        .unwrap();
    assert!(written.starts_with("[ACTION COMPLETED]"), "{written}");

    let out = engine
        .execute_file_tool(
            "copy_file",
            &serde_json::json!({ "source": "knowhow/notes.md", "destination": "knowhow/notes.md" }),
            thread,
            None,
        )
        .await
        .unwrap();

    assert!(out.starts_with("Error:"), "{out}");
    let on_disk = std::fs::read_to_string(workspace.path().join("data/knowhow/notes.md")).unwrap();
    assert_eq!(on_disk, "keep me");

    engine.pool.close().await;
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_file_tools_announce_an_apps_birth_and_death_but_not_its_edits() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let engine = engine_on(&db, workspace.path()).await;
    let mut log = BroadcastLog {
        rx: engine.event_bus.subscribe(),
        seen: Vec::new(),
    };
    let thread = uuid::Uuid::new_v4();
    let manifest = |name: &str| serde_json::json!({ "name": name, "description": "d" }).to_string();

    let write = |path: &'static str, content: String| serde_json::json!({ "path": path, "content": content });
    let out = engine
        .execute_file_tool(
            "write_file",
            &write("apps/demo/manifest.json", manifest("Demo")),
            thread,
            None,
        )
        .await
        .unwrap();
    assert!(out.starts_with("[ACTION COMPLETED]"), "{out}");
    assert_eq!(log.count("AppCreated"), 1);

    engine
        .execute_file_tool(
            "write_file",
            &write("apps/demo/manifest.json", manifest("Demo renamed")),
            thread,
            None,
        )
        .await
        .unwrap();
    assert_eq!(log.count("AppCreated"), 1);
    assert_eq!(log.count("AppUpdated"), 0);

    engine
        .execute_file_tool(
            "write_file",
            &write("knowhow/seed.json", manifest("Copied")),
            thread,
            None,
        )
        .await
        .unwrap();
    let out = engine
        .execute_file_tool(
            "copy_file",
            &serde_json::json!({
                "source": "knowhow/seed.json",
                "destination": "apps/copied/manifest.json",
            }),
            thread,
            None,
        )
        .await
        .unwrap();
    assert!(out.starts_with("[ACTION COMPLETED]"), "{out}");
    assert_eq!(log.count("AppCreated"), 2);

    let out = engine
        .execute_file_tool(
            "delete_file",
            &serde_json::json!({ "path": "apps/demo/manifest.json" }),
            thread,
            None,
        )
        .await
        .unwrap();
    assert!(out.starts_with("[ACTION COMPLETED]"), "{out}");
    assert_eq!(log.count("AppDeleted"), 1);

    engine.pool.close().await;
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_failed_glob_reaches_the_model_with_the_error_prefix() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let engine = engine_on(&db, workspace.path()).await;

    let out = engine
        .execute_file_tool(
            "glob_files",
            &serde_json::json!({ "pattern": "" }),
            uuid::Uuid::new_v4(),
            None,
        )
        .await
        .unwrap();

    assert!(out.starts_with("Error:"), "{out}");

    engine.pool.close().await;
    pool.close().await;
    teardown_test_db(&db).await;
}

mod guard_tests {
    #[test]
    fn no_http_handler_claims_in_process_authorship() {
        let offenders: Vec<String> = crate::test_support::source_scan::production_sources()
            .into_iter()
            .filter(|(rel, text)| {
                rel.starts_with("api/") && text.contains("WriteAuthorship::InProcessTool")
            })
            .map(|(rel, _)| rel)
            .collect();
        assert!(
            offenders.is_empty(),
            "an HTTP handler cannot vouch for a handshake script: {offenders:?}"
        );
    }
}
