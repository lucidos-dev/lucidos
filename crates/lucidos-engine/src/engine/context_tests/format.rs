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
    assert!(!out.contains("...["));
}

#[test]
fn format_history_steps_marks_failures() {
    let steps = vec![tool_step(
        "load_knowhow",
        "Loading know-how 'oops'...",
        false,
    )];
    let out = format_history_steps(&steps, &no_skip()).unwrap();
    assert!(out.contains("[FAIL]"));
    assert!(!out.contains("[ok]"));
}

#[test]
fn format_history_steps_capped_at_2k() {
    let steps: Vec<Step> = (0..50)
        .map(|i| {
            tool_step(
                "read_file",
                &format!("Reading {}.rs...", "x".repeat(80 + i)),
                true,
            )
        })
        .collect();
    let out = format_history_steps(&steps, &no_skip()).unwrap();
    assert!(
        out.len() <= 2_200,
        "expected output to be capped, got {} bytes",
        out.len()
    );
    assert!(out.contains("chars omitted"), "expected truncation marker");
}

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
    assert!(
        !out.contains("Loading"),
        "skipped tool must not appear: {}",
        out
    );
    assert!(
        out.contains("Emitting"),
        "non-skipped tool must remain: {}",
        out
    );
}

#[test]
fn short_message_unchanged() {
    let content = "Hello, how are you?";
    let loaded: Vec<&str> = Vec::new();
    assert_eq!(
        format_history_content(content, "user", true, &loaded),
        content
    );
    assert_eq!(
        format_history_content(content, "assistant", true, &loaded),
        content
    );
    assert_eq!(
        format_history_content(content, "user", false, &loaded),
        content
    );
    assert_eq!(
        format_history_content(content, "assistant", false, &loaded),
        content
    );
}

#[test]
fn verbatim_tail_only_safety_net() {
    let content = "x".repeat(2000);
    let loaded: Vec<&str> = Vec::new();
    let result = format_history_content(&content, "assistant", true, &loaded);
    assert_eq!(
        result, content,
        "verbatim tail assistant msg under 15K should be untouched"
    );
}

#[test]
fn middle_tier_assistant_compacted() {
    let content = "x".repeat(2000);
    let loaded: Vec<&str> = Vec::new();
    let result = format_history_content(&content, "assistant", false, &loaded);
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
    let content = "x".repeat(2000);
    let loaded: Vec<&str> = Vec::new();
    let result = format_history_content(&content, "user", false, &loaded);
    assert_eq!(
        result, content,
        "user messages should never be compacted (only safety net at 15K)"
    );
}

#[test]
fn safety_net_truncation_at_15k() {
    let content = "x".repeat(20_000);
    let loaded: Vec<&str> = Vec::new();
    let result = format_history_content(&content, "user", true, &loaded);
    assert!(result.len() < 20_000, "should be truncated");
    assert!(
        result.contains("chars omitted"),
        "should have omission marker"
    );
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
    trim_history_from_oldest(&mut history, 6);
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
    let mut history = "hello—world\nrecent".to_string();
    trim_history_from_oldest(&mut history, 6);
    assert!(history.contains("recent"), "should preserve recent content");
}

#[test]
fn strips_loaded_body_substring() {
    let body =
        "[SYSTEM-KNOWHOW: Nightly Ops]\nfull body line 1\nfull body line 2\n[END SYSTEM-KNOWHOW]";
    let content = format!("Before\n{body}\nAfter");
    let out = format_history_content(&content, "user", false, &[body]);
    assert!(
        !out.contains("full body line 1"),
        "body must be stripped: {}",
        out
    );
    assert!(
        !out.contains("[END SYSTEM-KNOWHOW]"),
        "end marker must be stripped: {}",
        out
    );
    assert!(
        out.contains("(body in [LOADED KNOWHOW] section above)"),
        "must include pointer: {}",
        out
    );
    assert!(out.contains("Before"), "prefix must survive: {}", out);
    assert!(out.contains("After"), "suffix must survive: {}", out);
}

#[test]
fn keeps_block_when_body_not_loaded() {
    let unrelated_body = "[SYSTEM-KNOWHOW: Some Other Doc]\nunrelated body\n[END SYSTEM-KNOWHOW]";
    let content = format!("Before\n{unrelated_body}\nAfter");
    let loaded = ["[SYSTEM-KNOWHOW: Nightly Ops]\nfull body\n[END SYSTEM-KNOWHOW]"];
    let out = format_history_content(&content, "user", false, &loaded);
    assert_eq!(
        out, content,
        "non-matching body must pass through unchanged"
    );
}

#[test]
fn handles_multiple_blocks() {
    let body_a = "[SYSTEM-KNOWHOW: Doc A]\nbody A line one\nbody A line two\n[END SYSTEM-KNOWHOW]";
    let body_b = "[SYSTEM-KNOWHOW: Doc B]\nbody B line one\nbody B line two\n[END SYSTEM-KNOWHOW]";
    let content = format!("{body_a}\nmiddle\n{body_b}");
    let out = format_history_content(&content, "user", false, &[body_a]);
    assert!(
        !out.contains("body A line one"),
        "loaded body A must be stripped: {}",
        out
    );
    assert!(
        out.contains("body B line one"),
        "unloaded body B must remain: {}",
        out
    );
    assert!(
        out.contains("(body in [LOADED KNOWHOW] section above)"),
        "loaded pointer must appear: {}",
        out
    );
    assert!(
        out.contains("[SYSTEM-KNOWHOW: Doc B]"),
        "unloaded header must remain: {}",
        out
    );
    assert!(
        out.contains("middle"),
        "interleaved text must survive: {}",
        out
    );
}

#[test]
fn organic_marker_in_assistant_text_survives() {
    let content =
        "I think we want to grep for [SYSTEM-KNOWHOW: Nightly Ops] in the conversation and see what comes up.";
    let loaded = ["[SYSTEM-KNOWHOW: Nightly Ops]\nbody\n[END SYSTEM-KNOWHOW]"];
    let out = format_history_content(content, "assistant", true, &loaded);
    assert_eq!(
        out, content,
        "stray marker that isn't a loaded body substring must be left alone"
    );
}

#[test]
fn trim_history_from_oldest_preserves_newest() {
    let mut history =
        "old message 1\nold message 2\nrecent message 1\nrecent message 2".to_string();
    trim_history_from_oldest(&mut history, 30);
    assert!(
        history.contains("recent message"),
        "should preserve recent messages"
    );
    assert!(
        !history.contains("old message 1"),
        "should have trimmed old message 1"
    );
}

#[test]
fn one_user_image_carries_its_handle_beside_its_position() {
    let refs = format_image_refs(Some(4), &["img-0123456789abcdef".to_string()]);
    assert_eq!(refs, "thread:5, img-0123456789abcdef");
}

#[test]
fn every_image_in_a_message_states_its_own_handle() {
    let handles = vec![
        "img-aaaaaaaaaaaaaaaa".to_string(),
        "img-bbbbbbbbbbbbbbbb".to_string(),
        "img-cccccccccccccccc".to_string(),
    ];
    let refs = format_image_refs(Some(4), &handles);
    assert_eq!(
        refs,
        "thread:5-thread:7, img-aaaaaaaaaaaaaaaa img-bbbbbbbbbbbbbbbb img-cccccccccccccccc"
    );
}

#[test]
fn an_image_the_walk_cannot_place_prints_only_its_handle() {
    let refs = format_image_refs(None, &["img-0123456789abcdef".to_string()]);
    assert_eq!(refs, "img-0123456789abcdef");
}
