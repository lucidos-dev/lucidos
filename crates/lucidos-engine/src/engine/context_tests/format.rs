use super::*;
use crate::core::store::types::Step;
use std::collections::HashSet;

fn no_skip() -> HashSet<String> {
    HashSet::new()
}

fn tool_step(name: &str, description: &str, success: bool) -> Step {
    Step {
        description: description.to_string(),
        tool_name: Some(name.to_string()),
        success,
        context_tokens: None,
        context_messages: None,
        trimmed: None,
        tool_called_event_id: None,
    }
}

fn synthetic_step(description: &str) -> Step {
    Step {
        description: description.to_string(),
        tool_name: None,
        success: true,
        context_tokens: None,
        context_messages: None,
        trimmed: None,
        tool_called_event_id: None,
    }
}

#[test]
fn format_history_steps_empty_returns_none() {
    assert!(format_history_steps(&[], &no_skip()).is_none());
}

#[test]
fn format_history_steps_skips_thinking_and_memory() {
    let steps = vec![
        synthetic_step("Requesting"),
        synthetic_step("Memory: 3 results"),
    ];
    assert!(
        format_history_steps(&steps, &no_skip()).is_none(),
        "synthetic (no tool_name) steps must not appear in history"
    );
}

#[test]
fn format_history_steps_renders_tool_calls() {
    let steps = vec![
        tool_step(
            "load_knowhow",
            "Loading know-how 'ops/nightly-pipeline'...",
            true,
        ),
        tool_step("emit_event", "Emitting BuildClean event...", true),
    ];
    let out = format_history_steps(&steps, &no_skip()).unwrap();
    assert!(out.starts_with(" [tools: "));
    assert!(out.contains("Loading know-how 'ops/nightly-pipeline'"));
    assert!(out.contains("Emitting BuildClean event"));
    assert!(out.contains("[ok]"));
    // No trailing "..." inherited from describe_tool's progress format
    assert!(!out.contains("...["));
}

#[test]
fn format_history_steps_marks_failures() {
    let steps = vec![tool_step("load_knowhow", "Loading know-how 'oops'...", false)];
    let out = format_history_steps(&steps, &no_skip()).unwrap();
    assert!(out.contains("[FAIL]"));
    assert!(!out.contains("[ok]"));
}

#[test]
fn format_history_steps_capped_at_2k() {
    // 50 tool calls each with a long description should be truncated, not unbounded.
    let steps: Vec<Step> = (0..50)
        .map(|i| tool_step("read_file", &format!("Reading {}.rs...", "x".repeat(80 + i)), true))
        .collect();
    let out = format_history_steps(&steps, &no_skip()).unwrap();
    // Cap is 2KB on the joined inner string + a small prefix; allow some slack.
    assert!(
        out.len() <= 2_200,
        "expected output to be capped, got {} bytes",
        out.len()
    );
    assert!(out.contains("chars omitted"), "expected truncation marker");
}

/// Skipping a tool by event id removes it from the summary entirely. Used by
/// the resume path to dedupe tools that were already emitted as full
/// `Message::Blocks(...)` pairs.
#[test]
fn format_history_steps_skips_tools_in_skip_set() {
    let mut load = tool_step("load_knowhow", "Loading 'r'...", true);
    load.tool_called_event_id = Some("evt-load-id".into());
    let mut emit = tool_step("emit_event", "Emitting...", true);
    emit.tool_called_event_id = Some("evt-emit-id".into());
    let steps = vec![load, emit];
    let mut skip = HashSet::new();
    skip.insert("evt-load-id".to_string());
    let out = format_history_steps(&steps, &skip).unwrap();
    assert!(!out.contains("Loading"), "skipped tool must not appear: {}", out);
    assert!(out.contains("Emitting"), "non-skipped tool must remain: {}", out);
}

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
