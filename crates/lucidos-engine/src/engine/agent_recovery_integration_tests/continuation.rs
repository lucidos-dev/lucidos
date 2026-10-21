use std::sync::Arc;
use uuid::Uuid;

use super::{ENGINE_RESTART_INTERRUPT_REASON, USER_CLICKED_CONTINUE_REASON};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

/// The continue endpoint's contract: a `ContinuationRequested` on the CC
/// channel persists with the user's reason, and the spawn dispatcher forwards
/// it as a `SpawnRequest::Continue`. Exercises "endpoint emits, bus sends,
/// dispatcher forwards" without the Axum router, which needs a whole engine.
#[tokio::test]
async fn continuation_requested_emission_produces_a_spawn_request() {
    use crate::engine::agent_session::spawn_dispatcher::{SpawnDispatcher, SpawnRequest};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

    let thread_id = Uuid::new_v4();
    // Seed SessionStarted so the lifecycle contract accepts CC events.
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    crate::test_support::start_cc_session(&bus, thread_id, "claude-code/cont", None).await;

    // What the continue endpoint does:
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

/// Recovery's synthetic `CodingAgentIdled { engine_restart_interrupt }` must
/// never become a spawn request. Otherwise the event that surfaces the
/// Continue affordance would loop back into auto-spawning the agent.
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

    // Simulate recovery surfacing the interrupt:
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
        },
        meta: cc_meta,
    })
    .await
    .expect("emit succeeds")
    .expect("persisted");

    // The bus sends in PostCommit, before `emit` returns, so an empty channel
    // here is final.
    assert!(
        requests.try_recv().is_err(),
        "synthetic CodingAgentIdled must not produce a spawn request"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The stale-resume retry of a continuation runs with NO resume sid — the one
/// we had is dead — so the fresh subprocess starts with zero context. Its input
/// must therefore carry the reconstructed conversation ahead of
/// `CONTINUE_RESUME_USER_MESSAGE`, or the agent is told to "continue from where
/// you left off" with no idea where that was. Same contract the chat path's
/// retry has always had (`chat::process_cc`).
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
    let continue_pos = input
        .find(CONTINUE_RESUME_USER_MESSAGE)
        .expect("retry input must still carry the continue message");
    assert!(
        recap_pos < continue_pos,
        "the recap must come FIRST so the agent reads what happened before being told to continue"
    );
    assert!(
        input.contains("Traced it to the debounced compose PUT."),
        "prior assistant turn missing from the retry input"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A thread with no reconstructable history still yields a usable retry input:
/// the bare continue message, never an empty string. An empty stdin parks
/// `claude --print --resume` forever waiting for input — the exact hang
/// `CONTINUE_RESUME_USER_MESSAGE` exists to prevent.
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
        input, CONTINUE_RESUME_USER_MESSAGE,
        "with nothing to recap the retry input must fall back to the bare continue message"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ---------------------------------------------------------------------------
// continue_input_for_reason: what the resumed agent is actually told.
//
// Regression suite for 2026-08-10 (thread 728de3cc). A question answered after
// the subprocess was torn down used to resume with nothing but "Continue from
// where you left off." The answer reached the model nowhere, and Claude Code's
// own transcript had already closed the tool call as "the user doesn't want to
// proceed", so the model announced "you declined the card and said continue,
// so I am treating that as approval" and started implementing.
// ---------------------------------------------------------------------------

/// Seed a coding-agent thread parked on a question the user answered by typing,
/// the exact shape of the reproduction.
async fn seed_typed_answer(bus: &EventBus, thread_id: Uuid, question: &str, typed: &str) {
    use crate::engine::thread_events::{AnswerKind, QuestionOption};

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
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
                },
                QuestionOption {
                    id: "opt-1".into(),
                    label: "Derive the deadline instead".into(),
                    description: None,
                    preview: None,
                    widget: None,
                },
            ],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
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
            tool_use_id: tool_use_id.into(),
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

    // 1. The answer reaches the model at all. This is the whole bug.
    assert!(
        input.contains(typed),
        "the resume must carry the user's answer verbatim: {input}"
    );
    assert!(
        input.contains(question),
        "the resume must say which question it answers: {input}"
    );

    // 2. A typed reply is not a selection, so it cannot be read as "Approve".
    assert!(
        input.contains("picks none of them"),
        "a typed reply must be marked as selecting no option: {input}"
    );

    // 3. The teardown stamp is disarmed in both directions: the user declined
    //    nothing, and equally approved nothing.
    assert!(
        input.contains("the user declined nothing"),
        "the resume must contradict the transcript's rejection stamp: {input}"
    );
    assert!(
        input.contains("did not approve anything"),
        "the resume must also refuse the opposite reading: {input}"
    );

    // 4. And it must not read as licence to carry on with the pre-question plan.
    //    Yet a typed reply often asks something else, and leaves the question
    //    open for a re-ask.
    assert!(
        input.contains("Do not re-ask the same question unless the answer left it open"),
        "the resume must block a re-ask of an answered question only: {input}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Only `answered_after_idle` changes. A user-clicked Continue, the hang
/// watchdog, the api-error auto-resume and the switch auto-resume all keep
/// sending exactly today's message, even on a thread that happens to carry an
/// answered question from an earlier turn.
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

/// The empty-stdin floor. With no answer to recap the input falls back to the
/// exact constant, never to an empty string: an empty stdin parks
/// `claude --print --resume` forever and zombies the thread.
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

/// The sibling path. When `--resume` finds a dead session id the continuation
/// retries with NO resume sid, so the fresh subprocess's entire context is the
/// reconstruction plus this tail. `reconstruct_summary` does not project
/// `UserQuestionAsked` / `UserQuestionAnswered` (see `fetch_relevant_events`),
/// so if the tail were the bare continue message the answer would be lost all
/// over again, on the path likeliest to hit it: a thread parked long enough to
/// be answered after teardown is also a thread parked long enough for its
/// transcript to age out.
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

/// Harden resumes the held thread to run `/harden`, never with the bare
/// continue message. Its turn end then proposes through the normal floor.
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
        crate::engine::claude_code::AUTO_HARDEN_MESSAGE,
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An effort restart resumes with the nudge that says why the turn stopped,
/// so the agent never reads the interruption as the user's request.
#[tokio::test]
async fn an_effort_restart_resumes_with_the_effort_nudge() {
    use crate::engine::agent_recovery::{
        continue_input_for_reason, EFFORT_RESTART_REASON, EFFORT_RESTART_USER_MESSAGE,
    };

    let (pool, db_name) = setup_test_db().await;
    assert_eq!(
        continue_input_for_reason(
            &pool,
            std::path::Path::new("/nonexistent"),
            Uuid::new_v4(),
            Some(EFFORT_RESTART_REASON)
        )
        .await,
        EFFORT_RESTART_USER_MESSAGE,
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Try again after a start failure must hand the agent the message it never
/// read, with the same words the user typed (the I7 contract). The events are
/// the ones a real start failure leaves.
#[tokio::test]
async fn a_start_failure_leaves_its_message_owed_to_the_next_continue() {
    use crate::engine::agent_recovery::undelivered_input;
    use crate::engine::thread_events::{CodingAgentExit, CodingAgentExitStage};

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
            agent_exit: Some(CodingAgentExit {
                stage: CodingAgentExitStage::Startup,
                code: Some(1),
            }),
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

/// Emit `event` on `thread_id`'s Claude Code channel and return its id.
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

/// What an effort pick before the agent's first read leaves: the prompt
/// "Hey", a follow-up forwarded behind it, the cancel, and the discard.
/// Returns the prompt's and the follow-up's `MessageReceived` ids.
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

/// The ids of the follow-ups a session starting with `first_prompt` redelivers.
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

/// An effort pick that interrupts a turn before the agent read its first
/// prompt leaves that prompt owed to the resume. A follow-up the session
/// forwarded stays out: `stranded_follow_ups` redelivers it.
#[tokio::test]
async fn an_effort_restart_before_the_first_read_leaves_its_message_owed() {
    use crate::engine::agent_recovery::discarded_input;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let (prompt, _) = effort_pick_discards_the_first_prompt(&bus, thread_id).await;

    let owed = discarded_input(&pool, thread_id)
        .await
        .expect("the first prompt the agent never read is owed");
    assert_eq!(owed.text, "Hey");
    assert_eq!(owed.event_ids, [prompt]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Race 1: the user's message wins the gap between the effort pick and the
/// resume's claim. Its session, not the resume, delivers the discarded prompt.
#[tokio::test]
async fn a_message_that_wins_the_gap_after_an_effort_pick_gets_the_discarded_prompt() {
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

/// Race 2: the follow-up queued behind the discarded prompt is read after the
/// interrupt, and its turn ends on its own. The prompt is still owed, the
/// resume carries it, and its read closes the discard.
#[tokio::test]
async fn a_follow_up_read_after_an_effort_pick_leaves_the_discarded_prompt_owed() {
    use crate::engine::agent_recovery::discarded_input;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let (prompt, follow_up) = effort_pick_discards_the_first_prompt(&bus, thread_id).await;
    let read = |input_event_id| ThreadEvent::CodingAgentInputRead {
        input_event_id,
        started_turn: false,
        verbatim: false,
    };
    emit_cc(&bus, thread_id, read(follow_up)).await;
    emit_cc(
        &bus,
        thread_id,
        ThreadEvent::ResponseGenerated {
            text: "done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;

    let owed = discarded_input(&pool, thread_id)
        .await
        .expect("the turn that read the follow-up never read the prompt");
    assert_eq!(owed.text, "Hey");
    assert_eq!(owed.event_ids, [prompt]);

    let resume = Uuid::new_v4();
    assert!(
        stranded_ids(&pool, thread_id, &[resume, prompt])
            .await
            .is_empty(),
        "the resume carries the prompt as its first prompt, so it does not redeliver it"
    );
    emit_cc(&bus, thread_id, read(prompt)).await;
    assert!(
        discarded_input(&pool, thread_id).await.is_none(),
        "the resume's read closes the discard"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A child wake an effort pick discarded, ahead of a turn that ended on its own.
/// Returns the wake's `ChildThreadCompleted` id.
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

/// Race 2 for a child wake: the effort resume opens with the completion card,
/// and its read closes the discard.
#[tokio::test]
async fn the_effort_resume_carries_a_discarded_child_wake() {
    use crate::engine::agent_recovery::discarded_input;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let wake = effort_pick_discards_a_child_wake(&bus, thread_id).await;

    let owed = discarded_input(&pool, thread_id)
        .await
        .expect("the wake no turn read is owed");
    assert!(
        owed.text.contains("[CHILD THREAD COMPLETED]"),
        "{}",
        owed.text
    );
    assert!(owed.text.contains("child did the thing"));
    assert_eq!(owed.event_ids, [wake]);

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
        discarded_input(&pool, thread_id).await.is_none(),
        "the resume's read closes the discard"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The resume after a discarded continuation sends its own nudge, and a
/// keep-alive only warms the cache: neither is owed.
#[tokio::test]
async fn a_discarded_continuation_or_keep_alive_leaves_nothing_owed() {
    use crate::engine::agent_recovery::discarded_input;
    use crate::engine::thread_events::{ActorMode, EngineReason, MessageOrigin};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    crate::test_support::start_cc_session(&bus, thread_id, "claude-code/effort", None).await;
    let continuation = emit_cc(
        &bus,
        thread_id,
        ThreadEvent::ContinuationRequested {
            reason: crate::engine::agent_recovery::EFFORT_RESTART_REASON.into(),
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

    assert!(discarded_input(&pool, thread_id).await.is_none());

    pool.close().await;
    teardown_test_db(&db_name).await;
}
