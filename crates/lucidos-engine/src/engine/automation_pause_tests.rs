use super::*;
use crate::engine::permission::PermissionChoice;
use crate::test_support::{seed_chat_thread, seed_coding_thread, setup_test_db, teardown_test_db};

async fn pause_events(pool: &PgPool, thread_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT event_type FROM events WHERE thread_id = $1 \
         AND event_type IN ('AutomationPaused', 'AutomationUnpaused') \
         ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// The pause lives only in events, so a fresh read after a restart sees the
/// same state, and an unpause ends it.
#[tokio::test]
async fn the_pause_folds_from_events_and_survives_a_restart() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    assert_eq!(automation_pause(&pool, thread).await.unwrap(), None);
    assert!(!automation_is_paused(&pool, thread).await);

    let paused = pause_automation(&pool, &bus, thread, "  checkpoint and pause  ", None)
        .await
        .unwrap();
    assert_eq!(paused, PauseChange::Recorded);

    let (restarted, _rx) = EventBus::new(pool.clone());
    drop(restarted);
    let pause = automation_pause(&pool, thread)
        .await
        .unwrap()
        .expect("paused");
    assert_eq!(pause.reason, "checkpoint and pause");
    assert!(automation_is_paused(&pool, thread).await);

    let unpaused = unpause_automation(
        &pool,
        &bus,
        thread,
        AutomationUnpauseCause::Unpause,
        device("phone"),
    )
    .await
    .unwrap();
    assert_eq!(unpaused, PauseChange::Recorded);
    assert_eq!(automation_pause(&pool, thread).await.unwrap(), None);
    assert_eq!(
        pause_events(&pool, thread).await,
        ["AutomationPaused", "AutomationUnpaused"]
    );
    teardown_test_db(&db_name).await;
}

/// A repeat records nothing: the first reason stands, and unpausing a thread
/// with no pause is a no-op.
#[tokio::test]
async fn repeats_record_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;

    let not_paused =
        unpause_automation(&pool, &bus, thread, AutomationUnpauseCause::Archived, None)
            .await
            .unwrap();
    assert_eq!(not_paused, PauseChange::Unchanged);

    pause_automation(&pool, &bus, thread, "first", None)
        .await
        .unwrap();
    let again = pause_automation(&pool, &bus, thread, "second", None)
        .await
        .unwrap();
    assert_eq!(again, PauseChange::Unchanged);
    assert_eq!(
        automation_pause(&pool, thread)
            .await
            .unwrap()
            .unwrap()
            .reason,
        "first"
    );
    assert_eq!(pause_events(&pool, thread).await, ["AutomationPaused"]);
    teardown_test_db(&db_name).await;
}

/// A chat thread has no automatic steps to pause, a blank reason says
/// nothing, and an unknown thread is refused. None records an event.
#[tokio::test]
async fn refusals_record_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let chat = seed_chat_thread(&bus, "hello").await;
    let coding = seed_coding_thread(&bus, None).await;
    let unknown = Uuid::new_v4();

    let refused = pause_automation(&pool, &bus, chat, "pause", None).await;
    assert!(matches!(refused, Err(PauseError::Refused(why)) if why.contains("coding-agent")));
    let blank = pause_automation(&pool, &bus, coding, "   ", None).await;
    assert!(matches!(blank, Err(PauseError::Refused(why)) if why.contains("reason")));
    let missing = pause_automation(&pool, &bus, unknown, "pause", None).await;
    assert!(matches!(missing, Err(PauseError::NotFound)));

    assert!(matches!(
        require_pausable_thread(&pool, chat).await,
        Err(PauseError::Refused(_))
    ));
    assert!(matches!(
        require_pausable_thread(&pool, unknown).await,
        Err(PauseError::NotFound)
    ));
    require_pausable_thread(&pool, coding).await.unwrap();

    for thread in [chat, coding, unknown] {
        assert!(pause_events(&pool, thread).await.is_empty());
    }
    teardown_test_db(&db_name).await;
}

/// A database that cannot answer is our failure, never the caller's refusal.
#[tokio::test]
async fn an_unreadable_database_is_internal() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let coding = seed_coding_thread(&bus, None).await;
    pool.close().await;

    let paused = pause_automation(&pool, &bus, coding, "pause", None).await;
    assert!(matches!(paused, Err(PauseError::Internal(_))), "{paused:?}");
    let checked = require_pausable_thread(&pool, coding).await;
    assert!(
        matches!(checked, Err(PauseError::Internal(_))),
        "{checked:?}"
    );
    teardown_test_db(&db_name).await;
}

const BACKENDS: [crate::runtime::CodingAgent; 2] = [
    crate::runtime::CodingAgent::ClaudeCode,
    crate::runtime::CodingAgent::Codex,
];

/// A coding-agent thread with a session on `backend`.
async fn seed_on(bus: &EventBus, backend: crate::runtime::CodingAgent) -> Uuid {
    let thread_id = seed_coding_thread(bus, None).await;
    crate::test_support::seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: backend,
            session_id: "test-session".into(),
            branch: format!("claude-code/{thread_id}"),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
    )
    .await;
    thread_id
}

include!("automation_pause_test_actor.rs");
include!("automation_pause_test_queries.rs");

/// Without a pause every step runs as it always did, and nobody is asked
/// (I8).
#[tokio::test]
async fn an_unpaused_step_runs_and_asks_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for backend in BACKENDS {
        let thread = seed_on(&bus, backend).await;
        for step in Action::ENGINE_STEPS {
            assert_eq!(ask_before_step(&bus, thread, step).await, StepVerdict::Run);
        }
        assert!(step_requests(&pool, thread).await.is_empty());
    }
    teardown_test_db(&db_name).await;
}

/// A paused thread's step asks instead of running, on either backend (I1).
/// The engine words every option, a step coming due again joins its open
/// request, and the open request makes the thread wait on its owner.
#[tokio::test]
async fn a_paused_step_raises_one_engine_worded_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for backend in BACKENDS {
        let thread = seed_on(&bus, backend).await;
        pause_automation(&pool, &bus, thread, "checkpoint and pause", None)
            .await
            .unwrap();
        for step in Action::ENGINE_STEPS {
            assert_eq!(
                ask_before_step(&bus, thread, step).await,
                StepVerdict::Asked
            );
            assert_eq!(
                ask_before_step(&bus, thread, step).await,
                StepVerdict::Asked
            );
        }
        let requests = step_requests(&pool, thread).await;
        let actions: Vec<&str> = requests.iter().map(|(a, _)| a.as_str()).collect();
        assert_eq!(
            actions,
            [
                "harden",
                "set-plan-marker",
                "build-approved-plan",
                "hand-off",
                "read-waiting-reports"
            ]
        );
        for (_, labels) in &requests {
            assert_eq!(labels.len(), 3);
            assert_eq!(&labels[1..], ["Unpause", "Keep paused"]);
        }
        assert_eq!(requests[0].1[0], "Continue hardening");
        assert_eq!(status(&pool, thread).await, "waiting_for_user_answer");
    }
    teardown_test_db(&db_name).await;
}

/// The Unpause option ends the pause, as the owner's act. An agent's pick of
/// it changes nothing (I3).
#[tokio::test]
async fn only_the_owner_unpauses_through_a_request() {
    use crate::engine::permission::{card_request, resolve_answer, PickedBy, Resolution};
    use crate::engine::thread_events::AnswerKind;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    pause_automation(&pool, &bus, thread, "pause", None)
        .await
        .unwrap();
    ask_before_step(&bus, thread, Action::Harden).await;
    let card = open_step_request(&pool, thread, Action::Harden)
        .await
        .unwrap()
        .expect("an open request");
    let request = card_request(&pool, thread, &card).await.unwrap().unwrap();
    let unpause_id = request
        .choices
        .iter()
        .find(|(_, choice)| **choice == PermissionChoice::Unpause)
        .map(|(id, _)| id.clone())
        .unwrap();
    let pick = AnswerKind::Selected {
        option_id: unpause_id,
    };
    let agent = crate::engine::thread_events::agent_tool_actor(Uuid::new_v4());
    let by_agent = resolve_answer(
        &bus,
        thread,
        &card,
        &request,
        &pick,
        Some(&agent),
        PickedBy::Tap,
    )
    .await
    .unwrap();
    assert_eq!(by_agent, Resolution::Ignored);
    assert!(automation_is_paused(&pool, thread).await);

    let owner = device("phone").unwrap();
    let by_owner = resolve_answer(
        &bus,
        thread,
        &card,
        &request,
        &pick,
        Some(&owner),
        PickedBy::Tap,
    )
    .await
    .unwrap();
    assert_eq!(by_owner, Resolution::Unpaused);
    assert!(!automation_is_paused(&pool, thread).await);
    teardown_test_db(&db_name).await;
}

/// A nudge queued before the pause is held back at the forward (I2). One
/// queued after it, a message, and an unpaused thread all pass.
#[tokio::test]
async fn a_nudge_queued_before_the_pause_is_held_at_the_forward() {
    use crate::engine::thread_events::{ActorMode, EngineReason};
    use crate::engine::{AgentInputKind, AgentUserInput};
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for backend in BACKENDS {
        let thread = seed_on(&bus, backend).await;
        let nudge = |reason: EngineReason| ThreadEvent::PromptInjected {
            text: "nudge".into(),
            mode: ActorMode::Engine,
            origin: Some(MessageOrigin::engine(reason)),
            injected_message_id: None,
            delivered_event_id: None,
        };
        let input = |anchor: Uuid, kind: AgentInputKind| AgentUserInput {
            text: "nudge".into(),
            images: None,
            origin_event_id: Some(anchor),
            kind,
            completion: None,
        };
        let seed = |reason| crate::test_support::seed_thread_event(&bus, thread, nudge(reason));
        let harden = seed(EngineReason::MissingHardening).await;
        let plan = seed(EngineReason::MissingPlan).await;
        let approved = seed(EngineReason::ApprovedPlanUnstarted).await;
        let ceiling = seed(EngineReason::ContextCeiling).await;
        let engine_kind = AgentInputKind::ReentryFromEngine;
        assert_eq!(
            withheld_queued_step(&pool, thread, &input(harden, engine_kind)).await,
            None,
            "nothing is held without a pause"
        );

        pause_automation(&pool, &bus, thread, "pause", None)
            .await
            .unwrap();
        assert_eq!(
            withheld_queued_step(&pool, thread, &input(harden, engine_kind)).await,
            Some(Action::Harden)
        );
        assert_eq!(
            withheld_queued_step(&pool, thread, &input(plan, engine_kind)).await,
            Some(Action::SetPlanMarker)
        );
        assert_eq!(
            withheld_queued_step(&pool, thread, &input(approved, engine_kind)).await,
            Some(Action::BuildApprovedPlan)
        );
        assert_eq!(
            withheld_queued_step(
                &pool,
                thread,
                &input(ceiling, AgentInputKind::CeilingReentry)
            )
            .await,
            Some(Action::HandOff)
        );
        assert_eq!(
            withheld_queued_step(&pool, thread, &input(harden, AgentInputKind::User)).await,
            None,
            "a message is never held"
        );
        let after = seed(EngineReason::MissingHardening).await;
        assert_eq!(
            withheld_queued_step(&pool, thread, &input(after, engine_kind)).await,
            None,
            "a nudge sent after the pause was allowed"
        );
    }
    teardown_test_db(&db_name).await;
}

/// A wait delivery the pause withheld stays owed until the agent reads it.
/// One from before the pause is not the pause's to hold.
#[tokio::test]
async fn withheld_wait_reentries_are_the_unread_ones_after_the_pause() {
    use crate::engine::thread_events::{ActorMode, EngineReason, EventWaitOutcome};
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let delivery = |text: &str| ThreadEvent::PromptInjected {
        text: text.into(),
        mode: ActorMode::Agent,
        origin: Some(MessageOrigin::engine(EngineReason::EventWait {
            outcome: EventWaitOutcome::Delivered,
            watched: vec![],
            wait_reason: "build".into(),
        })),
        injected_message_id: None,
        delivered_event_id: None,
    };
    crate::test_support::seed_thread_event(&bus, thread, delivery("before")).await;
    pause_automation(&pool, &bus, thread, "pause", None)
        .await
        .unwrap();
    let read = crate::test_support::seed_thread_event(&bus, thread, delivery("read")).await;
    let owed = crate::test_support::seed_thread_event(&bus, thread, delivery("owed")).await;
    crate::test_support::seed_thread_event(
        &bus,
        thread,
        ThreadEvent::CodingAgentInputRead {
            input_event_id: read,
            started_turn: false,
            verbatim: false,
        },
    )
    .await;
    let withheld = withheld_wait_reentries(&pool, thread).await.unwrap();
    let ids: Vec<Uuid> = withheld.iter().map(|r| r.anchor_event_id).collect();
    assert_eq!(ids, [owed]);
    assert_eq!(withheld[0].text, "owed");
    teardown_test_db(&db_name).await;
}

/// The SQL that finds a wait re-entry's anchor names the serde tag.
#[test]
fn the_event_wait_kind_is_the_serde_tag() {
    use crate::engine::thread_events::{EngineReason, EventWaitOutcome, EVENT_WAIT_REASON_KIND};
    let reason = EngineReason::EventWait {
        outcome: EventWaitOutcome::Delivered,
        watched: vec![],
        wait_reason: String::new(),
    };
    assert_eq!(
        serde_json::to_value(reason).unwrap()["kind"],
        EVENT_WAIT_REASON_KIND
    );
}

/// Both backends share the run loop, so these two gates cover both (I1, I2).
/// The idle asks before it sends a re-entry, and the forward re-checks a
/// queued one before the agent reads it.
#[test]
fn the_run_loop_asks_before_each_re_entry() {
    const RUN_SRC: &str =
        include_str!("../../../lucidos-agents-api/src/engine/agent_session/run_session/run.rs");
    let due = RUN_SRC
        .find("reentry_ledger.take_due(&gate)")
        .expect("the idle still takes the due re-entry");
    let asked = RUN_SRC[due..]
        .find("ask_before_step(&self.event_bus, thread_id, due.step())")
        .expect("the idle asks before a paused thread's re-entry");
    let sent = RUN_SRC[due..]
        .find("nudge_held_proposal_in_session(thread_id, hold, &msg_tx)")
        .expect("the idle still sends the hold's re-entry");
    assert!(asked < sent, "the ask comes before the send");

    let forward = RUN_SRC
        .find("Some(mut user_input) = msg_rx.recv()")
        .expect("the forward arm");
    let held = RUN_SRC[forward..]
        .find("withheld_queued_step(self.pool(), thread_id, &user_input)")
        .expect("the forward re-checks a queued nudge");
    let forwarded = RUN_SRC[forward..]
        .find("forward_ceiling_input(reentry,")
        .expect("the forward still delivers the ceiling re-entry");
    assert!(held < forwarded, "a held nudge never reaches the agent");
}
