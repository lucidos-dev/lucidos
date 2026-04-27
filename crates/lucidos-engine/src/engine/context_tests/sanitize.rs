use super::*;

#[test]
fn small_content_returned_verbatim() {
    let out = sanitize_file_content_for_llm("hello world".to_string(), "x.md", 0);
    assert_eq!(out, "hello world");
}

#[test]
fn oversized_content_truncated_with_continuation_hint() {
    let big = "a".repeat(READ_FILE_MAX_BYTES * 2);
    let out = sanitize_file_content_for_llm(big.clone(), "big.md", 0);
    assert!(out.starts_with(&"a".repeat(READ_FILE_MAX_BYTES)));
    assert!(
        out.contains(&format!("offset={}", READ_FILE_MAX_BYTES)),
        "truncation message must tell LLM the next offset to call:\n{out}"
    );
    assert!(
        out.contains(&format!("{}", big.len())),
        "should mention total size"
    );
}

#[test]
fn nonzero_offset_returns_tail_slice() {
    let content = "0123456789abcdef".to_string();
    let out = sanitize_file_content_for_llm(content, "x.md", 5);
    assert!(out.starts_with("56789abcdef"), "got: {out}");
}

#[test]
fn offset_past_end_returns_clear_error() {
    let out = sanitize_file_content_for_llm("short".to_string(), "x.md", 1000);
    assert!(out.contains("offset"), "should mention offset: {out}");
    assert!(out.contains("5"), "should mention actual size: {out}");
}

#[test]
fn offset_chunk_continuation_hint_uses_absolute_offset() {
    let big = "z".repeat(READ_FILE_MAX_BYTES * 3);
    let out = sanitize_file_content_for_llm(big, "big.md", READ_FILE_MAX_BYTES);
    // Next chunk should start at 2 * READ_FILE_MAX_BYTES, not READ_FILE_MAX_BYTES.
    assert!(
        out.contains(&format!("offset={}", READ_FILE_MAX_BYTES * 2)),
        "continuation must point past current chunk:\n{out}"
    );
}

#[test]
fn offset_exactly_at_end_returns_empty_not_error() {
    // The previous chunk's continuation hint for an exact-multiple file lands here.
    let out = sanitize_file_content_for_llm("hello".to_string(), "x.md", 5);
    assert_eq!(out, "", "EOF sentinel must return empty, not error: {out}");
}

#[test]
fn empty_file_with_nonzero_offset_errors() {
    let out = sanitize_file_content_for_llm(String::new(), "x.md", 1);
    assert!(out.starts_with("Error:"), "got: {out}");
}

#[test]
fn data_uri_stripped_before_truncation() {
    let uri = format!("data:image/png;base64,{}", "A".repeat(1000));
    let content = format!("prefix {} suffix", uri);
    let out = sanitize_file_content_for_llm(content, "x.md", 0);
    assert!(
        out.contains("[embedded image"),
        "should strip data URI: {out}"
    );
    assert!(!out.contains("AAAA"), "raw base64 must be gone");
}
