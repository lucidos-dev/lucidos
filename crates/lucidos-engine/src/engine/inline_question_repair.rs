//! Defense against Opus 4.7's recurring habit of emitting the
//! `ask_user_question` tool call as inline `<ask_user_question>...</ask_user_question>`
//! XML text instead of as a structured tool call. The canonical leak was
//! observed in a single thread / engine instance even after the
//! sharpened `ASK_USER_QUESTION_RULE` instruction explicitly told the model
//! "STOP if about to type `<ask_user_question`" — and the model still
//! emitted it.
//!
//! This is a **model-tolerance measure** — tracked in
//! `docs/temporary-measures.md` §2 under the `model-tool-call-as-text`
//! investigation (the generic `<invoke>` form of the same leak lives in
//! [`super::inline_tool_call_repair`]). Remove it when the model stops leaking
//! the tag.
//!
//! Two helpers live here:
//! - [`detect_inline_ask_user_question`] — pure function the agentic loop
//!   calls AFTER the LLM returns its final text. If the text contains a
//!   wrapper tag whose body parses as the JSON-array question schema, the
//!   detector returns the parsed payload + the cleaned text (tag removed,
//!   surrounding whitespace collapsed).
//! - [`buffer_contains_inline_tag`] — cheap substring check the streaming
//!   callback runs on every delta so it can stop flushing `TextStreamed` /
//!   `CumulativeTextUpdated` once the tag starts forming. Mid-stream
//!   suppression hides the raw tag from the user's live view; the
//!   post-response detector still runs and re-routes the question through
//!   `walk_question_batch`.

/// What the detector found. Carries the parsed question array (suitable for
/// wrapping as `{"questions": ...}` and feeding the existing
/// `parse_ask_user_question_inputs` parser) and the text with the wrapper
/// tag (plus the leading whitespace that immediately precedes it) stripped.
#[derive(Debug, Clone)]
pub(crate) struct DetectedQuestion {
    /// The body parsed from inside `<ask_user_question>...</ask_user_question>`.
    /// Always a JSON array of question objects.
    pub questions_json: serde_json::Value,
    /// The original assistant text minus the wrapper tag and any
    /// surrounding whitespace that would otherwise dangle.
    pub cleaned_text: String,
}

/// Scan `text` for an inline `<ask_user_question>...</ask_user_question>`
/// wrapper. Returns `Some(DetectedQuestion)` when the wrapper is present and
/// its body parses as a JSON array of question objects, `None` otherwise.
///
/// Recognises only the JSON-array body form
/// (`<ask_user_question>[{"question":..., "options":[...]}]</ask_user_question>`)
/// — the variant Opus 4.7 has consistently emitted across the observed
/// leaks. The earliest leak used a markdown-bullet body inside the
/// tag; that variant is left untouched here (the agentic loop logs the
/// detection miss so monitoring still surfaces it).
///
/// The match is anchored on the literal opening tag `<ask_user_question>`
/// and the closing `</ask_user_question>`. Whitespace either side of the
/// body is tolerated.
pub(crate) fn detect_inline_ask_user_question(text: &str) -> Option<DetectedQuestion> {
    const OPEN: &str = "<ask_user_question>";
    const CLOSE: &str = "</ask_user_question>";
    let open_at = text.find(OPEN)?;
    let body_start = open_at + OPEN.len();
    let close_rel = text[body_start..].find(CLOSE)?;
    let body_end = body_start + close_rel;
    let close_end = body_end + CLOSE.len();
    let body = text[body_start..body_end].trim();
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    if !parsed.is_array() {
        return None;
    }
    let before = text[..open_at].trim_end();
    let after = text[close_end..].trim_start();
    let cleaned_text = match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (false, true) => before.to_string(),
        (true, false) => after.to_string(),
        (false, false) => format!("{}\n\n{}", before, after),
    };
    Some(DetectedQuestion {
        questions_json: parsed,
        cleaned_text,
    })
}

/// Cheap substring check used by the streaming callback on every token
/// delta. Returns `true` once the buffer contains the opening fragment of
/// an inline `<ask_user_question` tag (closing `>` not required, so we
/// catch the suppression window before the body even arrives). Once this
/// returns `true`, the callback stops emitting `CumulativeTextUpdated` and
/// `TextStreamed` deltas for the remainder of the LLM turn — the
/// post-response repair path takes over.
pub(crate) fn buffer_contains_inline_tag(buf: &str) -> bool {
    buf.contains("<ask_user_question")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- detect_inline_ask_user_question -----

    #[test]
    fn detect_returns_none_on_plain_text() {
        assert!(detect_inline_ask_user_question("hello world").is_none());
    }

    #[test]
    fn detect_returns_none_when_only_open_tag() {
        let text = r#"intro <ask_user_question>[{"question":"x","options":[{"label":"a"}]}]"#;
        assert!(detect_inline_ask_user_question(text).is_none());
    }

    #[test]
    fn detect_returns_none_when_body_not_json() {
        let text = "<ask_user_question>not json</ask_user_question>";
        assert!(detect_inline_ask_user_question(text).is_none());
    }

    #[test]
    fn detect_returns_none_when_body_not_array() {
        // JSON object (single question without array wrapper) — out of
        // scope. The schema the parser consumes is `{"questions": [...]}`
        // wrapping an ARRAY. A bare object falls through.
        let text =
            r#"<ask_user_question>{"question":"x","options":[{"label":"a"}]}</ask_user_question>"#;
        assert!(detect_inline_ask_user_question(text).is_none());
    }

    #[test]
    fn detect_parses_json_array_body() {
        let text = r#"<ask_user_question>[{"question":"Continue?","options":[{"label":"Yes"},{"label":"No"}]}]</ask_user_question>"#;
        let d = detect_inline_ask_user_question(text).expect("should detect");
        assert!(d.questions_json.is_array());
        assert_eq!(d.questions_json.as_array().unwrap().len(), 1);
        assert_eq!(d.cleaned_text, "");
    }

    #[test]
    fn detect_feeds_existing_parser_via_questions_wrap() {
        // The whole point of returning `questions_json` is that the
        // existing CC parser path accepts it once wrapped as
        // `{"questions": <array>}`. This test pins that compatibility.
        let text = r#"<ask_user_question>[{"question":"Continue?","options":[{"label":"Yes","description":"go ahead"},{"label":"No"}]}]</ask_user_question>"#;
        let d = detect_inline_ask_user_question(text).expect("should detect");
        let parser_input = serde_json::json!({ "questions": d.questions_json });
        let parsed = crate::engine::agent_session::parse_ask_user_question_inputs(&parser_input);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].question, "Continue?");
        assert_eq!(parsed[0].options.len(), 2);
        assert_eq!(parsed[0].options[0].label, "Yes");
    }

    #[test]
    fn detect_strips_tag_and_preserves_preceding_text() {
        let text = "Some prose.\n\n<ask_user_question>[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]</ask_user_question>";
        let d = detect_inline_ask_user_question(text).expect("should detect");
        assert_eq!(d.cleaned_text, "Some prose.");
    }

    #[test]
    fn detect_collapses_double_blank_lines_around_tag() {
        // Most common live shape: prose preamble, blank line, tag, end of
        // text. Cleaned should drop the trailing blank line so the
        // assistant text doesn't end in a phantom paragraph break.
        let text = "Paragraph.\n\n<ask_user_question>[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]</ask_user_question>\n\n";
        let d = detect_inline_ask_user_question(text).expect("should detect");
        assert_eq!(d.cleaned_text, "Paragraph.");
    }

    #[test]
    fn detect_preserves_trailing_prose_after_tag() {
        let text = "Before.\n\n<ask_user_question>[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]</ask_user_question>\n\nAfter.";
        let d = detect_inline_ask_user_question(text).expect("should detect");
        assert_eq!(d.cleaned_text, "Before.\n\nAfter.");
    }

    #[test]
    fn detect_real_leak_fixture() {
        // Verbatim from a real observed leak event, edited only to shorten
        // the descriptions to fit a Rust string literal.
        let text = r#"#1 fixes today's instance. #2 fixes future drift. They're independent.

<ask_user_question>
[{"question":"How do you want to close the bug-filing gap?","options":[{"label":"Both — file now, strengthen prompt","description":"manual emit gets CC moving on the executable-bar guard immediately"},{"label":"File the bug now","description":"Just kick the fix workflow with what we know"},{"label":"Just strengthen the prompt","description":"Trust the next codex session to file it correctly"},{"label":"Look at the scratch space first","description":"Read the last few session logs to extract the actual repro"}]}]
</ask_user_question>"#;
        let d = detect_inline_ask_user_question(text).expect("should detect");
        assert!(d.cleaned_text.starts_with("#1 fixes today's instance."));
        assert!(!d.cleaned_text.contains("<ask_user_question"));
        let parser_input = serde_json::json!({ "questions": d.questions_json });
        let parsed = crate::engine::agent_session::parse_ask_user_question_inputs(&parser_input);
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].question.starts_with("How do you want"));
        assert_eq!(parsed[0].options.len(), 4);
    }

    // ----- buffer_contains_inline_tag -----

    #[test]
    fn buffer_check_true_on_partial_open_tag() {
        // Detection must fire BEFORE the closing `>` arrives so the
        // streaming callback suppresses the tag-body delta before the
        // user sees it. The fragment "<ask_user_question" is enough.
        assert!(buffer_contains_inline_tag("hello <ask_user_question"));
    }

    #[test]
    fn buffer_check_true_on_full_open_tag() {
        assert!(buffer_contains_inline_tag("hello <ask_user_question>"));
    }

    #[test]
    fn buffer_check_true_on_full_wrapped_block() {
        assert!(buffer_contains_inline_tag(
            "hi <ask_user_question>body</ask_user_question> bye"
        ));
    }

    #[test]
    fn buffer_check_false_on_plain_text() {
        assert!(!buffer_contains_inline_tag("hello world"));
    }

    #[test]
    fn buffer_check_false_on_bare_tool_name_in_prose() {
        // The rule mentions the tool name `ask_user_question` lots of
        // times. Those mentions must NOT trip suppression — only the
        // angle-bracket wrapper does.
        assert!(!buffer_contains_inline_tag(
            "I'll use the ask_user_question tool to ask."
        ));
    }
}
