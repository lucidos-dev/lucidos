//! What the `judge` handler does before and after the network call.
//!
//! Resolving the provider needs a key and a server. Everything either side of
//! that is pinned here: the argument reading, the capture the call leaves
//! behind, and the body the answers are rendered into.

use std::collections::HashMap;

use super::*;
use crate::engine::event_bus::EventBus;
use crate::llm::judgment::{Answer, Answers, ChoiceAnswer, Judgment, JudgmentUsage, NoulAnswer};
use crate::test_support::{aux_captures, setup_test_db, teardown_test_db};
use uuid::Uuid;

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

/// A judgment provider that answers nothing and reports a fixed cost. What the
/// capture tests care about is the usage, which an empty answer set still has.
struct CostlyStub;

#[async_trait::async_trait]
impl JudgmentProvider for CostlyStub {
    async fn ask(
        &self,
        _state: Value,
        _questions: Vec<(String, Question)>,
    ) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Judgment {
            answers: Answers::default(),
            usage: JudgmentUsage {
                input_tokens: 1_204,
                output_tokens: 16,
            },
            model: Some("jev-1.13.0".to_string()),
            request_chars: 890,
        })
    }
}

/// The tool logged its token counts and dropped them, so an agent's own
/// judgments were spend nobody could see.
#[tokio::test]
async fn the_judge_tool_records_what_the_judgment_cost() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let capture = AuxCapture::new(&bus, thread_id, ContextPurpose::JudgeTool);

    ask_and_render(
        &CostlyStub,
        json!({}),
        vec![],
        Duration::from_secs(60),
        Some(&capture),
    )
    .await
    .expect("the stub answers");

    let captures = aux_captures(&pool, thread_id, "judge_tool").await;
    assert_eq!(captures.len(), 1, "one call, one row: {captures:?}");
    assert_eq!(captures[0]["producer"], "auxiliary");
    assert_eq!(captures[0]["usage"]["input_tokens"], 1_204);
    assert_eq!(captures[0]["usage"]["output_tokens"], 16);
    assert_eq!(captures[0]["model"], "jev-1.13.0");
    assert_eq!(captures[0]["sections"][0]["content_chars"], 890);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A call that never answered spent nothing here, because the provider errors
/// before it reports usage. The tool's own failure path stays a failure.
#[tokio::test]
async fn a_failed_judgment_records_nothing_and_reports_the_error() {
    struct Failing;
    #[async_trait::async_trait]
    impl JudgmentProvider for Failing {
        async fn ask(
            &self,
            _state: Value,
            _questions: Vec<(String, Question)>,
        ) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
            Err("TypeSafe returned 429".into())
        }
    }

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let capture = AuxCapture::new(&bus, thread_id, ContextPurpose::JudgeTool);

    let error = ask_and_render(
        &Failing,
        json!({}),
        vec![],
        Duration::from_secs(60),
        Some(&capture),
    )
    .await
    .expect_err("the provider failed");
    assert!(error.contains("429"), "{error}");
    assert!(aux_captures(&pool, thread_id, "judge_tool")
        .await
        .is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}
