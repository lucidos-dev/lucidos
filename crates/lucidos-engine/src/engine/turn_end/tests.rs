//! The turn-end gate: each test pins one invariant of
//! `docs/plans/2026-10-10-turn-end-gate-enforces-valid-states.md`.

use super::read_decision::{coding_agent_turn_read, record_forced_decision, ForcedReadDecision};
use super::*;
use crate::engine::aux_purpose::AuxCall;
use crate::engine::change_ops::PlanHold;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::model_call::AuxCapture;
use crate::engine::read_request::ReadDecision;
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::engine::ContextPurpose;
use crate::llm::mock::{MockProvider, MOCK_MODEL, MOCK_READ_YES_SENTINEL};
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

fn forced<'a>(call: &'a AuxCall, capture: &'a AuxCapture) -> ForcedReadDecision<'a> {
    ForcedReadDecision { call, capture }
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
        decide.decide("thanks", "You're welcome.").await.unwrap(),
        ReadDecision::NotRequested
    );
    let yes = format!("research it {MOCK_READ_YES_SENTINEL}");
    assert_eq!(
        decide
            .decide(&yes, "Findings: three causes.")
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
        forced(&call, &capture).decide("go", "  ").await.unwrap(),
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
        .decide("report please", "Here it is.")
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
    let idled: ThreadEvent =
        serde_json::from_value(serde_json::json!({ "type": "CodingAgentIdled", "data": {} }))
            .unwrap();
    emit(&bus, thread_id, cc, idled).await;
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
