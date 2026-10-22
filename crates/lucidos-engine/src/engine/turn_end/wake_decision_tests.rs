//! The wake decision: I4 and I10 of
//! `docs/plans/2026-10-11-sub-threads-wake-parents-only-when-needed.md`.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::*;
use crate::engine::turn_end::OpeningSource;
use crate::llm::judgment::{Answer, Judgment, NoulAnswer};
use uuid::Uuid;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

struct Failing;

#[async_trait]
impl JudgmentProvider for Failing {
    async fn ask(
        &self,
        _: Value,
        _: Vec<(String, Question)>,
        _: crate::llm::metered::CallToken,
    ) -> Result<Judgment, BoxError> {
        Err("System One endpoint returned 529".into())
    }
}

/// Answers the gate's question with `Some(p)`, or with nothing at all.
struct Answering(Option<f64>);

#[async_trait]
impl JudgmentProvider for Answering {
    async fn ask(
        &self,
        _: Value,
        questions: Vec<(String, Question)>,
        _: crate::llm::metered::CallToken,
    ) -> Result<Judgment, BoxError> {
        let answers: HashMap<String, Answer> = match self.0 {
            Some(p) => questions
                .into_iter()
                .map(|(id, _)| (id, Answer::Noul(NoulAnswer { noul: p })))
                .collect(),
            None => HashMap::new(),
        };
        Ok(Judgment {
            answers: Answers::new(answers),
            ..Judgment::default()
        })
    }
}

/// Never answers, so only the deadline ends the call.
struct Hanging;

#[async_trait]
impl JudgmentProvider for Hanging {
    async fn ask(
        &self,
        _: Value,
        _: Vec<(String, Question)>,
        _: crate::llm::metered::CallToken,
    ) -> Result<Judgment, BoxError> {
        std::future::pending().await
    }
}

/// One call a provider received: the state and the question set.
type Asked = (Value, Vec<(String, Question)>);

/// Answers with `p`, and keeps every state and question set it was asked.
struct Recording {
    p: f64,
    asked: std::sync::Mutex<Vec<Asked>>,
}

#[async_trait]
impl JudgmentProvider for Recording {
    async fn ask(
        &self,
        state: Value,
        questions: Vec<(String, Question)>,
        call: crate::llm::metered::CallToken,
    ) -> Result<Judgment, BoxError> {
        self.asked
            .lock()
            .unwrap()
            .push((state.clone(), questions.clone()));
        Answering(Some(self.p)).ask(state, questions, call).await
    }
}

async fn decide(provider: &dyn JudgmentProvider, deadline: Duration) -> WakeDecision {
    let capture = AuxCapture::discarding(ContextPurpose::WakeDecision);
    decide_with(provider, &capture, deadline, json!({})).await
}

#[test]
fn only_a_confident_no_files_and_a_missing_answer_wakes() {
    assert_eq!(
        WakeDecision::from_probability(None),
        WakeDecision::Requested
    );
    assert_eq!(
        WakeDecision::from_probability(Some(0.0)),
        WakeDecision::NotRequested
    );
    assert_eq!(
        WakeDecision::from_probability(Some(WAKE_THRESHOLD - 0.01)),
        WakeDecision::NotRequested
    );
    assert_eq!(
        WakeDecision::from_probability(Some(WAKE_THRESHOLD)),
        WakeDecision::Requested,
        "the threshold itself wakes"
    );
    assert_eq!(
        WakeDecision::from_probability(Some(0.5)),
        WakeDecision::Requested,
        "an even guess is a doubt, and a doubt wakes"
    );
}

#[tokio::test]
async fn a_failed_call_wakes_the_parent() {
    assert_eq!(
        decide(&Failing, Duration::from_secs(5)).await,
        WakeDecision::Requested
    );
}

#[tokio::test]
async fn a_call_with_no_answer_wakes_the_parent() {
    assert_eq!(
        decide(&Answering(None), Duration::from_secs(5)).await,
        WakeDecision::Requested
    );
}

#[tokio::test]
async fn a_timed_out_call_wakes_the_parent() {
    assert_eq!(
        decide(&Hanging, Duration::from_millis(20)).await,
        WakeDecision::Requested
    );
}

#[tokio::test]
async fn the_answer_decides_on_either_side_of_the_threshold() {
    let deadline = Duration::from_secs(5);
    assert_eq!(
        decide(&Answering(Some(0.05)), deadline).await,
        WakeDecision::NotRequested
    );
    assert_eq!(
        decide(&Answering(Some(0.9)), deadline).await,
        WakeDecision::Requested
    );
}

#[test]
fn the_tool_needs_an_explicit_boolean() {
    assert_eq!(
        WakeDecision::from_tool_args(&json!({ "wake": false })),
        Ok(WakeDecision::NotRequested)
    );
    assert_eq!(
        WakeDecision::from_tool_args(&json!({ "wake": true })),
        Ok(WakeDecision::Requested)
    );
    assert!(WakeDecision::from_tool_args(&json!({})).is_err());
    assert!(WakeDecision::from_tool_args(&json!({ "wake": "no" })).is_err());
}

/// I10: the call answers one yes-or-no question and is offered nothing else.
#[test]
fn the_call_asks_one_yes_or_no_question() {
    let questions = wake_questions();
    assert_eq!(questions.len(), 1);
    assert!(matches!(questions[0].1, Question::Noul { .. }));
}

#[test]
fn the_state_carries_both_sides_of_the_report() {
    let turn = TurnRead {
        message: "Find the venue's opening hours".to_string(),
        source: OpeningSource::AgentOrEngine,
        reply: "Open 9 to 5".to_string(),
        ..TurnRead::default()
    };
    let parent = ParentView {
        latest_reply: "I'll combine the three answers once they are back.".to_string(),
        siblings_running: 2,
    };
    let state = wake_state(&turn, ChildCompletionStatus::Success, &parent);
    assert_eq!(
        state["message_that_opened_the_childs_turn"],
        "Find the venue's opening hours"
    );
    assert_eq!(state["childs_reply"], "Open 9 to 5");
    assert_eq!(
        state["parents_latest_reply"],
        "I'll combine the three answers once they are back."
    );
    assert_eq!(state["childs_finish"], "success");
    assert_eq!(
        state["parents_other_children"],
        "2 other children of the parent are still working."
    );
    assert_eq!(
        state["what_opened_the_childs_turn"],
        OpeningSource::AgentOrEngine.heading()
    );
}

/// The call reads the siblings in words, since a bare count misled the
/// decision models (ADR 0448). Only a count of 0 calls the report the last.
#[test]
fn only_the_last_running_child_reads_as_the_last() {
    assert!(siblings_note(0).contains("the last"));
    for running in [1, 3] {
        let note = siblings_note(running);
        assert!(note.starts_with(&running.to_string()), "{note}");
        assert!(!note.contains("last"), "{note}");
    }
}

/// The chat backend over the mock, which answers a forced call with a
/// confident no and an unforced one in text.
fn mock_chat() -> ChatJudgmentProvider {
    wake_decision_chat_judge(
        std::sync::Arc::new(crate::llm::mock::MockProvider::new(
            crate::llm::mock::MOCK_MODEL.to_string(),
        )),
        None,
    )
}

/// The chat backend forces its answer tool, so the mock's no arrives. An
/// unforced call would answer in text, which wakes.
#[tokio::test]
async fn the_chat_backend_is_forced_to_answer() {
    assert_eq!(
        decide(&mock_chat(), Duration::from_secs(30)).await,
        WakeDecision::NotRequested
    );
}

async fn decide_on(
    system_one: Option<Box<dyn JudgmentProvider>>,
    chat: Option<ChatJudgmentProvider>,
) -> Option<WakeDecision> {
    let provider = wake_judge(system_one, chat)?;
    Some(decide(provider.as_ref(), Duration::from_secs(30)).await)
}

/// A pinned or measured chat model answers, alone or behind a failed System
/// One call.
#[tokio::test]
async fn a_measured_or_pinned_chat_model_answers() {
    assert_eq!(
        decide_on(None, Some(mock_chat())).await,
        Some(WakeDecision::NotRequested)
    );
    assert_eq!(
        decide_on(Some(Box::new(Failing)), Some(mock_chat())).await,
        Some(WakeDecision::NotRequested),
        "the chat model answers for a failed System One call"
    );
}

/// With no measured model reachable, no chat model is asked. A System One
/// pick answers alone, its failure wakes, and without one the parent wakes
/// with no call.
#[tokio::test]
async fn with_no_measured_model_no_chat_model_is_asked() {
    assert_eq!(decide_on(None, None).await, None);
    assert_eq!(
        decide_on(Some(Box::new(Answering(Some(0.05)))), None).await,
        Some(WakeDecision::NotRequested)
    );
    assert_eq!(
        decide_on(Some(Box::new(Failing)), None).await,
        Some(WakeDecision::Requested)
    );
}

fn owed(holds_live_wait: bool) -> OwedReport {
    OwedReport {
        parent_id: Uuid::new_v4(),
        decision: None,
        wake_mode: None,
        batch_still_working: false,
        holds_live_wait,
    }
}

/// Whether a report wakes the parent follows the fan-in's own delivery rule
/// for a finished turn (ADR 0446, I3). A cut-short turn wakes, unless the
/// child is `no-wake` (ADR 0545). A held card waits for its event wait, so
/// this turn end wakes nothing.
#[test]
fn the_report_wakes_the_parent_exactly_as_the_fan_in_delivers_it() {
    use ChildCompletionStatus::*;
    let modes = [None, Some(WakeMode::NoWake), Some(WakeMode::Wake)]
        .into_iter()
        .chain([Some(WakeMode::WakeWhenAllDone)]);
    for status in [Success, Failure, NoChanges, Canceled, Interrupted] {
        for (wake_mode, batch_still_working) in
            modes.clone().flat_map(|mode| [(mode, false), (mode, true)])
        {
            let settled = OwedReport {
                wake_mode,
                batch_still_working,
                ..owed(false)
            };
            for decision in [
                None,
                Some(WakeDecision::Requested),
                Some(WakeDecision::NotRequested),
            ] {
                let delivered = Delivery::for_report(
                    status,
                    wake_mode,
                    decision.map(WakeDecision::wake),
                    batch_still_working,
                );
                assert_eq!(
                    settled.wakes_parent(TurnFinish::Finished(status), decision),
                    delivered == Delivery::Wake,
                    "{status:?} {wake_mode:?} batch={batch_still_working} {decision:?}"
                );
            }
        }
    }
    let no = Some(WakeDecision::NotRequested);
    assert!(owed(false).wakes_parent(TurnFinish::CutShort, no));
    let no_wake = OwedReport {
        wake_mode: Some(WakeMode::NoWake),
        ..owed(false)
    };
    assert!(
        !no_wake.wakes_parent(TurnFinish::CutShort, None),
        "a no-wake child's cut-short turn is filed too"
    );
    let yes = Some(WakeDecision::Requested);
    assert!(!owed(true).wakes_parent(TurnFinish::Finished(Success), yes));
}

/// The eval scores exactly what the gate asks: one state, one question set,
/// one threshold.
#[tokio::test]
async fn the_eval_asks_what_the_gate_asks() {
    let turn = TurnRead {
        message: "Collect the venue quotes".to_string(),
        source: OpeningSource::AgentOrEngine,
        reply: "Three quotes, the lowest from Venue B".to_string(),
        ..TurnRead::default()
    };
    let parent = ParentView {
        latest_reply: "I'll pick a venue once the quotes are in.".to_string(),
        siblings_running: 0,
    };
    let status = ChildCompletionStatus::NoChanges;
    for p in [WAKE_THRESHOLD - 0.01, WAKE_THRESHOLD] {
        let provider = Recording {
            p,
            asked: Default::default(),
        };
        let capture = AuxCapture::discarding(ContextPurpose::WakeDecision);
        let gate = decide_with(
            &provider,
            &capture,
            Duration::from_secs(5),
            wake_state(&turn, status, &parent),
        )
        .await;
        let eval = decide_wake_for_eval(
            &provider,
            &turn,
            status,
            &parent,
            crate::llm::metered::CallToken::for_test(),
        )
        .await
        .unwrap();
        assert_eq!(eval.wake, gate == WakeDecision::Requested, "at {p}");
        assert_eq!(eval.probability, Some(p));
        let asked = provider.asked.lock().unwrap();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0].0, asked[1].0, "one state");
        assert_eq!(
            format!("{:?}", asked[0].1),
            format!("{:?}", asked[1].1),
            "one question set"
        );
    }
}

/// I3 and I4 of `docs/plans/2026-10-12-the-spawn-declares-the-wake-mode.md`:
/// a declared child is never asked, and still wakes when cut short or empty.
#[test]
fn a_declared_child_is_never_asked_and_its_doubts_still_wake() {
    let report = |declared: bool, decided: bool| OwedReport {
        parent_id: Uuid::new_v4(),
        decision: decided.then_some(WakeDecision::NotRequested),
        wake_mode: declared.then_some(WakeMode::NoWake),
        batch_still_working: false,
        holds_live_wait: false,
    };
    let done = TurnFinish::Finished(ChildCompletionStatus::Success);
    let woken = WakeStep::Record(WakeDecision::Requested);

    assert_eq!(
        wake_step(&report(true, false), "Here it is.", done),
        WakeStep::Skip
    );
    assert_eq!(wake_step(&report(true, false), "  ", done), woken);
    assert_eq!(
        wake_step(&report(true, false), "Half.", TurnFinish::CutShort),
        woken
    );
    assert_eq!(
        wake_step(&report(false, false), "Here it is.", done),
        WakeStep::Ask(ChildCompletionStatus::Success),
        "an undeclared child keeps the wake decision"
    );
    assert_eq!(
        wake_step(&report(false, true), "Here it is.", done),
        WakeStep::Skip
    );
}

/// The declared answer names the mode and records nothing.
#[test]
fn the_declared_answer_names_the_mode() {
    for mode in WakeMode::ALL {
        let ack = WakeDecision::declared_ack(mode);
        assert!(ack.starts_with("Nothing recorded."), "{ack}");
        assert!(ack.contains(&format!("`{}`", mode.as_str())), "{ack}");
    }
}

mod db {
    use super::*;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta};
    use crate::test_support::{setup_test_db, teardown_test_db};

    async fn message(bus: &EventBus, thread_id: Uuid, parent: Option<Uuid>, text: &str) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: text.into(),
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                parent_thread_id: parent,
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }

    /// The gate asks only for a child owing its parent a card, and only once
    /// per turn: a decision the turn already made is kept.
    #[tokio::test]
    async fn a_child_owes_a_report_until_its_turn_decides() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let (parent, child) = (Uuid::new_v4(), Uuid::new_v4());
        message(&bus, parent, None, "plan the trip").await;
        message(&bus, child, Some(parent), "find the hotel").await;

        assert!(OwedReport::of_thread(&pool, parent).await.is_none());
        let owed = OwedReport::of_thread(&pool, child)
            .await
            .expect("a running child owes its parent a report");
        assert_eq!(owed.parent_id, parent);
        assert!(owed.decision.is_none() && owed.wake_mode.is_none() && !owed.holds_live_wait);

        emit(&bus, child, ThreadEvent::ParentWakeNotRequested).await;
        assert_eq!(
            OwedReport::of_thread(&pool, child).await.unwrap().decision,
            Some(WakeDecision::NotRequested)
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The read decision learns where the report goes from the same column
    /// the fan-in delivers by (ADR 0446, I4): a recorded no files a clean
    /// report, and a recorded yes wakes the parent.
    #[tokio::test]
    async fn the_owed_report_reads_the_decision_the_fan_in_delivers_by() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let parent = Uuid::new_v4();
        let (filed, woken) = (Uuid::new_v4(), Uuid::new_v4());
        message(&bus, parent, None, "plan the trip").await;
        message(&bus, filed, Some(parent), "find the hotel").await;
        message(&bus, woken, Some(parent), "find the train").await;
        emit(&bus, filed, ThreadEvent::ParentWakeNotRequested).await;
        emit(&bus, woken, ThreadEvent::ParentWakeRequested).await;

        let clean = TurnFinish::Finished(ChildCompletionStatus::Success);
        for (child, wakes) in [(filed, false), (woken, true)] {
            let owed = OwedReport::of_thread(&pool, child).await.unwrap();
            assert_eq!(owed.wakes_parent(clean, owed.decision), wakes);
        }

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// I4 and I10: only an undeclared child owes a wake decision, so only it
    /// is offered `wake_parent` and told the wake rule.
    #[tokio::test]
    async fn only_an_undeclared_child_owes_a_wake_decision() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let (parent, undeclared, declared) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        message(&bus, parent, None, "plan the trip").await;
        message(&bus, undeclared, Some(parent), "find the hotel").await;
        bus.emit(BusEvent::Thread {
            thread_id: declared,
            event: crate::engine::chat::make_message_received(
                std::path::Path::new("/tmp"),
                "find the train",
                None,
                None,
                Some(parent),
                None,
                Some(WakeMode::NoWake),
                ActorMode::Agent,
                None,
                None,
                None,
                None,
                None,
            ),
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();

        assert_eq!(WakeDuty::of_thread(&pool, parent).await, WakeDuty::NoParent);
        assert_eq!(
            WakeDuty::of_thread(&pool, undeclared).await,
            WakeDuty::Decide
        );
        assert_eq!(
            WakeDuty::of_thread(&pool, declared).await,
            WakeDuty::Declared(WakeMode::NoWake)
        );
        assert!(!WakeDuty::owes_a_decision(&pool, declared).await);
        assert_eq!(
            OwedReport::of_thread(&pool, declared)
                .await
                .unwrap()
                .wake_mode,
            Some(WakeMode::NoWake)
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The call reads what the parent last said and counts only the other
    /// children still working.
    #[tokio::test]
    async fn the_parent_view_reads_its_reply_and_running_siblings() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let parent = Uuid::new_v4();
        let (child, running, finished) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        message(&bus, parent, None, "plan the trip").await;
        emit(
            &bus,
            parent,
            ThreadEvent::ResponseGenerated {
                text: "I'll combine the three answers.".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
        )
        .await;
        for id in [child, running, finished] {
            message(&bus, id, Some(parent), "one part").await;
        }
        emit(
            &bus,
            finished,
            ThreadEvent::ResponseGenerated {
                text: "Done.".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
        )
        .await;

        let view = ParentView::read(&pool, parent, child).await;
        assert_eq!(view.latest_reply, "I'll combine the three answers.");
        assert_eq!(view.siblings_running, 1, "only `running` is still working");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
