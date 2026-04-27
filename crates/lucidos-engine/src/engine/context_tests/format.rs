use super::*;

#[test]
fn short_message_unchanged() {
    let content = "Hello, how are you?";
    assert_eq!(format_history_content(content, "user", true), content);
    assert_eq!(format_history_content(content, "assistant", true), content);
    assert_eq!(format_history_content(content, "user", false), content);
    assert_eq!(format_history_content(content, "assistant", false), content);
}

#[test]
fn verbatim_tail_only_safety_net() {
    // A 2000-char assistant message in the verbatim tail should NOT be compacted
    let content = "x".repeat(2000);
    let result = format_history_content(&content, "assistant", true);
    assert_eq!(
        result, content,
        "verbatim tail assistant msg under 15K should be untouched"
    );
}

#[test]
fn middle_tier_assistant_compacted() {
    // A 2000-char assistant message outside verbatim tail should be compacted to ~1500
    let content = "x".repeat(2000);
    let result = format_history_content(&content, "assistant", false);
    assert!(
        result.len() < 2000,
        "middle tier assistant should be compacted"
    );
    assert!(
        result.contains("chars omitted"),
        "should have omission marker"
    );
}

#[test]
fn middle_tier_user_not_compacted() {
    // A 2000-char user message outside verbatim tail should NOT be compacted
    let content = "x".repeat(2000);
    let result = format_history_content(&content, "user", false);
    assert_eq!(
        result, content,
        "user messages should never be compacted (only safety net at 15K)"
    );
}

#[test]
fn safety_net_truncation_at_15k() {
    let content = "x".repeat(20_000);
    let result = format_history_content(&content, "user", true);
    assert!(result.len() < 20_000, "should be truncated");
    assert!(
        result.contains("chars omitted"),
        "should have omission marker"
    );
    // Should preserve head and tail
    assert!(
        result.starts_with("xxx"),
        "should start with original content"
    );
    assert!(result.ends_with("xxx"), "should end with original content");
}

#[test]
fn truncate_head_tail_preserves_both_ends() {
    let content = format!("{}MIDDLE{}", "A".repeat(500), "Z".repeat(500));
    let result = truncate_head_tail(&content, 200);
    assert!(result.starts_with("AAAA"), "should preserve start");
    assert!(result.ends_with("ZZZZ"), "should preserve end");
    assert!(result.contains("chars omitted"));
}

#[test]
fn truncate_head_tail_under_limit_unchanged() {
    let content = "short message";
    assert_eq!(truncate_head_tail(content, 200), content);
}

#[test]
fn trim_history_from_oldest_removes_start() {
    let mut history = "line1\nline2\nline3\nline4\nline5".to_string();
    trim_history_from_oldest(&mut history, 6); // trim past "line1\n"
    assert!(
        history.starts_with("line2") || history.starts_with("line3"),
        "should have trimmed from start, got: {}",
        history
    );
    assert!(history.contains("line5"), "should preserve end");
}

#[test]
fn trim_history_from_oldest_clears_if_excess() {
    let mut history = "short".to_string();
    trim_history_from_oldest(&mut history, 1000);
    assert!(
        history.is_empty(),
        "should be empty when trimming more than available"
    );
}

#[test]
fn trim_history_from_oldest_multibyte_safe() {
    // Em-dash (U+2014) is 3 bytes — trimming into the middle must not panic
    let mut history = "hello—world\nrecent".to_string();
    trim_history_from_oldest(&mut history, 6); // byte 6 is inside the em-dash
    assert!(history.contains("recent"), "should preserve recent content");
}

#[test]
fn trim_history_from_oldest_preserves_newest() {
    let mut history =
        "old message 1\nold message 2\nrecent message 1\nrecent message 2".to_string();
    trim_history_from_oldest(&mut history, 30); // trim past both old messages
    assert!(
        history.contains("recent message"),
        "should preserve recent messages"
    );
    assert!(
        !history.contains("old message 1"),
        "should have trimmed old message 1"
    );
}
