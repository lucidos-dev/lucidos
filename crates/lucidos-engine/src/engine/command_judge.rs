//! Command-guard *judge* (ADR 0002, Phase 3): the fallible half of the hybrid
//! classifier.
//!
//! The static [`static_classify`](crate::engine::command_guard::static_classify)
//! fast-path settles the catastrophic deny-list and an obviously-safe allowlist.
//! Everything else is the *ambiguous middle* and lands here. The judge asks two
//! typed questions ([`command_judge_questions`]) of whichever judgment provider
//! the user picked: the chat model `model_command_judge` names by default, or a
//! System One row (ADR 0363).
//!
//! **The fail-safe is a Rust threshold** (I13 of
//! `docs/plans/2026-10-04-tree-memory-module-and-the-home-thread.md`). A
//! missing, malformed or unsure answer resolves to `IrreversibleDanger`, on
//! every provider. Only a call that never answered is an `Err`, and the caller
//! then falls back to the static list
//! ([`fallback_classify`](crate::engine::command_guard::fallback_classify)).

use std::time::Duration;

use crate::engine::command_guard::{JudgeInput, RiskLane, SideEffectCategory};
use crate::engine::command_judge_questions;
use crate::engine::{AuxCapture, ContextPurpose, LucidosEngine};
use crate::llm::judgment::{
    for_site, system_one_for, ChatJudgmentProvider, JudgmentProvider, JudgmentSite,
};

/// The judge's classification of one ambiguous command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeVerdict {
    pub lane: RiskLane,
    /// The side-effect category: `Some` only when `lane == IrreversibleDanger`
    /// (gates the trigger side-effect grant), `None` for safe/reversible.
    pub category: Option<SideEffectCategory>,
    /// One-line card text shown to the user when the lane is `IrreversibleDanger`.
    pub summary: String,
    /// The distribution behind the verdict, logged and not shown to the user.
    pub reason: String,
}

impl JudgeVerdict {
    /// The verdict for an answer the guard cannot read: err toward *ask*, the
    /// design's tie-break. An unclassifiable irreversible command is
    /// [`SideEffectCategory::Other`], so an unattended trigger blocks it
    /// unless it granted `other`.
    pub(crate) fn uncertain() -> Self {
        Self {
            lane: RiskLane::IrreversibleDanger,
            category: Some(SideEffectCategory::Other),
            summary: "Could not classify this command; treating it as a possible irreversible side-effect.".to_string(),
            reason: "the judge gave no readable lane".to_string(),
        }
    }
}

/// Run one judge classification as two typed Choice questions, on any
/// judgment provider.
///
/// Split out of [`LucidosEngine::judge_command`] so a stubbed provider
/// exercises it offline. [`command_judge_questions::read`] turns the answers
/// into a verdict, resolving a weak or missing distribution to *ask*.
///
/// A timeout surfaces as a boxed [`tokio::time::error::Elapsed`]. `deadline`
/// bounds the ask alone, so an answer that arrived in time is never lost to
/// its record.
pub(crate) async fn judge_with<J: JudgmentProvider + ?Sized>(
    provider: &J,
    input: &JudgeInput,
    deadline: Duration,
    capture: &AuxCapture,
) -> Result<JudgeVerdict, Box<dyn std::error::Error + Send + Sync>> {
    let judgment = capture
        .until(tokio::time::Instant::now() + deadline)
        .judge(
            provider,
            command_judge_questions::state(input),
            command_judge_questions::questions(),
        )
        .await?;
    Ok(command_judge_questions::read(&judgment.answers))
}

impl LucidosEngine {
    /// Ask the picked judgment provider to classify one ambiguous command.
    ///
    /// Returns `Err` only when nothing answered: the call failed, or it ran out
    /// of time. The caller then falls back to
    /// the static "dangerous" list. A reply it cannot read is `Ok` with
    /// [`JudgeVerdict::uncertain`] (ask).
    ///
    /// `judgment_command_guard` picks the backend and is `chat` unless the
    /// user set it. A System One pick has the chat model behind it, inside the
    /// same deadline, because a user waits on the permission card.
    ///
    /// `thread_id` anchors the capture. Every caller runs inside a turn and
    /// holds one, so the judge's spend is never filed against nothing.
    pub(crate) async fn judge_command(
        &self,
        input: &JudgeInput,
        thread_id: uuid::Uuid,
    ) -> Result<JudgeVerdict, Box<dyn std::error::Error + Send + Sync>> {
        let call = self.aux_call(ContextPurpose::CommandJudge).await;
        let deadline = call.deadline();
        let capture = AuxCapture::new(&self.event_bus, thread_id, ContextPurpose::CommandJudge);
        let system_one = system_one_for(
            &self.pool,
            JudgmentSite::CommandGuard,
            call.attempt_timeout(),
        )
        .await;
        let chat = ChatJudgmentProvider::new(call.provider(), call.reasoning().map(str::to_string));
        let provider = for_site(system_one, chat);
        judge_with(provider.as_ref(), input, deadline, &capture)
            .await
            .map_err(|e| {
                if e.is::<tokio::time::error::Elapsed>() {
                    format!("command judge timed out after {:?}", deadline).into()
                } else {
                    e
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::EventBus;
    use crate::llm::judgment::{Answer, Answers, ChoiceAnswer, Judgment, JudgmentUsage, Question};
    use crate::llm::tool_names as tn;
    use crate::test_support::{aux_captures, setup_test_db, teardown_test_db};
    use uuid::Uuid;

    /// A capture for a judge test that reads no row.
    fn discard() -> AuxCapture {
        AuxCapture::discarding(crate::engine::ContextPurpose::CommandJudge)
    }

    /// A deadline no offline test can reach. The stubs answer instantly, so
    /// what these tests exercise is the call and its capture, never the bound.
    const TEST_DEADLINE: Duration = Duration::from_secs(30);

    fn ji(tool: &str, cmd: &str, oow: bool) -> JudgeInput {
        JudgeInput {
            tool_name: tool.to_string(),
            command: cmd.to_string(),
            out_of_workspace: oow,
            fast_path_refused: false,
        }
    }

    /// A stubbed System One [`JudgmentProvider`] that records what it was
    /// asked, so the glue is exercised offline.
    ///
    /// It reports the usage and the model a real response carries, because the
    /// capture the glue emits is built out of both.
    struct StubJudge {
        answers: Answers,
        asked: std::sync::Mutex<Vec<AskedCall>>,
    }

    /// One recorded call: the state and the questions the glue sent.
    type AskedCall = (serde_json::Value, Vec<(String, Question)>);

    /// What [`StubJudge`] reports having spent. Any non-zero pair does, since
    /// what the tests assert is that the numbers reach the row unchanged.
    const STUB_USAGE: JudgmentUsage = JudgmentUsage {
        input_tokens: 312,
        output_tokens: 48,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
    };

    /// The version a real response names, where the request asks for an alias.
    const STUB_MODEL: &str = "jev-1.13.0";

    #[async_trait::async_trait]
    impl JudgmentProvider for StubJudge {
        async fn ask(
            &self,
            state: serde_json::Value,
            questions: Vec<(String, Question)>,
            _call: crate::llm::metered::CallToken,
        ) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
            // Sized through the real builder, so the stub cannot disagree with
            // the provider about what one request weighs.
            let request_chars = crate::llm::judgment::system_one::build_request_body(
                STUB_MODEL, &state, &questions,
            )
            .to_string()
            .chars()
            .count();
            self.asked.lock().unwrap().push((state, questions));
            Ok(Judgment {
                answers: self.answers.clone(),
                usage: STUB_USAGE,
                model: Some(STUB_MODEL.to_string()),
                request_chars,
            })
        }
    }

    fn choice(id: &str, probabilities: &[(&str, f64)]) -> (String, Answer) {
        let probabilities: std::collections::HashMap<String, f64> = probabilities
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect();
        let choice = probabilities
            .iter()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(k, _)| k.clone())
            .unwrap_or_default();
        (
            id.to_string(),
            Answer::Choice(ChoiceAnswer {
                choice,
                probabilities,
                confidence: 0.9,
            }),
        )
    }

    fn stub_judge(answers: Vec<(String, Answer)>) -> StubJudge {
        StubJudge {
            answers: Answers::new(answers.into_iter().collect()),
            asked: std::sync::Mutex::new(vec![]),
        }
    }

    #[tokio::test]
    async fn a_system_one_judge_classifies_and_categorises() {
        let jev = stub_judge(vec![
            choice(
                command_judge_questions::LANE,
                &[("safe", 0.02), ("reversible", 0.03), ("irreversible", 0.95)],
            ),
            choice(command_judge_questions::CATEGORY, &[("external_api", 0.96)]),
        ]);
        let v = judge_with(
            &jev,
            &ji(tn::RUN_BASH, "curl -X POST https://api/charge", false),
            TEST_DEADLINE,
            &discard(),
        )
        .await
        .unwrap();
        assert_eq!(v.lane, RiskLane::IrreversibleDanger);
        assert_eq!(v.category, Some(SideEffectCategory::ExternalApi));
        assert!(v.summary.contains("mutating HTTP request"), "{}", v.summary);
    }

    /// One request carries the state and both questions, and the command
    /// inside it is already redacted.
    #[tokio::test]
    async fn a_system_one_judge_sends_one_redacted_request() {
        let jev = stub_judge(vec![choice(
            command_judge_questions::LANE,
            &[("safe", 0.99)],
        )]);
        let v = judge_with(
            &jev,
            &ji(
                tn::RUN_BASH,
                "psql postgresql://lucidos:hunter2@db.example.com:5432/app -c 'select 1'",
                false,
            ),
            TEST_DEADLINE,
            &discard(),
        )
        .await
        .unwrap();
        assert_eq!(v.lane, RiskLane::Safe);

        let asked = jev.asked.lock().unwrap();
        assert_eq!(asked.len(), 1, "both questions ride in one request");
        let (state, questions) = &asked[0];
        assert_eq!(questions.len(), 2);
        let body = serde_json::to_string(state).unwrap();
        assert!(!body.contains("hunter2"), "{body}");
        assert!(body.contains("select 1"), "{body}");
    }

    /// A call that never answered is `Err`, so the caller falls back to the
    /// static dangerous list.
    #[tokio::test]
    async fn a_failing_system_one_call_is_an_infra_error() {
        struct Failing;
        #[async_trait::async_trait]
        impl JudgmentProvider for Failing {
            async fn ask(
                &self,
                _state: serde_json::Value,
                _questions: Vec<(String, Question)>,
                _call: crate::llm::metered::CallToken,
            ) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
                Err("TypeSafe returned 429".into())
            }
        }
        assert!(judge_with(
            &Failing,
            &ji(tn::RUN_BASH, "ls", false),
            TEST_DEADLINE,
            &discard()
        )
        .await
        .is_err());
    }

    /// `judge_command` names a timeout in its error, so the two must not
    /// arrive as the same error. A timeout is a boxed `Elapsed` and nothing
    /// else is.
    #[tokio::test]
    async fn a_timeout_is_told_apart_from_a_failure() {
        struct Slow;
        #[async_trait::async_trait]
        impl JudgmentProvider for Slow {
            async fn ask(
                &self,
                _state: serde_json::Value,
                _questions: Vec<(String, Question)>,
                _call: crate::llm::metered::CallToken,
            ) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(Judgment::default())
            }
        }
        struct Failing;
        #[async_trait::async_trait]
        impl JudgmentProvider for Failing {
            async fn ask(
                &self,
                _state: serde_json::Value,
                _questions: Vec<(String, Question)>,
                _call: crate::llm::metered::CallToken,
            ) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
                Err("TypeSafe returned 429".into())
            }
        }

        let timed_out = judge_with(
            &Slow,
            &ji(tn::RUN_BASH, "ls", false),
            Duration::from_millis(1),
            &discard(),
        )
        .await
        .expect_err("it ran out of time");
        assert!(timed_out.is::<tokio::time::error::Elapsed>());

        let failed = judge_with(
            &Failing,
            &ji(tn::RUN_BASH, "ls", false),
            TEST_DEADLINE,
            &discard(),
        )
        .await
        .expect_err("the backend refused");
        assert!(
            !failed.is::<tokio::time::error::Elapsed>(),
            "a refusal is not a timeout"
        );
    }

    /// An empty answer set is *ask*, not an error. The typed path has no
    /// "unclear response" to detect, so this is where that tolerance lives.
    #[tokio::test]
    async fn a_system_one_answer_with_no_lane_asks() {
        let v = judge_with(
            &stub_judge(vec![]),
            &ji(tn::RUN_BASH, "ls", false),
            TEST_DEADLINE,
            &discard(),
        )
        .await
        .unwrap();
        assert_eq!(v, JudgeVerdict::uncertain());
    }

    // --- what the judge spends ----------------------------------------------
    //
    // The judge runs in front of every ambiguous command, on whichever backend
    // the user chose. Both paths reach the cost rollup, under one purpose.

    /// The purpose these rows are filed under, as the wire spells it.
    const CAPTURE_PURPOSE: &str = "command_judge";

    #[tokio::test]
    async fn a_system_one_judgment_records_what_it_cost() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let capture = AuxCapture::new(&bus, thread_id, ContextPurpose::CommandJudge);

        let jev = stub_judge(vec![choice(
            command_judge_questions::LANE,
            &[("safe", 0.99)],
        )]);
        let verdict = judge_with(
            &jev,
            &ji(tn::RUN_BASH, "curl https://api/data", false),
            TEST_DEADLINE,
            &capture,
        )
        .await
        .expect("the stub answers");
        assert_eq!(verdict.lane, RiskLane::Safe, "the verdict is unchanged");

        let captures = aux_captures(&pool, thread_id, CAPTURE_PURPOSE).await;
        assert_eq!(captures.len(), 1, "one call, one row: {captures:?}");
        assert_eq!(captures[0]["producer"], "auxiliary");
        assert_eq!(
            captures[0]["usage"]["input_tokens"],
            STUB_USAGE.input_tokens
        );
        assert_eq!(
            captures[0]["usage"]["output_tokens"],
            STUB_USAGE.output_tokens
        );
        assert_eq!(
            captures[0]["model"], STUB_MODEL,
            "the version that answered, not the alias the request asked for"
        );
        assert!(
            captures[0]["sections"][0]["content_chars"]
                .as_u64()
                .is_some_and(|n| n > 0),
            "the row is sized by what the provider sent: {captures:?}"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    // --- the guard on the chat judgment provider ------------------------------
    //
    // The default backend, a chat model answering the typed questions
    // (ADR 0363, I13).

    use crate::llm::judgment::ChatJudgmentProvider;
    use crate::test_support::JudgmentChatStub;
    use std::sync::Arc;

    fn on_chat(stub: JudgmentChatStub) -> (Arc<JudgmentChatStub>, ChatJudgmentProvider) {
        let stub = Arc::new(stub);
        let chat = ChatJudgmentProvider::new(stub.clone(), Some("none".to_string()));
        (stub, chat)
    }

    async fn verdict_on_chat(answer: serde_json::Value, input: &JudgeInput) -> JudgeVerdict {
        let (_, chat) = on_chat(JudgmentChatStub::answering(answer));
        judge_with(&chat, input, TEST_DEADLINE, &discard())
            .await
            .expect("a chat model that answered is never an infra error")
    }

    #[tokio::test]
    async fn the_chat_judgment_classifies_each_lane() {
        // Mutating POST → ask, tagged external_api.
        let v = verdict_on_chat(
            serde_json::json!({
                "lane": { "safe": 0.02, "reversible": 0.03, "irreversible": 0.95 },
                "category": { "external_api": 0.96, "other": 0.04 },
            }),
            &ji(tn::RUN_BASH, "curl -X POST https://api/charge", false),
        )
        .await;
        assert_eq!(v.lane, RiskLane::IrreversibleDanger);
        assert_eq!(v.category, Some(SideEffectCategory::ExternalApi));
        assert!(v.summary.contains("mutating HTTP request"), "{}", v.summary);

        // GET → safe, no category.
        let v = verdict_on_chat(
            serde_json::json!({
                "lane": { "safe": 0.97, "reversible": 0.02, "irreversible": 0.01 },
                "category": { "external_api": 0.9 },
            }),
            &ji(tn::RUN_BASH, "curl https://api/data", false),
        )
        .await;
        assert_eq!(v.lane, RiskLane::Safe);
        assert_eq!(v.category, None, "non-irreversible lanes carry no category");

        // Irreversible with no category answer → Other.
        let v = verdict_on_chat(
            serde_json::json!({ "lane": { "irreversible": 1 } }),
            &ji(tn::RUN_BASH, "weird-tool --send", false),
        )
        .await;
        assert_eq!(v.category, Some(SideEffectCategory::Other));

        // In-workspace rm → reversible.
        let v = verdict_on_chat(
            serde_json::json!({ "lane": { "safe": 0.05, "reversible": 0.9, "irreversible": 0.05 } }),
            &ji(tn::RUN_BASH, "rm -rf data/tmp", false),
        )
        .await;
        assert_eq!(v.lane, RiskLane::ReversibleDanger);
    }

    /// I13: a reply the guard cannot read asks. That covers prose, an empty
    /// reply, and an answer naming options the question never offered.
    #[tokio::test]
    async fn an_unreadable_chat_judgment_asks() {
        for reply in ["the model rambled with no json at all", "   ", ""] {
            let (_, chat) = on_chat(JudgmentChatStub::replying(reply));
            let v = judge_with(
                &chat,
                &ji(tn::RUN_BASH, "x", false),
                TEST_DEADLINE,
                &discard(),
            )
            .await
            .expect("a reply is not an infra error");
            assert_eq!(v, JudgeVerdict::uncertain(), "{reply:?}");
        }

        let v = verdict_on_chat(
            serde_json::json!({ "lane": { "SAFE": 1.0, "harmless": 1.0 } }),
            &ji(tn::RUN_BASH, "x", false),
        )
        .await;
        assert_eq!(v, JudgeVerdict::uncertain(), "aliases are not options");

        let v = verdict_on_chat(
            serde_json::json!({ "category": { "email": 1.0 } }),
            &ji(tn::RUN_BASH, "x", false),
        )
        .await;
        assert_eq!(v, JudgeVerdict::uncertain(), "a missing lane asks");
    }

    /// ADR 0002's tie-break is a threshold in Rust, so a chat model leaning
    /// safe without conviction still gets a card.
    #[tokio::test]
    async fn an_unsure_chat_judgment_picks_the_danger_lane() {
        let v = verdict_on_chat(
            serde_json::json!({ "lane": { "safe": 0.6, "reversible": 0.0, "irreversible": 0.4 } }),
            &ji(tn::RUN_BASH, "x", false),
        )
        .await;
        assert_eq!(v.lane, RiskLane::IrreversibleDanger);

        let v = verdict_on_chat(
            serde_json::json!({ "lane": { "safe": 0.0, "reversible": 0.55, "irreversible": 0.45 } }),
            &ji(tn::RUN_BASH, "x", false),
        )
        .await;
        assert_eq!(
            v.lane,
            RiskLane::IrreversibleDanger,
            "unsure between the danger lanes picks irreversible"
        );
    }

    #[tokio::test]
    async fn a_failed_chat_judgment_call_is_an_infra_error() {
        let (_, chat) = on_chat(JudgmentChatStub::failing("network down"));
        assert!(
            judge_with(
                &chat,
                &ji(tn::RUN_BASH, "x", false),
                TEST_DEADLINE,
                &discard()
            )
            .await
            .is_err(),
            "a call that never answered falls back to the static list"
        );
    }

    #[tokio::test]
    async fn a_weak_chat_category_falls_back_to_other() {
        let v = verdict_on_chat(
            serde_json::json!({
                "lane": { "irreversible": 1.0 },
                "category": { "email": 0.4, "external_api": 0.35, "cloud_cli": 0.25 },
            }),
            &ji(tn::RUN_BASH, "x", false),
        )
        .await;
        assert_eq!(v.category, Some(SideEffectCategory::Other));
        assert!(v.summary.contains("irreversible"), "{}", v.summary);
    }

    /// A model that skipped the tool and wrote the object in text, fenced, is
    /// read just the same.
    #[tokio::test]
    async fn a_fenced_chat_judgment_in_text_is_read() {
        let (_, chat) = on_chat(JudgmentChatStub::replying(
            "Here is my verdict:\n```json\n{\"lane\":{\"safe\":0.99}}\n```\nThat's all.",
        ));
        let v = judge_with(
            &chat,
            &ji(tn::RUN_BASH, "ls", false),
            TEST_DEADLINE,
            &discard(),
        )
        .await
        .expect("answered");
        assert_eq!(v.lane, RiskLane::Safe);
    }

    /// The state leaves the machine for whatever the judge model is. A
    /// connection string is redacted first, and the rest still reaches it.
    #[tokio::test]
    async fn the_chat_judgment_sees_a_redacted_labelled_command() {
        let (stub, chat) = on_chat(JudgmentChatStub::answering(serde_json::json!({})));
        judge_with(
            &chat,
            &ji(
                tn::RUN_BASH,
                "psql postgresql://lucidos:hunter2@db.example.com:5432/app -c 'DROP TABLE events'",
                true,
            ),
            TEST_DEADLINE,
            &discard(),
        )
        .await
        .expect("answered");
        let sent = stub.messages();
        assert_eq!(sent.len(), 1, "both questions ride in one request");
        assert!(!sent[0].contains("hunter2"), "{}", sent[0]);
        assert!(sent[0].contains("DROP TABLE events"), "{}", sent[0]);
        assert!(sent[0].contains("Shell command"), "{}", sent[0]);
        assert!(
            sent[0].contains("\"target_outside_workspace\": true"),
            "{}",
            sent[0]
        );

        let (stub, chat) = on_chat(JudgmentChatStub::answering(serde_json::json!({})));
        judge_with(
            &chat,
            &ji(tn::RUN_PYTHON, "requests.post(u)", false),
            TEST_DEADLINE,
            &discard(),
        )
        .await
        .expect("answered");
        assert!(stub.messages()[0].contains("Python code"));
    }

    /// The chat model is the default backend, so its cost must reach the
    /// rollup under the guard's own purpose.
    #[tokio::test]
    async fn the_chat_judgment_records_what_it_cost() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let capture = AuxCapture::new(&bus, thread_id, ContextPurpose::CommandJudge);

        let (_, chat) = on_chat(JudgmentChatStub::answering(serde_json::json!({
            "lane": { "safe": 1.0 },
        })));
        let verdict = judge_with(
            &chat,
            &ji(tn::RUN_BASH, "curl https://api/data", false),
            TEST_DEADLINE,
            &capture,
        )
        .await
        .expect("answered");
        assert_eq!(verdict.lane, RiskLane::Safe);

        let captures = aux_captures(&pool, thread_id, CAPTURE_PURPOSE).await;
        assert_eq!(captures.len(), 1, "one call, one row: {captures:?}");
        assert_eq!(captures[0]["producer"], "auxiliary");
        assert_eq!(
            captures[0]["model"],
            crate::core::prefs::MODEL_COMMAND_JUDGE.default_text()
        );
        assert_eq!(captures[0]["usage"]["input_tokens"], 210);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// An unreadable reply asks, and it was paid for all the same.
    #[tokio::test]
    async fn an_unreadable_chat_judgment_is_still_recorded() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let capture = AuxCapture::new(&bus, thread_id, ContextPurpose::CommandJudge);

        let (_, chat) = on_chat(JudgmentChatStub::replying("   "));
        let verdict = judge_with(
            &chat,
            &ji(tn::RUN_BASH, "x", false),
            TEST_DEADLINE,
            &capture,
        )
        .await
        .expect("a reply is not an infra error");
        assert_eq!(verdict, JudgeVerdict::uncertain());
        assert_eq!(
            aux_captures(&pool, thread_id, CAPTURE_PURPOSE).await.len(),
            1
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
