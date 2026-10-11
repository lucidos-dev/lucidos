//! The turn-end gate: each test pins one invariant of
//! `docs/plans/2026-10-10-turn-end-gate-enforces-valid-states.md`.

use super::read_decision::{
    chat_turn_read, coding_agent_turn_read, read_decision_prompt, record_forced_decision,
    ForcedReadDecision, InFlight, OpeningSource, TurnRead, READ_DECISION_SYSTEM_PROMPT,
};
use super::*;
use crate::engine::aux_purpose::AuxCall;
use crate::engine::change_ops::PlanHold;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::model_call::AuxCapture;
use crate::engine::read_request::ReadDecision;
use crate::engine::thread_events::{
    ActorMode, EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::engine::ContextPurpose;
use crate::llm::mock::{MockProvider, MOCK_MODEL, MOCK_READ_YES_SENTINEL};
use crate::runtime::agent_runtime::CodingAgent;
use crate::test_support::{
    aux_captures, setup_test_db, start_cc_session, teardown_test_db, ScriptedProvider,
};
use std::sync::Arc;
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

fn forced<'a>(call: &'a AuxCall, capture: &'a AuxCapture) -> ForcedReadDecision<'a> {
    ForcedReadDecision { call, capture }
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

    let call = AuxCall::over(
        Arc::new(MockProvider::new(MOCK_MODEL.to_string())),
        ContextPurpose::ReadDecision,
    );
    let capture = AuxCapture::new(&bus, thread_id, ContextPurpose::ReadDecision);
    let decide = forced(&call, &capture);
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
        "every forced call records what it cost"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_empty_reply_is_a_no_without_a_call() {
    let call = AuxCall::over(
        Arc::new(ScriptedProvider::new("m", vec![])),
        ContextPurpose::ReadDecision,
    );
    let capture = AuxCapture::discarding(ContextPurpose::ReadDecision);
    assert_eq!(
        forced(&call, &capture)
            .decide(&turn("go", "  "))
            .await
            .unwrap(),
        ReadDecision::NotRequested
    );
}

/// A call that fails or answers in text records a read request in the
/// engine's name. The reply then waits in Review rather than hiding.
#[tokio::test]
async fn a_failed_forced_call_records_a_read_request_from_the_engine() {
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

    let call = AuxCall::over(
        Arc::new(ScriptedProvider::new("m", vec!["I think yes."])),
        ContextPurpose::ReadDecision,
    );
    let capture = AuxCapture::discarding(ContextPurpose::ReadDecision);
    let outcome = forced(&call, &capture)
        .decide(&turn("report please", "Here it is."))
        .await;
    assert!(outcome.is_err(), "a text answer is no decision");
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

    let call = AuxCall::over(
        Arc::new(ScriptedProvider::new("m", vec![])),
        ContextPurpose::ReadDecision,
    );
    let capture = AuxCapture::new(&bus, thread_id, ContextPurpose::ReadDecision);
    let outcome = forced(&call, &capture).decide(&turn).await;
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
    assert!(READ_DECISION_SYSTEM_PROMPT.contains(
        "When a closing note says the thread's work is still in flight, a progress report"
    ));
}

fn child_message(text: &str, parent: Uuid) -> ThreadEvent {
    ThreadEvent::MessageReceived {
        provider: None,
        voice_session_id: None,
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
