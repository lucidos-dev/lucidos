use super::*;

#[test]
fn read_only_reason_blocks_system_knowhow() {
    assert!(read_only_reason("system-knowhow/best-practices.md").is_some());
    assert!(read_only_reason("system-knowhow/scripts/list.sh").is_some());
}

#[test]
fn read_only_reason_allows_user_paths() {
    assert!(read_only_reason("artifacts/notes.md").is_none());
    assert!(read_only_reason("knowhow/lucidos/best-practices.md").is_none());
    assert!(read_only_reason("apps/foo/index.html").is_none());
    assert!(read_only_reason("triggers/daily/check.md").is_none());
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

    // Verify parsing matches agentic_loop logic
    let rest = marker.strip_prefix("[IMAGE_CONTENT:").unwrap();
    let end_bracket = rest.find("]\n").unwrap();
    let parsed_media = &rest[..end_bracket];
    let parsed_data = rest[end_bracket + 2..].trim();

    assert_eq!(parsed_media, "image/png");
    assert_eq!(parsed_data, "iVBORw0KGgo=");
}

#[test]
fn test_image_size_guard() {
    // Verify the constant is 5 MB
    assert_eq!(IMAGE_MAX_BYTES, 5 * 1024 * 1024);
}

#[test]
fn test_read_image_file_returns_base64_marker() {
    let dir = tempfile::tempdir().unwrap();
    let img_path = dir.path().join("test.png");
    // Minimal valid PNG (1x1 transparent pixel)
    let png_bytes: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
        0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49,
        0x44, 0x41, 0x54, 0x78, 0x9C, 0x62, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, 0x27,
        0xDE, 0xFC, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    std::fs::write(&img_path, &png_bytes).unwrap();

    let extension = "png";
    let media_type = image_media_type(extension).unwrap();
    let bytes = std::fs::read(&img_path).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let result = format!("[IMAGE_CONTENT:{}]\n{}", media_type, b64);

    assert!(result.starts_with("[IMAGE_CONTENT:image/png]\n"));
    // Verify round-trip: decode the base64 back
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
        assert!(stub.contains("image/png"), "stub mentions media type: {}", stub);
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
    // 4 base64 chars = ~3 decoded bytes
    let stub_small = strip_image_content_marker("[IMAGE_CONTENT:image/png]\nABCD").unwrap();
    assert!(stub_small.contains("bytes"), "{}", stub_small);

    // ~1.4 MB of base64 → ~1 MB decoded
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
    // Missing the closing `]\n` separator
    assert!(strip_image_content_marker("[IMAGE_CONTENT:image/png ABCDEFGH").is_none());
    // Has prefix but no body separator
    assert!(strip_image_content_marker("[IMAGE_CONTENT:image/png]ABCDEFGH").is_none());
}

#[test]
fn strip_image_content_marker_strips_real_size() {
    // Sanity: a 2 MB base64 string should produce a stub under 100 bytes — the whole
    // point of the helper. This is the behaviour the bugfix relies on.
    let huge = "A".repeat(2 * 1024 * 1024);
    let input = format!("[IMAGE_CONTENT:image/png]\n{}", huge);
    let stub = strip_image_content_marker(&input).unwrap();
    assert!(stub.len() < 100, "stub size {} should be < 100", stub.len());
    assert!(input.len() > 1_000_000);
}

#[test]
fn test_large_image_error_message() {
    let size: u64 = 6 * 1024 * 1024; // 6 MB
    assert!(size > IMAGE_MAX_BYTES);
    let mb = size as f64 / (1024.0 * 1024.0);
    let msg = format!("Image too large to read directly ({:.1} MB). Max 5MB.", mb);
    assert_eq!(msg, "Image too large to read directly (6.0 MB). Max 5MB.");
}

#[test]
fn test_svg_not_treated_as_image() {
    // SVG should not match image_media_type — it's text-based
    assert_eq!(image_media_type("svg"), None);
    // SVG is also not in is_binary_extension, so it goes through text path
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
    let mut doc: serde_json::Value =
        serde_json::from_str(r#"{"meta": {"version": 1}}"#).unwrap();
    let new_val = serde_json::json!({"version": 2, "author": "test"});
    let result = json_set_value(&mut doc, "/meta", new_val.clone());
    assert!(result.is_ok());
    assert_eq!(doc["meta"], new_val);
}

#[test]
fn test_json_set_value_replace_array_element() {
    let mut doc: serde_json::Value =
        serde_json::from_str(r#"{"items": ["a", "b", "c"]}"#).unwrap();
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
    assert_eq!(
        dot_path_to_pointer(r#"data["foo.bar"]"#),
        "/data/foo.bar"
        );
    }

    #[test]
    fn test_dot_path_to_pointer_quoted_key_pointer_escapes() {
        // RFC 6901: '~' → '~0', '/' → '~1'.
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
    assert!(result.is_ok(), "Failed to set value at {}: {:?}", pointer, result);
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

// --- slice_lines ------------------------------------------------------

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

// --- split_archive_path -----------------------------------------------

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
    // Just the archive itself — caller wants the bytes, not unzip semantics.
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
    // We don't recurse into nested archives; first-match wins.
    assert_eq!(
        split_archive_path("artifacts/a.zip/b.zip/c.txt"),
        Some(("artifacts/a.zip".to_string(), "b.zip/c.txt".to_string()))
    );
}

// --- read_text_from_zip -----------------------------------------------

#[test]
fn read_text_from_zip_returns_entry_content() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("test.zip");
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    zw.start_file("inner/hello.txt", opts).unwrap();
    zw.write_all(b"hello world").unwrap();
    zw.finish().unwrap();

    let got =
        read_text_from_zip(&zip_path, "inner/hello.txt", READ_FILE_FROM_ARCHIVE_MAX_BYTES)
            .unwrap();
    assert_eq!(got, "hello world");
}

#[test]
fn read_text_from_zip_rejects_oversized_entry() {
    // Zip-bomb defense: an inner entry whose uncompressed size exceeds the cap
    // is rejected before we allocate the buffer for it. Test uses a tiny cap so
    // the fixture stays small.
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("big.zip");
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    zw.start_file("big.txt", opts).unwrap();
    zw.write_all(&[b'x'; 200]).unwrap();
    zw.finish().unwrap();

    let err = read_text_from_zip(&zip_path, "big.txt", 100).unwrap_err();
    assert!(
        err.contains("too large") || err.contains("exceeds"),
        "oversized entry must be rejected with a size hint, got: {}",
        err
    );
    // Cap is reported so the LLM knows the limit.
    assert!(err.contains("100"), "error should mention the cap, got: {}", err);
}

// --- line_window_from_args -------------------------------------------

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
        // Must mention the inner path so the LLM knows what was rejected.
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
    // Empty inner names are rejected by validate_archive_entry_path itself
    // (see core::plugins::tests::rejects_empty_path), surfaced here via the
    // shared "rejected zip entry" wrapper.
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("test.zip");
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
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
    // Zip-slip defense: an inner path with `..` or a leading slash must be rejected
    // before we even hash-lookup the entry, matching what extract_zip enforces.
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("test.zip");
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    zw.start_file("a.txt", opts).unwrap();
    zw.write_all(b"a").unwrap();
    zw.finish().unwrap();

    for bad in ["../a.txt", "/etc/passwd", "foo/../../etc/passwd", "\\windows"] {
        let err = read_text_from_zip(&zip_path, bad, READ_FILE_FROM_ARCHIVE_MAX_BYTES)
            .unwrap_err();
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
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    zw.start_file("a.txt", opts).unwrap();
    zw.write_all(b"a").unwrap();
    zw.finish().unwrap();

    let err =
        read_text_from_zip(&zip_path, "missing.txt", READ_FILE_FROM_ARCHIVE_MAX_BYTES)
            .unwrap_err();
    assert!(
        err.contains("missing.txt"),
        "error should name the missing entry, got: {}",
        err
    );
}
