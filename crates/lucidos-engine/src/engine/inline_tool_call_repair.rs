//! Defense against the chat model emitting a tool call as inline
//! `<invoke name="TOOL"><parameter name="K">V</parameter>...</invoke>` XML
//! *text* instead of a structured `tool_use` block. This is Claude's
//! training-time tool-call serialization leaking into the content channel —
//! the same failure class as [`super::inline_question_repair`], a different
//! tag. The canonical leak was observed in a release-polling thread where the
//! model wrote `bash_output` as text mid-loop, so the turn terminated with the
//! raw XML (including a stray task-id UUID) as its `ResponseGenerated` instead
//! of resuming the poll.
//!
//! This is a **model-tolerance measure** — tracked in
//! `docs/temporary-measures.md` §2 under the `model-tool-call-as-text`
//! investigation. Remove it when the model stops leaking `<invoke>` text.
//!
//! Two leak shapes are recognised:
//! - **Named**: a whole `<invoke name="NAME">...</invoke>` block.
//! - **Headless**: `<parameter>` children and a closing `</invoke>`, with no
//!   opener. The model lost track of where its prose ended: it closed a
//!   `<copy>` block with `</parameter>`, then wrote the rest of an
//!   `ask_user_question` call as text. The tool is inferred from the schemas.
//!
//! Two helpers, mirroring `inline_question_repair`:
//! - [`detect_inline_tool_call`] — pure function the agentic loop calls AFTER
//!   the LLM returns, ONLY when `response.tool_calls` is empty. For a leak
//!   outside a fenced code block that resolves to a registered tool, it
//!   returns the parsed call (name + JSON arguments) plus the cleaned text
//!   (block removed, surrounding whitespace collapsed).
//! - [`buffer_contains_inline_tool_call`] — cheap substring check the streaming
//!   callback runs on every delta so it stops flushing `TextStreamed` /
//!   `CumulativeTextUpdated` once the tag starts forming, hiding the raw XML
//!   from the user's live view.

use crate::llm::provider::ToolDefinition;

#[derive(Debug, Clone)]
pub struct DetectedToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
    pub cleaned_text: String,
}

const OPEN_PREFIX: &str = "<invoke name=\"";
const INVOKE_CLOSE: &str = "</invoke>";
const PARAM_OPEN: &str = "<parameter name=\"";
const PARAM_CLOSE: &str = "</parameter>";
const CALLS_OPEN: &str = "<function_calls>";
const CALLS_CLOSE: &str = "</function_calls>";

pub fn detect_inline_tool_call(text: &str, tools: &[ToolDefinition]) -> Option<DetectedToolCall> {
    text.match_indices(INVOKE_CLOSE)
        .map(|(close_at, _)| close_at)
        .filter(|close_at| !inside_fence(text, *close_at))
        .find_map(|close_at| detect_block(text, close_at, tools))
}

fn detect_block(text: &str, close_at: usize, tools: &[ToolDefinition]) -> Option<DetectedToolCall> {
    let block_start = text[..close_at]
        .rfind(INVOKE_CLOSE)
        .map_or(0, |at| at + INVOKE_CLOSE.len());
    match text[block_start..close_at].rfind(OPEN_PREFIX) {
        Some(rel) => detect_named(text, block_start + rel, close_at, tools),
        None => detect_headless(text, block_start, close_at, tools),
    }
}

fn detect_named(
    text: &str,
    open_at: usize,
    close_at: usize,
    tools: &[ToolDefinition],
) -> Option<DetectedToolCall> {
    if inside_fence(text, open_at) {
        return None;
    }
    let name_start = open_at + OPEN_PREFIX.len();
    let name_rel = text[name_start..close_at].find('"')?;
    let name = &text[name_start..name_start + name_rel];
    if !tools.iter().any(|t| t.name == name) {
        return None;
    }
    let after_name = name_start + name_rel + 1;
    let gt_rel = text[after_name..close_at].find('>')?;
    let body_start = after_name + gt_rel + 1;

    Some(DetectedToolCall {
        name: name.to_string(),
        arguments: parse_parameters(&text[body_start..close_at]),
        cleaned_text: cleaned_text(text, open_at, close_at, false),
    })
}

fn detect_headless(
    text: &str,
    block_start: usize,
    close_at: usize,
    tools: &[ToolDefinition],
) -> Option<DetectedToolCall> {
    let block = &text[block_start..close_at];
    let leak_at = block_start
        + [PARAM_OPEN, PARAM_CLOSE]
            .iter()
            .filter_map(|tag| block.find(tag))
            .min()?;
    if inside_fence(text, leak_at) {
        return None;
    }
    let arguments = parse_parameters(&text[leak_at..close_at]);
    let keys: Vec<&str> = arguments.as_object()?.keys().map(String::as_str).collect();
    if keys.is_empty() {
        return None;
    }
    let mut fitting = tools.iter().filter(|t| schema_fits(&t.parameters, &keys));
    let tool = fitting.next()?;
    if fitting.next().is_some() {
        return None;
    }
    let orphan_close = text[leak_at..].starts_with(PARAM_CLOSE);
    Some(DetectedToolCall {
        name: tool.name.clone(),
        arguments,
        cleaned_text: cleaned_text(text, leak_at, close_at, orphan_close),
    })
}

fn inside_fence(text: &str, at: usize) -> bool {
    text[..at].matches("```").count() % 2 == 1
}

fn schema_fits(schema: &serde_json::Value, keys: &[&str]) -> bool {
    let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) else {
        return false;
    };
    let mut required = schema
        .get("required")
        .and_then(|r| r.as_array())
        .into_iter()
        .flatten()
        .filter_map(|r| r.as_str());
    keys.iter().all(|k| properties.contains_key(*k)) && required.all(|r| keys.contains(&r))
}

fn cleaned_text(text: &str, leak_at: usize, close_at: usize, close_copy: bool) -> String {
    let mut before = text[..leak_at].trim_end();
    before = before.strip_suffix(CALLS_OPEN).unwrap_or(before).trim_end();
    let mut after = text[close_at + INVOKE_CLOSE.len()..].trim_start();
    after = after
        .strip_prefix(CALLS_CLOSE)
        .unwrap_or(after)
        .trim_start();

    let mut before = before.to_string();
    if close_copy && before.rfind("<copy>") > before.rfind("</copy>") {
        before.push_str("</copy>");
    }
    [before.as_str(), after]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn parse_parameters(body: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    let mut rest = body;
    while let Some(open_at) = rest.find(PARAM_OPEN) {
        let name_start = open_at + PARAM_OPEN.len();
        let Some(name_rel) = rest[name_start..].find('"') else {
            break;
        };
        let key = rest[name_start..name_start + name_rel].to_string();
        let after_name = name_start + name_rel + 1;
        let Some(gt_rel) = rest[after_name..].find('>') else {
            break;
        };
        let val_start = after_name + gt_rel + 1;
        let Some(close_rel) = rest[val_start..].find(PARAM_CLOSE) else {
            break;
        };
        let raw_value = rest[val_start..val_start + close_rel].trim();
        let value = serde_json::from_str::<serde_json::Value>(raw_value)
            .unwrap_or_else(|_| serde_json::Value::String(raw_value.to_string()));
        if !key.is_empty() {
            map.insert(key, value);
        }
        rest = &rest[val_start + close_rel + PARAM_CLOSE.len()..];
    }
    serde_json::Value::Object(map)
}

/// INVARIANT: Cheap substring check used by the streaming callback on every token delta.
/// Returns `true` once the buffer contains, outside a code fence, the opening
/// fragment of an inline `<invoke name=` tag or a parameter tag. The
/// closing `>` is not required, so suppression starts before the body arrives.
/// Once `true`, the callback stops emitting `CumulativeTextUpdated` /
/// `TextStreamed` deltas for the rest of the turn, and the post-response
/// repair path takes over.
pub fn buffer_contains_inline_tool_call(buf: &str) -> bool {
    ["<invoke name=", "<parameter name=", PARAM_CLOSE]
        .iter()
        .flat_map(|tag| buf.match_indices(tag))
        .any(|(at, _)| !inside_fence(buf, at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::tools::{get_default_tools, ToolCapabilities};
    use std::sync::LazyLock;

    static KNOWN: LazyLock<Vec<ToolDefinition>> =
        LazyLock::new(|| get_default_tools(&ToolCapabilities::all_open()));

    const HEADLESS_LEAK: &str = "Paste this:\n\n<copy>App name: Demo\nRedirect URI: https://example.com/cb</parameter>\n<parameter name=\"questions\">[{\"question\": \"How do you want to handle sign-in?\", \"options\": [{\"label\": \"File and wait\", \"description\": \"Leave it.\"}, {\"label\": \"Keys meanwhile\", \"description\": \"Test now.\"}]}]</parameter>\n</invoke>\n\n</function_calls>";

    #[test]
    fn detect_headless_leak_infers_the_question_tool() {
        let d = detect_inline_tool_call(HEADLESS_LEAK, &KNOWN).expect("should detect");
        assert_eq!(d.name, crate::llm::tool_names::ASK_USER_QUESTION);
        assert_eq!(
            d.arguments["questions"][0]["question"],
            serde_json::json!("How do you want to handle sign-in?")
        );
        assert_eq!(
            d.cleaned_text,
            "Paste this:\n\n<copy>App name: Demo\nRedirect URI: https://example.com/cb</copy>"
        );
    }

    #[test]
    fn detect_headless_returns_none_when_no_tool_fits() {
        let text = "Prose.\n<parameter name=\"no_such_param\">1</parameter>\n</invoke>";
        assert!(detect_inline_tool_call(text, &KNOWN).is_none());
    }

    #[test]
    fn detect_headless_returns_none_when_a_required_key_is_missing() {
        let text = "<parameter name=\"message\">hi</parameter>\n</invoke>";
        assert!(detect_inline_tool_call(text, &KNOWN).is_none());
    }

    #[test]
    fn detect_headless_returns_none_inside_code_fence() {
        let text = format!("Example:\n\n```\n{HEADLESS_LEAK}\n```");
        assert!(detect_inline_tool_call(&text, &KNOWN).is_none());
    }

    #[test]
    fn detect_strips_function_calls_wrapper_around_named_block() {
        let text = "Before.\n<function_calls>\n<invoke name=\"read_file\"><parameter name=\"path\">a.txt</parameter></invoke>\n</function_calls>";
        let d = detect_inline_tool_call(text, &KNOWN).expect("should detect");
        assert_eq!(d.cleaned_text, "Before.");
    }

    #[test]
    fn detect_headless_after_a_fenced_invoke_example() {
        let text = format!(
            "Calls look like:\n\n```\n<invoke name=\"read_file\"><parameter name=\"path\">a</parameter></invoke>\n```\n\n{HEADLESS_LEAK}"
        );
        let d = detect_inline_tool_call(&text, &KNOWN).expect("should detect");
        assert_eq!(d.name, crate::llm::tool_names::ASK_USER_QUESTION);
        assert!(d.cleaned_text.starts_with("Calls look like:"));
    }

    #[test]
    fn named_leak_leaves_an_inline_copy_mention_alone() {
        let text = "Wrap commands in `<copy>` tags.\n<invoke name=\"read_file\"><parameter name=\"path\">a</parameter></invoke>";
        let d = detect_inline_tool_call(text, &KNOWN).expect("should detect");
        assert_eq!(d.cleaned_text, "Wrap commands in `<copy>` tags.");
    }

    #[test]
    fn buffer_check_false_on_parameter_tag_inside_code_fence() {
        assert!(!buffer_contains_inline_tool_call(
            "A report:\n\n```xml\n<parameter name=\"ReportTitle\"/>\n</parameter>"
        ));
    }

    #[test]
    fn buffer_check_true_on_orphan_parameter_close() {
        assert!(buffer_contains_inline_tool_call(
            "<copy>App name: Demo</parameter>"
        ));
    }

    #[test]
    fn detect_returns_none_on_plain_text() {
        assert!(detect_inline_tool_call("hello world", &KNOWN).is_none());
    }

    #[test]
    fn detect_returns_none_when_no_close_tag() {
        let text = r#"intro <invoke name="bash_output"><parameter name="task_id">x</parameter>"#;
        assert!(detect_inline_tool_call(text, &KNOWN).is_none());
    }

    #[test]
    fn detect_returns_none_for_unknown_tool_name() {
        let text = r#"<invoke name="not_a_real_tool"><parameter name="x">1</parameter></invoke>"#;
        assert!(detect_inline_tool_call(text, &KNOWN).is_none());
    }

    #[test]
    fn detect_returns_none_when_inside_code_fence() {
        let text = "Here's how tool calls look:\n\n```\n<invoke name=\"bash_output\"><parameter name=\"task_id\">x</parameter></invoke>\n```";
        assert!(detect_inline_tool_call(text, &KNOWN).is_none());
    }

    #[test]
    fn detect_real_leak_fixture() {
        let text = "court\n<invoke name=\"bash_output\">\n<parameter name=\"task_id\">ac300db8-6165-496d-872d-5da7996c6927</parameter>\n<parameter name=\"wait_secs\">120</parameter>\n</invoke>";
        let d = detect_inline_tool_call(text, &KNOWN).expect("should detect");
        assert_eq!(d.name, "bash_output");
        assert_eq!(d.arguments["wait_secs"], serde_json::json!(120));
        assert_eq!(
            d.arguments["task_id"],
            serde_json::json!("ac300db8-6165-496d-872d-5da7996c6927")
        );
        assert_eq!(d.cleaned_text, "court");
    }

    #[test]
    fn detect_strips_block_and_keeps_surrounding_prose() {
        let text = "Before.\n\n<invoke name=\"read_file\"><parameter name=\"path\">a.txt</parameter></invoke>\n\nAfter.";
        let d = detect_inline_tool_call(text, &KNOWN).expect("should detect");
        assert_eq!(d.name, "read_file");
        assert_eq!(d.arguments["path"], serde_json::json!("a.txt"));
        assert_eq!(d.cleaned_text, "Before.\n\nAfter.");
    }

    #[test]
    fn detect_empty_cleaned_text_when_block_is_whole_message() {
        let text = r#"<invoke name="bash_output"><parameter name="task_id">t</parameter></invoke>"#;
        let d = detect_inline_tool_call(text, &KNOWN).expect("should detect");
        assert_eq!(d.cleaned_text, "");
    }

    #[test]
    fn detect_handles_block_with_no_parameters() {
        let text = r#"<invoke name="read_file"></invoke>"#;
        let d = detect_inline_tool_call(text, &KNOWN).expect("should detect");
        assert_eq!(d.name, "read_file");
        assert_eq!(d.arguments, serde_json::json!({}));
    }

    #[test]
    fn parse_parameters_coerces_types() {
        let body = r#"<parameter name="n">42</parameter><parameter name="b">true</parameter><parameter name="s">hello world</parameter>"#;
        let v = parse_parameters(body);
        assert_eq!(v["n"], serde_json::json!(42));
        assert_eq!(v["b"], serde_json::json!(true));
        assert_eq!(v["s"], serde_json::json!("hello world"));
    }

    #[test]
    fn buffer_check_true_on_partial_open_tag() {
        assert!(buffer_contains_inline_tool_call("court\n<invoke name="));
    }

    #[test]
    fn buffer_check_true_on_full_block() {
        assert!(buffer_contains_inline_tool_call(
            r#"<invoke name="bash_output"><parameter name="x">1</parameter></invoke>"#
        ));
    }

    #[test]
    fn buffer_check_false_on_plain_text() {
        assert!(!buffer_contains_inline_tool_call("hello world"));
    }

    #[test]
    fn buffer_check_false_on_bare_tool_name_in_prose() {
        assert!(!buffer_contains_inline_tool_call(
            "I'll call bash_output to check the task."
        ));
    }
}
