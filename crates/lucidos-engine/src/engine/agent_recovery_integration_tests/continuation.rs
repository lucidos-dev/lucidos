use std::sync::Arc;
use uuid::Uuid;

use super::{ENGINE_RESTART_INTERRUPT_REASON, USER_CLICKED_CONTINUE_REASON};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

/// The Phase 5.3 contract for the continue endpoint: emitting a
/// `ContinuationRequested` on the CC channel persists with the user's reason tag,
/// and the spawn dispatcher's classifier (subscribed to the same bus) will
/// see it as a `SpawnTrigger::ContinuationRequested`. This test exercises the
/// "endpoint emits → bus receives → dispatcher classifies" chain without
/// the full Axum router (which requires a complete engine).
#[tokio::test]
async fn continuation_requested_emission_classifies_as_spawn_trigger() {
    use crate::engine::agent_session::spawn_dispatcher::{SpawnDispatcher, SpawnRequest};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (tx, mut spawn_rx) = tokio::sync::mpsc::unbounded_channel::<SpawnRequest>();
    let dispatcher = SpawnDispatcher::new(pool.clone(), tx);
    // Subscribe before starting the loop — mirrors `SpawnDispatcher::spawn()`,
    // so no settling sleep is needed before producing.
    let rx = bus.subscribe();
    let handle = tokio::spawn(async move { dispatcher.run(rx).await });

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

    let received = tokio::time::timeout(std::time::Duration::from_secs(2), spawn_rx.recv())
        .await
        .expect("dispatcher must produce a SpawnRequest within 2s")
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

/// Phase 5.3: when recovery emits a synthetic `CodingAgentIdled` with
/// `reason = engine_restart_interrupt`, the spawn dispatcher's classifier
/// must NOT treat it as a trigger. Without this guarantee, the very
/// "interrupted" event we emit to surface the continue affordance would
/// loop back into auto-spawning CC — exactly the behavior we removed.
#[tokio::test]
async fn synthetic_idled_with_engine_restart_interrupt_does_not_dispatch() {
    use crate::engine::agent_session::spawn_dispatcher::{SpawnDispatcher, SpawnRequest};
    use std::sync::atomic::Ordering;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (tx, _spawn_rx) = tokio::sync::mpsc::unbounded_channel::<SpawnRequest>();
    let dispatcher = SpawnDispatcher::new(pool.clone(), tx);
    let dispatch_count = dispatcher.dispatch_count.clone();
    let rx = bus.subscribe();
    let handle = tokio::spawn(async move { dispatcher.run(rx).await });

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

    // Give the dispatcher time to (NOT) act.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
            dispatch_count.load(Ordering::SeqCst),
            0,
            "synthetic CodingAgentIdled must not produce any dispatch — only the user's continue click does"
        );

    handle.abort();
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
        },
        meta: cc_meta,
    })
    .await
    .expect("emit succeeds")
    .expect("persisted");

    let input = continue_retry_input(&pool, thread_id, Some(USER_CLICKED_CONTINUE_REASON)).await;

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

    let input = continue_retry_input(&pool, thread_id, Some(USER_CLICKED_CONTINUE_REASON)).await;

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
                },
                QuestionOption {
                    id: "opt-1".into(),
                    label: "Derive the deadline instead".into(),
                    description: None,
                },
            ],
            worktree_path: None,
            multi_select: false,
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
            answer: AnswerKind::FreeText { text: typed.into() },
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

    let input = continue_input_for_reason(&pool, thread_id, Some(ANSWERED_AFTER_IDLE_REASON)).await;

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
    assert!(
        input.contains("Do not re-ask the same question"),
        "the resume must block a re-ask: {input}"
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
            continue_input_for_reason(&pool, thread_id, reason).await,
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
        continue_input_for_reason(&pool, thread_id, Some(ANSWERED_AFTER_IDLE_REASON)).await,
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

    let input = continue_retry_input(&pool, thread_id, Some(ANSWERED_AFTER_IDLE_REASON)).await;

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
