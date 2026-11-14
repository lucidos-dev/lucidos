use super::*;
use crate::test_support::{seed_chat_thread, seed_coding_thread, setup_test_db, teardown_test_db};

fn device(id: &str) -> Option<MessageOrigin> {
    Some(MessageOrigin::Device {
        device_id: id.into(),
    })
}

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

const STEPS: [Action; 4] = [
    Action::Harden,
    Action::SetPlanMarker,
    Action::HandOff,
    Action::ReadWaitingReports,
];

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

/// Every step request on the thread: its action and its option labels.
async fn step_requests(pool: &PgPool, thread_id: Uuid) -> Vec<(String, Vec<String>)> {
    let rows: Vec<(Option<String>, serde_json::Value)> = sqlx::query_as(
        "SELECT payload->'permission_request'->>'action', payload->'options' FROM events \
         WHERE thread_id = $1 AND event_type = 'UserQuestionAsked' ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .unwrap();
    rows.into_iter()
        .map(|(action, options)| {
            let labels = options
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o["label"].as_str().unwrap().to_string())
                .collect();
            (action.unwrap_or_default(), labels)
        })
        .collect()
}

async fn status(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Without a pause every step runs as it always did, and nobody is asked
/// (I8).
#[tokio::test]
async fn an_unpaused_step_runs_and_asks_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for backend in BACKENDS {
        let thread = seed_on(&bus, backend).await;
        for step in STEPS {
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
        for step in STEPS {
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
    let agent = crate::engine::tools::agent_tool_actor(Uuid::new_v4());
    let by_agent = resolve_answer(
        &bus,
        std::path::Path::new("/nonexistent/grants"),
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
        std::path::Path::new("/nonexistent/grants"),
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
    const RUN_SRC: &str = include_str!("agent_session/run_session/run.rs");
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
        .find("ceiling_reentry_forwarded = true")
        .expect("the forward still marks the ceiling re-entry");
    assert!(held < forwarded, "a held nudge never reaches the agent");
}

/// The engine's answer path, end to end (I1, I3, I6). The step once grants
/// it once, spends it and resumes the thread with the step, still paused.
/// Keep paused does nothing. A follow-up never supersedes a request. Unpause
/// ends the pause and runs its step plus every other open request's step.
#[tokio::test]
async fn an_answered_step_request_runs_the_step_it_allows() {
    use crate::engine::agent_question::{answer_pending_question, AnswerResult};
    use crate::engine::permission::card_request;
    use crate::engine::thread_events::AnswerKind;
    use crate::{llm::ProviderBuildContext, test_support::*};
    use std::sync::Arc;

    const CHILD: &str = "LUCIDOS_AUTOMATION_PAUSE_ENGINE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let home = tempfile::tempdir().unwrap();
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            tokio::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "engine::automation_pause::tests::an_answered_step_request_runs_the_step_it_allows",
                    "--nocapture",
                ])
                .env("RUST_MIN_STACK", "16777216")
                .env(CHILD, "1")
                .env("HOME", home.path())
                .env_remove("LUCIDOS_GATEWAY_DATA")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let engine = Arc::new(
        crate::engine::LucidosEngine::new(
            workspace.path().to_path_buf(),
            &test_db_url(&db),
            ProviderBuildContext {
                default_model: "mock".into(),
                model_is_mock: true,
                vertex_project_id: String::new(),
                vertex_location: crate::llm::vertex::location_handle(String::new()),
                vertex_token_cache: None,
                model_registry: Default::default(),
                not_served: Default::default(),
                claude_gateway_access: None,
                chatgpt_plan_access: None,
            },
        )
        .await
        .unwrap(),
    );
    engine.set_self_arc(&engine);
    let bus = &engine.event_bus;
    let thread = seed_coding_thread(bus, None).await;
    seed_thread_event(
        bus,
        thread,
        ThreadEvent::ProposalWithheld {
            branch_name: format!("claude-code/{thread}"),
            files: vec!["a.txt".into()],
            reason: crate::engine::thread_events::UnproposedReason::PlanMissing,
        },
    )
    .await;
    pause_automation(&pool, bus, thread, "pause", None)
        .await
        .unwrap();

    let answer = |step: Action, pick: fn(&PermissionChoice) -> bool| {
        let engine = engine.clone();
        let pool = pool.clone();
        async move {
            ask_before_step(&engine.event_bus, thread, step).await;
            let card = open_step_request(&pool, thread, step)
                .await
                .unwrap()
                .expect("an open request");
            let request = card_request(&pool, thread, &card).await.unwrap().unwrap();
            let option_id = request
                .choices
                .iter()
                .find(|(_, choice)| pick(choice))
                .map(|(id, _)| id.clone())
                .unwrap();
            let result = answer_pending_question(
                &engine,
                thread,
                card,
                AnswerKind::Selected { option_id },
                device("phone"),
            )
            .await;
            assert!(matches!(result, AnswerResult::Resumed), "{step:?}");
        }
    };
    let resumes = || async {
        let mut reasons: Vec<String> = Vec::new();
        for _ in 0..50 {
            reasons = sqlx::query_scalar(
                "SELECT payload->>'reason' FROM events \
                 WHERE thread_id = $1 AND event_type = 'ContinuationRequested' ORDER BY sequence",
            )
            .bind(thread)
            .fetch_all(&pool)
            .await
            .unwrap();
            if !reasons.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        reasons
    };
    let count = |event_type: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = $2",
            )
            .bind(thread)
            .bind(event_type)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    answer(Action::Harden, |c| matches!(c, PermissionChoice::Refuse)).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        count("PermissionGranted").await,
        0,
        "Keep paused grants nothing"
    );
    assert_eq!(count("ContinuationRequested").await, 0);

    answer(Action::Harden, |c| {
        matches!(c, PermissionChoice::Grant { .. })
    })
    .await;
    assert_eq!(resumes().await, ["harden_requested"]);
    assert_eq!(count("PermissionGranted").await, 1);
    assert_eq!(count("PermissionSpent").await, 1, "the once grant is spent");
    assert!(automation_is_paused(&pool, thread).await, "still paused");

    ask_before_step(bus, thread, Action::Harden).await;
    let harden = open_step_request(&pool, thread, Action::Harden)
        .await
        .unwrap()
        .expect("a second harden request");
    let superseded = crate::engine::agent_question::resolve_pending_question_as_superseded(
        &engine,
        thread,
        device("phone"),
    )
    .await;
    assert!(!superseded, "a follow-up leaves a permission request open");
    assert_eq!(
        open_step_request(&pool, thread, Action::Harden)
            .await
            .unwrap(),
        Some(harden)
    );

    answer(Action::SetPlanMarker, |c| *c == PermissionChoice::Unpause).await;
    for _ in 0..50 {
        if count("ContinuationRequested").await == 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        resumes().await,
        ["harden_requested", "plan_requested", "harden_requested"],
        "Unpause also runs the step the other open request asked for"
    );
    assert_eq!(
        open_step_request(&pool, thread, Action::Harden)
            .await
            .unwrap(),
        None,
        "and closes that request"
    );
    assert!(!automation_is_paused(&pool, thread).await);

    pool.close().await;
    teardown_test_db(&db).await;
}
