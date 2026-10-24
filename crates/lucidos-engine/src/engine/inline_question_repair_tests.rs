use super::*;

/// One question as it would reach the card, flattened so this file does not
/// need the parser's private `ParsedQuestion` type.
#[derive(Debug)]
struct Card {
    question: String,
    labels: Vec<String>,
    multi_select: bool,
}

/// The cards a `Dispatch` outcome would put on screen, plus its cleaned text.
/// Routes through the real parser, so a payload that parses but would not
/// render cannot pass. Panics on any other outcome, so a test naming a dispatch
/// cannot silently pass on a degenerate one.
fn dispatched(text: &str) -> (Vec<Card>, String) {
    match detect_inline_ask_user_question(text) {
        Some(InlineQuestionLeak::Dispatch {
            questions_json,
            cleaned_text,
        }) => {
            let parser_input = serde_json::json!({ "questions": questions_json });
            let cards = crate::engine::agent_session::parse_ask_user_question_inputs(&parser_input)
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

/// The cleaned text of a `Degenerate` outcome. Panics on any other outcome.
fn degenerated(text: &str) -> String {
    match detect_inline_ask_user_question(text) {
        Some(InlineQuestionLeak::Degenerate { cleaned_text }) => cleaned_text,
        other => panic!("expected Degenerate, got {:?}", other),
    }
}

// ----- the two observed leaks, verbatim -----

#[test]
fn detect_dispatches_the_observed_object_body_leak() {
    // Form A of the reported leak. The SHAPE is verbatim, which is the part
    // under test: a single-key `{"questions": [...]}` OBJECT carrying `header`,
    // `question`, and four options with `label` + `description`. That body is
    // the tool-argument schema itself, so the shipped detector's `is_array()`
    // gate dropped it and the user read the raw tag. Nothing was lost except
    // the dispatch.
    //
    // The option text is placeholdered: this file ships to the public mirror
    // and the real payload is a workspace's internal work
    // (`.claude/rules/no-private-data.md`). The verbatim event is quoted in
    // `docs/plans/2026-08-15-inline-question-leak-object-body-and-degenerate-tag.md`,
    // which the release drops.
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
    // Form B, verbatim from the reported event. No JSON, no options, so there
    // is nothing to dispatch. The tag characters must not reach the screen and
    // the sentence must survive: it is the only question the turn asked.
    let text = "Here is what I found.\n\n<ask_user_question>\nGiven that, which do you want?\n</ask_user_question>";
    let cleaned = degenerated(text);
    assert_eq!(
        cleaned,
        "Here is what I found.\n\nGiven that, which do you want?"
    );
    assert!(!cleaned.contains("<ask_user_question"));
    assert!(!cleaned.contains("</ask_user_question"));
}

// ----- tagged bodies -----

#[test]
fn detect_dispatches_a_json_array_body() {
    // The shape the module shipped with. Still recognised.
    let text = r#"<ask_user_question>[{"question":"Continue?","options":[{"label":"Yes","description":"go ahead"},{"label":"No"}]}]</ask_user_question>"#;
    let (cards, cleaned) = dispatched(text);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].question, "Continue?");
    assert_eq!(cards[0].labels, vec!["Yes", "No"]);
    assert_eq!(cleaned, "");
}

#[test]
fn detect_degrades_a_malformed_json_body() {
    // Truncated JSON: parses as neither array nor object.
    let text =
        r#"<ask_user_question>{"questions":[{"question":"Continue?","opt</ask_user_question>"#;
    let cleaned = degenerated(text);
    assert!(!cleaned.contains("<ask_user_question"));
    assert!(cleaned.contains(r#""question":"Continue?""#));
}

#[test]
fn detect_degrades_an_empty_questions_array() {
    // Parses fine, but `walk_question_batch` would walk nothing. Dispatching
    // would leave the user with no card AND no prose.
    let text = r#"<ask_user_question>{"questions":[]}</ask_user_question>"#;
    let cleaned = degenerated(text);
    assert!(!cleaned.contains("<ask_user_question"));
}

#[test]
fn detect_degrades_a_question_with_no_question_text() {
    // The `header` chip-label is never a substitute for `question`, and the
    // walk rejects the batch up front. Degrade rather than synthesise a call
    // that turns into an invisible tool error.
    let text = r#"<ask_user_question>{"questions":[{"header":"Next","options":[{"label":"Yes"}]}]}</ask_user_question>"#;
    let cleaned = degenerated(text);
    assert!(!cleaned.contains("<ask_user_question"));
}

#[test]
fn detect_dispatches_an_unterminated_tag_whose_body_is_a_payload() {
    // The model wrote the payload and forgot the closing tag. The shipped
    // detector required one and returned None, so the whole thing reached the
    // screen. An unterminated body runs to the end of the text, so a payload
    // there still dispatches.
    let text = r#"intro <ask_user_question>[{"question":"x","options":[{"label":"a"}]}]"#;
    let (cards, cleaned) = dispatched(text);
    assert_eq!(cards[0].question, "x");
    assert_eq!(cleaned, "intro");
}

#[test]
fn detect_degrades_an_unterminated_tag() {
    // The model typed the tag and never closed it. Still a leak, so the
    // fragment must not reach the screen.
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

// ----- text placement around a tagged leak -----

#[test]
fn detect_strips_tag_and_preserves_preceding_text() {
    let text = "Some prose.\n\n<ask_user_question>[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]</ask_user_question>";
    let (_, cleaned) = dispatched(text);
    assert_eq!(cleaned, "Some prose.");
}

#[test]
fn detect_collapses_double_blank_lines_around_tag() {
    // Most common live shape: prose preamble, blank line, tag, end of text.
    // Cleaned drops the trailing blank line so the text does not end in a
    // phantom paragraph break.
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

// ----- negatives: legitimate prose must survive untouched -----

#[test]
fn detect_returns_none_on_plain_text() {
    assert!(detect_inline_ask_user_question("hello world").is_none());
}

#[test]
fn detect_returns_none_for_a_tag_inside_a_code_fence() {
    // A model documenting the format is not leaking it. An odd number of
    // fences before the opener means the tag sits inside one.
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
    // Tilde fences are valid CommonMark. Counting only backticks would read
    // this example as a leak and delete it from the answer.
    let text = "Never emit it as text:\n\n~~~\n<ask_user_question>\n{\"questions\":[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]}\n~~~\n\nCall the tool.";
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn detect_returns_none_for_a_tag_in_an_indented_code_block() {
    // Four spaces and nothing else ahead of the tag is an indented block.
    let text = "Never emit it as text:\n\n    <ask_user_question>\n    {\"questions\":[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]}\n\nCall the tool.";
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn detect_still_finds_a_leak_indented_less_than_a_code_block() {
    // Two spaces is not an indented code block, so a tag there is still a leak.
    // The indent guard must not become a blanket "any leading space" escape.
    let text = "  <ask_user_question>\n{\"questions\":[{\"question\":\"Which?\",\"options\":[{\"label\":\"A\"}]}]}\n</ask_user_question>";
    let (cards, _) = dispatched(text);
    assert_eq!(cards[0].question, "Which?");
}

#[test]
fn detect_returns_none_for_a_tag_inside_inline_backticks() {
    // Inline code is a code region too, counted per line rather than per
    // document. Without this the sentence below loses its second half.
    let text = "I'll use the `ask_user_question` tool, never `<ask_user_question>` as text.";
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn detect_still_finds_a_leak_after_a_quoted_tag() {
    // An explainer can quote the tag AND then leak one. Skipping the quoted
    // occurrence must not abandon the scan.
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

// ----- the bare trailing payload, no tag at all -----

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
    // The fence marks it as an illustration. Recovering it would delete the
    // example from an answer explaining the schema, and that deletion is not
    // something the user can undo.
    let text = "The arguments look like:\n\n```json\n{\"questions\":[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}]}\n```";
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn detect_returns_none_for_a_payload_with_an_extra_top_level_key() {
    // Not the tool-argument schema, so not a leak. The single-key rule is what
    // keeps a quoted JSON blob from becoming a card.
    let text = "An example response body:\n\n{\"questions\":[{\"question\":\"x\",\"options\":[{\"label\":\"a\"}]}],\"note\":\"illustrative\"}";
    assert!(detect_inline_ask_user_question(text).is_none());
}

#[test]
fn detect_returns_none_for_a_payload_followed_by_prose() {
    // Mid-prose, so the model was quoting the shape rather than asking.
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
    // Same downstream gate as the tagged form: a batch the walk would reject
    // must not become a synthesised call.
    let text =
        "Choices:\n\n{\"questions\":[{\"header\":\"Next\",\"options\":[{\"label\":\"A\"}]}]}";
    assert!(detect_inline_ask_user_question(text).is_none());
}

// ----- buffer_contains_inline_tag -----

#[test]
fn buffer_check_true_on_partial_open_tag() {
    // Detection must fire BEFORE the closing `>` arrives, so the streaming
    // callback suppresses the body delta before the user sees it.
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
    // The rule mentions the tool name many times. Those mentions must not trip
    // suppression: only the angle-bracket wrapper does.
    assert!(!buffer_contains_inline_tag(
        "I'll use the ask_user_question tool to ask."
    ));
}

#[test]
fn buffer_check_false_on_a_bare_payload() {
    // Deliberately not suppressed. "Alone at the end" is not decidable from a
    // prefix, and the final flush emits the cleaned text anyway.
    assert!(!buffer_contains_inline_tag(
        r#"{"questions":[{"question":"x"}]}"#
    ));
}
