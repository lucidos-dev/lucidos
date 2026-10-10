//! The chat judgment provider, against a stubbed chat model.

use super::*;
use crate::llm::judgment::NoulCriteria;
use crate::test_support::JudgmentChatStub;

fn lane_and_memory() -> Vec<(String, Question)> {
    vec![
        (
            "lane".to_string(),
            Question::Choice {
                instructions: "Which risk lane?".to_string(),
                criteria: vec![
                    ("safe".to_string(), Some("Harmless".to_string())),
                    ("reversible".to_string(), None),
                    ("irreversible".to_string(), None),
                ],
            },
        ),
        (
            "needs_memory".to_string(),
            Question::Noul {
                instructions: "Needs memory?".to_string(),
                criteria: Some(NoulCriteria {
                    yes: "It refers back".to_string(),
                    no: "It does not".to_string(),
                }),
            },
        ),
    ]
}

fn provider(stub: JudgmentChatStub) -> (Arc<JudgmentChatStub>, ChatJudgmentProvider) {
    let stub = Arc::new(stub);
    let provider = ChatJudgmentProvider::new(stub.clone(), Some("none".to_string()));
    (stub, provider)
}

#[tokio::test]
async fn a_tool_answer_reads_back_as_typed_answers() {
    let (_, chat) = provider(JudgmentChatStub::answering(json!({
        "lane": { "safe": 0.1, "reversible": 0.1, "irreversible": 0.8 },
        "needs_memory": 0.6,
    })));
    let judgment = chat
        .ask(
            json!({ "command": "ls" }),
            lane_and_memory(),
            crate::llm::metered::CallToken::for_test(),
        )
        .await
        .expect("answered");
    let lane = judgment.answers.choice("lane").expect("a lane answer");
    assert_eq!(lane.choice, "irreversible");
    assert!((lane.probability("irreversible") - 0.8).abs() < 1e-9);
    assert_eq!(judgment.answers.noul("needs_memory"), Some(0.6));
}

/// Every question, its id, its option names and the state reach the model,
/// and the tool schema requires an answer to each.
#[tokio::test]
async fn the_model_sees_every_question_and_the_schema_pins_the_options() {
    let (stub, chat) = provider(JudgmentChatStub::answering(json!({})));
    chat.ask(
        json!({ "command": "psql -c 'select 1'" }),
        lane_and_memory(),
        crate::llm::metered::CallToken::for_test(),
    )
    .await
    .expect("answered");

    let sent = stub.sent.lock().unwrap();
    let (message, tools) = &sent[0];
    assert!(message.contains("psql -c 'select 1'"), "{message}");
    assert!(message.contains("\"lane\"") && message.contains("\"needs_memory\""));
    assert!(message.contains("Harmless"), "the rubric text rides along");

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, ANSWER_TOOL);
    let schema = &tools[0].parameters;
    assert_eq!(schema["required"], json!(["lane", "needs_memory"]));
    assert_eq!(
        schema["properties"]["lane"]["required"],
        json!(["safe", "reversible", "irreversible"])
    );
    assert_eq!(schema["properties"]["needs_memory"]["type"], "number");
}

/// The distribution is normalised in Rust, so a model answering in percent
/// reads the same as one answering in fractions.
#[test]
fn a_choice_is_normalised_to_sum_to_one() {
    let criteria = vec![("a".to_string(), None), ("b".to_string(), None)];
    let answer = read_choice(&criteria, &json!({ "a": 90, "b": 10 })).expect("readable");
    assert!((answer.probability("a") - 0.9).abs() < 1e-9);
    assert!((answer.probability("b") - 0.1).abs() < 1e-9);
    assert_eq!(answer.choice, "a");
}

/// An option left out reads as zero. A name nobody offered is ignored, so a
/// misspelled option can never carry weight.
#[test]
fn missing_options_read_as_zero_and_invented_ones_are_ignored() {
    let criteria = vec![
        ("safe".to_string(), None),
        ("irreversible".to_string(), None),
    ];
    let answer =
        read_choice(&criteria, &json!({ "irreversible": 0.4, "SAFE": 0.6 })).expect("readable");
    assert_eq!(answer.probability("safe"), 0.0);
    assert_eq!(answer.probability("irreversible"), 1.0);
}

#[test]
fn a_choice_that_breaks_the_schema_is_dropped() {
    let criteria = vec![("a".to_string(), None), ("b".to_string(), None)];
    for value in [
        json!({ "a": -0.1, "b": 1.1 }),
        json!({ "a": "high", "b": 0.1 }),
        json!({ "a": 0, "b": 0 }),
        json!({}),
        json!("a"),
        json!(0.5),
    ] {
        assert!(read_choice(&criteria, &value).is_none(), "{value}");
    }
}

#[test]
fn a_noul_outside_zero_to_one_is_dropped() {
    assert!(read_noul(&json!(1.2)).is_none());
    assert!(read_noul(&json!(-0.01)).is_none());
    assert!(read_noul(&json!("yes")).is_none());
    assert_eq!(read_noul(&json!(0.0)).map(|a| a.noul), Some(0.0));
}

#[test]
fn a_uniform_choice_has_no_confidence_and_a_certain_one_full() {
    let criteria = vec![("a".to_string(), None), ("b".to_string(), None)];
    let uniform = read_choice(&criteria, &json!({ "a": 1, "b": 1 })).expect("readable");
    assert!(uniform.confidence.abs() < 1e-9, "{}", uniform.confidence);
    let certain = read_choice(&criteria, &json!({ "a": 1 })).expect("readable");
    assert!((certain.confidence - 1.0).abs() < 1e-9);
}

/// A model that ignored the tool and wrote the object in text, fences and
/// all, is still read.
#[tokio::test]
async fn an_object_in_text_is_read_when_the_tool_was_ignored() {
    let (_, chat) = provider(JudgmentChatStub::replying(
        "Here you go:\n```json\n{\"needs_memory\": 0.2}\n```",
    ));
    let judgment = chat
        .ask(
            json!({}),
            lane_and_memory(),
            crate::llm::metered::CallToken::for_test(),
        )
        .await
        .expect("answered");
    assert_eq!(judgment.answers.noul("needs_memory"), Some(0.2));
    assert_eq!(judgment.answers.choice("lane"), None);
}

/// Prose with no object is an empty answer set, not an error. The reply was
/// paid for, and each site reads a missing answer as its safe default.
#[tokio::test]
async fn an_unreadable_reply_is_an_empty_answer_set() {
    for reply in ["", "   ", "I think it is probably fine."] {
        let (_, chat) = provider(JudgmentChatStub::replying(reply));
        let judgment = chat
            .ask(
                json!({}),
                lane_and_memory(),
                crate::llm::metered::CallToken::for_test(),
            )
            .await
            .expect("answered");
        assert!(judgment.answers.is_empty(), "{reply:?}");
        assert_eq!(judgment.usage.input_tokens, 210, "the cost still reports");
    }
}

#[tokio::test]
async fn a_failed_call_is_an_error() {
    let (_, chat) = provider(JudgmentChatStub::failing("network down"));
    assert!(chat
        .ask(
            json!({}),
            lane_and_memory(),
            crate::llm::metered::CallToken::for_test()
        )
        .await
        .is_err());
}

/// The capture needs the model, the usage and the size of what was sent.
#[tokio::test]
async fn the_judgment_reports_what_it_cost() {
    let (_, chat) = provider(JudgmentChatStub::answering(json!({ "needs_memory": 1 })));
    let judgment = chat
        .ask(
            json!({ "message": "hi" }),
            lane_and_memory(),
            crate::llm::metered::CallToken::for_test(),
        )
        .await
        .expect("answered");
    assert_eq!(
        judgment.model.as_deref(),
        Some(crate::core::prefs::MODEL_COMMAND_JUDGE.default_text())
    );
    assert_eq!(judgment.usage.input_tokens, 210);
    assert_eq!(judgment.usage.output_tokens, 4);
    assert!(judgment.request_chars > SYSTEM_PROMPT.chars().count());
}

#[tokio::test]
async fn no_questions_make_no_call() {
    let (stub, chat) = provider(JudgmentChatStub::failing("must not be called"));
    let judgment = chat
        .ask(
            json!({}),
            vec![],
            crate::llm::metered::CallToken::for_test(),
        )
        .await
        .expect("nothing to ask");
    assert!(judgment.answers.is_empty());
    assert!(stub.sent.lock().unwrap().is_empty());
}
