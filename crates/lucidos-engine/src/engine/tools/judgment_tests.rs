//! What the `judge` handler does before and after the network call.
//!
//! The call itself needs a key and a server. What is worth pinning is the
//! argument reading, and the body the answers are rendered into.

use std::collections::HashMap;

use super::*;
use crate::llm::judgment::{Answer, Answers, ChoiceAnswer, NoulAnswer};

fn noul(instructions: &str) -> Value {
    json!({ "type": "noul", "instructions": instructions })
}

#[test]
fn a_question_the_agent_wrote_reads_into_the_wire_type() {
    let value = json!({
        "urgent": noul("Does this convey urgency?"),
        "team": {
            "type": "choice",
            "instructions": "Who handles this?",
            "criteria": { "billing": "Payments", "technical": null },
        },
    });
    let questions = parse_questions(Some(&value)).expect("both questions read");
    assert_eq!(questions.len(), 2);
    let ids: Vec<&str> = questions.iter().map(|(id, _)| id.as_str()).collect();
    assert!(ids.contains(&"urgent") && ids.contains(&"team"), "{ids:?}");
}

/// A malformed question names itself, so the agent can fix that one rather
/// than guessing which of fifty it got wrong.
#[test]
fn a_malformed_question_is_refused_by_id() {
    let value = json!({
        "urgent": noul("Does this convey urgency?"),
        "team": { "type": "choice", "instructions": "Who handles this?" },
    });
    let error = parse_questions(Some(&value)).expect_err("a choice needs criteria");
    assert!(error.contains("'team'"), "{error}");
}

#[test]
fn a_missing_or_empty_questions_object_is_refused() {
    assert!(parse_questions(None).is_err());
    assert!(parse_questions(Some(&json!([]))).is_err());
    let empty = parse_questions(Some(&json!({}))).expect_err("nothing to ask");
    assert!(empty.contains("empty"), "{empty}");
}

/// The whole distribution reaches the agent, not just the chosen option. A
/// result carrying only `choice` would throw away the reason for the call.
#[test]
fn the_rendered_body_carries_every_probability_and_the_counts() {
    let answers = Answers::new(HashMap::from([
        (
            "urgent".to_string(),
            Answer::Noul(NoulAnswer { noul: 0.31 }),
        ),
        (
            "team".to_string(),
            Answer::Choice(ChoiceAnswer {
                choice: "billing".to_string(),
                probabilities: HashMap::from([
                    ("billing".to_string(), 0.6),
                    ("technical".to_string(), 0.4),
                ]),
                confidence: 0.55,
            }),
        ),
    ]));
    let body = json!({ "answers": answers, "asked": 3, "answered": answers.len() });

    assert_eq!(body["answers"]["urgent"]["noul"], 0.31);
    assert_eq!(body["answers"]["team"]["probabilities"]["technical"], 0.4);
    assert_eq!(body["answers"]["team"]["confidence"], 0.55);
    // A dropped answer is visible rather than silent: the model can see that
    // one of the three questions it asked came back unreadable.
    assert_eq!(body["asked"], 3);
    assert_eq!(body["answered"], 2);
}
