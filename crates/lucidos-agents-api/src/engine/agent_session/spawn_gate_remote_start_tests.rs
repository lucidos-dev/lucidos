use super::*;
use crate::engine::agent_question::{
    answer_pending_question, resolve_pending_question_as_canceled, AnswerResult,
};
use crate::engine::chat::process::ChatProcessExt;
use crate::engine::event_bus::BusEvent;
use crate::engine::start_failure::awaits_owner;
use crate::engine::thread_events::SpawnVerdict;
use crate::engine::thread_events::{AbortCause, ThreadEvent};
use crate::test_support::{
    boot_mock_engine, run_on_engine_runtime, setup_test_db, teardown_test_db,
};

const MESSAGE: &str = "Summarise the open tickets";

fn ask_origin() -> MessageOrigin {
    MessageOrigin::Workspace {
        workspace: "source".into(),
        caller_gateway: Some("source-gateway".into()),
        routing_gateway: Some("routing-gateway".into()),
        thread_id: Some(Uuid::new_v4()),
        event_id: None,
        user_agent: None,
        mode: ActorMode::Agent,
        verdict: Some(SpawnVerdict::Allowed),
        remote_start: Some(Box::new(RemoteStartAsk {
            pairing_id: "pairing-1".into(),
            pairing_label: "Laptop".into(),
        })),
    }
}

async fn start(engine: &LucidosEngine) -> (Uuid, Uuid) {
    let thread_id = Uuid::new_v4();
    let error = engine
        .process_message_with_steps(
            MESSAGE,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(thread_id),
            None,
            None,
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            None,
            None,
            Some(ask_origin()),
            None,
            crate::engine::FollowUpUrgency::Normal,
            None,
        )
        .await
        .expect_err("an Ask start parks on its card");
    assert!(awaits_owner(error.as_ref()), "{error}");
    let origin_id = sqlx::query_scalar(
        "SELECT id FROM events WHERE thread_id = $1 AND event_type = 'MessageReceived'",
    )
    .bind(thread_id)
    .fetch_one(engine.pool())
    .await
    .unwrap();
    (thread_id, origin_id)
}

async fn count(engine: &LucidosEngine, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = $2")
        .bind(thread_id)
        .bind(event_type)
        .fetch_one(engine.pool())
        .await
        .unwrap()
}

async fn wait_for(engine: &LucidosEngine, thread_id: Uuid, event_type: &str) -> Uuid {
    for _ in 0..100 {
        let anchor: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT NULLIF(payload->>'request_event_id', '')::uuid FROM events \
             WHERE thread_id = $1 AND event_type = $2",
        )
        .bind(thread_id)
        .bind(event_type)
        .fetch_optional(engine.pool())
        .await
        .unwrap();
        if let Some(Some(anchor)) = anchor {
            return anchor;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("no {event_type} landed");
}

async fn card_option(engine: &LucidosEngine, thread_id: Uuid, label: &str) -> (String, String) {
    sqlx::query_as(
        "SELECT payload->>'tool_use_id', o->>'id' FROM events, \
                jsonb_array_elements(payload->'options') o \
          WHERE thread_id = $1 AND event_type = 'UserQuestionAsked' AND o->>'label' = $2",
    )
    .bind(thread_id)
    .bind(label)
    .fetch_one(engine.pool())
    .await
    .unwrap()
}

async fn pick(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    label: &str,
    actor: Option<MessageOrigin>,
) -> AnswerResult {
    let (card_id, option_id) = card_option(engine, thread_id, label).await;
    answer_pending_question(
        engine,
        thread_id,
        card_id,
        AnswerKind::Selected { option_id },
        actor,
    )
    .await
}

fn owner() -> Option<MessageOrigin> {
    Some(MessageOrigin::Device {
        device_id: "test-device".into(),
    })
}

async fn assert_refused(engine: &LucidosEngine, thread_id: Uuid) {
    let later = gate_remote_start(&engine.event_bus, thread_id, EventChannel::Chat)
        .await
        .expect_err("a refused start never runs");
    assert!(!awaits_owner(later.as_ref()), "{later}");
    assert_eq!(later.to_string(), START_REFUSED_ERROR);
}

async fn allow_once_runs_the_parked_turn_and_nothing_before() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let (thread_id, origin_id) = start(&engine).await;

    for event_type in ["ContextCaptured", "ResponseGenerated", "ResponseFailed"] {
        assert_eq!(
            count(&engine, thread_id, event_type).await,
            0,
            "{event_type}"
        );
    }
    assert_eq!(count(&engine, thread_id, "UserQuestionAsked").await, 1);

    let parked = gate_remote_start(&engine.event_bus, thread_id, EventChannel::Chat).await;
    assert!(parked.is_err_and(|e| awaits_owner(e.as_ref())));
    assert_eq!(count(&engine, thread_id, "UserQuestionAsked").await, 1);

    assert!(matches!(
        pick(&engine, thread_id, ALLOW_ONCE_LABEL, None).await,
        AnswerResult::Conflict(_)
    ));
    assert_eq!(count(&engine, thread_id, "ContextCaptured").await, 0);

    let (card_id, _) = card_option(&engine, thread_id, ALLOW_ONCE_LABEL).await;
    let typed = AnswerKind::FreeText {
        text: "what is this?".into(),
        image_hashes: vec![],
    };
    assert!(matches!(
        answer_pending_question(&engine, thread_id, card_id, typed, owner()).await,
        AnswerResult::Conflict(_)
    ));
    assert_eq!(count(&engine, thread_id, "UserQuestionAnswered").await, 0);

    assert!(matches!(
        pick(&engine, thread_id, ALLOW_ONCE_LABEL, owner()).await,
        AnswerResult::Resumed
    ));
    assert_eq!(
        wait_for(&engine, thread_id, "ResponseGenerated").await,
        origin_id,
        "the reply answers the parked message"
    );
    assert_eq!(count(&engine, thread_id, "ResponseGenerated").await, 1);
    assert!(count(&engine, thread_id, "ContextCaptured").await >= 1);
    assert_eq!(count(&engine, thread_id, "MessageReceived").await, 1);
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

async fn dont_allow_ends_the_thread_unstarted() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let (thread_id, origin_id) = start(&engine).await;

    assert!(matches!(
        pick(&engine, thread_id, DONT_ALLOW_LABEL, owner()).await,
        AnswerResult::Resumed
    ));
    assert_eq!(
        wait_for(&engine, thread_id, "ResponseCanceled").await,
        origin_id
    );
    let cause: String = sqlx::query_scalar(
        "SELECT payload->>'cause' FROM events WHERE thread_id = $1 \
         AND event_type = 'ResponseCanceled'",
    )
    .bind(thread_id)
    .fetch_one(engine.pool())
    .await
    .unwrap();
    assert_eq!(cause, CancelCause::Refused.as_str());
    assert_refused(&engine, thread_id).await;
    let purposes: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT payload->>'purpose' FROM events WHERE thread_id = $1 \
         AND event_type = 'ContextCaptured' ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(engine.pool())
    .await
    .unwrap();
    assert_eq!(purposes, Vec::<Option<String>>::new());
    assert_eq!(count(&engine, thread_id, "ResponseGenerated").await, 0);
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

#[test]
fn allow_once_runs_the_parked_turn_and_no_model_call_happens_before() {
    run_on_engine_runtime(
        "engine::agent_session::spawn_gate::remote_start::tests::allow_once_runs_the_parked_turn_and_no_model_call_happens_before",
        allow_once_runs_the_parked_turn_and_nothing_before,
    );
}

#[test]
fn dont_allow_ends_a_remote_start_unstarted() {
    run_on_engine_runtime(
        "engine::agent_session::spawn_gate::remote_start::tests::dont_allow_ends_a_remote_start_unstarted",
        dont_allow_ends_the_thread_unstarted,
    );
}

async fn an_allowed_start_that_never_began_begins_at_boot() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let (thread_id, origin_id) = start(&engine).await;
    let (card_id, option_id) = card_option(&engine, thread_id, ALLOW_ONCE_LABEL).await;
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::UserQuestionAnswered {
                compose: None,
                tool_use_id: card_id,
                answer: AnswerKind::Selected { option_id },
                quiet: true,
            },
            meta: EventMeta::with_actor(owner()),
        })
        .await
        .unwrap();
    crate::engine::thread_events::emit_response_aborted(
        &engine.event_bus,
        thread_id,
        AbortCause::RecoveryAfterRestart,
        String::new(),
        vec![],
        None,
        None,
        EventMeta {
            request_event_id: Some(origin_id),
            ..EventMeta::NONE
        },
        "[Test] ResponseAborted",
    )
    .await;
    assert_eq!(count(&engine, thread_id, "ContextCaptured").await, 0);

    begin_allowed_starts_on_startup(&engine).await;
    assert_eq!(
        wait_for(&engine, thread_id, "ResponseGenerated").await,
        origin_id,
        "the reply answers the parked message"
    );

    begin_allowed_starts_on_startup(&engine).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(count(&engine, thread_id, "ResponseGenerated").await, 1);
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

async fn stop_cancels_a_parked_start() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let (thread_id, _) = start(&engine).await;

    assert!(resolve_pending_question_as_canceled(&engine, thread_id, owner()).await);
    let answer: serde_json::Value = sqlx::query_scalar(
        "SELECT payload->'answer' FROM events WHERE thread_id = $1 \
         AND event_type = 'UserQuestionAnswered'",
    )
    .bind(thread_id)
    .fetch_one(engine.pool())
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_value::<AnswerKind>(answer).unwrap(),
        AnswerKind::Canceled
    );
    assert_refused(&engine, thread_id).await;
    assert!(spend_is_held(engine.pool(), thread_id, None).await);

    begin_allowed_starts_on_startup(&engine).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(count(&engine, thread_id, "ContextCaptured").await, 0);
    assert_eq!(count(&engine, thread_id, "ResponseGenerated").await, 0);
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

async fn an_allowed_coding_agent_start_resumes_the_coding_agent() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let thread_id = Uuid::new_v4();
    let mut message = crate::test_support::message_received(MESSAGE);
    if let ThreadEvent::MessageReceived { origin, .. } = &mut message {
        *origin = Some(ask_origin());
    }
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: message,
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .expect("the start message is recorded");
    let parked = gate_remote_start(&engine.event_bus, thread_id, EventChannel::ClaudeCode).await;
    assert!(parked.is_err_and(|e| awaits_owner(e.as_ref())));

    assert!(matches!(
        pick(&engine, thread_id, ALLOW_ONCE_LABEL, owner()).await,
        AnswerResult::Resumed
    ));
    let mut reason: Option<String> = None;
    for _ in 0..100 {
        reason = sqlx::query_scalar(
            "SELECT payload->>'reason' FROM events \
             WHERE thread_id = $1 AND event_type = 'ContinuationRequested'",
        )
        .bind(thread_id)
        .fetch_optional(engine.pool())
        .await
        .unwrap();
        if reason.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        reason.as_deref(),
        Some(crate::engine::continuation_reason::OWNER_ALLOWED_START_REASON)
    );
    let chat_turns: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 \
         AND event_type = 'ResponseGenerated' AND payload->>'channel' = 'chat'",
    )
    .bind(thread_id)
    .fetch_one(engine.pool())
    .await
    .unwrap();
    assert_eq!(chat_turns, 0, "the start did not run as a chat turn");
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

#[test]
fn an_allowed_coding_agent_start_resumes_the_coding_agent_not_a_chat_turn() {
    run_on_engine_runtime(
        "engine::agent_session::spawn_gate::remote_start::tests::an_allowed_coding_agent_start_resumes_the_coding_agent_not_a_chat_turn",
        an_allowed_coding_agent_start_resumes_the_coding_agent,
    );
}

#[test]
fn an_allowed_start_that_never_began_begins_at_boot_once() {
    run_on_engine_runtime(
        "engine::agent_session::spawn_gate::remote_start::tests::an_allowed_start_that_never_began_begins_at_boot_once",
        an_allowed_start_that_never_began_begins_at_boot,
    );
}

#[test]
fn stop_cancels_a_parked_start_and_no_turn_runs() {
    run_on_engine_runtime(
        "engine::agent_session::spawn_gate::remote_start::tests::stop_cancels_a_parked_start_and_no_turn_runs",
        stop_cancels_a_parked_start,
    );
}

#[test]
fn the_card_offers_both_remember_choices_and_names_the_pairing() {
    let thread_id = Uuid::new_v4();
    let pairing = RemoteStartAsk {
        pairing_id: "pairing-1".into(),
        pairing_label: "Laptop".into(),
    };
    let card = card(thread_id, &pairing);
    assert_eq!(card.action, Action::StartRemoteThread);
    assert!(card.question.contains("\u{201c}Laptop\u{201d}"));
    let choices: Vec<_> = card
        .options
        .iter()
        .map(|o| (o.label.as_str(), o.choice.clone()))
        .collect();
    assert_eq!(
        choices,
        vec![
            (
                ALLOW_ONCE_LABEL,
                PermissionChoice::grant(Lifetime::Once, Holder::Thread { thread_id })
            ),
            (
                ALWAYS_ALLOW_LABEL,
                PermissionChoice::grant(Lifetime::Standing, Holder::Workspace)
            ),
            (ALWAYS_DENY_LABEL, PermissionChoice::RefuseStanding),
            (DONT_ALLOW_LABEL, PermissionChoice::Refuse),
        ]
    );
}

#[tokio::test]
async fn an_unreadable_start_state_holds_the_spend() {
    let (pool, db) = setup_test_db().await;
    pool.close().await;
    assert!(spend_is_held(&pool, Uuid::new_v4(), None).await);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_unrecorded_first_message_holds_the_spend_only_when_its_origin_asks() {
    let (pool, db) = setup_test_db().await;
    let thread_id = Uuid::new_v4();
    let device = MessageOrigin::Device {
        device_id: "d1".into(),
    };
    assert!(spend_is_held(&pool, thread_id, Some(&ask_origin())).await);
    assert!(!spend_is_held(&pool, thread_id, Some(&device)).await);
    assert!(!spend_is_held(&pool, thread_id, None).await);

    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2, 'MessageReceived', $3, NOW(), $2::uuid)",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id.to_string())
    .bind(serde_json::json!({ "origin": device }))
    .execute(&pool)
    .await
    .expect("record a first message that is no Ask start");
    assert!(!spend_is_held(&pool, thread_id, Some(&ask_origin())).await);
    pool.close().await;
    teardown_test_db(&db).await;
}

async fn an_ask_start_parks_a_coding_agent_run_at_the_spawn_gate() {
    use crate::engine::agent_session::spawn_gate::SpawnGateExt;

    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let thread_id = Uuid::new_v4();
    let mut message = crate::test_support::message_received(MESSAGE);
    if let ThreadEvent::MessageReceived { origin, .. } = &mut message {
        *origin = Some(ask_origin());
    }
    let origin_id = engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: message,
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .expect("the start message is recorded")
        .event_id;

    let error = engine
        .gate_coding_agent_spawn(thread_id, origin_id)
        .await
        .expect_err("an Ask start never starts a coding agent before the owner allows it");
    assert!(awaits_owner(error.as_ref()), "{error}");
    assert_eq!(count(&engine, thread_id, "UserQuestionAsked").await, 1);
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

#[test]
fn an_ask_start_parks_a_coding_agent_run_before_any_process_starts() {
    run_on_engine_runtime(
        "engine::agent_session::spawn_gate::remote_start::tests::an_ask_start_parks_a_coding_agent_run_before_any_process_starts",
        an_ask_start_parks_a_coding_agent_run_at_the_spawn_gate,
    );
}
