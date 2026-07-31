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
//! Two helpers, mirroring `inline_question_repair`:
//! - [`detect_inline_tool_call`] — pure function the agentic loop calls AFTER
//!   the LLM returns, ONLY when `response.tool_calls` is empty. If the text
//!   contains a well-formed `<invoke name="...">...</invoke>` block whose name
//!   is a registered tool and which is NOT inside a fenced code block, it
//!   returns the parsed call (name + JSON arguments) plus the cleaned text
//!   (block removed, surrounding whitespace collapsed).
//! - [`buffer_contains_inline_tool_call`] — cheap substring check the streaming
//!   callback runs on every delta so it stops flushing `TextStreamed` /
//!   `CumulativeTextUpdated` once the tag starts forming, hiding the raw XML
//!   from the user's live view.

/// What the detector found: the reconstructed tool call (suitable for pushing
/// into `response.tool_calls` as a synthesized `ToolCall`) and the assistant
/// text with the `<invoke>` block (plus surrounding whitespace) stripped.
#[derive(Debug, Clone)]
pub(crate) struct DetectedToolCall {
    /// The invoked tool name (guaranteed to be one of `known_tool_names`).
    pub name: String,
    /// `<parameter name="K">V</parameter>` children parsed into a JSON object.
    /// Each value is coerced via `serde_json::from_str` with a string fallback,
    /// so `wait_secs` `120` becomes a number and a bare UUID stays a string.
    pub arguments: serde_json::Value,
    /// The original assistant text minus the `<invoke>` block and any
    /// surrounding whitespace that would otherwise dangle.
    pub cleaned_text: String,
}

const OPEN_PREFIX: &str = "<invoke name=\"";
const INVOKE_CLOSE: &str = "</invoke>";

/// Scan `text` for an inline `<invoke name="NAME">...</invoke>` block that
/// leaked a tool call. Returns `Some(DetectedToolCall)` only when ALL hold:
/// - a well-formed `<invoke name="NAME">` opener with a matching `</invoke>`,
/// - `NAME` is one of `known_tool_names` (guards against incidental prose), and
/// - the opener is NOT inside a fenced code block (guards against the model
///   *demonstrating* the format in a ``` example).
///
/// Otherwise returns `None` and the caller falls back to normal handling.
pub(crate) fn detect_inline_tool_call(
    text: &str,
    known_tool_names: &[&str],
) -> Option<DetectedToolCall> {
    let open_at = text.find(OPEN_PREFIX)?;
    // Guard: skip a block that sits inside a fenced code block. An odd number
    // of ``` fences before the opener means we're inside one — an example, not
    // a leak.
    if text[..open_at].matches("```").count() % 2 == 1 {
        return None;
    }
    let name_start = open_at + OPEN_PREFIX.len();
    let name_rel = text[name_start..].find('"')?;
    let name = &text[name_start..name_start + name_rel];
    if !known_tool_names.contains(&name) {
        return None;
    }
    // The opening tag ends at the first `>` after the name's closing quote.
    let after_name = name_start + name_rel + 1;
    let gt_rel = text[after_name..].find('>')?;
    let body_start = after_name + gt_rel + 1;
    let close_rel = text[body_start..].find(INVOKE_CLOSE)?;
    let body_end = body_start + close_rel;
    let close_end = body_end + INVOKE_CLOSE.len();

    let arguments = parse_parameters(&text[body_start..body_end]);

    let before = text[..open_at].trim_end();
    let after = text[close_end..].trim_start();
    let cleaned_text = match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (false, true) => before.to_string(),
        (true, false) => after.to_string(),
        (false, false) => format!("{}\n\n{}", before, after),
    };

    Some(DetectedToolCall {
        name: name.to_string(),
        arguments,
        cleaned_text,
    })
}

/// Parse `<parameter name="K">V</parameter>` children of an `<invoke>` body
/// into a JSON object. Each value `V` is coerced with `serde_json::from_str`
/// (so numbers/bools/JSON parse) and falls back to a JSON string on parse
/// failure (bare identifiers, UUIDs, free text). Unparseable / nameless
/// `<parameter>` tags are skipped. Always returns a JSON object (possibly
/// empty).
fn parse_parameters(body: &str) -> serde_json::Value {
    const PARAM_OPEN: &str = "<parameter name=\"";
    const PARAM_CLOSE: &str = "</parameter>";
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

/// Cheap substring check used by the streaming callback on every token delta.
/// Returns `true` once the buffer contains the opening fragment of an inline
/// `<invoke name=` tag (closing `>` not required, so suppression starts before
/// the body arrives). Once `true`, the callback stops emitting
/// `CumulativeTextUpdated` / `TextStreamed` deltas for the rest of the turn —
/// the post-response repair path takes over.
pub(crate) fn buffer_contains_inline_tool_call(buf: &str) -> bool {
    buf.contains("<invoke name=")
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: &[&str] = &["bash_output", "run_bash", "read_file"];

    // ----- detect_inline_tool_call -----

    #[test]
    fn detect_returns_none_on_plain_text() {
        assert!(detect_inline_tool_call("hello world", KNOWN).is_none());
    }

    #[test]
    fn detect_returns_none_when_no_close_tag() {
        let text = r#"intro <invoke name="bash_output"><parameter name="task_id">x</parameter>"#;
        assert!(detect_inline_tool_call(text, KNOWN).is_none());
    }

    #[test]
    fn detect_returns_none_for_unknown_tool_name() {
        // A well-formed block whose name is NOT a registered tool is incidental
        // prose, not a leak.
        let text = r#"<invoke name="not_a_real_tool"><parameter name="x">1</parameter></invoke>"#;
        assert!(detect_inline_tool_call(text, KNOWN).is_none());
    }

    #[test]
    fn detect_returns_none_when_inside_code_fence() {
        // The model demonstrating the format in a fenced block must NOT trip
        // the repair (it would nuke a legitimate answer).
        let text = "Here's how tool calls look:\n\n```\n<invoke name=\"bash_output\"><parameter name=\"task_id\">x</parameter></invoke>\n```";
        assert!(detect_inline_tool_call(text, KNOWN).is_none());
    }

    #[test]
    fn detect_real_leak_fixture() {
        // Verbatim shape from the observed release-poll leak.
        let text = "court\n<invoke name=\"bash_output\">\n<parameter name=\"task_id\">ac300db8-6165-496d-872d-5da7996c6927</parameter>\n<parameter name=\"wait_secs\">120</parameter>\n</invoke>";
        let d = detect_inline_tool_call(text, KNOWN).expect("should detect");
        assert_eq!(d.name, "bash_output");
        // wait_secs coerces to a JSON number, the UUID stays a string.
        assert_eq!(d.arguments["wait_secs"], serde_json::json!(120));
        assert_eq!(
            d.arguments["task_id"],
            serde_json::json!("ac300db8-6165-496d-872d-5da7996c6927")
        );
        // "court" precedes the block; it's preserved as the cleaned preamble.
        assert_eq!(d.cleaned_text, "court");
    }

    #[test]
    fn detect_strips_block_and_keeps_surrounding_prose() {
        let text = "Before.\n\n<invoke name=\"read_file\"><parameter name=\"path\">a.txt</parameter></invoke>\n\nAfter.";
        let d = detect_inline_tool_call(text, KNOWN).expect("should detect");
        assert_eq!(d.name, "read_file");
        assert_eq!(d.arguments["path"], serde_json::json!("a.txt"));
        assert_eq!(d.cleaned_text, "Before.\n\nAfter.");
    }

    #[test]
    fn detect_empty_cleaned_text_when_block_is_whole_message() {
        let text = r#"<invoke name="bash_output"><parameter name="task_id">t</parameter></invoke>"#;
        let d = detect_inline_tool_call(text, KNOWN).expect("should detect");
        assert_eq!(d.cleaned_text, "");
    }

    #[test]
    fn detect_handles_block_with_no_parameters() {
        let text = r#"<invoke name="read_file"></invoke>"#;
        let d = detect_inline_tool_call(text, KNOWN).expect("should detect");
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

    // ----- buffer_contains_inline_tool_call -----

    #[test]
    fn buffer_check_true_on_partial_open_tag() {
        // Must fire before the closing `>` so the streaming callback suppresses
        // the body before the user sees it.
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
        // Mentioning a tool name in prose must NOT trip suppression — only the
        // angle-bracket wrapper does.
        assert!(!buffer_contains_inline_tool_call(
            "I'll call bash_output to check the task."
        ));
    }
}
