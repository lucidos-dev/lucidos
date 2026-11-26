use crate::engine::chat::process::ChatProcessExt;
use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::engine::types::{AgentSession, AgentUserInput};

fn make_test_session(
    process_exited: bool,
) -> (AgentSession, mpsc::UnboundedReceiver<AgentUserInput>) {
    let (mut session, msg_rx) = AgentSession::for_test();
    session.is_waiting = !process_exited;
    session.process_exited = process_exited;
    (session, msg_rx)
}

use super::should_redirect_followup;
use crate::runtime::CodingAgent;

#[test]
fn redirect_fires_for_codex_user_followup_mid_turn() {
    assert!(should_redirect_followup(
        CodingAgent::Codex,
        true,
        true,
        false,
        false
    ));
    assert!(should_redirect_followup(
        CodingAgent::Codex,
        true,
        true,
        true,
        false
    ));
}

#[test]
fn redirect_skips_codex_when_idle() {
    assert!(!should_redirect_followup(
        CodingAgent::Codex,
        false,
        true,
        false,
        false
    ));
    assert!(!should_redirect_followup(
        CodingAgent::Codex,
        false,
        true,
        true,
        false
    ));
}

#[test]
fn redirect_skips_codex_child_wake() {
    assert!(!should_redirect_followup(
        CodingAgent::Codex,
        true,
        false,
        false,
        false
    ));
    assert!(!should_redirect_followup(
        CodingAgent::Codex,
        true,
        false,
        true,
        false
    ));
}

#[test]
fn redirect_skips_claude_code_unless_urgent() {
    assert!(!should_redirect_followup(
        CodingAgent::ClaudeCode,
        true,
        true,
        false,
        false
    ));
}

#[test]
fn redirect_fires_for_urgent_claude_code_followup_mid_turn() {
    assert!(should_redirect_followup(
        CodingAgent::ClaudeCode,
        true,
        true,
        true,
        false
    ));
}

#[test]
fn redirect_skips_idle_claude_code_even_when_urgent() {
    assert!(!should_redirect_followup(
        CodingAgent::ClaudeCode,
        false,
        true,
        true,
        false
    ));
}

#[test]
fn redirect_skips_urgent_claude_code_child_wake() {
    assert!(!should_redirect_followup(
        CodingAgent::ClaudeCode,
        true,
        false,
        true,
        false
    ));
}

use super::arm_followup_redirect;

fn codex_in_flight_session() -> (AgentSession, mpsc::UnboundedReceiver<AgentUserInput>) {
    let (mut s, msg_rx) = make_test_session(false);
    s.is_waiting = false;
    s.coding_agent = CodingAgent::Codex;
    s.can_steer = false;
    (s, msg_rx)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arm_redirect_fires_for_codex_mid_turn_user_followup() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let (session, _msg_rx) = codex_in_flight_session();
    let interrupt = session.interrupt.clone();
    sessions.insert(thread_id, session);

    let idle = arm_followup_redirect(&mut sessions, thread_id, true, false, &None);

    assert!(
        idle.is_some(),
        "a Codex mid-turn user follow-up must arm the redirect and return idle_notify"
    );
    assert_eq!(
        sessions.get(&thread_id).unwrap().redirect_cause,
        Some(crate::engine::thread_events::CancelCause::SupersededByFollowup),
        "arming the redirect must flag the session so the interrupt arm classifies \
         the interrupted turn as SupersededByFollowup (neutral), not UserStop"
    );
    assert!(
        sessions.get(&thread_id).unwrap().redirect_followup_pending,
        "arming must also reserve the subprocess: the caller routes the message only \
         after the interrupted turn reaches a boundary, so at the idle decision the \
         message provably is not in msg_rx yet and this flag is the only thing \
         standing between it and a terminated subprocess"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), interrupt.notified())
            .await
            .is_ok(),
        "the live turn's interrupt must have been fired"
    );
}

fn claude_code_in_flight_session() -> (AgentSession, mpsc::UnboundedReceiver<AgentUserInput>) {
    let (mut s, msg_rx) = make_test_session(false);
    s.is_waiting = false;
    debug_assert_eq!(s.coding_agent, CodingAgent::ClaudeCode);
    (s, msg_rx)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arm_redirect_skips_plain_claude_code_mid_turn() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let (session, _msg_rx) = claude_code_in_flight_session();
    let interrupt = session.interrupt.clone();
    sessions.insert(thread_id, session);

    let idle = arm_followup_redirect(&mut sessions, thread_id, true, false, &None);

    assert!(
        idle.is_none(),
        "a plain (non-urgent) CC follow-up must not be interrupted: it steers via stdin"
    );
    assert!(
        sessions.get(&thread_id).unwrap().redirect_cause.is_none(),
        "a plain CC follow-up must not be flagged for redirect"
    );
    assert!(
        !sessions.get(&thread_id).unwrap().redirect_followup_pending,
        "no interrupt means no reservation: the message goes straight into msg_rx, \
         where the idle decision can see it for itself"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), interrupt.notified())
            .await
            .is_err(),
        "no interrupt should have been fired for a plain CC follow-up"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arm_redirect_never_interrupts_the_ceiling_reentry() {
    for (mut session, _msg_rx) in [codex_in_flight_session(), claude_code_in_flight_session()] {
        let thread_id = Uuid::new_v4();
        session.in_ceiling_reentry = true;
        let mut sessions = HashMap::from([(thread_id, session)]);
        let idle = arm_followup_redirect(&mut sessions, thread_id, true, true, &None);
        assert!(idle.is_none());
        assert!(!sessions[&thread_id].redirect_followup_pending);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arm_redirect_fires_for_urgent_claude_code_mid_turn() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let (session, _msg_rx) = claude_code_in_flight_session();
    let interrupt = session.interrupt.clone();
    sessions.insert(thread_id, session);

    let idle = arm_followup_redirect(&mut sessions, thread_id, true, true, &None);

    assert!(
        idle.is_some(),
        "an urgent CC mid-turn follow-up must arm the redirect and return idle_notify"
    );
    assert_eq!(
        sessions.get(&thread_id).unwrap().redirect_cause,
        Some(crate::engine::thread_events::CancelCause::SupersededByFollowup),
        "the interrupted turn must classify as SupersededByFollowup (neutral), not UserStop"
    );
    assert!(
        sessions.get(&thread_id).unwrap().redirect_followup_pending,
        "the interrupted turn's idle must keep the subprocess alive for the reservation"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), interrupt.notified())
            .await
            .is_ok(),
        "the live CC turn's interrupt must have been fired"
    );
}

#[test]
fn arm_redirect_skips_idle_claude_code_even_when_urgent() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let (session, _msg_rx) = make_test_session(false);
    sessions.insert(thread_id, session);

    assert!(
        arm_followup_redirect(&mut sessions, thread_id, true, true, &None).is_none(),
        "an idle CC session reads stdin at once: there is no turn to interrupt"
    );
    assert!(!sessions.get(&thread_id).unwrap().redirect_followup_pending);
}

#[test]
fn arm_redirect_skips_idle_codex() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let (mut session, _msg_rx) = make_test_session(false);
    session.coding_agent = CodingAgent::Codex;
    sessions.insert(thread_id, session);

    assert!(arm_followup_redirect(&mut sessions, thread_id, true, false, &None).is_none());
    assert!(!sessions.get(&thread_id).unwrap().redirect_followup_pending);
}

#[test]
fn arm_redirect_skips_child_wake() {
    let thread_id = Uuid::new_v4();
    let mut sessions = HashMap::new();
    let (session, _msg_rx) = codex_in_flight_session();
    sessions.insert(thread_id, session);

    assert!(arm_followup_redirect(&mut sessions, thread_id, false, false, &None).is_none());
}

#[test]
fn arm_redirect_none_when_no_session() {
    let mut sessions = HashMap::new();
    assert!(arm_followup_redirect(&mut sessions, Uuid::new_v4(), true, false, &None).is_none());
}

#[test]
fn a_redirect_armed_inside_the_result_arm_does_not_outlive_its_turn() {
    let thread_id = Uuid::new_v4();
    let (session, _msg_rx) = codex_in_flight_session();
    let mut sessions = HashMap::from([(thread_id, session)]);
    let origin = Some(crate::engine::thread_events::MessageOrigin::Device {
        device_id: "d-1".into(),
    });

    assert!(arm_followup_redirect(&mut sessions, thread_id, true, false, &origin).is_some());

    let s = sessions.get_mut(&thread_id).unwrap();
    crate::engine::agent_session::lifecycle::mark_turn_boundary(s);
    assert!(
        std::mem::take(&mut s.redirect_followup_pending),
        "the follow-up promise survives to the idle decision, which keeps the subprocess"
    );

    assert!(
        s.redirect_cause.is_none(),
        "the next real Stop would render as superseded"
    );
    assert_eq!(
        s.cancel_actor, None,
        "the next Stop would inherit this device"
    );
    assert!(
        futures::FutureExt::now_or_never(s.interrupt.notified()).is_none(),
        "the stored permit would interrupt the follow-up's own turn"
    );
}

use super::follow_up_keeps_open_question;
use crate::engine::agent_question::aq_test_helpers::{cc_meta, emit_user_question, seed_cc_thread};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::ThreadEvent;
use crate::engine::PreEmittedOrigin;
use crate::test_support::{setup_test_db, teardown_test_db};

async fn seed_parked_question(bus: &EventBus) -> Uuid {
    let thread_id = Uuid::new_v4();
    seed_cc_thread(bus, thread_id).await;
    emit_user_question(bus, thread_id, "toolu-parked#q0").await;
    thread_id
}

fn live_sessions(thread_id: Uuid) -> (TokioSessions, mpsc::UnboundedReceiver<AgentUserInput>) {
    let (session, msg_rx) = make_test_session(false);
    let sessions = tokio::sync::Mutex::new(HashMap::from([(thread_id, session)]));
    (sessions, msg_rx)
}

type TokioSessions = tokio::sync::Mutex<HashMap<Uuid, AgentSession>>;

#[tokio::test]
async fn an_engine_reentry_keeps_a_live_question_open() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_parked_question(&bus).await;
    let (sessions, _msg_rx) = live_sessions(thread_id);

    for origin in [
        PreEmittedOrigin::EngineReentry(Uuid::new_v4()),
        PreEmittedOrigin::WaitReentry(Uuid::new_v4()),
    ] {
        assert!(
            follow_up_keeps_open_question(&pool, &sessions, thread_id, Some(origin)).await,
            "{origin:?} must not supersede a question the user can still answer"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_message_still_supersedes_the_question() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_parked_question(&bus).await;
    let (sessions, _msg_rx) = live_sessions(thread_id);

    for origin in [None, Some(PreEmittedOrigin::Message(Uuid::new_v4()))] {
        assert!(
            !follow_up_keeps_open_question(&pool, &sessions, thread_id, origin).await,
            "{origin:?} must supersede, or its prompt kills the card and deadlocks the agent"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_engine_reentry_supersedes_an_overtaken_question() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_parked_question(&bus).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentToolCalled {
            name: "Bash".into(),
            args: serde_json::json!({"command": "ls"}),
            description: String::new(),
            coding_agent: CodingAgent::ClaudeCode,
            tool_use_id: "toolu-sibling".into(),
            parent_tool_use_id: None,
            api_call_id: None,
        },
        meta: cc_meta(),
    })
    .await
    .expect("CodingAgentToolCalled emit")
    .expect("CodingAgentToolCalled persisted");
    let (sessions, _msg_rx) = live_sessions(thread_id);

    let origin = Some(PreEmittedOrigin::EngineReentry(Uuid::new_v4()));
    assert!(!follow_up_keeps_open_question(&pool, &sessions, thread_id, origin).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_engine_reentry_supersedes_when_no_session_is_live() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_parked_question(&bus).await;
    let (exited, _msg_rx) = make_test_session(true);

    for sessions in [
        TokioSessions::default(),
        tokio::sync::Mutex::new(HashMap::from([(thread_id, exited)])),
    ] {
        let origin = Some(PreEmittedOrigin::EngineReentry(Uuid::new_v4()));
        assert!(!follow_up_keeps_open_question(&pool, &sessions, thread_id, origin).await);
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

use super::{chat_follow_up_supersedes_cards, reentry_reaches_live_session};
use crate::engine::thread_events::ActorMode;

#[tokio::test]
async fn an_engine_reentry_on_a_live_session_keeps_the_permission_card() {
    let thread_id = Uuid::new_v4();
    let (sessions, _msg_rx) = live_sessions(thread_id);
    for origin in [
        PreEmittedOrigin::EngineReentry(Uuid::new_v4()),
        PreEmittedOrigin::WaitReentry(Uuid::new_v4()),
    ] {
        assert!(reentry_reaches_live_session(&sessions, thread_id, Some(origin)).await);
    }
}

#[tokio::test]
async fn a_message_or_a_dead_session_still_supersedes_the_permission_card() {
    let thread_id = Uuid::new_v4();
    let (sessions, _msg_rx) = live_sessions(thread_id);
    for origin in [None, Some(PreEmittedOrigin::Message(Uuid::new_v4()))] {
        assert!(!reentry_reaches_live_session(&sessions, thread_id, origin).await);
    }
    let (exited, _exited_rx) = make_test_session(true);
    let reentry = Some(PreEmittedOrigin::EngineReentry(Uuid::new_v4()));
    for sessions in [
        TokioSessions::default(),
        tokio::sync::Mutex::new(HashMap::from([(thread_id, exited)])),
    ] {
        assert!(!reentry_reaches_live_session(&sessions, thread_id, reentry).await);
    }
}

#[test]
fn only_a_person_denies_a_card_a_live_chat_turn_waits_on() {
    assert!(chat_follow_up_supersedes_cards(ActorMode::Human, true));
    assert!(!chat_follow_up_supersedes_cards(ActorMode::Agent, true));
    assert!(!chat_follow_up_supersedes_cards(ActorMode::Engine, true));
    for mode in [ActorMode::Human, ActorMode::Agent, ActorMode::Engine] {
        assert!(chat_follow_up_supersedes_cards(mode, false));
    }
}

#[test]
fn capable_codex_steers_plain_messages_but_interrupts_urgent_messages() {
    assert!(!should_redirect_followup(
        CodingAgent::Codex,
        true,
        true,
        false,
        true
    ));
    assert!(should_redirect_followup(
        CodingAgent::Codex,
        true,
        true,
        true,
        true
    ));
}

#[tokio::test]
async fn a_plain_followup_preserves_a_capable_codex_turn() {
    let thread = Uuid::new_v4();
    let (mut session, _rx) = codex_in_flight_session();
    session.can_steer = true;
    let mut sessions = HashMap::from([(thread, session)]);
    assert!(arm_followup_redirect(&mut sessions, thread, true, false, &None).is_none());
    let session = &sessions[&thread];
    assert!(session.redirect_cause.is_none());
    assert!(!session.redirect_followup_pending);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), session.interrupt.notified())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_human_side_request_routes_into_the_live_codex_turn() {
    use crate::engine::chat::process::OrphanLane;
    use crate::engine::thread_events::ActorMode;
    use crate::engine::SessionPhase;
    use crate::test_support::{
        boot_mock_engine, in_engine_child, seed_coding_thread, seed_thread_event, setup_test_db,
        teardown_test_db,
    };
    if !in_engine_child("engine::chat::process_helpers::process_helpers_tests::a_human_side_request_routes_into_the_live_codex_turn").await {
        return;
    }
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let thread = seed_coding_thread(&engine.event_bus, None).await;
    let mut claim = engine
        .claim_thread_for_spawn(thread, None)
        .await
        .unwrap()
        .unwrap();
    let (control, _control_rx) = mpsc::unbounded_channel();
    {
        let mut sessions = engine.agent_sessions.lock().await;
        let session = sessions.get_mut(&thread).unwrap();
        session.phase = SessionPhase::Running {
            control_tx: control,
        };
        session.coding_agent = CodingAgent::Codex;
        session.can_steer = true;
        session.is_waiting = false;
    }
    let input = seed_thread_event(
        &engine.event_bus,
        thread,
        crate::test_support::message_received("Show me the colors too"),
    )
    .await;
    engine
        .process_orphan(
            "Show me the colors too",
            None,
            thread,
            None,
            ActorMode::Human,
            Some(PreEmittedOrigin::Message(input)),
            OrphanLane::CodingAgent(Vec::new()),
        )
        .await
        .unwrap();
    let forwarded = claim.entry.msg_rx.try_recv().unwrap();
    assert_eq!(forwarded.text, "Show me the colors too");
    assert_eq!(forwarded.origin_event_id, Some(input));
    assert!(claim.entry.msg_rx.try_recv().is_err());
    let sessions = engine.agent_sessions.lock().await;
    let session = &sessions[&thread];
    assert!(session.redirect_cause.is_none());
    assert!(!session.redirect_followup_pending);
    drop(sessions);
    let canceled: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'ResponseCanceled'",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        canceled, 0,
        "a side request must preserve the original turn"
    );
    drop(claim);
    teardown_test_db(&db).await;
}

#[test]
fn a_run_still_starting_is_never_interrupted_by_a_followup() {
    let thread_id = Uuid::new_v4();
    let (mut session, _msg_rx) = codex_in_flight_session();
    session.phase = crate::engine::SessionPhase::Starting {
        startup_cancel: tokio_util::sync::CancellationToken::new(),
    };
    let mut sessions = HashMap::from([(thread_id, session)]);

    let idle = arm_followup_redirect(&mut sessions, thread_id, true, true, &None);

    assert!(idle.is_none());
    assert!(!sessions[&thread_id].redirect_followup_pending);
    assert!(sessions[&thread_id].redirect_cause.is_none());
}

#[test]
fn the_follow_up_wait_cap_outlasts_the_interrupt_hard_kill() {
    const RUN_SRC: &str = include_str!("../agent_session/run_session/run.rs");
    let marker =
        "const INTERRUPT_ESCALATE_AFTER: std::time::Duration = std::time::Duration::from_secs(";
    let start = RUN_SRC.find(marker).expect("the escalation constant") + marker.len();
    let end = start + RUN_SRC[start..].find(')').expect("closing paren");
    let escalate_secs: u64 = RUN_SRC[start..end].parse().expect("whole seconds");

    assert!(
        super::REDIRECT_INTERRUPT_MAX_WAIT.as_secs() > escalate_secs,
        "the hard-kill must land before the follow-up stops waiting"
    );
}

#[test]
fn a_trigger_knowhow_description_is_capped_like_the_workspace_list() {
    use crate::engine::chat::process::workspace_prompt_footprint::KNOWHOW_DESCRIPTION_MAX_CHARS;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp
        .path()
        .join("nightly-build")
        .join("knowhow")
        .join("long.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let long = "routing ".repeat(500);
    std::fs::write(
        &path,
        format!("---\nname: Long\ndescription: {long}\n---\nBody."),
    )
    .unwrap();

    let section = super::build_trigger_knowhow_section(tmp.path(), "nightly-build");

    let line = section
        .lines()
        .find(|l| l.contains("triggers/nightly-build/long"))
        .expect("the listed row");
    assert!(
        line.chars().count() <= KNOWHOW_DESCRIPTION_MAX_CHARS + 80,
        "row is {} chars",
        line.chars().count()
    );
}
