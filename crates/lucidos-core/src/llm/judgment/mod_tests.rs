//! The wire shape of a question, and the reading of an answer.
//!
//! Every expected body here is copied from the TypeSafe API reference. They are
//! the contract, so a change to one is a change to the integration.

use super::*;

fn choice_question() -> Question {
    Question::Choice {
        instructions: "Which team should handle this?".to_string(),
        criteria: vec![
            (
                "billing".to_string(),
                Some("Payments, invoicing".to_string()),
            ),
            ("technical".to_string(), Some("Bugs, outages".to_string())),
            ("sales".to_string(), None),
        ],
    }
}

#[test]
fn a_noul_serializes_with_its_two_renamed_criteria() {
    let question = Question::Noul {
        instructions: "Does this convey urgency?".to_string(),
        criteria: Some(NoulCriteria {
            yes: "Explicitly time-sensitive".to_string(),
            no: "No urgency expressed".to_string(),
        }),
    };
    let expected = serde_json::json!({
        "type": "noul",
        "instructions": "Does this convey urgency?",
        "criteria": { "true": "Explicitly time-sensitive", "false": "No urgency expressed" },
    });
    assert_eq!(serde_json::to_value(&question).unwrap(), expected);
}

#[test]
fn a_noul_without_criteria_omits_the_key() {
    let question = Question::Noul {
        instructions: "Does this convey urgency?".to_string(),
        criteria: None,
    };
    let value = serde_json::to_value(&question).unwrap();
    assert!(value.get("criteria").is_none(), "{value}");
}

#[test]
fn a_choice_serializes_its_options_as_a_map_in_the_declared_order() {
    let json = serde_json::to_string(&choice_question()).unwrap();
    let expected = r#"{"type":"choice","instructions":"Which team should handle this?","criteria":{"billing":"Payments, invoicing","technical":"Bugs, outages","sales":null}}"#;
    assert_eq!(json, expected);
}

/// The `judge` tool takes its questions from the agent as JSON, so the type
/// that writes the wire body has to read one too. A second struct mirroring
/// this one is the drift this test exists to stop.
#[test]
fn a_question_reads_back_from_the_shape_it_writes() {
    for question in [
        choice_question(),
        Question::Noul {
            instructions: "Does this convey urgency?".to_string(),
            criteria: Some(NoulCriteria {
                yes: "Explicitly time-sensitive".to_string(),
                no: "No urgency expressed".to_string(),
            }),
        },
        Question::Noul {
            instructions: "Is this a question?".to_string(),
            criteria: None,
        },
    ] {
        let json = serde_json::to_string(&question).unwrap();
        let back: Question = serde_json::from_str(&json).unwrap();
        assert_eq!(back, question, "{json}");
    }
}

/// Decoding straight from the wire keeps the author's option order, because a
/// `MapAccess` visits entries in document order. Nothing depends on the order,
/// but a decode that silently sorted would make a captured payload unreadable.
#[test]
fn a_choice_decoded_from_the_wire_keeps_its_declared_order() {
    let json = r#"{"type":"choice","instructions":"Which team?","criteria":{"technical":null,"billing":null,"sales":null}}"#;
    let Question::Choice { criteria, .. } = serde_json::from_str(json).unwrap() else {
        panic!("expected a choice question");
    };
    let names: Vec<&str> = criteria.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["technical", "billing", "sales"]);
}

#[test]
fn each_answer_type_reads_back_from_its_documented_shape() {
    let noul: Answer = serde_json::from_value(serde_json::json!({
        "type": "noul", "noul": 0.92
    }))
    .unwrap();
    assert_eq!(noul, Answer::Noul(NoulAnswer { noul: 0.92 }));

    let choice: Answer = serde_json::from_value(serde_json::json!({
        "type": "choice",
        "choice": "technical",
        "probabilities": { "billing": 0.08, "technical": 0.85, "sales": 0.07 },
        "confidence": 0.82,
    }))
    .unwrap();
    let Answer::Choice(choice) = choice else {
        panic!("expected a choice answer");
    };
    assert_eq!(choice.choice, "technical");
    assert_eq!(choice.confidence, 0.82);
    assert_eq!(choice.probability("technical"), 0.85);
}

/// TypeSafe's third primitive has no variant here, because no call site asks
/// one. An answer of a type the enum cannot read is dropped rather than
/// failing the whole call, which is what keeps that omission harmless.
#[test]
fn a_score_answer_is_simply_unreadable() {
    let score = serde_json::from_value::<Answer>(serde_json::json!({
        "type": "score",
        "score": 1.6,
        "probabilities": { "0": 0.35, "1": 0.65 },
        "confidence": 0.78,
    }));
    assert!(score.is_err());
}

/// The rule the command guard's tie-break rests on. An option the answer never
/// mentioned reads as impossible, so asking for the harmless option's
/// probability can only ever under-state it.
#[test]
fn an_option_the_answer_omits_has_probability_zero() {
    let answer = ChoiceAnswer {
        choice: "irreversible".to_string(),
        probabilities: HashMap::from([("irreversible".to_string(), 1.0)]),
        confidence: 0.99,
    };
    assert_eq!(answer.probability("safe"), 0.0);
    assert_eq!(answer.probability("irreversible"), 1.0);
}

/// What the `judge` tool hands back: the answers keyed by the ids the agent
/// chose, each carrying its whole distribution. Returning only `choice` would
/// throw away the one thing Jev is asked for.
#[test]
fn answers_serialize_as_an_object_keyed_by_question_id() {
    let answers = Answers::new(HashMap::from([
        ("urgent".to_string(), Answer::Noul(NoulAnswer { noul: 0.7 })),
        (
            "team".to_string(),
            Answer::Choice(ChoiceAnswer {
                choice: "billing".to_string(),
                probabilities: HashMap::from([
                    ("billing".to_string(), 0.9),
                    ("sales".to_string(), 0.1),
                ]),
                confidence: 0.88,
            }),
        ),
    ]));
    let value = serde_json::to_value(&answers).unwrap();
    assert_eq!(
        value["urgent"],
        serde_json::json!({"type": "noul", "noul": 0.7})
    );
    assert_eq!(value["team"]["type"], "choice");
    assert_eq!(value["team"]["probabilities"]["billing"], 0.9);
    assert_eq!(value["team"]["confidence"], 0.88);
}

#[test]
fn an_accessor_returns_none_for_a_missing_id_or_the_wrong_type() {
    let answers = Answers::new(HashMap::from([(
        "urgent".to_string(),
        Answer::Noul(NoulAnswer { noul: 0.7 }),
    )]));
    assert_eq!(answers.noul("urgent"), Some(0.7));
    assert_eq!(answers.noul("absent"), None);
    assert!(answers.choice("urgent").is_none(), "a noul is not a choice");
}
