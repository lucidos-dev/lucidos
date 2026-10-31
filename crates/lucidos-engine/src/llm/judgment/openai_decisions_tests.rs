//! The request body and the response parse, both without a server. The
//! fixtures copy the shapes a live call returned.

use super::*;
use crate::llm::judgment::NoulCriteria;

fn two_questions() -> Vec<(String, Question)> {
    vec![
        (
            "read".to_string(),
            Question::Noul {
                instructions: "Should the user read the reply?".to_string(),
                criteria: Some(NoulCriteria {
                    yes: "It holds something new".to_string(),
                    no: "It only confirms".to_string(),
                }),
            },
        ),
        (
            "lane".to_string(),
            Question::Choice {
                instructions: "Which risk lane?".to_string(),
                criteria: vec![
                    ("safe".to_string(), Some("Reads only".to_string())),
                    ("irreversible".to_string(), None),
                ],
            },
        ),
    ]
}

#[test]
fn the_url_is_the_decisions_path_under_the_openai_root() {
    assert_eq!(decisions_url(), "https://api.openai.com/v1/decisions");
}

/// Every question travels in one body, named by the id the caller chose, in
/// the order it was asked.
#[test]
fn every_question_travels_in_one_body_under_its_own_name() {
    let body = build_request_body(OPENAI_DECISIONS_MODEL, &json!({"a": 1}), &two_questions());
    assert_eq!(body["model"], OPENAI_DECISIONS_MODEL);
    let questions = body["questions"].as_array().expect("a questions array");
    assert_eq!(questions.len(), 2);
    assert_eq!(questions[0]["type"], "predicate");
    assert_eq!(questions[0]["name"], "read");
    assert_eq!(questions[1]["type"], "choice");
    assert_eq!(questions[1]["name"], "lane");
}

/// A text state goes as itself, never as a quoted JSON string.
#[test]
fn a_text_state_travels_as_itself() {
    let body = build_request_body(OPENAI_DECISIONS_MODEL, &json!("line one\nline two"), &[]);
    assert_eq!(body["input"], "line one\nline two");
}

/// `input` refuses an object, so the state goes as its JSON text, unchanged.
#[test]
fn the_state_travels_as_its_json_text() {
    let state = json!({ "reply": "psql postgres://u:[REDACTED]@h/db" });
    let body = build_request_body(OPENAI_DECISIONS_MODEL, &state, &two_questions());
    let input = body["input"].as_str().expect("input is text");
    assert_eq!(serde_json::from_str::<Value>(input).unwrap(), state);
}

/// A `predicate` has no criteria field and refuses one, so both sides ride in
/// the instructions.
#[test]
fn a_noul_carries_its_criteria_inside_its_instructions() {
    let body = build_request_body(OPENAI_DECISIONS_MODEL, &json!("x"), &two_questions());
    let predicate = &body["questions"][0];
    assert!(predicate.get("criteria").is_none());
    let instructions = predicate["instructions"].as_str().unwrap();
    assert!(instructions.starts_with("Should the user read the reply?"));
    assert!(instructions.contains("True means: It holds something new"));
    assert!(instructions.contains("False means: It only confirms"));
}

#[test]
fn a_choice_lists_each_option_as_a_value_with_its_description() {
    let body = build_request_body(OPENAI_DECISIONS_MODEL, &json!("x"), &two_questions());
    assert_eq!(
        body["questions"][1]["choices"],
        json!([
            { "value": "safe", "description": "Reads only" },
            { "value": "irreversible" }
        ])
    );
}

#[test]
fn a_response_reads_back_as_answers_usage_and_model() {
    let body = r#"{
        "model": "gpt-6-luna",
        "answers": [
            {"type":"predicate","name":"read","probability":0.83},
            {"type":"choice","name":"lane","choice":"safe",
             "probabilities":[{"value":"safe","probability":0.9},{"value":"irreversible","probability":0.1}],
             "confidence":0.7}
        ],
        "usage": {"input_tokens":264,"input_tokens_details":{"cached_tokens":12,"cache_write_tokens":0},
                  "output_tokens":3,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":267}
    }"#;
    let judgment = parse_response(body, &two_questions()).expect("a readable response");
    assert_eq!(judgment.answers.noul("read"), Some(0.83));
    let lane = judgment.answers.choice("lane").expect("a choice");
    assert_eq!(lane.choice, "safe");
    assert_eq!(lane.probability("irreversible"), 0.1);
    assert_eq!(lane.confidence, 0.7);
    assert_eq!(judgment.usage.map(|u| u.input_tokens), Some(264));
    assert_eq!(judgment.usage.map(|u| u.output_tokens), Some(3));
    assert_eq!(judgment.usage.map(|u| u.cache_read_tokens), Some(12));
    assert_eq!(judgment.model.as_deref(), Some("gpt-6-luna"));
}

/// A refusal or any type this adapter does not know is a missing answer, and
/// its siblings survive. The site then applies its safe default.
#[test]
fn a_refusal_is_a_missing_answer_and_its_siblings_survive() {
    let body = r#"{"answers":[
        {"type":"refusal","name":"read","refusal":"I can't help with that."},
        {"type":"choice","name":"lane","choice":"safe",
         "probabilities":[{"value":"safe","probability":1.0}],"confidence":1.0}
    ]}"#;
    let judgment = parse_response(body, &two_questions()).expect("a readable response");
    assert_eq!(judgment.answers.noul("read"), None);
    assert!(judgment.answers.choice("lane").is_some());
}

#[test]
fn an_out_of_range_probability_is_dropped_never_clamped() {
    let body = r#"{"answers":[{"type":"predicate","name":"read","probability":1.4}]}"#;
    let judgment = parse_response(body, &two_questions()).expect("a readable response");
    assert!(judgment.answers.is_empty());
}

#[test]
fn an_answer_to_a_question_nobody_asked_is_dropped() {
    let body = r#"{"answers":[{"type":"predicate","name":"other","probability":0.9}]}"#;
    let judgment = parse_response(body, &two_questions()).expect("a readable response");
    assert!(judgment.answers.is_empty());
}

#[test]
fn a_response_with_no_answers_array_is_an_error() {
    assert!(parse_response(r#"{"model":"gpt-6-luna"}"#, &two_questions()).is_err());
    assert!(parse_response(r#"{"answers":{}}"#, &two_questions()).is_err());
    assert!(parse_response("not json", &two_questions()).is_err());
}

#[test]
fn absent_usage_reads_as_none() {
    let judgment = parse_response(r#"{"answers":[]}"#, &two_questions()).expect("readable");
    assert_eq!(judgment.usage, None);
    assert_eq!(judgment.model, None);
}
