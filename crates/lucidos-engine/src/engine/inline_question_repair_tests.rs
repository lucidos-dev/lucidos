use super::*;

#[derive(Debug)]
struct Card {
    question: String,
    labels: Vec<String>,
    multi_select: bool,
}

fn dispatched(text: &str) -> (Vec<Card>, String) {
    match detect_inline_ask_user_question(text) {
        Some(InlineQuestionLeak::Dispatch {
            questions_json,
            cleaned_text,
        }) => {
            let parser_input = serde_json::json!({ "questions": questions_json });
            let cards =
                crate::engine::question_parsing::parse_ask_user_question_inputs(&parser_input)
                    .into_iter()
                    .map(|q| Card {
                        question: q.question,
                        labels: q.options.into_iter().map(|o| o.label).collect(),
                        multi_select: q.multi_select,
                    })
                    .collect();
            (cards, cleaned_text)
        }
        other => panic!("expected Dispatch, got {:?}", other),
    }
}

fn degenerated(text: &str) -> String {
    match detect_inline_ask_user_question(text) {
        Some(InlineQuestionLeak::Degenerate { cleaned_text }) => cleaned_text,
        other => panic!("expected Degenerate, got {:?}", other),
    }
}

#[test]
fn detect_dispatches_the_observed_object_body_leak() {
    let text = r#"Both investigations are done.

<ask_user_question>
{"questions":[{"header":"Next","question":"Both investigations are done. What do you want to do with them?","options":[{"label":"Build the reminder trigger","description":"Notifications and threads, logged entries only, everything the milestone proves"},{"label":"Write the judging knowhow first","description":"The procedure the trigger loads: which views, the day-6 floor, the running comparison"},{"label":"Wire up the outbound webhook now","description":"apis.json entry + handshake script; you handle the token and the approval"},{"label":"Revive the forecast job","description":"Chase the batch-inference job that stopped writing"}]}]}
</ask_user_question>"#;
    let (cards, cleaned) = dispatched(text);
    assert_eq!(cards.len(), 1);
    assert_eq!(
        cards[0].question,
        "Both investigations are done. What do you want to do with them?"
    );
    assert_eq!(
        cards[0].labels,
        vec![
            "Build the reminder trigger",
            "Write the judging knowhow first",
            "Wire up the outbound webhook now",
            "Revive the forecast job",
        ],
        "every option must survive, not just the first"
    );
    assert_eq!(cleaned, "Both investigations are done.");
}

#[test]
fn detect_degrades_the_observed_bare_sentence_leak() {
    let text = "Here is what I found.\n\n<ask_user_question>\nGiven that, which do you want?\n</ask_user_question>";
    let cleaned = degenerated(text);
    assert_eq!(
        cleaned,
        "Here is what I found.\n\nGiven that, which do you want?"
    );
    assert!(!cleaned.contains("<ask_user_question"));
    assert!(!cleaned.contains("</ask_user_question"));
}

#[test]
fn detect_dispatches_a_json_array_body() {
    let text = r#"<ask_user_question>[{"question":"Continue?","options":[{"label":"Yes","description":"go ahead"},{"label":"No"}]}]</ask_user_question>"#;
    let (cards, cleaned) = dispatched(text);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].question, "Continue?");
    assert_eq!(cards[0].labels, vec!["Yes", "No"]);
    assert_eq!(cleaned, "");
}

#[test]
fn detect_degrades_a_malformed_json_body() {
    let text =
        r#"<ask_user_question>{"questions":[{"question":"Continue?","opt</ask_user_question>"#;
    let cleaned = degenerated(text);
    assert!(!cleaned.contains("<ask_user_question"));
    assert!(cleaned.contains(r#""question":"Continue?""#));
}

#[test]
fn detect_degrades_an_empty_questions_array() {
    let text = r#"<ask_user_question>{"questions":[]}</ask_user_question>"#;
    let cleaned = degenerated(text);
    assert!(!cleaned.contains("<ask_user_question"));
}

#[test]
fn detect_degrades_a_question_with_no_question_text() {
    let text = r#"<ask_user_question>{"questions":[{"header":"Next","options":[{"label":"Yes"}]}]}</ask_user_question>"#;
    let cleaned = degenerated(text);
    assert!(!cleaned.contains("<ask_user_question"));
}

#[test]
fn detect_dispatches_an_unterminated_tag_whose_body_is_a_payload() {
    let text = r#"intro <ask_user_question>[{"question":"x","options":[{"label":"a"}]}]"#;
    let (cards, cleaned) = dispatched(text);
    assert_eq!(cards[0].question, "x");
    assert_eq!(cleaned, "intro");
}

#[test]
fn detect_degrades_an_unterminated_tag() {
    let text = "Some prose.\n\n<ask_user_question>\nWhich one?";
    let cleaned = degenerated(text);
    assert_eq!(cleaned, "Some prose.\n\nWhich one?");
}

#[test]
fn detect_dispatches_an_object_body_with_multiple_questions() {
    let text = r#"<ask_user_question>{"questions":[{"question":"First?","options":[{"label":"A"}]},{"question":"Second?","options":[{"label":"B"}],"multiSelect":true}]}</ask_user_question>"#;
    let (cards, _) = dispatched(text);
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[1].question, "Second?");
    assert!(cards[1].multi_select);
}

#[test]
fn detect_strips_tag_and_preserves_preceding_text() {
    let text = "Some prose.\n\n<ask_user_question>[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]</ask_user_question>";
    let (_, cleaned) = dispatched(text);
    assert_eq!(cleaned, "Some prose.");
}

#[test]
fn detect_collapses_double_blank_lines_around_tag() {
    let text = "Paragraph.\n\n<ask_user_question>[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]</ask_user_question>\n\n";
    let (_, cleaned) = dispatched(text);
    assert_eq!(cleaned, "Paragraph.");
}

#[test]
fn detect_preserves_trailing_prose_after_tag() {
    let text = "Before.\n\n<ask_user_question>[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]</ask_user_question>\n\nAfter.";
    let (_, cleaned) = dispatched(text);
    assert_eq!(cleaned, "Before.\n\nAfter.");
}

#[test]
fn detect_returns_none_on_plain_text() {
    assert!(detect_inline_ask_user_question("hello world").is_none());
}

#[test]
fn detect_returns_none_for_a_tag_inside_a_code_fence() {
    let text = r#"Never emit the tag as text, like this:

```
<ask_user_question>
{"questions":[{"question":"Continue?","options":[{"label":"Yes"}]}]}
</ask_user_question>
```

Call the tool instead."#;
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn detect_returns_none_for_a_tag_inside_a_tilde_fence() {
    let text = "Never emit it as text:\n\n~~~\n<ask_user_question>\n{\"questions\":[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]}\n~~~\n\nCall the tool.";
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn detect_returns_none_for_a_tag_in_an_indented_code_block() {
    let text = "Never emit it as text:\n\n    <ask_user_question>\n    {\"questions\":[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]}\n\nCall the tool.";
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn detect_still_finds_a_leak_indented_less_than_a_code_block() {
    let text = "  <ask_user_question>\n{\"questions\":[{\"question\":\"Which?\",\"options\":[{\"label\":\"A\"}]}]}\n</ask_user_question>";
    let (cards, _) = dispatched(text);
    assert_eq!(cards[0].question, "Which?");
}

#[test]
fn detect_returns_none_for_a_tag_inside_inline_backticks() {
    let text = "I'll use the `ask_user_question` tool, never `<ask_user_question>` as text.";
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn detect_still_finds_a_leak_after_a_quoted_tag() {
    let text = r#"The tag is `<ask_user_question>`, which I must never type.

<ask_user_question>
{"questions":[{"question":"Understood?","options":[{"label":"Yes"}]}]}
</ask_user_question>"#;
    let (cards, cleaned) = dispatched(text);
    assert_eq!(cards[0].question, "Understood?");
    assert_eq!(
        cleaned,
        "The tag is `<ask_user_question>`, which I must never type."
    );
}

#[test]
fn detect_returns_none_for_prose_naming_the_tool() {
    assert!(
        detect_inline_ask_user_question("I'll use the ask_user_question tool to ask.").is_none()
    );
}

#[test]
fn detect_dispatches_a_bare_trailing_payload() {
    let text = "Here are the choices.\n\n{\"questions\":[{\"question\":\"Which one?\",\"options\":[{\"label\":\"A\"},{\"label\":\"B\"}]}]}";
    let (cards, cleaned) = dispatched(text);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].question, "Which one?");
    assert_eq!(cleaned, "Here are the choices.");
}

#[test]
fn detect_dispatches_a_bare_payload_that_is_the_whole_response() {
    let text = "{\"questions\":[{\"question\":\"Which one?\",\"options\":[{\"label\":\"A\"}]}]}";
    let (cards, cleaned) = dispatched(text);
    assert_eq!(cards[0].question, "Which one?");
    assert_eq!(cleaned, "");
}

#[test]
fn detect_returns_none_for_a_fenced_trailing_payload() {
    let text = "The arguments look like:\n\n```json\n{\"questions\":[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]}\n```";
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn detect_returns_none_for_a_payload_with_an_extra_top_level_key() {
    let text = "An example response body:\n\n{\"questions\":[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}],\"note\":\"illustrative\"}";
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn detect_returns_none_for_a_payload_followed_by_prose() {
    let text = "The tool takes\n{\"questions\":[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]}\nas its arguments.";
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn detect_returns_none_for_a_trailing_object_that_is_not_a_question_payload() {
    let text = "The config ends up as:\n\n{\"model\":\"opus\",\"effort\":\"high\"}";
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn detect_returns_none_for_a_trailing_payload_with_no_question_text() {
    let text =
        "Choices:\n\n{\"questions\":[{\"header\":\"Next\",\"options\":[{\"label\":\"A\"}]}]}";
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn buffer_check_true_on_partial_open_tag() {
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
    assert!(!buffer_contains_inline_tag(
        "I'll use the ask_user_question tool to ask."
    ));
}

#[test]
fn buffer_check_false_on_a_bare_payload() {
    assert!(!buffer_contains_inline_tag(
        r#"{"questions":[{"question":"x"}]}"#
    ));
}
