use super::read_decision::{
    chat_turn_read, coding_agent_turn_read, decision_from, read_decision_chat_judge,
    read_decision_prompt, record_forced_decision, wake_settles_first, AnsweredQuestion,
    ForcedReadDecision, InFlight, OpeningSource, PushedNotification, TurnRead,
    PARENT_ACTS_READ_RULE, PARENT_MAY_ACT_READ_RULE, PUSH_MESSAGE_BYTES,
    READ_DECISION_INSTRUCTIONS, READ_QUESTION_ID, READ_THRESHOLD,
};
use super::wake_decision::{record_wake_decision, OwedReport, WakeDecision};
use super::*;
use crate::engine::change_ops::PlanHold;
use crate::engine::context_ceiling::{CeilingCrossing, ContextCeiling};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::model_call::AuxCapture;
use crate::engine::read_request::ReadDecision;
use crate::engine::successor::SessionReading;
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
        approved_plan_unstarted: false,
    }
}

fn idle(facts: SessionFacts) -> TurnEndState {
    TurnEndState::at_session_idle(true, true, facts, None)
}

const PLAN: ProposalHold = ProposalHold::Plan(PlanHold::Missing);
const HARDEN: ProposalHold = ProposalHold::HardeningMissing;

#[test]
fn an_undecided_turn_owes_a_read_decision_and_a_decided_one_does_not() {
    let undecided = TurnEndState::default();
    assert_eq!(undecided.unmet(), vec![TurnEndRequirement::ReadDecision]);
    let decided = TurnEndState {
        read_decided: true,
        ..TurnEndState::default()
    };
    assert!(decided.unmet().is_empty());
}

#[test]
fn the_read_decision_is_forced_and_a_hold_re_enters() {
    let state = TurnEndState::at_session_idle(false, true, session("a1", Some(HARDEN)), None);
    assert_eq!(
        state.unmet(),
        vec![
            TurnEndRequirement::ReadDecision,
            TurnEndRequirement::Reentry(HARDEN),
        ]
    );
}

#[test]
fn a_held_proposal_nudges_once_per_branch_head() {
    let mut ledger = ReentryLedger::default();
    let held = |head| idle(session(head, Some(PLAN)));
    assert_eq!(ledger.take_due(&held("a1")), Some(DueReentry::Hold(PLAN)));
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
    let stopped = TurnEndState::at_session_idle(true, false, session("a1", Some(PLAN)), None);
    assert!(ledger.take_due(&stopped).is_none());
}

#[test]
fn an_unreadable_head_nudges_once() {
    let mut ledger = ReentryLedger::default();
    let held = idle(SessionFacts {
        head: None,
        held: Some(PLAN),
        approved_plan_unstarted: false,
    });
    assert!(ledger.take_due(&held).is_some());
    assert!(ledger.take_due(&held).is_none());
}

#[test]
fn a_settled_plan_lets_the_harden_ask_through_at_the_same_head() {
    let mut ledger = ReentryLedger::default();
    assert_eq!(
        ledger.take_due(&idle(session("a1", Some(PLAN)))),
        Some(DueReentry::Hold(PLAN))
    );
    assert_eq!(
        ledger.take_due(&idle(session("a1", Some(HARDEN)))),
        Some(DueReentry::Hold(HARDEN))
    );
    assert!(ledger
        .take_due(&idle(session("a1", Some(HARDEN))))
        .is_none());
    assert!(ledger
        .take_due(&idle(session("b2", Some(HARDEN))))
        .is_some());
}

#[test]
fn a_plan_only_hold_sends_the_agent_nowhere() {
    let mut ledger = ReentryLedger::default();
    let plan_only = ProposalHold::Plan(PlanHold::PlanOnlyAwaitingApproval);
    assert!(ledger
        .take_due(&idle(session("a1", Some(plan_only))))
        .is_none());
    let mixed = ProposalHold::Plan(PlanHold::AwaitingApproval);
    assert_eq!(
        ledger.take_due(&idle(session("a1", Some(mixed)))),
        Some(DueReentry::Hold(mixed))
    );
}

#[test]
fn work_with_no_hold_is_left_alone() {
    let mut ledger = ReentryLedger::default();
    assert!(ledger.take_due(&idle(session("a1", None))).is_none());
}

#[test]
fn integrated_work_sends_the_agent_nowhere() {
    let mut ledger = ReentryLedger::default();
    let held = idle(session("a1", Some(ProposalHold::IntegratedIntoParent)));
    assert!(ledger.take_due(&held).is_none());
}

const K: u64 = 1_000;

fn past(tokens: u64) -> CeilingFacts {
    CeilingFacts {
        owed: ContextCeiling::Tokens(300 * K).crossed_by(SessionReading {
            tokens: Some(tokens),
            window: Some(1_000 * K),
        }),
        busy: false,
    }
}

#[test]
fn ceiling_fires_past_the_effective_ceiling_only() {
    assert_eq!(
        past(335 * K).crossing(),
        Some(CeilingCrossing {
            context_tokens: 335 * K,
            ceiling: 300 * K,
            window: Some(1_000 * K),
        })
    );
    assert_eq!(past(300 * K).crossing(), None);
    assert_eq!(past(120 * K).crossing(), None);
}

#[test]
fn ceiling_fires_at_the_capped_value() {
    let crossing = ContextCeiling::Tokens(300 * K).crossed_by(SessionReading {
        tokens: Some(160 * K),
        window: Some(200 * K),
    });
    assert_eq!(crossing.map(|c| c.ceiling), Some(150 * K));
}

#[test]
fn ceiling_never_fires_on_an_unknown_reading_or_an_unsized_ceiling() {
    assert_eq!(
        ContextCeiling::Tokens(300 * K).crossed_by(SessionReading {
            tokens: None,
            window: Some(1_000 * K),
        }),
        None
    );
    assert_eq!(
        ContextCeiling::Off.crossed_by(SessionReading {
            tokens: Some(900 * K),
            window: Some(1_000 * K),
        }),
        None
    );
    assert_eq!(
        ContextCeiling::WindowShare(30).crossed_by(SessionReading {
            tokens: Some(900 * K),
            window: None,
        }),
        None
    );
}

#[test]
fn ceiling_delivery_needs_a_recorded_crossing() {
    let facts = CeilingFacts {
        owed: None,
        busy: false,
    };
    assert_eq!(facts.crossing(), None);
}

#[test]
fn ceiling_never_fires_while_the_session_is_busy() {
    let busy = CeilingFacts {
        busy: true,
        ..past(400 * K)
    };
    assert_eq!(busy.crossing(), None);
}

#[test]
fn ceiling_outranks_the_proposal_hold() {
    let crossing = past(400 * K).crossing();
    let both = TurnEndState::at_session_idle(false, true, session("a1", Some(HARDEN)), crossing);
    assert_eq!(
        both.unmet(),
        vec![
            TurnEndRequirement::ReadDecision,
            TurnEndRequirement::CeilingHandoff(crossing.unwrap()),
            TurnEndRequirement::Reentry(HARDEN),
        ]
    );
    let mut ledger = ReentryLedger::default();
    assert_eq!(
        ledger.take_due(&both),
        Some(DueReentry::Ceiling(crossing.unwrap()))
    );
    assert_eq!(
        ledger.take_due(&idle(session("a1", Some(HARDEN)))),
        Some(DueReentry::Hold(HARDEN)),
        "the ceiling spent nothing of the hold's"
    );
}

#[test]
fn the_ledger_sends_the_ceiling_once_even_with_no_session_facts() {
    let crossing = past(400 * K).crossing();
    let external = TurnEndState::at_session_idle(true, false, SessionFacts::default(), crossing);
    let mut ledger = ReentryLedger::default();
    assert_eq!(
        ledger.take_due(&external),
        Some(DueReentry::Ceiling(crossing.unwrap()))
    );
    assert_eq!(ledger.take_due(&external), None);
}

#[test]
fn a_withheld_reentry_comes_due_again() {
    let crossing = past(400 * K).crossing();
    let external = TurnEndState::at_session_idle(true, false, SessionFacts::default(), crossing);
    let mut ledger = ReentryLedger::default();
    let withheld = ledger.take_due(&external).unwrap();
    ledger.forget(withheld);
    assert_eq!(ledger.take_due(&external), Some(withheld));

    let held = idle(session("a1", Some(HARDEN)));
    let mut ledger = ReentryLedger::default();
    let withheld = ledger.take_due(&held).unwrap();
    ledger.forget(withheld);
    assert_eq!(ledger.take_due(&held), Some(withheld));
    assert_eq!(ledger.take_due(&held), None, "a sent one still sends once");
}

#[test]
fn session_13_shape_delivers_the_recorded_crossing_once() {
    let mut ledger = ReentryLedger::default();
    let turn_end =
        TurnEndState::at_session_idle(true, true, session("a1", None), past(301 * K).crossing());
    assert!(
        matches!(ledger.take_due(&turn_end), Some(DueReentry::Ceiling(
        CeilingCrossing { context_tokens, .. }
    )) if context_tokens == 301 * K)
    );
    assert_eq!(ledger.take_due(&turn_end), None);
}

async fn emit(bus: &EventBus, thread_id: Uuid, channel: EventChannel, event: ThreadEvent) -> Uuid {
    bus.emit_for_id(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta {
            channel: Some(channel),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap()
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
        record_forced_decision(&pool, &bus, thread_id, None, outcome).await,
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

#[test]
fn trigger_read_yes_goes_to_the_judge_and_no_stands() {
    crate::test_support::run_on_engine_runtime(
        "engine::turn_end::tests::trigger_read_yes_goes_to_the_judge_and_no_stands",
        trigger_read_decision_case,
    );
}

async fn trigger_read_decision_case() {
    use super::read_decision::TurnEndDecisionExt;
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    let trigger: ThreadEvent = serde_json::from_value(serde_json::json!({
        "type": "TriggerStarted",
        "trigger_id": "daily-check", "prompt": "Check for new mail."
    }))
    .unwrap();
    let wakes = [
        serde_json::json!({"type": "ChildThreadCompleted",
            "child_thread_id": Uuid::new_v4(), "status": "success", "summary": "Mail checked."
        }),
        serde_json::json!({"type": "PromptInjected",
            "text": "The mail check timed out.", "mode": "agent",
            "origin": {"kind": "engine", "reason": {
                "kind": "event_wait", "outcome": "expired",
                "watched": ["MailChecked"], "wait_reason": "Waiting for the mail check."
            }}
        }),
        serde_json::json!({"type": "ContinuationStarted"}),
    ];
    for wake in wakes {
        let wake: ThreadEvent = serde_json::from_value(wake).unwrap();
        let [run, started, coalesced, trigger_wake, promoted_wake, follow_up, follow_up_wake, chat, chat_wake] =
            std::array::from_fn(|_| Uuid::new_v4());
        let settings = serde_json::from_value(serde_json::json!({
            "type": "ChatTurnSettingsResolved", "model": MOCK_MODEL
        }))
        .unwrap();
        for (thread_id, id, request_event_id, event) in [
            (run, started, None, trigger.clone()),
            (run, coalesced, Some(started), message("Check again.")),
            (run, trigger_wake, None, wake.clone()),
            (run, Uuid::new_v4(), Some(coalesced), settings),
            (run, promoted_wake, None, wake.clone()),
            (run, follow_up, None, message("What did you check?")),
            (run, follow_up_wake, None, wake.clone()),
            (chat, Uuid::new_v4(), None, message("Check my mail.")),
            (chat, chat_wake, None, wake),
        ] {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: EventMeta {
                    event_id: Some(id),
                    request_event_id,
                    ..EventMeta::NONE
                },
            })
            .await
            .unwrap();
        }

        for (thread, opener, requested, deferred, continuation) in [
            (run, started, true, true, false),
            (run, started, false, false, false),
            (run, trigger_wake, true, true, true),
            (run, promoted_wake, true, false, false),
            (run, follow_up, true, false, false),
            (run, follow_up_wake, true, false, false),
            (chat, chat_wake, true, false, false),
        ] {
            let read_events = || {
                sqlx::query_scalar::<_, String>(
                    "SELECT event_type FROM events WHERE thread_id = $1
                     AND event_type IN ('ThreadReadRequested', 'ThreadReadNotRequested') ORDER BY sequence",
                )
                .bind(thread)
                .fetch_all(&pool)
            };
            let before = read_events().await.unwrap().len();
            let (recorded, ack) = engine
                .decide_chat_agent_read(thread, opener, &serde_json::json!({"read": requested}))
                .await
                .unwrap();
            assert_eq!(recorded, !deferred);
            if deferred {
                assert_eq!(ack, "The engine will decide at the end of the turn.");
            }
            let reply = "No new mail.";
            let turn = chat_turn_read(&pool, thread, opener, reply).await;
            assert_eq!(turn.continues_trigger_run, continuation);
            let calls = aux_captures(&pool, thread, "read_decision").await.len();
            engine
                .gate_chat_turn_end(
                    thread,
                    opener,
                    recorded,
                    reply,
                    TurnFinish::Finished(ChildCompletionStatus::Success),
                )
                .await;
            assert_eq!(
                aux_captures(&pool, thread, "read_decision").await.len() - calls,
                usize::from(deferred),
            );
            let desired = if requested && recorded {
                "ThreadReadRequested"
            } else {
                "ThreadReadNotRequested"
            };
            assert_eq!(&read_events().await.unwrap()[before..], [desired]);
        }
    }
    drop(engine);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_coding_agent_turn_reads_only_its_own_handoff() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let cc = EventChannel::ClaudeCode;
    let thread_id = Uuid::new_v4();
    emit(&bus, thread_id, cc, message("check the build")).await;
    start_cc_session(&bus, thread_id, "claude-code/test", None).await;
    let handoff = |starts| ThreadEvent::SessionHandedOff {
        plan_path: None,
        cause: None,
        starts,
        crossing: None,
        opening: crate::engine::thread_events::HandoffOpening::Narrative,
        message: "Carry on with verification.".into(),
        checkpoint: None,
        progress: None,
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        coding_agent_model: None,
        reasoning_effort: None,
        request: None,
    };
    use crate::engine::thread_events::SessionStart;
    emit(&bus, thread_id, cc, handoff(SessionStart::OnNextInput)).await;
    let paused = coding_agent_turn_read(&pool, thread_id, None)
        .await
        .unwrap();
    assert!(!paused.in_flight.successor_session);

    emit(&bus, thread_id, cc, handoff(SessionStart::AfterThisTurn)).await;

    let step = coding_agent_turn_read(&pool, thread_id, None)
        .await
        .unwrap();
    assert!(step.in_flight.successor_session);
    assert!(read_decision_prompt(&step).ends_with(
        "Closing note: the thread's work is still in flight. A successor session carries it on."
    ));

    emit(&bus, thread_id, cc, ThreadEvent::SessionHandoffWithdrawn).await;
    let withdrawn = coding_agent_turn_read(&pool, thread_id, None)
        .await
        .unwrap();
    assert!(!withdrawn.in_flight.successor_session);

    emit(&bus, thread_id, cc, handoff(SessionStart::AfterThisTurn)).await;
    start_cc_session(&bus, thread_id, "claude-code/test", None).await;
    let successor_start: i64 = sqlx::query_scalar(
        "SELECT MAX(sequence) FROM events WHERE thread_id = $1 AND event_type = 'SessionStarted'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let old = coding_agent_turn_read(&pool, thread_id, None)
        .await
        .unwrap();
    assert!(old.in_flight.successor_session);
    let successor = coding_agent_turn_read(&pool, thread_id, Some(successor_start))
        .await
        .unwrap();
    assert!(!successor.in_flight.successor_session);

    emit(&bus, thread_id, cc, idled_event()).await;
    let next = coding_agent_turn_read(&pool, thread_id, None)
        .await
        .unwrap();
    assert!(!next.in_flight.successor_session);
    assert!(!read_decision_prompt(&next).contains("Closing note"));

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_coding_agent_turn_reads_its_own_decision_message_and_reply() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let cc = EventChannel::ClaudeCode;
    let thread_id = Uuid::new_v4();
    emit(&bus, thread_id, cc, message("fix the build")).await;
    start_cc_session(&bus, thread_id, "claude-code/test", None).await;

    emit(
        &bus,
        thread_id,
        cc,
        ThreadEvent::ThreadReadNotRequested {
            supersedes_request: false,
        },
    )
    .await;
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

    let turn = coding_agent_turn_read(&pool, thread_id, None)
        .await
        .unwrap();
    assert!(!turn.decided, "the earlier turn's no is not this turn's");
    assert_eq!(turn.message, "now investigate the crash");
    assert_eq!(turn.source, OpeningSource::UserMessage);
    assert_eq!(turn.reply, "The crash comes from the parser.");

    emit(
        &bus,
        thread_id,
        cc,
        ThreadEvent::ThreadReadRequested {
            parent_report: false,
        },
    )
    .await;
    assert!(
        coding_agent_turn_read(&pool, thread_id, None)
            .await
            .unwrap()
            .decided
    );

    let mid_turn = ThreadEvent::PromptInjected {
        text: "also check the logs".into(),
        mode: ActorMode::Human,
        origin: None,
        injected_message_id: None,
        delivered_event_id: None,
    };
    emit(&bus, thread_id, cc, mid_turn).await;
    let turn = coding_agent_turn_read(&pool, thread_id, None)
        .await
        .unwrap();
    assert!(turn.decided);
    assert_eq!(turn.message, "now investigate the crash");

    teardown_test_db(&db).await;
}

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

    let turn = coding_agent_turn_read(&pool, thread_id, None)
        .await
        .unwrap();
    assert_eq!(turn.message, "The background task finished.");
    assert_eq!(turn.source, OpeningSource::AgentOrEngine);

    emit(&bus, thread_id, cc, idled_event()).await;
    let retrigger = ThreadEvent::CodingAgentPromptSent {
        input_event_id: None,
        text: "Resolve the merge conflict.".into(),
        coding_agent: CodingAgent::ClaudeCode,
        origin: Some(MessageOrigin::engine(EngineReason::MergeConflict)),
    };
    emit(&bus, thread_id, cc, retrigger).await;
    emit(&bus, thread_id, cc, reply("Resolved.")).await;
    let turn = coding_agent_turn_read(&pool, thread_id, None)
        .await
        .unwrap();
    assert_eq!(turn.message, "Resolve the merge conflict.");
    assert_eq!(turn.source, OpeningSource::AgentOrEngine);

    teardown_test_db(&db).await;
}

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
        !coding_agent_turn_read(&pool, thread_id, None)
            .await
            .unwrap()
            .change_replaces_read_request
    );

    let proposed = ThreadEvent::ChangeProposed {
        content: Default::default(),
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
    let turn = coding_agent_turn_read(&pool, thread_id, None)
        .await
        .unwrap();
    assert!(turn.change_replaces_read_request);

    let judge = chat_judge(ScriptedProvider::new("m", vec![]));
    let capture = AuxCapture::new(&bus, thread_id, ContextPurpose::ReadDecision);
    let outcome = forced(&judge, &capture).decide(&turn).await;
    assert_eq!(
        record_forced_decision(&pool, &bus, thread_id, Some(&turn), outcome).await,
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

#[test]
fn a_turn_ending_in_flight_says_what_it_waits_on_and_the_end_does_not() {
    let step = TurnRead {
        source: OpeningSource::ChildThreadReport,
        in_flight: InFlight {
            sub_threads: 2,
            event_waits: 1,
            successor_session: false,
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
            successor_session: false,
        },
        ..step.clone()
    };
    assert!(read_decision_prompt(&one_child).ends_with("It waits on 1 sub-thread."));

    let end = TurnRead {
        in_flight: InFlight::default(),
        ..step
    };
    assert!(!read_decision_prompt(&end).contains("Closing note"));

    assert!(READ_DECISION_INSTRUCTIONS.contains(
        "When a closing note says the thread's work is still in flight, a progress report"
    ));
}

const PARENT_NOTE: &str =
    "The parent agent acts on a waking report now and reads a filed repeat on its next turn.";

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

    let in_flight = TurnRead {
        in_flight: InFlight {
            sub_threads: 1,
            event_waits: 0,
            successor_session: false,
        },
        ..archive_report()
    };
    let both = read_decision_prompt(&in_flight);
    assert!(both.contains("It waits on 1 sub-thread.\n\n"), "{both}");
    assert!(both.ends_with(PARENT_NOTE), "{both}");

    assert!(READ_DECISION_INSTRUCTIONS
        .contains("When a closing note says the parent agent acts on this reply now"));
}

#[test]
fn the_judge_keeps_its_parent_paragraph_byte_for_byte() {
    assert!(READ_DECISION_INSTRUCTIONS.contains(
        "When a closing note says the parent agent acts on this reply now, a routine result for \
         it is not worth the user's read. It is still worth reading when it holds a failure, a \
         gap the user must know about, or something only the user can do or decide, such as a \
         payment, a sign-in or an approval. The parent agent seeing it does not make it the \
         parent's to settle.\n\n"
    ));
    assert!(PARENT_ACTS_READ_RULE.contains(
        "a failure, a gap the user must know about, or something only the user can do or \
         decide, such as a payment, a sign-in or an approval"
    ));
}

#[test]
fn a_decision_an_open_card_already_asks_is_no_read() {
    let rule = "On its own, a decision that an open card in this workspace already asks is a no. \
                Blocked lists the thread that holds that question or permission card.";
    assert!(READ_DECISION_INSTRUCTIONS.contains(rule), "judge");
    assert!(PARENT_ACTS_READ_RULE.contains(rule), "wake child");
    assert!(PARENT_MAY_ACT_READ_RULE.contains(rule), "decide child");
}

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

#[tokio::test]
async fn integrated_read_request_is_carried_only_for_agent_turns() {
    use super::read_decision::parent_carries_read;
    use crate::engine::read_request::record_read_decision;

    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    let cc = EventChannel::ClaudeCode;
    emit(&bus, parent, cc, message("Coordinate the work")).await;
    emit(&bus, child, cc, child_message("Write the report", parent)).await;
    sqlx::query("UPDATE thread_summaries SET coding_agent_integrated = TRUE, is_coding_agent = TRUE WHERE thread_id = $1")
        .bind(child)
        .execute(&pool)
        .await
        .unwrap();

    let decision = ReadDecision::Requested;
    let turn = coding_agent_turn_read(&pool, child, None).await.unwrap();
    assert!(!turn.reply_is_the_users());
    let parent_carries =
        parent_carries_read(&pool, child, decision, async { turn.reply_is_the_users() }).await;
    assert!(parent_carries.is_integrated());
    record_read_decision(&bus, child, decision, None, parent_carries.is_carried())
        .await
        .unwrap();
    assert!(
        coding_agent_turn_read(&pool, child, None)
            .await
            .unwrap()
            .decided
    );

    emit(&bus, child, cc, idled_event()).await;
    bus.emit(BusEvent::Thread {
        thread_id: child,
        event: message("Show me the report"),
        meta: EventMeta {
            channel: Some(cc),
            actor: Some(MessageOrigin::Device {
                device_id: "test-device".into(),
            }),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    let turn = coding_agent_turn_read(&pool, child, None).await.unwrap();
    assert!(turn.reply_is_the_users());
    assert!(
        !parent_carries_read(&pool, child, decision, async { turn.reply_is_the_users() })
            .await
            .is_carried()
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

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
        permission_request: None,
    }
}

fn answered(tool_use_id: &str, answer: AnswerKind) -> ThreadEvent {
    ThreadEvent::UserQuestionAnswered {
        compose: None,
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

    assert!(READ_DECISION_INSTRUCTIONS.contains("The user answered"));
}

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

    let turn = coding_agent_turn_read(&pool, thread_id, None)
        .await
        .unwrap();
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
    let next = coding_agent_turn_read(&pool, thread_id, None)
        .await
        .unwrap();
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
            successor_session: false,
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
    let turn = coding_agent_turn_read(&pool, agent, None).await.unwrap();
    assert_eq!(
        turn.in_flight,
        InFlight {
            sub_threads: 0,
            event_waits: 1,
            successor_session: false,
        }
    );

    teardown_test_db(&db).await;
}

fn pushed(title: &str, message: &str) -> PushedNotification {
    PushedNotification {
        title: title.into(),
        message: message.into(),
    }
}

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

    assert!(READ_DECISION_INSTRUCTIONS
        .contains("When a closing note says the turn already sent the user a notification"));
}

#[test]
fn a_routine_trigger_report_is_a_no_even_when_requested() {
    assert!(
        READ_DECISION_INSTRUCTIONS.contains("A scheduled report with no material change is a no")
    );
    assert!(READ_DECISION_INSTRUCTIONS.contains("is a finding even inside a scheduled report."));
    assert!(READ_DECISION_INSTRUCTIONS
        .contains("The one exception is a failure the notification does not mention."));
}

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

async fn send_notification(bus: &EventBus, thread_id: Uuid, title: &str, success: bool) {
    let called = emit(
        bus,
        thread_id,
        EventChannel::Trigger,
        ThreadEvent::ToolCalled {
            name: crate::llm::tool_names::SEND_NOTIFICATION.into(),
            args: serde_json::json!({ "title": title, "message": format!("{title} body") }),
            description: String::new(),
        },
    )
    .await;
    emit(
        bus,
        thread_id,
        EventChannel::Trigger,
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

#[tokio::test]
async fn a_chat_turn_reads_only_the_pushes_it_delivered() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let run = Uuid::new_v4();

    send_notification(&bus, run, "earlier turn", true).await;
    let opener = emit(
        &bus,
        run,
        EventChannel::Trigger,
        message("triage the inbox"),
    )
    .await;
    send_notification(&bus, run, "failed", false).await;
    send_notification(&bus, Uuid::new_v4(), "another thread", true).await;
    send_notification(&bus, run, "delivered", true).await;

    let turn = chat_turn_read(&pool, run, opener, "One message needs you.").await;
    assert_eq!(turn.pushed, vec![pushed("delivered", "delivered body")]);

    teardown_test_db(&db).await;
}

fn unstarted(head: &str) -> SessionFacts {
    SessionFacts {
        approved_plan_unstarted: true,
        ..session(head, None)
    }
}

#[test]
fn the_approved_plan_re_entry_sends_once_and_only_after_a_completed_turn() {
    let mut ledger = ReentryLedger::default();
    let stopped = TurnEndState::at_session_idle(true, false, unstarted("a1"), None);
    assert_eq!(ledger.take_due(&stopped), None);
    assert_eq!(
        ledger.take_due(&idle(unstarted("a1"))),
        Some(DueReentry::ApprovedPlan)
    );
    assert_eq!(ledger.take_due(&idle(unstarted("b2"))), None);

    let mut asked = ReentryLedger::default();
    let due = asked.take_due(&idle(unstarted("a1"))).unwrap();
    asked.forget(due);
    assert_eq!(
        asked.take_due(&idle(unstarted("a1"))),
        Some(DueReentry::ApprovedPlan)
    );
}

#[test]
fn the_approved_plan_sits_between_the_ceiling_and_the_hold() {
    let crossing = CeilingCrossing {
        context_tokens: 300_000,
        ceiling: 200_000,
        window: None,
    };
    let facts = SessionFacts {
        held: Some(PLAN),
        ..unstarted("a1")
    };
    let state = TurnEndState::at_session_idle(true, true, facts, Some(crossing));
    assert_eq!(
        state.unmet(),
        vec![
            TurnEndRequirement::CeilingHandoff(crossing),
            TurnEndRequirement::StartApprovedPlan,
            TurnEndRequirement::Reentry(PLAN),
        ]
    );
}

#[test]
fn peeking_a_hold_agrees_with_taking_it_without_consuming_it() {
    let mut ledger = ReentryLedger::default();
    for hold in [PLAN, HARDEN] {
        assert!(ledger.would_send(hold, "a1"));
        assert!(ledger.would_send(hold, "a1"));
        assert_eq!(
            ledger.take_due(&idle(session("a1", Some(hold)))),
            Some(DueReentry::Hold(hold))
        );
        assert!(!ledger.would_send(hold, "a1"));
        assert!(ledger.would_send(hold, "b2"));
    }
    assert!(!ledger.would_send(ProposalHold::Plan(PlanHold::PlanOnlyAwaitingApproval), "a1"));
    assert!(!ledger.would_send(ProposalHold::CarriedBySuccessor, "a1"));
}

#[test]
fn only_an_unpaused_due_hold_hides_its_owner_reason() {
    let mut ledger = ReentryLedger::default();
    assert!(ledger.hold_reentry("a1", false, false).contains(PLAN));
    assert!(ledger.hold_reentry("a1", false, false).contains(HARDEN));
    assert!(!ledger.hold_reentry("a1", true, false).contains(PLAN));
    assert!(!ledger.hold_reentry("a1", true, false).contains(HARDEN));
    assert!(ledger.hold_reentry("a1", false, true).contains(PLAN));
    ledger.take_due(&idle(session("a1", Some(PLAN))));
    assert!(!ledger.hold_reentry("a1", false, false).contains(PLAN));
    assert!(ledger.hold_reentry("a1", false, false).contains(HARDEN));
    assert!(!ledger.hold_reentry("a1", false, true).contains(HARDEN));
    ledger.take_due(&idle(session("a1", Some(HARDEN))));
    assert!(!ledger.hold_reentry("a1", false, false).contains(HARDEN));
    assert!(ledger.hold_reentry("b2", false, false).contains(HARDEN));
    assert!(!ledger
        .hold_reentry("b2", false, false)
        .contains(ProposalHold::Plan(PlanHold::PlanOnlyAwaitingApproval)));
}

async fn read_requested(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT read_requested FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn opened_by(source: OpeningSource) -> TurnRead {
    TurnRead {
        source,
        ..TurnRead::default()
    }
}

fn child_opening(
    parent: Uuid,
    wake_mode: Option<crate::engine::thread_events::WakeMode>,
) -> ThreadEvent {
    let mut opening = message("report please");
    if let ThreadEvent::MessageReceived {
        parent_thread_id,
        wake_mode: mode,
        mode: actor_mode,
        ..
    } = &mut opening
    {
        *parent_thread_id = Some(parent);
        *mode = wake_mode;
        *actor_mode = ActorMode::Agent;
    }
    opening
}

async fn asked_to_be_read(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    parent: Option<Uuid>,
    wake_mode: Option<crate::engine::thread_events::WakeMode>,
    opener: OpeningSource,
) -> Uuid {
    let thread_id = Uuid::new_v4();
    let opening = match parent {
        Some(parent) => child_opening(parent, wake_mode),
        None => message("report please"),
    };
    emit(bus, thread_id, EventChannel::Chat, opening).await;
    let turn = opened_by(opener);
    record_forced_decision(
        pool,
        bus,
        thread_id,
        Some(&turn),
        Ok(ReadDecision::Requested),
    )
    .await;
    emit(
        bus,
        thread_id,
        EventChannel::Chat,
        reply("Blocked on the shell."),
    )
    .await;
    thread_id
}

async fn decide_no(pool: &sqlx::PgPool, bus: &EventBus, thread_id: Uuid) {
    let turn = opened_by(OpeningSource::AgentOrEngine);
    record_forced_decision(
        pool,
        bus,
        thread_id,
        Some(&turn),
        Ok(ReadDecision::NotRequested),
    )
    .await;
}

#[tokio::test]
async fn a_later_no_clears_an_unseen_request_only_where_the_parent_acts() {
    use crate::engine::thread_events::WakeMode;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = Uuid::new_v4();
    emit(&bus, parent, EventChannel::Chat, message("run the program")).await;

    let cases = [
        (Some(parent), Some(WakeMode::Wake), false),
        (Some(parent), Some(WakeMode::WakeWhenAllDone), false),
        (Some(parent), Some(WakeMode::NoWake), true),
        (Some(parent), None, true),
        (None, None, true),
    ];
    for (parent_id, wake_mode, still_requested) in cases {
        let opener = OpeningSource::ChildThreadReport;
        let thread_id = asked_to_be_read(&pool, &bus, parent_id, wake_mode, opener).await;
        decide_no(&pool, &bus, thread_id).await;
        assert_eq!(
            read_requested(&pool, thread_id).await,
            still_requested,
            "parent {parent_id:?}, wake mode {wake_mode:?}"
        );
    }

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_later_no_leaves_a_request_from_a_turn_the_user_opened() {
    use crate::engine::thread_events::WakeMode;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = Uuid::new_v4();
    emit(&bus, parent, EventChannel::Chat, message("run the program")).await;

    let opener = OpeningSource::UserMessage;
    let users = asked_to_be_read(&pool, &bus, Some(parent), Some(WakeMode::Wake), opener).await;
    decide_no(&pool, &bus, users).await;
    assert!(read_requested(&pool, users).await);

    let unread = Uuid::new_v4();
    emit(&bus, unread, EventChannel::Chat, message("report please")).await;
    record_forced_decision(&pool, &bus, unread, None, Err("unreadable".into())).await;
    decide_no(&pool, &bus, unread).await;
    assert!(read_requested(&pool, unread).await);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_parent_yes_over_an_unseen_user_yes_keeps_it_for_a_later_no() {
    use crate::engine::thread_events::WakeMode;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = Uuid::new_v4();
    emit(&bus, parent, EventChannel::Chat, message("run the program")).await;

    let opener = OpeningSource::UserMessage;
    let child = asked_to_be_read(&pool, &bus, Some(parent), Some(WakeMode::Wake), opener).await;
    let parents_turn = opened_by(OpeningSource::ChildThreadReport);
    record_forced_decision(
        &pool,
        &bus,
        child,
        Some(&parents_turn),
        Ok(ReadDecision::Requested),
    )
    .await;
    decide_no(&pool, &bus, child).await;
    assert!(read_requested(&pool, child).await);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_later_no_leaves_a_request_from_a_turn_the_user_wrote_into() {
    use crate::engine::thread_events::WakeMode;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = Uuid::new_v4();
    emit(&bus, parent, EventChannel::Chat, message("run the program")).await;

    let child = Uuid::new_v4();
    let opening = child_opening(parent, Some(WakeMode::Wake));
    let opener = emit(&bus, child, EventChannel::Chat, opening).await;
    let follow_up = child_opening(parent, Some(WakeMode::Wake));
    emit(&bus, child, EventChannel::Chat, follow_up).await;
    let parents_turn = chat_turn_read(&pool, child, opener, "").await;
    assert!(!parents_turn.reply_is_the_users());

    let question = message("what did you find?");
    emit(&bus, child, EventChannel::Chat, question).await;
    let turn = chat_turn_read(&pool, child, opener, "Found it.").await;
    assert!(turn.user_wrote_in_turn);
    record_forced_decision(&pool, &bus, child, Some(&turn), Ok(ReadDecision::Requested)).await;
    emit(&bus, child, EventChannel::Chat, reply("Found it.")).await;
    decide_no(&pool, &bus, child).await;
    assert!(read_requested(&pool, child).await);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_coding_agent_turn_counts_only_human_input_as_the_users() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let cc = EventChannel::ClaudeCode;
    let parent = Uuid::new_v4();
    emit(&bus, parent, EventChannel::Chat, message("run the program")).await;
    let child = Uuid::new_v4();
    emit(&bus, child, cc, child_opening(parent, None)).await;
    start_cc_session(&bus, child, "claude-code/test", None).await;

    let follow_up = emit(&bus, child, cc, child_opening(parent, None)).await;
    let forwarded = ThreadEvent::CodingAgentPromptSent {
        text: "report please".into(),
        coding_agent: CodingAgent::ClaudeCode,
        origin: None,
        input_event_id: Some(follow_up),
    };
    emit(&bus, child, cc, forwarded).await;
    let parents_turn = coding_agent_turn_read(&pool, child, None).await.unwrap();
    assert!(!parents_turn.user_wrote_in_turn);

    let mid_turn = ThreadEvent::PromptInjected {
        text: "what did you find?".into(),
        mode: ActorMode::Human,
        origin: None,
        injected_message_id: None,
        delivered_event_id: None,
    };
    emit(&bus, child, cc, mid_turn).await;
    let turn = coding_agent_turn_read(&pool, child, None).await.unwrap();
    assert!(turn.user_wrote_in_turn);

    teardown_test_db(&db).await;
}
