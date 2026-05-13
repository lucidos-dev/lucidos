/// One parsed question from CC's `AskUserQuestion` tool input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedQuestion {
    pub question: String,
    pub options: Vec<crate::engine::thread_events::QuestionOption>,
    pub multi_select: bool,
}

/// Extract every question from CC's `AskUserQuestion` tool input. The schema
/// allows 1–4 questions per call; Lucidos renders them sequentially (one card
/// at a time on screen) and returns a combined answers map back to CC. Option
/// ids are synthesized from the array index because CC doesn't supply stable
/// ids; they stay stable across reloads because the `UserQuestionAsked` event
/// is persisted intact.
pub(crate) fn parse_ask_user_question_inputs(
    input: &serde_json::Value,
) -> Vec<ParsedQuestion> {
    let Some(arr) = input.get("questions").and_then(|q| q.as_array()) else {
        return Vec::new();
    };
    arr.iter().map(parse_one_question).collect()
}

fn parse_one_question(q: &serde_json::Value) -> ParsedQuestion {
    let question = q
        .get("question")
        .and_then(|v| v.as_str())
        .unwrap_or("(no question text)")
        .to_string();
    let options: Vec<crate::engine::thread_events::QuestionOption> = q
        .get("options")
        .and_then(|o| o.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .map(|(i, opt)| {
                    let label = opt
                        .get("label")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string();
                    let description = opt
                        .get("description")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    crate::engine::thread_events::QuestionOption {
                        id: format!("opt-{}", i),
                        label,
                        description,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let multi_select = q
        .get("multiSelect")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    ParsedQuestion {
        question,
        options,
        multi_select,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholder() -> ParsedQuestion {
        ParsedQuestion {
            question: "(no question text)".into(),
            options: Vec::new(),
            multi_select: false,
        }
    }

    #[test]
    fn parse_extracts_question_and_options() {
        let input = serde_json::json!({
            "questions": [{
                "question": "Should I use Postgres or SQLite?",
                "options": [
                    { "label": "Postgres", "description": "Production-grade" },
                    { "label": "SQLite" },
                ],
            }],
        });
        let parsed = parse_ask_user_question_inputs(&input);
        assert_eq!(parsed.len(), 1);
        let q = &parsed[0];
        assert_eq!(q.question, "Should I use Postgres or SQLite?");
        assert_eq!(q.options.len(), 2);
        assert_eq!(q.options[0].id, "opt-0");
        assert_eq!(q.options[0].label, "Postgres");
        assert_eq!(q.options[0].description.as_deref(), Some("Production-grade"));
        assert_eq!(q.options[1].id, "opt-1");
        assert_eq!(q.options[1].label, "SQLite");
        assert!(
            q.options[1].description.is_none(),
            "missing description must round-trip as None, not empty string"
        );
        assert!(!q.multi_select, "absence of multiSelect defaults to false");
    }

    #[test]
    fn parse_returns_empty_for_missing_questions_array() {
        let parsed = parse_ask_user_question_inputs(&serde_json::json!({}));
        assert!(
            parsed.is_empty(),
            "missing questions array yields no parsed questions"
        );
    }

    #[test]
    fn parse_uses_placeholder_text_for_question_without_text() {
        // The handler still needs a renderable question even if CC sent a
        // malformed entry — placeholder keeps the loop honest about the index.
        let input = serde_json::json!({"questions": [{"options": []}]});
        let parsed = parse_ask_user_question_inputs(&input);
        assert_eq!(parsed, vec![placeholder()]);
    }

    #[test]
    fn parse_filters_empty_descriptions() {
        let input = serde_json::json!({
            "questions": [{
                "question": "Pick one",
                "options": [{ "label": "A", "description": "" }],
            }],
        });
        let parsed = parse_ask_user_question_inputs(&input);
        assert!(
            parsed[0].options[0].description.is_none(),
            "empty description string must be normalized to None"
        );
    }

    #[test]
    fn parse_extracts_multi_select() {
        let input_true = serde_json::json!({
            "questions": [{
                "question": "Pick many:",
                "multiSelect": true,
                "options": [{"label": "A"}, {"label": "B"}],
            }],
        });
        assert!(
            parse_ask_user_question_inputs(&input_true)[0].multi_select,
            "multiSelect:true should round-trip"
        );

        let input_false = serde_json::json!({
            "questions": [{"question": "?", "multiSelect": false, "options": []}],
        });
        assert!(!parse_ask_user_question_inputs(&input_false)[0].multi_select);

        let input_missing = serde_json::json!({
            "questions": [{"question": "?", "options": []}],
        });
        assert!(
            !parse_ask_user_question_inputs(&input_missing)[0].multi_select,
            "missing multiSelect defaults to false"
        );
    }

    #[test]
    fn parse_extracts_all_questions_in_order() {
        // The whole point of this refactor: every question in the array must
        // come through, in order, with its own per-question multiSelect flag
        // and option list. Index-based opt-N ids are independent per question.
        let input = serde_json::json!({
            "questions": [
                {
                    "question": "Where should it live?",
                    "multiSelect": false,
                    "options": [
                        {"label": "New page"},
                        {"label": "Existing page"},
                    ],
                },
                {
                    "question": "Pick filters",
                    "multiSelect": true,
                    "options": [
                        {"label": "Date"},
                        {"label": "Product"},
                    ],
                },
                {
                    "question": "Commit strategy?",
                    "options": [{"label": "Squash"}],
                },
            ],
        });
        let parsed = parse_ask_user_question_inputs(&input);
        assert_eq!(parsed.len(), 3, "all three questions must parse");
        assert_eq!(parsed[0].question, "Where should it live?");
        assert!(!parsed[0].multi_select);
        assert_eq!(parsed[0].options.len(), 2);
        assert_eq!(parsed[0].options[0].id, "opt-0");
        assert_eq!(parsed[0].options[0].label, "New page");

        assert_eq!(parsed[1].question, "Pick filters");
        assert!(parsed[1].multi_select, "second question's multiSelect must be honored independently");
        assert_eq!(parsed[1].options.len(), 2);
        // Per-question option ids restart at opt-0 — the answer-resolution
        // layer keys on (question_index, option_id), so this is correct.
        assert_eq!(parsed[1].options[0].id, "opt-0");
        assert_eq!(parsed[1].options[1].id, "opt-1");

        assert_eq!(parsed[2].question, "Commit strategy?");
        assert!(!parsed[2].multi_select, "absent multiSelect on third defaults to false");
        assert_eq!(parsed[2].options.len(), 1);
    }
}
