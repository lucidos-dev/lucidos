//! The turn-end gate: each test pins one invariant of
//! `docs/plans/2026-10-10-turn-end-gate-enforces-valid-states.md`.

use super::read_decision::{
    chat_turn_read, coding_agent_turn_read, decision_from, read_decision_chat_judge,
    read_decision_prompt, record_forced_decision, wake_settles_first, AnsweredQuestion,
    ForcedReadDecision, InFlight, OpeningSource, PushedNotification, TurnRead, PUSH_MESSAGE_BYTES,
    READ_DECISION_INSTRUCTIONS, READ_QUESTION_ID, READ_THRESHOLD,
};
use super::wake_decision::{record_wake_decision, OwedReport, WakeDecision};
use super::*;
use crate::engine::change_ops::PlanHold;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::model_call::AuxCapture;
use crate::engine::read_request::ReadDecision;
use crate::engine::thread_events::{
    ActorMode, AnswerKind, ChildCompletionStatus, EngineReason, EventChannel, EventMeta,
    MessageOrigin, QuestionOption, ThreadEvent,
};
use crate::engine::ContextPurpose;
use crate::llm::judgment::{
    Answer, Answers, ChatJudgmentProvider, Judgment, JudgmentProvider, NoulAnswer, Question,
};
use crate::llm::mock::{MockProvider, MOCK_MODEL, MOCK_READ_YES_SENTINEL};
use crate::runtime::agent_runtime::CodingAgent;
use crate::test_support::{
    aux_captures, setup_test_db, start_cc_session, teardown_test_db, ScriptedProvider,
};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

fn session(head: &str, held: Option<ProposalHold>) -> SessionFacts {
    SessionFacts {
        head: Some(head.to_string()),
        held,
    }
}

fn idle(facts: SessionFacts) -> TurnEndState {
    TurnEndState::at_session_idle(true, true, facts)
}

const PLAN: ProposalHold = ProposalHold::Plan(PlanHold::Missing);
const HARDEN: ProposalHold = ProposalHold::HardeningMissing;

// ---- the requirements ----

#[test]
fn an_undecided_turn_owes_a_read_decision_and_a_decided_one_does_not() {
    let undecided = TurnEndState::default();
    assert_eq!(undecided.unmet(), vec![TurnEndRequirement::ReadDecision]);
    let decided = TurnEndState {
        read_decided: true,
        session: None,
    };
    assert!(decided.unmet().is_empty());
}

#[test]
fn the_read_decision_is_forced_and_a_hold_re_enters() {
    let state = TurnEndState::at_session_idle(false, true, session("a1", Some(HARDEN)));
    assert_eq!(
        state.unmet(),
        vec![
            TurnEndRequirement::ReadDecision,
            TurnEndRequirement::Reentry(HARDEN),
        ]
    );
}

// ---- re-entry bounds ----

#[test]
fn a_held_proposal_nudges_once_per_branch_head() {
    let mut ledger = ReentryLedger::default();
    let held = |head| idle(session(head, Some(PLAN)));
    assert_eq!(ledger.take_due(&held("a1")), Some(PLAN));
    assert!(
        ledger.take_due(&held("a1")).is_none(),
        "an agent that ignored the nudge must end its turn, not loop"
    );
    assert!(
        ledger.take_due(&held("b2")).is_some(),
        "new commits earn a new nudge"
    );
}

#[test]
fn a_held_proposal_never_nudges_after_a_user_stop() {
    let mut ledger = ReentryLedger::default();
    let stopped = TurnEndState::at_session_idle(true, false, session("a1", Some(PLAN)));
    assert!(ledger.take_due(&stopped).is_none());
}

#[test]
fn an_unreadable_head_nudges_once() {
    let mut ledger = ReentryLedger::default();
    let held = idle(SessionFacts {
        head: None,
        held: Some(PLAN),
    });
    assert!(ledger.take_due(&held).is_some());
    assert!(ledger.take_due(&held).is_none());
}

/// The plan and the harden keep separate keys: once the plan is settled at
/// the same HEAD, the harden ask is still owed, and a new commit re-arms it.
#[test]
fn a_settled_plan_lets_the_harden_ask_through_at_the_same_head() {
    let mut ledger = ReentryLedger::default();
    assert_eq!(
        ledger.take_due(&idle(session("a1", Some(PLAN)))),
        Some(PLAN)
    );
    assert_eq!(
        ledger.take_due(&idle(session("a1", Some(HARDEN)))),
        Some(HARDEN)
    );
    assert!(ledger
        .take_due(&idle(session("a1", Some(HARDEN))))
        .is_none());
    assert!(ledger
        .take_due(&idle(session("b2", Some(HARDEN))))
        .is_some());
}

#[test]
fn work_with_no_hold_is_left_alone() {
    let mut ledger = ReentryLedger::default();
    assert!(ledger.take_due(&idle(session("a1", None))).is_none());
}

/// An integrated sub-thread's handed-up work is its finished state, so its
/// turn end sends the agent nowhere.
#[test]
fn integrated_work_sends_the_agent_nowhere() {
    let mut ledger = ReentryLedger::default();
    let held = idle(session("a1", Some(ProposalHold::IntegratedIntoParent)));
    assert!(ledger.take_due(&held).is_none());
}

// ---- the forced read decision ----

async fn emit(bus: &EventBus, thread_id: Uuid, channel: EventChannel, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta {
            channel: Some(channel),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

fn message(text: &str) -> ThreadEvent {
    ThreadEvent::MessageReceived {
        provider: None,
        voice_session_id: None,
        wake_mode: None,
        text: text.into(),
        user_image_hashes: vec![],
        device_id: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    }
}

fn reply(text: &str) -> ThreadEvent {
    ThreadEvent::ResponseGenerated {
        text: text.into(),
        images: vec![],
        model: None,
        reasoning_effort: None,
    }
}

fn idled_event() -> ThreadEvent {
    serde_json::from_value(serde_json::json!({ "type": "CodingAgentIdled", "data": {} })).unwrap()
}

/// A deadline no offline test reaches.
const TEST_DEADLINE: Duration = Duration::from_secs(30);

fn forced<'a>(
    provider: &'a dyn JudgmentProvider,
    capture: &'a AuxCapture,
) -> ForcedReadDecision<'a> {
    ForcedReadDecision {
        provider,
        deadline: TEST_DEADLINE,
        capture,
    }
}

/// The read decision's chat backend over `provider`.
fn chat_judge(provider: impl crate::llm::LlmProvider + 'static) -> ChatJudgmentProvider {
    read_decision_chat_judge(Arc::new(provider), None)
}

fn turn(message: &str, reply: &str) -> TurnRead {
    TurnRead {
        message: message.into(),
        reply: reply.into(),
        ..TurnRead::default()
    }
}

/// The Home turn that landed in Review: a finished child thread's report opened
/// it, and the reply said the change was applied. The report must never reach
/// the forced call dressed as the user's request.
#[test]
fn a_child_report_reaches_the_forced_call_as_a_report_not_the_users_message() {
    let home = TurnRead {
        source: OpeningSource::ChildThreadReport,
        ..turn(
            "The last check passed. All 6 agent file tool tests in the API e2e passed. \
             The tree is clean, and the change is ready to apply.",
            "I applied the app-lint and hover fix to main: 13 commits and 19 files. \
             It is not live yet. The lint note and the gated SDK hover arrive when you \
             switch to the new version.",
        )
    };
    let prompt = read_decision_prompt(&home);
    assert!(!prompt.contains("The user's message"), "{prompt}");
    assert!(prompt.contains("not from the user"), "{prompt}");
    assert!(prompt.contains("The last check passed"));

    let asked = read_decision_prompt(&turn("research it", "Findings: three causes."));
    assert!(asked.starts_with("The user's message:\n"), "{asked}");
}

/// The event names pin the wire contract: they are the stored event types.
#[test]
fn the_opening_source_follows_the_event_and_its_sender() {
    let source = OpeningSource::from_event;
    assert_eq!(
        source("ChildThreadCompleted", None),
        OpeningSource::ChildThreadReport
    );
    assert_eq!(source("TriggerStarted", None), OpeningSource::TriggerRun);
    assert_eq!(
        source("PromptInjected", Some(ActorMode::Agent)),
        OpeningSource::AgentOrEngine
    );
    assert_eq!(
        source("PromptInjected", Some(ActorMode::Engine)),
        OpeningSource::AgentOrEngine
    );
    assert_eq!(
        source("MessageReceived", Some(ActorMode::Human)),
        OpeningSource::UserMessage
    );
    assert_eq!(
        source("CodingAgentUserMessageSent", None),
        OpeningSource::UserMessage
    );
}

#[tokio::test]
async fn the_forced_call_answers_no_or_yes_and_records_its_cost() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit(&bus, thread_id, EventChannel::Chat, message("hello")).await;

    let judge = chat_judge(MockProvider::new(MOCK_MODEL.to_string()));
    let capture = AuxCapture::new(&bus, thread_id, ContextPurpose::ReadDecision);
    let decide = forced(&judge, &capture);
    assert_eq!(
        decide
            .decide(&turn("thanks", "You're welcome."))
            .await
            .unwrap(),
        ReadDecision::NotRequested
    );
    let yes = format!("research it {MOCK_READ_YES_SENTINEL}");
    assert_eq!(
        decide
            .decide(&turn(&yes, "Findings: three causes."))
            .await
            .unwrap(),
        ReadDecision::Requested
    );
    assert_eq!(
        aux_captures(&pool, thread_id, "read_decision").await.len(),
        2,
        "every call records what it cost"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_empty_reply_is_a_no_without_a_call() {
    let judge = chat_judge(ScriptedProvider::new("m", vec![]));
    let capture = AuxCapture::discarding(ContextPurpose::ReadDecision);
    assert_eq!(
        forced(&judge, &capture)
            .decide(&turn("go", "  "))
            .await
            .unwrap(),
        ReadDecision::NotRequested
    );
}

/// A model that answers in text gave no answer, and a missing answer asks to
/// be read (I4).
#[tokio::test]
async fn a_text_answer_is_no_answer_and_asks_to_be_read() {
    let judge = chat_judge(ScriptedProvider::new("m", vec!["I think yes."]));
    let capture = AuxCapture::discarding(ContextPurpose::ReadDecision);
    assert_eq!(
        forced(&judge, &capture)
            .decide(&turn("report please", "Here it is."))
            .await
            .unwrap(),
        ReadDecision::Requested
    );
}

/// The threshold reads the probability of yes. A tie reads, and a missing
/// answer reads (I4).
#[test]
fn the_threshold_reads_a_tie_and_a_missing_answer_as_a_read() {
    let answered = |p: f64| {
        Answers::new(
            [(
                READ_QUESTION_ID.to_string(),
                Answer::Noul(NoulAnswer { noul: p }),
            )]
            .into_iter()
            .collect(),
        )
    };
    assert_eq!(
        decision_from(&answered(READ_THRESHOLD)),
        ReadDecision::Requested
    );
    assert_eq!(decision_from(&answered(0.9)), ReadDecision::Requested);
    assert_eq!(decision_from(&answered(0.1)), ReadDecision::NotRequested);
    assert_eq!(decision_from(&Answers::default()), ReadDecision::Requested);
}

/// What the read decision asks any backend: one yes/no question, over the
/// turn as text. Nothing else rides on the call (I10).
#[tokio::test]
async fn the_decision_asks_one_question_over_the_turn() {
    type Asked = (serde_json::Value, Vec<(String, Question)>);
    struct Recording(std::sync::Mutex<Vec<Asked>>);

    #[async_trait::async_trait]
    impl JudgmentProvider for Recording {
        async fn ask(
            &self,
            state: serde_json::Value,
            questions: Vec<(String, Question)>,
            _: crate::llm::metered::CallToken,
        ) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
            self.0.lock().unwrap().push((state, questions));
            Ok(Judgment::default())
        }
    }

    let recording = Recording(Default::default());
    let capture = AuxCapture::discarding(ContextPurpose::ReadDecision);
    let read = turn("research it", "Findings: three causes.");
    forced(&recording, &capture).decide(&read).await.unwrap();
    let asked = recording.0.lock().unwrap();
    let (state, questions) = &asked[0];
    assert_eq!(state.as_str(), Some(read_decision_prompt(&read).as_str()));
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].0, READ_QUESTION_ID);
    assert!(
        matches!(&questions[0].1, Question::Noul { instructions, criteria: Some(_) }
        if instructions == READ_DECISION_INSTRUCTIONS)
    );
}

/// A call that fails records a read request in the engine's name. The reply
/// then waits in Review rather than hiding.
#[tokio::test]
async fn a_failed_call_records_a_read_request_from_the_engine() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit(
        &bus,
        thread_id,
        EventChannel::Chat,
        message("report please"),
    )
    .await;

    let outcome: Result<ReadDecision, Box<dyn std::error::Error + Send + Sync>> =
        Err("the System One call and its chat fallback both failed".into());
    assert_eq!(
        record_forced_decision(&bus, thread_id, outcome).await,
        ReadDecision::Requested
    );

    let actor: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'actor' FROM events \
         WHERE thread_id = $1 AND event_type = 'ThreadReadRequested'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(actor, None, "the engine decided, so no agent is named");

    teardown_test_db(&db).await;
}

// ---- the coding-agent turn ----

/// An earlier turn's decision never satisfies a later turn: the turn starts
/// after the previous `CodingAgentIdled`.
#[tokio::test]
async fn a_coding_agent_turn_reads_its_own_decision_message_and_reply() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let cc = EventChannel::ClaudeCode;
    let thread_id = Uuid::new_v4();
    emit(&bus, thread_id, cc, message("fix the build")).await;
    start_cc_session(&bus, thread_id, "claude-code/test", None).await;

    emit(&bus, thread_id, cc, ThreadEvent::ThreadReadNotRequested).await;
    emit(&bus, thread_id, cc, reply("Fixed.")).await;
    emit(&bus, thread_id, cc, idled_event()).await;
    let follow_up = ThreadEvent::PromptInjected {
        text: "now investigate the crash".into(),
        mode: ActorMode::Human,
        origin: None,
        injected_message_id: None,
        delivered_event_id: None,
    };
    emit(&bus, thread_id, cc, follow_up).await;
    emit(
        &bus,
        thread_id,
        cc,
        reply("The crash comes from the parser."),
    )
    .await;

    let turn = coding_agent_turn_read(&pool, thread_id).await.unwrap();
    assert!(!turn.decided, "the earlier turn's no is not this turn's");
    assert_eq!(turn.message, "now investigate the crash");
    assert_eq!(turn.source, OpeningSource::UserMessage);
    assert_eq!(turn.reply, "The crash comes from the parser.");

    emit(&bus, thread_id, cc, ThreadEvent::ThreadReadRequested).await;
    assert!(
        coding_agent_turn_read(&pool, thread_id)
            .await
            .unwrap()
            .decided
    );

    // A follow-up that lands before the turn ends moves neither the
    // decision nor the opening message.
    let mid_turn = ThreadEvent::PromptInjected {
        text: "also check the logs".into(),
        mode: ActorMode::Human,
        origin: None,
        injected_message_id: None,
        delivered_event_id: None,
    };
    emit(&bus, thread_id, cc, mid_turn).await;
    let turn = coding_agent_turn_read(&pool, thread_id).await.unwrap();
    assert!(turn.decided);
    assert_eq!(turn.message, "now investigate the crash");

    teardown_test_db(&db).await;
}

/// An event-wait delivery or a re-entry speaks for the engine, so the forced
/// call must not read it as the user's request.
#[tokio::test]
async fn a_coding_agent_turn_opened_by_an_agent_prompt_says_so() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let cc = EventChannel::ClaudeCode;
    let thread_id = Uuid::new_v4();
    emit(&bus, thread_id, cc, message("fix the build")).await;
    start_cc_session(&bus, thread_id, "claude-code/test", None).await;
    emit(&bus, thread_id, cc, idled_event()).await;
    let delivery = ThreadEvent::PromptInjected {
        text: "The background task finished.".into(),
        mode: ActorMode::Agent,
        origin: None,
        injected_message_id: None,
        delivered_event_id: None,
    };
    emit(&bus, thread_id, cc, delivery).await;
    emit(&bus, thread_id, cc, reply("All tests pass.")).await;

    let turn = coding_agent_turn_read(&pool, thread_id).await.unwrap();
    assert_eq!(turn.message, "The background task finished.");
    assert_eq!(turn.source, OpeningSource::AgentOrEngine);

    // An engine retrigger names the engine only in its origin.
    emit(&bus, thread_id, cc, idled_event()).await;
    let retrigger = ThreadEvent::CodingAgentPromptSent {
        input_event_id: None,
        text: "Resolve the merge conflict.".into(),
        coding_agent: CodingAgent::ClaudeCode,
        origin: Some(MessageOrigin::engine(EngineReason::MergeConflict)),
    };
    emit(&bus, thread_id, cc, retrigger).await;
    emit(&bus, thread_id, cc, reply("Resolved.")).await;
    let turn = coding_agent_turn_read(&pool, thread_id).await.unwrap();
    assert_eq!(turn.message, "Resolve the merge conflict.");
    assert_eq!(turn.source, OpeningSource::AgentOrEngine);

    teardown_test_db(&db).await;
}

/// The change already asks for the look. A read request beside it would
/// outlive the Apply as an unread dot, so the turn decides no, unasked.
#[tokio::test]
async fn a_turn_holding_a_change_decides_no_without_a_call() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let cc = EventChannel::ClaudeCode;
    let thread_id = Uuid::new_v4();
    emit(&bus, thread_id, cc, message("move the widget pills")).await;
    start_cc_session(&bus, thread_id, "claude-code/test", None).await;
    emit(&bus, thread_id, cc, reply("Moved. The change is ready.")).await;
    assert!(
        !coding_agent_turn_read(&pool, thread_id)
            .await
            .unwrap()
            .change_replaces_read_request
    );

    let proposed = ThreadEvent::ChangeProposed {
        change_id: Uuid::new_v4().to_string(),
        description: Some("Move the widget pills".into()),
        files: vec!["a.rs".into()],
        requires_restart: false,
        origin: None,
        commit_sha: None,
        branch_name: "claude-code/test".into(),
        repo_root: "/tmp".into(),
        hardened: true,
        incomplete: false,
        set_aside: false,
        path: String::new(),
        diff: String::new(),
    };
    emit(&bus, thread_id, cc, proposed).await;
    let turn = coding_agent_turn_read(&pool, thread_id).await.unwrap();
    assert!(turn.change_replaces_read_request);

    let judge = chat_judge(ScriptedProvider::new("m", vec![]));
    let capture = AuxCapture::new(&bus, thread_id, ContextPurpose::ReadDecision);
    let outcome = forced(&judge, &capture).decide(&turn).await;
    assert_eq!(
        record_forced_decision(&bus, thread_id, outcome).await,
        ReadDecision::NotRequested
    );
    assert!(aux_captures(&pool, thread_id, "read_decision")
        .await
        .is_empty());
    let recorded: Vec<String> = sqlx::query_scalar(
        "SELECT event_type FROM events WHERE thread_id = $1 \
         AND event_type IN ('ThreadReadRequested', 'ThreadReadNotRequested')",
    )
    .bind(thread_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(recorded, vec!["ThreadReadNotRequested".to_string()]);

    teardown_test_db(&db).await;
}

// ---- work still in flight ----

/// The nightly orchestrator's progress turn: a sub-thread reported, and the
/// thread still waits on the others. The forced call must learn that, and the
/// final turn, with nothing left, must read as it did before.
#[test]
fn a_turn_ending_in_flight_says_what_it_waits_on_and_the_end_does_not() {
    let step = TurnRead {
        source: OpeningSource::ChildThreadReport,
        in_flight: InFlight {
            sub_threads: 2,
            event_waits: 1,
        },
        ..turn(
            "Triage finished: 40 rows.",
            "Triage finished. Two steps still run.",
        )
    };
    let prompt = read_decision_prompt(&step);
    assert!(
        prompt.ends_with(
            "Closing note: the thread's work is still in flight. \
             It waits on 2 sub-threads and 1 event wait."
        ),
        "{prompt}"
    );

    let one_child = TurnRead {
        in_flight: InFlight {
            sub_threads: 1,
            event_waits: 0,
        },
        ..step.clone()
    };
    assert!(read_decision_prompt(&one_child).ends_with("It waits on 1 sub-thread."));

    let end = TurnRead {
        in_flight: InFlight::default(),
        ..step
    };
    assert!(!read_decision_prompt(&end).contains("Closing note"));

    // The note means nothing unless the system prompt says what it implies.
    assert!(READ_DECISION_INSTRUCTIONS.contains(
        "When a closing note says the thread's work is still in flight, a progress report"
    ));
}

// ---- a report the parent agent acts on (ADR 0446) ----

const PARENT_NOTE: &str = "the parent agent acts on it now.";

/// The archive sub-thread that landed in Review: a parent agent asked for a
/// run, the reply reported it, and the report woke the parent. The forced call
/// must learn that the parent acts on it (I2).
fn archive_report() -> TurnRead {
    TurnRead {
        source: OpeningSource::AgentOrEngine,
        report_wakes_parent: true,
        ..turn(
            "Run the archive script, then report the summary line.",
            "Exit 0. 248 present, 0 missing, 0 unknown.",
        )
    }
}

#[test]
fn a_report_that_wakes_the_parent_agent_says_so() {
    let prompt = read_decision_prompt(&archive_report());
    assert!(prompt.ends_with(PARENT_NOTE), "{prompt}");

    // Both notes show, the in-flight one first.
    let in_flight = TurnRead {
        in_flight: InFlight {
            sub_threads: 1,
            event_waits: 0,
        },
        ..archive_report()
    };
    let both = read_decision_prompt(&in_flight);
    assert!(both.contains("It waits on 1 sub-thread.\n\n"), "{both}");
    assert!(both.ends_with(PARENT_NOTE), "{both}");

    // The note means nothing unless the system prompt says what it implies.
    assert!(READ_DECISION_INSTRUCTIONS
        .contains("When a closing note says the parent agent acts on this reply now"));
}

/// A filed report, and a turn the user opened or resumed, read as before.
#[test]
fn only_an_agent_opened_turn_waking_its_parent_carries_the_note() {
    let filed = TurnRead {
        report_wakes_parent: false,
        ..archive_report()
    };
    let users_own = TurnRead {
        source: OpeningSource::UserMessage,
        ..archive_report()
    };
    let resumed = TurnRead {
        answered: Some(AnsweredQuestion {
            question: "Archive the retakes too?".into(),
            answer: "Yes".into(),
        }),
        ..archive_report()
    };
    for quiet in [filed, users_own, resumed] {
        let prompt = read_decision_prompt(&quiet);
        assert!(!prompt.contains(PARENT_NOTE), "{prompt}");
    }
}

/// The read prompt claims a wake exactly when the fan-in delivers one (I4).
/// The test records the wake decision as the gate does, and builds the read
/// prompt from it. Then the child's reply reaches the parent.
#[tokio::test]
async fn the_parent_note_matches_what_the_fan_in_delivers() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let chat = EventChannel::Chat;
    let clean = TurnFinish::Finished(ChildCompletionStatus::Success);
    let report = "Exit 0. 248 present, 0 missing, 0 unknown.";
    for (decision, delivered) in [
        (WakeDecision::NotRequested, "ChildReportFiled"),
        (WakeDecision::Requested, "ChildThreadCompleted"),
    ] {
        let (parent, child, opener) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        emit(&bus, parent, chat, message("archive the take")).await;
        bus.emit(BusEvent::Thread {
            thread_id: child,
            event: child_message("Run the archive script and report.", parent),
            meta: EventMeta {
                channel: Some(chat),
                event_id: Some(opener),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        let owed = OwedReport::of_thread(&pool, child).await.unwrap();
        record_wake_decision(&bus, child, decision, None)
            .await
            .unwrap();
        let turn = TurnRead {
            report_wakes_parent: owed.wakes_parent(clean, Some(decision)),
            ..chat_turn_read(&pool, child, opener, report).await
        };
        let noted = read_decision_prompt(&turn).ends_with(PARENT_NOTE);

        emit(&bus, child, chat, reply(report)).await;
        let cards: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
               AND event_type = $2",
        )
        .bind(parent.to_string())
        .bind(delivered)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(cards, 1, "{decision:?} delivers a {delivered}");
        assert_eq!(noted, decision == WakeDecision::Requested, "{decision:?}");
    }

    teardown_test_db(&db).await;
}

/// The wake decision runs first only where the read decision reads it, so
/// every other turn still makes both calls at once (I7). An agent's own read
/// decision is kept, and no call reads the wake for it (I1).
#[test]
fn the_wake_decision_settles_first_only_for_a_forced_read_on_an_agent_turn() {
    let agent_turn = archive_report();
    assert!(wake_settles_first(false, &agent_turn));
    assert!(!wake_settles_first(true, &agent_turn));
    let users_own = TurnRead {
        source: OpeningSource::UserMessage,
        ..archive_report()
    };
    assert!(!wake_settles_first(false, &users_own));
}

// ---- a question answered inside the turn ----

fn asked(tool_use_id: &str, question: &str, options: &[(&str, &str)]) -> ThreadEvent {
    ThreadEvent::UserQuestionAsked {
        tool_use_id: tool_use_id.into(),
        cc_session_id: String::new(),
        question: question.into(),
        options: options
            .iter()
            .map(|(id, label)| QuestionOption {
                id: (*id).into(),
                label: (*label).into(),
                description: None,
                preview: None,
                widget: None,
                quiet: false,
                quiet_reply: None,
            })
            .collect(),
        worktree_path: None,
        multi_select: false,
        owner_approval: None,
    }
}

fn answered(tool_use_id: &str, answer: AnswerKind) -> ThreadEvent {
    ThreadEvent::UserQuestionAnswered {
        tool_use_id: tool_use_id.into(),
        quiet: false,
        answer,
    }
}

fn typed(text: &str) -> AnswerKind {
    AnswerKind::FreeText {
        text: text.into(),
        image_hashes: vec![],
    }
}

/// The trigger run that landed in Review: it asked the user a question, the
/// answer resumed the same turn, and the reply only confirmed that answer.
/// The forced call saw the run's instruction and the reply, never the answer.
#[test]
fn an_answer_inside_the_turn_reaches_the_forced_call_between_opener_and_reply() {
    let watch = TurnRead {
        source: OpeningSource::TriggerRun,
        answered: Some(AnsweredQuestion {
            question: "What do you want done with the tester's question?".into(),
            answer: "They were happy with my answer, it seems".into(),
        }),
        ..turn(
            "Catch me up on what my testers said in the channel.",
            "Yes, they were happy with it. I closed the board item.",
        )
    };
    let prompt = read_decision_prompt(&watch);
    let opener = prompt.find("Catch me up").unwrap();
    let question = prompt
        .find("The agent then asked the user:\nWhat do you want done")
        .unwrap();
    let answer = prompt
        .find("The user answered:\nThey were happy with my answer, it seems")
        .unwrap();
    let reply = prompt
        .find("The agent's reply:\nYes, they were happy")
        .unwrap();
    assert!(
        opener < question && question < answer && answer < reply,
        "{prompt}"
    );

    // The answer means nothing unless the system prompt says what it implies.
    assert!(READ_DECISION_INSTRUCTIONS.contains("The user answered"));
}

/// A chat turn reads the latest answer after its opener, never one from an
/// earlier turn, and a cancelled card is no answer.
#[tokio::test]
async fn a_chat_turn_reads_the_answer_after_its_opener_only() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let chat = EventChannel::Chat;
    let thread_id = Uuid::new_v4();

    emit(&bus, thread_id, chat, asked("q-old", "Earlier?", &[])).await;
    emit(
        &bus,
        thread_id,
        chat,
        answered("q-old", typed("from before")),
    )
    .await;
    let opener = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: message("triage the channel"),
        meta: EventMeta {
            channel: Some(chat),
            event_id: Some(opener),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    let unasked = chat_turn_read(&pool, thread_id, opener, "Nothing new.").await;
    assert_eq!(
        unasked.answered, None,
        "an earlier turn's answer never shows"
    );

    let options = [("opt-0", "Park it"), ("opt-1", "Drop it")];
    emit(&bus, thread_id, chat, asked("q-1", "What now?", &options)).await;
    let picked = AnswerKind::Selected {
        option_id: "opt-1".into(),
    };
    emit(&bus, thread_id, chat, answered("q-1", picked)).await;
    emit(&bus, thread_id, chat, asked("q-2", "Anything else?", &[])).await;
    emit(&bus, thread_id, chat, answered("q-2", AnswerKind::Canceled)).await;

    let turn = chat_turn_read(&pool, thread_id, opener, "Dropped.").await;
    assert_eq!(turn.message, "triage the channel");
    assert_eq!(
        turn.answered,
        Some(AnsweredQuestion {
            question: "What now?".into(),
            answer: "Drop it".into(),
        })
    );

    teardown_test_db(&db).await;
}

/// A coding-agent turn reads the answer given since its last idle.
#[tokio::test]
async fn a_coding_agent_turn_reads_the_answer_since_its_last_idle() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let cc = EventChannel::ClaudeCode;
    let thread_id = Uuid::new_v4();
    emit(&bus, thread_id, cc, message("fix the build")).await;
    start_cc_session(&bus, thread_id, "claude-code/test", None).await;
    emit(&bus, thread_id, cc, asked("q-1", "Which fix?", &[])).await;
    emit(
        &bus,
        thread_id,
        cc,
        answered("q-1", typed("the narrow one")),
    )
    .await;
    emit(&bus, thread_id, cc, reply("Done, the narrow fix.")).await;

    let turn = coding_agent_turn_read(&pool, thread_id).await.unwrap();
    assert_eq!(turn.message, "fix the build");
    assert_eq!(
        turn.answered,
        Some(AnsweredQuestion {
            question: "Which fix?".into(),
            answer: "the narrow one".into(),
        })
    );

    emit(&bus, thread_id, cc, idled_event()).await;
    let follow_up = ThreadEvent::PromptInjected {
        text: "now the docs".into(),
        mode: ActorMode::Human,
        origin: None,
        injected_message_id: None,
        delivered_event_id: None,
    };
    emit(&bus, thread_id, cc, follow_up).await;
    emit(&bus, thread_id, cc, reply("Docs updated.")).await;
    let next = coding_agent_turn_read(&pool, thread_id).await.unwrap();
    assert_eq!(
        next.answered, None,
        "the previous turn's answer never shows"
    );

    teardown_test_db(&db).await;
}

fn child_message(text: &str, parent: Uuid) -> ThreadEvent {
    ThreadEvent::MessageReceived {
        provider: None,
        voice_session_id: None,
        wake_mode: None,
        text: text.into(),
        user_image_hashes: vec![],
        device_id: None,
        image_description: None,
        parent_thread_id: Some(parent),
        spawning_event_id: None,
        mode: ActorMode::Agent,
        model: None,
        reasoning_effort: None,
        origin: None,
    }
}

/// Both turn readers take the in-flight facts from the summary as the turn
/// ends: a running sub-thread and the thread's own live event wait.
#[tokio::test]
async fn both_turn_readers_see_a_running_sub_thread_and_a_live_wait() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let chat = Uuid::new_v4();
    let opener = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: chat,
        event: message("run the nightly"),
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            event_id: Some(opener),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    let settled = chat_turn_read(&pool, chat, opener, "Started.").await;
    assert_eq!(settled.in_flight, InFlight::default());
    assert_eq!(settled.message, "run the nightly");

    let child = Uuid::new_v4();
    emit(
        &bus,
        child,
        EventChannel::Chat,
        child_message("step 1", chat),
    )
    .await;
    emit(
        &bus,
        chat,
        EventChannel::Chat,
        crate::test_support::event_wait_started(Uuid::new_v4()),
    )
    .await;
    let step = chat_turn_read(&pool, chat, opener, "Step 1 started.").await;
    assert_eq!(
        step.in_flight,
        InFlight {
            sub_threads: 1,
            event_waits: 1,
        }
    );

    let cc = EventChannel::ClaudeCode;
    let agent = Uuid::new_v4();
    emit(&bus, agent, cc, message("run the e2e suite")).await;
    start_cc_session(&bus, agent, "claude-code/test", None).await;
    emit(
        &bus,
        agent,
        cc,
        crate::test_support::event_wait_started(Uuid::new_v4()),
    )
    .await;
    emit(&bus, agent, cc, reply("The suite runs in the background.")).await;
    let turn = coding_agent_turn_read(&pool, agent).await.unwrap();
    assert_eq!(
        turn.in_flight,
        InFlight {
            sub_threads: 0,
            event_waits: 1,
        }
    );

    teardown_test_db(&db).await;
}

// ---- a notification the turn already sent ----

fn pushed(title: &str, message: &str) -> PushedNotification {
    PushedNotification {
        title: title.into(),
        message: message.into(),
    }
}

/// A turn that pushed its news and ended undecided: the forced call must
/// learn what the push already said (I5).
#[test]
fn a_turn_that_pushed_says_what_the_user_already_got() {
    let run = TurnRead {
        source: OpeningSource::TriggerRun,
        pushed: vec![pushed(
            "Inbox: 2 need you",
            "Landlord moves the inspection.",
        )],
        ..turn("Triage my inbox.", "Two messages need you. I sent a push.")
    };
    let prompt = read_decision_prompt(&run);
    assert!(
        prompt.ends_with(
            "Closing note: before this reply, the turn sent the user this notification, \
             so they have already seen it.\n\
             Title: Inbox: 2 need you\n\
             Message: Landlord moves the inspection."
        ),
        "{prompt}"
    );

    let two = TurnRead {
        pushed: vec![pushed("One", "first"), pushed("Two", "second")],
        ..run.clone()
    };
    let prompt = read_decision_prompt(&two);
    assert!(
        prompt.contains("sent the user these 2 notifications"),
        "{prompt}"
    );
    assert!(prompt.ends_with("Title: Two\nMessage: second"), "{prompt}");

    let quiet = TurnRead {
        pushed: vec![],
        ..run
    };
    assert!(!read_decision_prompt(&quiet).contains("Closing note"));

    // The note means nothing unless the question says what it implies.
    assert!(READ_DECISION_INSTRUCTIONS
        .contains("When a closing note says the turn already sent the user a notification"));
}

/// A long push reaches the call cut in the middle, like the reply.
#[test]
fn a_long_push_is_cut_to_its_budget() {
    let long = "x".repeat(10_000);
    let run = TurnRead {
        pushed: vec![pushed("Long", &long)],
        ..turn("Run.", "Done.")
    };
    let kept = read_decision_prompt(&run).matches('x').count();
    assert!(kept <= PUSH_MESSAGE_BYTES, "{kept} bytes of the push kept");
    assert!(
        kept > PUSH_MESSAGE_BYTES / 2,
        "both ends of the push survive"
    );
}

async fn emit_with_id(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) -> Uuid {
    let id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            event_id: Some(id),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    id
}

/// Emit a `send_notification` call and its result.
async fn send_notification(bus: &EventBus, thread_id: Uuid, title: &str, success: bool) {
    let called = emit_with_id(
        bus,
        thread_id,
        ThreadEvent::ToolCalled {
            name: crate::llm::tool_names::SEND_NOTIFICATION.into(),
            args: serde_json::json!({ "title": title, "message": format!("{title} body") }),
            description: String::new(),
        },
    )
    .await;
    emit_with_id(
        bus,
        thread_id,
        ThreadEvent::ToolResult {
            name: crate::llm::tool_names::SEND_NOTIFICATION.into(),
            result: if success {
                "Notification sent."
            } else {
                "No device."
            }
            .into(),
            images: vec![],
            success,
            tool_called_event_id: Some(called),
        },
    )
    .await;
}

/// The push note counts only what this turn delivered: never a push from an
/// earlier turn, a failed one, or one on another thread (I4).
#[tokio::test]
async fn a_chat_turn_reads_only_the_pushes_it_delivered() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let run = Uuid::new_v4();

    send_notification(&bus, run, "earlier turn", true).await;
    let opener = emit_with_id(&bus, run, message("triage the inbox")).await;
    send_notification(&bus, run, "failed", false).await;
    send_notification(&bus, Uuid::new_v4(), "another thread", true).await;
    send_notification(&bus, run, "delivered", true).await;

    let turn = chat_turn_read(&pool, run, opener, "One message needs you.").await;
    assert_eq!(turn.pushed, vec![pushed("delivered", "delivered body")]);

    teardown_test_db(&db).await;
}
