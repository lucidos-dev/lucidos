use std::sync::Arc;
use uuid::Uuid;

use super::{ENGINE_RESTART_INTERRUPT_REASON, USER_CLICKED_CONTINUE_REASON};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

#[tokio::test]
async fn continuation_requested_emission_produces_a_spawn_request() {
    use crate::engine::agent_session::spawn_dispatcher::{SpawnDispatcher, SpawnRequest};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

    let thread_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    crate::test_support::start_cc_session(&bus, thread_id, "claude-code/cont", None).await;

    let res = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinuationRequested {
                reason: USER_CLICKED_CONTINUE_REASON.to_string(),
            },
            meta: cc_meta,
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");

    let received = tokio::time::timeout(std::time::Duration::from_secs(5), spawn_rx.recv())
        .await
        .expect("dispatcher must produce a SpawnRequest")
        .expect("channel must yield");
    assert_eq!(
        received,
        SpawnRequest::Continue {
            thread_id,
            event_id: res.event_id,
        },
        "ContinuationRequested from the continue endpoint must produce SpawnRequest::Continue"
    );

    handle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn synthetic_idled_with_engine_restart_interrupt_does_not_dispatch() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut requests = bus.take_spawn_requests().expect("first take");

    let thread_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    crate::test_support::start_cc_session(&bus, thread_id, "claude-code/int", None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("sid-int".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: Some(ENGINE_RESTART_INTERRUPT_REASON.to_string()),
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        },
        meta: cc_meta,
    })
    .await
    .expect("emit succeeds")
    .expect("persisted");

    assert!(
        requests.try_recv().is_err(),
        "synthetic CodingAgentIdled must not produce a spawn request"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn continuation_retry_input_recaps_the_thread_before_the_continue_message() {
    use crate::engine::agent_recovery::{continue_retry_input, CONTINUE_RESUME_USER_MESSAGE};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    crate::test_support::start_cc_session(&bus, thread_id, "claude-code/retry", None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentUserMessageSent {
            text: "fix the flaky drafts spec".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("persisted");
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentTextStreamed {
            text: "Traced it to the debounced compose PUT.".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            parent_tool_use_id: None,
            progress_note: false,
        },
        meta: cc_meta,
    })
    .await
    .expect("emit succeeds")
    .expect("persisted");

    let input = continue_retry_input(
        &pool,
        std::path::Path::new("/nonexistent"),
        thread_id,
        Some(USER_CLICKED_CONTINUE_REASON),
    )
    .await;

    let recap_pos = input
        .find("fix the flaky drafts spec")
        .expect("prior user turn must be recapped into the retry input");
    let brief = crate::engine::successor::handoff_brief(
        thread_id,
        crate::engine::successor::BriefCause::ResumeFailed,
        true,
        None,
    );
    let brief_pos = input
        .find(&brief)
        .expect("a failed resume opens on a handoff brief");
    assert!(
        recap_pos < brief_pos,
        "the recap must come FIRST so the agent reads what happened before being told to continue"
    );
    assert!(input.ends_with(CONTINUE_RESUME_USER_MESSAGE));
    assert!(
        input.contains("Traced it to the debounced compose PUT."),
        "prior assistant turn missing from the retry input"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn continuation_retry_input_is_never_empty() {
    use crate::engine::agent_recovery::{continue_retry_input, CONTINUE_RESUME_USER_MESSAGE};

    let (pool, db_name) = setup_test_db().await;
    let thread_id = Uuid::new_v4();

    let input = continue_retry_input(
        &pool,
        std::path::Path::new("/nonexistent"),
        thread_id,
        Some(USER_CLICKED_CONTINUE_REASON),
    )
    .await;

    assert_eq!(
        input,
        crate::engine::pending_handoff::successor_first_message(
            &crate::engine::successor::handoff_brief(
                thread_id,
                crate::engine::successor::BriefCause::ResumeFailed,
                true,
                None,
            ),
            Some(CONTINUE_RESUME_USER_MESSAGE),
            CONTINUE_RESUME_USER_MESSAGE,
        ),
        "with nothing to recap the retry input is the brief alone"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn seed_typed_answer(bus: &EventBus, thread_id: Uuid, question: &str, typed: &str) {
    use crate::engine::thread_events::{AnswerKind, QuestionOption};

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-test".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("SessionStarted emit")
    .expect("SessionStarted persisted");

    let tool_use_id = "toolu_vrtx_01XT#q0";
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.into(),
            cc_session_id: "sid-test".into(),
            question: question.into(),
            options: vec![
                QuestionOption {
                    id: "opt-0".into(),
                    label: "Approve".into(),
                    description: None,
                    preview: None,
                    widget: None,
                    quiet: false,
                    quiet_reply: None,
                },
                QuestionOption {
                    id: "opt-1".into(),
                    label: "Derive the deadline instead".into(),
                    description: None,
                    preview: None,
                    widget: None,
                    quiet: false,
                    quiet_reply: None,
                },
            ],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            permission_request: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("UserQuestionAsked emit")
    .expect("UserQuestionAsked persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: tool_use_id.into(),
            quiet: false,
            answer: AnswerKind::FreeText {
                text: typed.into(),
                image_hashes: vec![],
            },
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("UserQuestionAnswered emit")
    .expect("UserQuestionAnswered persisted");
}

#[tokio::test]
async fn answered_after_idle_resume_carries_the_answer_and_denies_the_rejection() {
    use crate::engine::agent_recovery::{continue_input_for_reason, ANSWERED_AFTER_IDLE_REASON};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let question = "Approve this plan, or take the narrower deadline variant?";
    let typed = "think hard, do we really need this or is it overengineering?";
    seed_typed_answer(&bus, thread_id, question, typed).await;

    let input = continue_input_for_reason(
        &pool,
        std::path::Path::new("/nonexistent"),
        thread_id,
        Some(ANSWERED_AFTER_IDLE_REASON),
    )
    .await;

    assert!(
        input.contains(typed),
        "the resume must carry the user's answer verbatim: {input}"
    );
    assert!(
        input.contains(question),
        "the resume must say which question it answers: {input}"
    );

    assert!(
        input.contains("picks none of them"),
        "a typed reply must be marked as selecting no option: {input}"
    );

    assert!(
        input.contains("the user declined nothing"),
        "the resume must contradict the transcript's rejection stamp: {input}"
    );
    assert!(
        input.contains("did not approve anything"),
        "the resume must also refuse the opposite reading: {input}"
    );

    assert!(
        input.contains("Do not re-ask the same question unless the answer left it open"),
        "the resume must block a re-ask of an answered question only: {input}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn other_continuation_reasons_still_send_the_bare_continue_message() {
    use crate::engine::agent_recovery::{
        continue_input_for_reason, AUTO_RECOVERY_AFTER_HANG_REASON,
        AUTO_RESUME_AFTER_API_ERROR_REASON, AUTO_RESUME_AFTER_SWITCH_REASON,
        CONTINUE_RESUME_USER_MESSAGE, USER_CLICKED_CONTINUE_REASON,
    };

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_typed_answer(&bus, thread_id, "Which shape?", "neither").await;

    for reason in [
        Some(USER_CLICKED_CONTINUE_REASON),
        Some(AUTO_RECOVERY_AFTER_HANG_REASON),
        Some(AUTO_RESUME_AFTER_API_ERROR_REASON),
        Some(AUTO_RESUME_AFTER_SWITCH_REASON),
        Some("some_future_reason"),
        None,
    ] {
        assert_eq!(
            continue_input_for_reason(
                &pool,
                std::path::Path::new("/nonexistent"),
                thread_id,
                reason
            )
            .await,
            CONTINUE_RESUME_USER_MESSAGE,
            "reason {reason:?} must not start carrying question text"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn answered_after_idle_falls_back_to_the_bare_message_with_nothing_to_recap() {
    use crate::engine::agent_recovery::{
        continue_input_for_reason, ANSWERED_AFTER_IDLE_REASON, CONTINUE_RESUME_USER_MESSAGE,
    };

    let (pool, db_name) = setup_test_db().await;
    let thread_id = Uuid::new_v4();

    assert_eq!(
        continue_input_for_reason(
            &pool,
            std::path::Path::new("/nonexistent"),
            thread_id,
            Some(ANSWERED_AFTER_IDLE_REASON)
        )
        .await,
        CONTINUE_RESUME_USER_MESSAGE,
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_stale_resume_retry_also_carries_the_answer() {
    use crate::engine::agent_recovery::{continue_retry_input, ANSWERED_AFTER_IDLE_REASON};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let typed = "no, keep the deadline required";
    seed_typed_answer(&bus, thread_id, "Approve, or derive the deadline?", typed).await;

    let input = continue_retry_input(
        &pool,
        std::path::Path::new("/nonexistent"),
        thread_id,
        Some(ANSWERED_AFTER_IDLE_REASON),
    )
    .await;

    assert!(
        input.contains(typed),
        "the stale-resume retry must carry the answer too: {input}"
    );
    assert!(
        input.contains("the user declined nothing"),
        "and the framing that goes with it: {input}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_harden_request_resumes_with_the_harden_message() {
    use crate::engine::agent_recovery::{continue_input_for_reason, HARDEN_REQUESTED_REASON};

    let (pool, db_name) = setup_test_db().await;
    assert_eq!(
        continue_input_for_reason(
            &pool,
            std::path::Path::new("/nonexistent"),
            Uuid::new_v4(),
            Some(HARDEN_REQUESTED_REASON)
        )
        .await,
        crate::engine::continuation_reason::AUTO_HARDEN_MESSAGE,
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_start_failure_leaves_its_message_owed_to_the_next_continue() {
    use crate::engine::thread_events::{CodingAgentExit, CodingAgentExitStage};
    use crate::engine::undelivered_input::undelivered_input;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    crate::test_support::start_cc_session(&bus, thread_id, "claude-code/owed", None).await;

    assert!(
        undelivered_input(&pool, thread_id).await.is_none(),
        "a thread that never failed owes nothing"
    );

    let received = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: crate::test_support::message_received(
                "Make the OpenRouter calls use prompt caching.",
            ),
            meta: cc_meta.clone(),
        })
        .await
        .expect("emit succeeds")
        .expect("persisted");
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseFailed {
            error: "Couldn't load settings from Cloud gateway https://claude-gateway.example.com."
                .into(),
            spend_cap: None,
            agent_exit: Some(CodingAgentExit {
                stage: CodingAgentExitStage::Startup,
                code: Some(1),
            }),
            provider_error: None,
        },
        meta: EventMeta {
            request_event_id: Some(received.event_id),
            ..cc_meta
        },
    })
    .await
    .expect("emit succeeds")
    .expect("persisted");

    let owed = undelivered_input(&pool, thread_id)
        .await
        .expect("the message the agent never read is owed");
    assert_eq!(owed.text, "Make the OpenRouter calls use prompt caching.");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn aborted(cause: crate::engine::thread_events::AbortCause) -> ThreadEvent {
    ThreadEvent::ResponseAborted {
        text: String::new(),
        images: Vec::new(),
        model: None,
        reasoning_effort: None,
        cause,
    }
}

async fn seed_unread_engine_prompt_after_switch(
    bus: &EventBus,
    thread_id: Uuid,
    prompt: &str,
) -> Uuid {
    use crate::engine::thread_events::{AbortCause, ActorMode, MessageOrigin};

    let mut engine_prompt = crate::test_support::message_received(prompt);
    if let ThreadEvent::MessageReceived { mode, .. } = &mut engine_prompt {
        *mode = ActorMode::Engine;
    }
    let prompt_id = emit_cc(bus, thread_id, engine_prompt).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: aborted(AbortCause::EngineShutdown),
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            actor: Some(MessageOrigin::Device {
                device_id: "d1".into(),
            }),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("emit succeeds");
    emit_cc(bus, thread_id, aborted(AbortCause::RecoveryAfterRestart)).await;
    prompt_id
}

#[tokio::test]
async fn a_thread_whose_agent_read_nothing_gets_its_engine_prompt_on_every_resume() {
    use crate::engine::agent_recovery::{
        AUTO_RESUME_AFTER_SWITCH_REASON, USER_CLICKED_CONTINUE_REASON,
    };
    use crate::engine::undelivered_input::resume_input;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let prompt_id =
        seed_unread_engine_prompt_after_switch(&bus, thread_id, "Fix main: browser failed").await;

    for reason in [
        AUTO_RESUME_AFTER_SWITCH_REASON,
        USER_CLICKED_CONTINUE_REASON,
    ] {
        let owed = resume_input(&pool, thread_id, Some(reason))
            .await
            .unwrap_or_else(|| panic!("the unread prompt is owed to a `{reason}` resume"));
        assert_eq!(owed.text, "Fix main: browser failed");
        assert_eq!(owed.event_ids, vec![prompt_id]);
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_thread_that_started_but_read_nothing_gets_its_prompt_on_every_resume() {
    use crate::engine::agent_recovery::{
        AUTO_RESUME_AFTER_SWITCH_REASON, USER_CLICKED_CONTINUE_REASON,
    };
    use crate::engine::undelivered_input::resume_input;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    crate::test_support::start_cc_session(&bus, thread_id, "claude-code/started", None).await;
    let prompt_id =
        seed_unread_engine_prompt_after_switch(&bus, thread_id, "Fix main: browser failed").await;

    for reason in [
        AUTO_RESUME_AFTER_SWITCH_REASON,
        USER_CLICKED_CONTINUE_REASON,
    ] {
        let owed = resume_input(&pool, thread_id, Some(reason))
            .await
            .unwrap_or_else(|| panic!("the unread prompt is owed to a `{reason}` resume"));
        assert_eq!(owed.event_ids, vec![prompt_id]);
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_thread_whose_agent_read_its_prompt_keeps_the_bare_continue() {
    use crate::engine::agent_recovery::{
        AUTO_RESUME_AFTER_SWITCH_REASON, USER_CLICKED_CONTINUE_REASON,
    };
    use crate::engine::undelivered_input::resume_input;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    crate::test_support::start_cc_session(&bus, thread_id, "claude-code/read", None).await;
    let prompt_id =
        seed_unread_engine_prompt_after_switch(&bus, thread_id, "Fix main: browser failed").await;
    emit_cc(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentInputRead {
            input_event_id: prompt_id,
            started_turn: true,
            verbatim: false,
        },
    )
    .await;

    for reason in [
        AUTO_RESUME_AFTER_SWITCH_REASON,
        USER_CLICKED_CONTINUE_REASON,
    ] {
        assert!(
            resume_input(&pool, thread_id, Some(reason)).await.is_none(),
            "an agent that read its prompt resumes its session, so `{reason}` replays nothing"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_thread_whose_agent_produced_output_keeps_the_bare_continue() {
    use crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON;
    use crate::engine::undelivered_input::resume_input;

    let outputs = [
        ThreadEvent::CodingAgentTextStreamed {
            text: "on it".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            parent_tool_use_id: None,
            progress_note: false,
        },
        ThreadEvent::CodingAgentThoughtStreamed {
            text: "thinking".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
    ];
    for output in outputs {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_unread_engine_prompt_after_switch(&bus, thread_id, "Fix main").await;
        emit_cc(&bus, thread_id, output).await;

        assert!(
            resume_input(&pool, thread_id, Some(AUTO_RESUME_AFTER_SWITCH_REASON))
                .await
                .is_none(),
            "an agent that produced output was told its task"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

#[tokio::test]
async fn a_message_the_user_took_back_is_not_replayed() {
    use crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON;
    use crate::engine::undelivered_input::resume_input;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let kept = emit_cc(
        &bus,
        thread_id,
        crate::test_support::message_received("keep"),
    )
    .await;
    let removed = emit_cc(
        &bus,
        thread_id,
        crate::test_support::message_received("drop"),
    )
    .await;
    emit_cc(
        &bus,
        thread_id,
        ThreadEvent::QueuedMessageRemoved {
            removed_message_id: removed,
            restore_to_compose: false,
            compose: None,
        },
    )
    .await;

    let owed = resume_input(&pool, thread_id, Some(AUTO_RESUME_AFTER_SWITCH_REASON))
        .await
        .expect("the kept prompt is owed");
    assert_eq!(owed.event_ids, vec![kept]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn emit_cc(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) -> Uuid {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("emit succeeds")
    .expect("persisted")
    .event_id
}

async fn effort_pick_discards_the_first_prompt(bus: &EventBus, thread_id: Uuid) -> (Uuid, Uuid) {
    use crate::engine::thread_events::{emit_response_canceled, CancelCause};

    crate::test_support::start_cc_session(bus, thread_id, "claude-code/effort", None).await;
    let prompt = emit_cc(bus, thread_id, crate::test_support::message_received("Hey")).await;
    let follow_up = emit_cc(
        bus,
        thread_id,
        crate::test_support::message_received("and also this"),
    )
    .await;
    emit_cc(
        bus,
        thread_id,
        ThreadEvent::CodingAgentPromptSent {
            text: "and also this".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            origin: None,
            input_event_id: Some(follow_up),
        },
    )
    .await;
    emit_response_canceled(
        bus,
        bus.pool(),
        thread_id,
        CancelCause::EffortRestart,
        String::new(),
        vec![],
        None,
        None,
        EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
        "[test] ResponseCanceled",
    )
    .await;
    emit_cc(
        bus,
        thread_id,
        ThreadEvent::CodingAgentInputDiscarded {
            input_event_id: prompt,
        },
    )
    .await;
    (prompt, follow_up)
}

async fn stranded_ids(pool: &sqlx::PgPool, thread_id: Uuid, first_prompt: &[Uuid]) -> Vec<Uuid> {
    crate::engine::agent_session::stranded_follow_ups::stranded_follow_ups(
        pool,
        std::path::Path::new("/nonexistent"),
        thread_id,
        first_prompt,
    )
    .await
    .expect("the stranded query runs")
    .into_iter()
    .map(|follow_up| follow_up.event_id)
    .collect()
}

#[tokio::test]
async fn the_next_session_delivers_an_older_discarded_prompt() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let (prompt, follow_up) = effort_pick_discards_the_first_prompt(&bus, thread_id).await;
    let newer = emit_cc(
        &bus,
        thread_id,
        crate::test_support::message_received("are you there?"),
    )
    .await;

    assert_eq!(
        stranded_ids(&pool, thread_id, &[newer]).await,
        [prompt, follow_up],
        "the new message's session delivers the discarded prompt, oldest first"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn effort_pick_discards_a_child_wake(bus: &EventBus, thread_id: Uuid) -> Uuid {
    use crate::engine::thread_events::ChildCompletionStatus;

    crate::test_support::start_cc_session(bus, thread_id, "claude-code/effort", None).await;
    let wake = emit_cc(
        bus,
        thread_id,
        ThreadEvent::ChildThreadCompleted {
            child_thread_id: Uuid::new_v4(),
            child_thread_title: Some("child".into()),
            status: ChildCompletionStatus::Success,
            summary: "child did the thing".into(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
            integrated_branch: None,
        },
    )
    .await;
    emit_cc(
        bus,
        thread_id,
        ThreadEvent::CodingAgentInputDiscarded {
            input_event_id: wake,
        },
    )
    .await;
    emit_cc(
        bus,
        thread_id,
        ThreadEvent::ResponseGenerated {
            text: "done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;
    wake
}

#[tokio::test]
async fn an_older_discarded_child_wake_stays_owed_until_read() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let wake = effort_pick_discards_a_child_wake(&bus, thread_id).await;
    let next = Uuid::new_v4();

    assert_eq!(stranded_ids(&pool, thread_id, &[next]).await, [wake]);

    emit_cc(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentInputRead {
            input_event_id: wake,
            started_turn: true,
            verbatim: false,
        },
    )
    .await;
    assert!(
        stranded_ids(&pool, thread_id, &[next]).await.is_empty(),
        "a read closes the discard"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_discarded_continuation_or_keep_alive_leaves_nothing_owed() {
    use crate::engine::thread_events::{ActorMode, EngineReason, MessageOrigin};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    crate::test_support::start_cc_session(&bus, thread_id, "claude-code/effort", None).await;
    let continuation = emit_cc(
        &bus,
        thread_id,
        ThreadEvent::ContinuationRequested {
            reason: crate::engine::continuation_reason::EFFORT_RESTART_REASON.into(),
        },
    )
    .await;
    let keep_alive = emit_cc(
        &bus,
        thread_id,
        ThreadEvent::PromptInjected {
            text: "keep warm".into(),
            mode: ActorMode::Engine,
            origin: Some(MessageOrigin::engine(EngineReason::CacheKeepAlive)),
            injected_message_id: None,
            delivered_event_id: None,
        },
    )
    .await;
    for input_event_id in [continuation, keep_alive] {
        emit_cc(
            &bus,
            thread_id,
            ThreadEvent::CodingAgentInputDiscarded { input_event_id },
        )
        .await;
    }

    assert!(stranded_ids(&pool, thread_id, &[Uuid::new_v4()])
        .await
        .is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}
