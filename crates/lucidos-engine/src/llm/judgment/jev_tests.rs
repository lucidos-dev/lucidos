//! The request body and the response parse, both without a server.

use super::*;
use crate::llm::judgment::NoulCriteria;

fn two_questions() -> Vec<(String, Question)> {
    vec![
        (
            "needs_memory".to_string(),
            Question::Noul {
                instructions: "Does answering this need the user's long-term memory?".to_string(),
                criteria: None,
            },
        ),
        (
            "lane".to_string(),
            Question::Choice {
                instructions: "Which risk lane?".to_string(),
                criteria: vec![
                    ("safe".to_string(), None),
                    ("irreversible".to_string(), None),
                ],
            },
        ),
    ]
}

/// The one-request invariant, at the wire. Independent questions travel in one
/// body, so the state is sent once and the backend answers them in parallel.
#[test]
fn every_question_travels_in_one_body_under_its_own_id() {
    let body = build_request_body(
        JEV_DEFAULT_MODEL,
        &json!("psql -c 'select 1'"),
        &two_questions(),
    );
    assert_eq!(body["model"], JEV_DEFAULT_MODEL);
    assert_eq!(body["state"], "psql -c 'select 1'");
    let questions = body["questions"].as_object().expect("a questions object");
    assert_eq!(questions.len(), 2);
    assert_eq!(questions["needs_memory"]["type"], "noul");
    assert_eq!(questions["lane"]["type"], "choice");
}

/// The state is passed through verbatim, and nothing here rewrites it. So the
/// redaction a caller applied is the redaction that leaves the machine, and
/// the caller owns that step. The command guard's own test pins it there.
#[test]
fn the_state_reaches_the_body_unchanged() {
    let state = json!({ "command": "psql postgres://u:[REDACTED]@h/db -c 'select 1'" });
    let body = build_request_body(JEV_DEFAULT_MODEL, &state, &two_questions());
    assert_eq!(body["state"], state);
}

#[test]
fn a_structured_state_survives_as_an_object() {
    let state = json!({ "tool": "run_bash", "out_of_workspace": true });
    let body = build_request_body("jev-1.13", &state, &two_questions());
    assert_eq!(body["state"]["out_of_workspace"], true);
    assert_eq!(body["model"], "jev-1.13");
}

#[test]
fn a_noul_criteria_pair_reaches_the_body() {
    let questions = vec![(
        "urgent".to_string(),
        Question::Noul {
            instructions: "Urgent?".to_string(),
            criteria: Some(NoulCriteria {
                yes: "Time-sensitive".to_string(),
                no: "Not".to_string(),
            }),
        },
    )];
    let body = build_request_body(JEV_DEFAULT_MODEL, &json!("hi"), &questions);
    assert_eq!(
        body["questions"]["urgent"]["criteria"]["true"],
        "Time-sensitive"
    );
    assert_eq!(body["questions"]["urgent"]["criteria"]["false"], "Not");
}

#[test]
fn a_response_reads_back_as_answers_and_usage() {
    let body = r#"{
        "model": "jev-1.13",
        "answers": {
            "needs_memory": { "type": "noul", "noul": 0.92 },
            "lane": {
                "type": "choice",
                "choice": "safe",
                "probabilities": { "safe": 0.9, "irreversible": 0.1 },
                "confidence": 0.8
            }
        },
        "usage": { "input_tokens": 312, "output_tokens": 48 }
    }"#;
    let judgment = parse_response(body).expect("a readable response");
    assert_eq!(judgment.answers.noul("needs_memory"), Some(0.92));
    assert_eq!(
        judgment.answers.choice("lane").map(|c| c.choice.as_str()),
        Some("safe")
    );
    assert_eq!(judgment.usage.input_tokens, 312);
    assert_eq!(judgment.usage.output_tokens, 48);
}

/// The request asks for an alias and the response names the version that
/// answered. The caller records the version, so one model keeps one line in a
/// cost rollup instead of opening a second under its alias.
#[test]
fn the_response_names_the_version_that_answered() {
    let body = r#"{"model":"jev-1.13.0","answers":{},"usage":{"input_tokens":5}}"#;
    let judgment = parse_response(body).expect("a readable response");
    assert_eq!(judgment.model.as_deref(), Some("jev-1.13.0"));
    assert_ne!(judgment.model.as_deref(), Some(JEV_DEFAULT_MODEL));
}

/// A response naming no model leaves the caller to fall back to the alias it
/// asked for. Inventing a version here would be a guess about what ran.
#[test]
fn a_response_naming_no_model_reads_as_none() {
    let judgment = parse_response(r#"{"answers":{}}"#).expect("a readable response");
    assert_eq!(judgment.model, None);
}

/// One answer the enum cannot read must not cost the caller the others. The
/// caller sees the unreadable id as absent, which is the case it already
/// handles.
#[test]
fn an_unreadable_answer_is_dropped_and_its_siblings_survive() {
    let body = r#"{
        "answers": {
            "good": { "type": "noul", "noul": 0.4 },
            "future": { "type": "something_new", "value": 3 }
        },
        "usage": { "input_tokens": 1, "output_tokens": 2 }
    }"#;
    let judgment = parse_response(body).expect("a readable response");
    assert_eq!(judgment.answers.len(), 1);
    assert_eq!(judgment.answers.noul("good"), Some(0.4));
    assert_eq!(judgment.answers.noul("future"), None);
}

#[test]
fn a_response_with_no_answers_object_is_an_error() {
    assert!(parse_response(r#"{"model":"jev-1.13"}"#).is_err());
    assert!(parse_response("not json at all").is_err());
}

/// Absent usage is zero rather than a parse failure. The counts are for
/// reporting, so a backend that stops sending them must not break a judgment.
#[test]
fn absent_usage_reads_as_zero() {
    let judgment = parse_response(r#"{"answers":{}}"#).expect("a readable response");
    assert_eq!(judgment.usage, JudgmentUsage::default());
    assert!(judgment.answers.is_empty());
}
