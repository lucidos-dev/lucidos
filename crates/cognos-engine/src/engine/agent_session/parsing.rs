/// Extract the first question text + options from CC's `AskUserQuestion` tool input.
/// CC's schema allows multiple questions per call; we only act on the first because
/// the subprocess is killed before any subsequent question could matter. Option
/// ids are synthesized from the array index (CC doesn't supply stable ids) and
/// stay stable across reloads because the question event is persisted intact.
pub(super) fn parse_ask_user_question_input(
    input: &serde_json::Value,
) -> (String, Vec<crate::engine::thread_events::QuestionOption>) {
    let first_q = input
        .get("questions")
        .and_then(|q| q.as_array())
        .and_then(|arr| arr.first());
    let question = first_q
        .and_then(|q| q.get("question"))
        .and_then(|v| v.as_str())
        .unwrap_or("(no question text)")
        .to_string();
    let options: Vec<crate::engine::thread_events::QuestionOption> = first_q
        .and_then(|q| q.get("options"))
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
    (question, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ask_user_question_extracts_question_and_options() {
        let input = serde_json::json!({
            "questions": [{
                "question": "Should I use Postgres or SQLite?",
                "options": [
                    { "label": "Postgres", "description": "Production-grade" },
                    { "label": "SQLite" },
                ],
            }],
        });
        let (question, options) = parse_ask_user_question_input(&input);
        assert_eq!(question, "Should I use Postgres or SQLite?");
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].id, "opt-0");
        assert_eq!(options[0].label, "Postgres");
        assert_eq!(options[0].description.as_deref(), Some("Production-grade"));
        assert_eq!(options[1].id, "opt-1");
        assert_eq!(options[1].label, "SQLite");
        assert!(
            options[1].description.is_none(),
            "missing description must round-trip as None, not empty string"
        );
    }

    #[test]
    fn parse_ask_user_question_handles_missing_questions_array() {
        let input = serde_json::json!({});
        let (question, options) = parse_ask_user_question_input(&input);
        assert_eq!(question, "(no question text)");
        assert!(options.is_empty());
    }

    #[test]
    fn parse_ask_user_question_filters_empty_descriptions() {
        let input = serde_json::json!({
            "questions": [{
                "question": "Pick one",
                "options": [{ "label": "A", "description": "" }],
            }],
        });
        let (_, options) = parse_ask_user_question_input(&input);
        assert!(
            options[0].description.is_none(),
            "empty description string must be normalized to None"
        );
    }
}
