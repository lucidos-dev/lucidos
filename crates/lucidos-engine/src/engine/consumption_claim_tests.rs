use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, AnswerKind};
use crate::test_support::{
    message_received, seed_chat_thread, seed_coding_thread, setup_test_db, teardown_test_db,
};

type EmitResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent, meta: EventMeta) -> EmitResult {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta,
    })
    .await
    .map(|_| ())
}

async fn remove(bus: &EventBus, thread_id: Uuid, message: Uuid) -> EmitResult {
    emit(
        bus,
        thread_id,
        ThreadEvent::QueuedMessageRemoved {
            removed_message_id: message,
            restore_to_compose: false,
            compose: None,
        },
        EventMeta::NONE,
    )
    .await
}

/// A follow-up recorded while a turn runs: it names that turn's anchor.
async fn record_follow_up(bus: &EventBus, thread_id: Uuid, message: Uuid) -> EmitResult {
    emit(
        bus,
        thread_id,
        message_received("follow-up"),
        EventMeta {
            event_id: Some(message),
            request_event_id: Some(Uuid::new_v4()),
            ..EventMeta::NONE
        },
    )
    .await
}

const CONSUMERS: usize = 4;
const ANSWER: usize = 2;
const USER_ONLY: usize = 3;

/// Record `message` the way the consumer at `consumer` finds it.
async fn record_for(bus: &EventBus, thread_id: Uuid, consumer: usize, message: Uuid) {
    if consumer != ANSWER && consumer != USER_ONLY {
        record_follow_up(bus, thread_id, message).await.unwrap();
    }
}

/// Each delivery of `message`. Answers and user-only posts carry its own id;
/// injections and turn starters follow a separate recorded message.
fn consumers(message: Uuid) -> Vec<(&'static str, ThreadEvent, EventMeta)> {
    vec![
        (
            "inject",
            ThreadEvent::PromptInjected {
                text: "follow-up".into(),
                mode: ActorMode::Human,
                origin: None,
                injected_message_id: Some(message),
                delivered_event_id: None,
            },
            EventMeta::NONE,
        ),
        (
            "own turn",
            ThreadEvent::ChatTurnSettingsResolved {
                model: "mock".into(),
                reasoning_effort: None,
                provider: None,
            },
            EventMeta {
                request_event_id: Some(message),
                ..EventMeta::NONE
            },
        ),
        (
            "answer",
            ThreadEvent::UserQuestionAnswered {
                compose: None,
                tool_use_id: format!("toolu_{message}"),
                answer: AnswerKind::FreeText {
                    text: "follow-up".into(),
                    image_hashes: vec![],
                },
                quiet: false,
            },
            EventMeta {
                event_id: Some(message),
                ..EventMeta::NONE
            },
        ),
        (
            "user-only delivery",
            ThreadEvent::UserOnlyMessagePosted {
                text: "follow-up".into(),
                user_image_hashes: vec![],
                device_id: None,
                mentioned_user_ids: vec![],
            },
            EventMeta {
                event_id: Some(message),
                ..EventMeta::NONE
            },
        ),
    ]
}

async fn count(pool: &sqlx::PgPool, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = $2")
        .bind(thread_id)
        .bind(event_type)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_message_removed_before_recording_is_never_recorded_or_consumed() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_chat_thread(&bus, "hello").await;
    let message = Uuid::new_v4();
    remove(&bus, thread_id, message).await.unwrap();

    let recorded = record_follow_up(&bus, thread_id, message).await;
    assert!(is_removed_refusal(&*recorded.unwrap_err()));
    for (name, event, meta) in consumers(message) {
        let consumed = emit(&bus, thread_id, event, meta).await;
        assert!(is_removed_refusal(&*consumed.unwrap_err()), "{name}");
    }
    assert_eq!(count(&pool, thread_id, "MessageReceived").await, 1);
    assert_eq!(count(&pool, thread_id, "PromptInjected").await, 0);
    assert_eq!(count(&pool, thread_id, "ChatTurnSettingsResolved").await, 0);
    assert_eq!(count(&pool, thread_id, "UserQuestionAnswered").await, 0);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_recorded_message_removed_before_delivery_is_never_consumed() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_chat_thread(&bus, "hello").await;
    for consumer in 0..CONSUMERS {
        let message = Uuid::new_v4();
        record_for(&bus, thread_id, consumer, message).await;
        remove(&bus, thread_id, message).await.unwrap();
        let (name, event, meta) = consumers(message).swap_remove(consumer);
        let consumed = emit(&bus, thread_id, event, meta).await;
        assert!(is_removed_refusal(&*consumed.unwrap_err()), "{name}");
    }
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn removal_after_consumption_answers_already_read() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_chat_thread(&bus, "hello").await;
    for consumer in 0..CONSUMERS {
        let message = Uuid::new_v4();
        record_for(&bus, thread_id, consumer, message).await;
        let (name, event, meta) = consumers(message).swap_remove(consumer);
        emit(&bus, thread_id, event, meta).await.unwrap();
        let removed = remove(&bus, thread_id, message).await;
        assert!(is_consumed_refusal(&*removed.unwrap_err()), "{name}");
    }
    assert_eq!(count(&pool, thread_id, "QueuedMessageRemoved").await, 0);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_message_recorded_without_an_anchor_is_removable_until_its_turn_claims_it() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_chat_thread(&bus, "hello").await;
    // A turn starter, or a follow-up that arrived before the live turn
    // stamped its anchor: both are recorded with no `request_event_id`.
    let message = Uuid::new_v4();
    emit(
        &bus,
        thread_id,
        message_received("no anchor yet"),
        EventMeta {
            event_id: Some(message),
            ..EventMeta::NONE
        },
    )
    .await
    .unwrap();
    remove(&bus, thread_id, message).await.unwrap();

    let (_, event, meta) = consumers(message).swap_remove(1);
    let stamped = emit(&bus, thread_id, event, meta).await;
    assert!(is_removed_refusal(&*stamped.unwrap_err()));
    assert_eq!(count(&pool, thread_id, "ChatTurnSettingsResolved").await, 0);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_coding_agent_withdrawal_is_never_refused() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_coding_thread(&bus, None).await;
    let message = Uuid::new_v4();
    emit(
        &bus,
        thread_id,
        message_received("withdrawn by the agent"),
        EventMeta {
            event_id: Some(message),
            ..EventMeta::NONE
        },
    )
    .await
    .unwrap();
    remove(&bus, thread_id, message).await.unwrap();
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn removal_races_every_consumer_and_one_wins() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_chat_thread(&bus, "hello").await;
    for round in 0..8 {
        for consumer in 0..CONSUMERS {
            let message = Uuid::new_v4();
            record_for(&bus, thread_id, consumer, message).await;
            let (name, event, meta) = consumers(message).swap_remove(consumer);
            let (consumed, removed) = tokio::join!(
                emit(&bus, thread_id, event, meta),
                remove(&bus, thread_id, message)
            );
            assert!(
                consumed.is_ok() != removed.is_ok(),
                "{name}, round {round}: exactly one of the delivery and the removal commits"
            );
        }
    }
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_removed_unrecorded_input_frees_its_successor() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_chat_thread(&bus, "hello").await;
    let message = Uuid::new_v4();
    bus.restore_accepted_input_turn(Uuid::new_v4(), thread_id, message);
    let mut successor = bus.input_order.join(thread_id);
    assert!(successor.has_input_ahead());

    remove(&bus, thread_id, message).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        successor.wait_for_predecessor(),
    )
    .await
    .expect("a successor never waits on an input the user took back");
    let recorded = record_follow_up(&bus, thread_id, message).await;
    assert!(is_removed_refusal(&*recorded.unwrap_err()));
    teardown_test_db(&db).await;
}

/// Stop must find accepted inputs even before recording, and after the queue
/// executor has handed a recorded follow-up to the live turn.
#[tokio::test]
async fn stop_takes_back_queued_admitted_and_recorded_inputs() {
    use crate::engine::event_bus::SystemEvent;
    use crate::engine::thread_events::MessageOrigin;
    for stage in 0..5 {
        let (pool, db) = setup_test_db().await;
        let bus = EventBus::new(pool.clone()).0;
        let thread = seed_chat_thread(&bus, "active").await;
        let message = Uuid::new_v4();
        let actor = MessageOrigin::Device {
            device_id: "test-device".into(),
        };
        let entry = accept_human_input(&bus, thread, message).await;
        if stage > 0 {
            bus.emit(BusEvent::System(SystemEvent::ThreadQueueAdmitted {
                entry_id: entry,
                thread_id: Some(thread),
                actor: None,
            }))
            .await
            .unwrap();
        }
        if stage >= 2 {
            drop(bus.take_accepted_input_turn(entry));
            emit(
                &bus,
                thread,
                message_received("take me back"),
                EventMeta {
                    event_id: Some(message),
                    ..EventMeta::NONE
                },
            )
            .await
            .unwrap();
            bus.emit(BusEvent::System(SystemEvent::ThreadQueueCompleted {
                entry_id: entry,
            }))
            .await
            .unwrap();
        }
        if stage == 3 {
            let (_, event, meta) = consumers(message).remove(0);
            emit(&bus, thread, event, meta).await.unwrap();
        }
        if stage == 4 {
            sqlx::raw_sql("CREATE FUNCTION refuse_removal() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event_type = 'QueuedMessageRemoved' THEN RAISE EXCEPTION 'synthetic removal failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER refuse_removal BEFORE INSERT ON events FOR EACH ROW EXECUTE FUNCTION refuse_removal();")
                .execute(&pool).await.unwrap();
        }
        let result = take_back_unconsumed(&bus, Some(thread), Some(actor))
            .await
            .unwrap();
        if stage >= 3 {
            assert!(result.taken_back_messages.is_empty());
            assert_eq!(count(&pool, thread, "QueuedMessageRemoved").await, 0);
            assert_eq!(
                result.failed_take_back_ids,
                if stage == 4 { vec![message] } else { vec![] }
            );
            pool.close().await;
            teardown_test_db(&db).await;
            continue;
        }
        assert_eq!(
            result
                .taken_back_messages
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            vec![message]
        );
        assert_eq!(result.taken_back_messages[0].text, "take me back");
        assert_eq!(
            result.taken_back_messages[0].image_hashes,
            vec!["test-image"]
        );
        assert!(result.failed_take_back_ids.is_empty());
        assert_eq!(count(&pool, thread, "QueuedMessageRemoved").await, 1);
        let (kind, event, meta) = consumers(message).remove(1);
        let error = emit(&bus, thread, event, meta).await.expect_err(kind);
        assert!(is_removed_refusal(&*error));
        // Stop is idempotent: already removed text is never returned twice.
        assert!(
            take_back_unconsumed(&bus, Some(thread), Some(device_actor()))
                .await
                .unwrap()
                .taken_back_messages
                .is_empty()
        );
        pool.close().await;
        teardown_test_db(&db).await;
    }
}

async fn accept_human_input(bus: &EventBus, thread: Uuid, message: Uuid) -> Uuid {
    use crate::engine::event_bus::OrdinarySubmissionIntent;
    use crate::engine::thread_events::MessageOrigin;
    use crate::engine::thread_queue::ThreadQueueRequest;
    let actor = MessageOrigin::Device {
        device_id: "test-device".into(),
    };
    let request: ThreadQueueRequest = serde_json::from_value(serde_json::json!({
        "type":"agent-chat", "thread_id":thread, "event_id":message,
        "message":"take me back", "image_hashes":["test-image"], "mode":"human", "origin":actor,
    }))
    .unwrap();
    let intent = OrdinarySubmissionIntent::new(thread, message, &request).unwrap();
    bus.accept_ordinary_submission(request, actor, &bus.input_order, None, None, intent)
        .await
        .unwrap()
        .receipt
        .entry_id
}

#[tokio::test]
async fn workspace_stop_takes_back_chat_inputs_and_leaves_coding_agent_inputs() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let chats = [
        seed_chat_thread(&bus, "first").await,
        seed_chat_thread(&bus, "second").await,
    ];
    let coding = seed_coding_thread(&bus, None).await;
    for thread in chats.into_iter().chain([coding]) {
        accept_human_input(&bus, thread, Uuid::new_v4()).await;
    }
    let report = take_back_unconsumed(&bus, None, Some(device_actor()))
        .await
        .unwrap();
    assert_eq!(report.taken_back_messages.len(), 2);
    assert!(report
        .taken_back_messages
        .iter()
        .all(|message| chats.contains(&message.thread_id)));
    assert!(report.failed_take_back_ids.is_empty());
    assert_eq!(count(&pool, coding, "QueuedMessageRemoved").await, 0);
    pool.close().await;
    teardown_test_db(&db).await;
}

fn device_actor() -> crate::engine::thread_events::MessageOrigin {
    crate::engine::thread_events::MessageOrigin::Device {
        device_id: "test-device".into(),
    }
}

#[tokio::test]
async fn automation_cancel_leaves_human_inputs_alone() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let thread = seed_chat_thread(&bus, "active").await;
    let message = Uuid::new_v4();
    accept_human_input(&bus, thread, message).await;
    for actor in [
        None,
        Some(crate::engine::thread_events::MessageOrigin::Api {
            user_agent: None,
            mode: ActorMode::Agent,
            source_thread_id: Some(thread),
            emitting_trigger_id: None,
        }),
    ] {
        let report = take_back_unconsumed(&bus, Some(thread), actor)
            .await
            .unwrap();
        assert!(report.taken_back_messages.is_empty());
        assert_eq!(count(&pool, thread, "QueuedMessageRemoved").await, 0);
    }
    let report = take_back_unconsumed(&bus, Some(thread), Some(device_actor()))
        .await
        .unwrap();
    assert_eq!(report.taken_back_messages.len(), 1);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn stop_leaves_an_input_that_already_failed_alone() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let thread = seed_chat_thread(&bus, "active").await;
    let message = Uuid::new_v4();
    let entry = accept_human_input(&bus, thread, message).await;
    drop(bus.take_accepted_input_turn(entry));
    emit(
        &bus,
        thread,
        message_received("failed input"),
        EventMeta {
            event_id: Some(message),
            ..EventMeta::NONE
        },
    )
    .await
    .unwrap();
    emit(
        &bus,
        thread,
        ThreadEvent::ResponseFailed {
            error: "attachment missing".into(),
            agent_exit: None,
            provider_error: None,
            spend_cap: None,
        },
        EventMeta {
            request_event_id: Some(message),
            ..EventMeta::NONE
        },
    )
    .await
    .unwrap();
    let report = take_back_unconsumed(&bus, Some(thread), Some(device_actor()))
        .await
        .unwrap();
    assert!(report.taken_back_messages.is_empty());
    assert_eq!(count(&pool, thread, "QueuedMessageRemoved").await, 0);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn concurrent_take_backs_return_content_to_only_one_caller() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let thread = seed_chat_thread(&bus, "active").await;
    for _ in 0..8 {
        let message = Uuid::new_v4();
        let (left, right) = tokio::join!(
            take_back_message(&bus, thread, message, None),
            take_back_message(&bus, thread, message, None),
        );
        let outcomes = [left.unwrap(), right.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, TakeBackOutcome::Removed))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, TakeBackOutcome::AlreadyRemoved))
                .count(),
            1
        );
    }
    assert_eq!(count(&pool, thread, "QueuedMessageRemoved").await, 8);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_taken_back_user_only_message_completes_without_posting() {
    use crate::engine::thread_users::{post_user_only_message, UserOnlyMessage};
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let thread = seed_chat_thread(&bus, "active").await;
    let message = Uuid::new_v4();
    remove(&bus, thread, message).await.unwrap();
    post_user_only_message(
        &bus,
        thread,
        UserOnlyMessage {
            text: "taken back".into(),
            image_hashes: vec![],
            device_id: None,
            mentions: vec![],
            event_id: Some(message),
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(count(&pool, thread, "UserOnlyMessagePosted").await, 0);
    pool.close().await;
    teardown_test_db(&db).await;
}

async fn draft(pool: &sqlx::PgPool, thread: Uuid) -> (String, serde_json::Value, i64) {
    sqlx::query_as(
        "SELECT compose_text,compose_images,compose_epoch FROM thread_summaries WHERE thread_id=$1",
    )
    .bind(thread)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn stop_restores_durably_once_when_the_response_is_lost() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let mut announcements = bus.subscribe();
    let thread = seed_chat_thread(&bus, "active").await;
    accept_human_input(&bus, thread, Uuid::new_v4()).await;
    sqlx::query("UPDATE thread_summaries SET compose_text='newer draft',compose_images='[\"new-image\"]' WHERE thread_id=$1")
        .bind(thread).execute(&pool).await.unwrap();
    let before = draft(&pool, thread).await;
    while announcements.try_recv().is_ok() {}
    let (left, right) = tokio::join!(
        take_back_unconsumed(&bus, Some(thread), Some(device_actor())),
        take_back_unconsumed(&bus, Some(thread), Some(device_actor())),
    );
    assert_eq!(
        left.unwrap().taken_back_messages.len() + right.unwrap().taken_back_messages.len(),
        1
    );
    // Neither HTTP response is needed to recover the stored draft.
    let restored = draft(&pool, thread).await;
    assert_eq!(restored.0, "newer draft\n\ntake me back");
    assert_eq!(restored.1, serde_json::json!(["new-image", "test-image"]));
    assert_eq!(restored.2, before.2 + 1);
    let mut reports = Vec::new();
    while let Ok(frame) = announcements.try_recv() {
        if let BusEvent::System(crate::engine::event_bus::SystemEvent::ThreadComposeChanged {
            text,
            image_hashes,
            compose_epoch,
            ..
        }) = frame.typed
        {
            reports.push((text, serde_json::json!(image_hashes), compose_epoch));
        }
    }
    assert_eq!(reports, vec![restored.clone()]);
    let again = take_back_unconsumed(&bus, Some(thread), Some(device_actor()))
        .await
        .unwrap();
    assert!(again.taken_back_messages.is_empty());
    assert_eq!(draft(&pool, thread).await, restored);
    let stale = sqlx::query(
        "UPDATE thread_summaries SET compose_text='stale' WHERE thread_id=$1 AND compose_epoch=$2",
    )
    .bind(thread)
    .bind(before.2)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(stale.rows_affected(), 0);
    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE thread_id=$1 AND event_type='QueuedMessageRemoved'",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    let saved_restorations = payload["compose"]["restorations"].clone();
    let event = ThreadEvent::from_stored("QueuedMessageRemoved", payload).unwrap();
    // A replay repairs an older projection and never overwrites a later epoch.
    for newer in [false, true] {
        sqlx::query("UPDATE thread_summaries SET compose_text='later',compose_images='[]',compose_restorations='[]',compose_epoch=$2 WHERE thread_id=$1")
            .bind(thread).bind(if newer { restored.2 + 1 } else { before.2 }).execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        bus.update_thread_projection(&mut tx, thread, &event, &EventMeta::NONE)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let replayed = draft(&pool, thread).await;
        if newer {
            assert_eq!(replayed.0, "later");
        } else {
            assert_eq!(replayed, restored);
            let journal: serde_json::Value = sqlx::query_scalar(
                "SELECT compose_restorations FROM thread_summaries WHERE thread_id=$1",
            )
            .bind(thread)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(journal, saved_restorations);
        }
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn stop_restore_failure_keeps_the_input_and_draft() {
    use crate::core::store::{MAX_COMPOSE_IMAGES, MAX_COMPOSE_TEXT_BYTES};
    for case in 0..4 {
        let (pool, db) = setup_test_db().await;
        let bus = EventBus::new(pool.clone()).0;
        let thread = seed_chat_thread(&bus, "active").await;
        let message = Uuid::new_v4();
        accept_human_input(&bus, thread, message).await;
        match case {
            0 => {
                sqlx::query("UPDATE thread_summaries SET compose_text=$2 WHERE thread_id=$1")
                    .bind(thread)
                    .bind("x".repeat(MAX_COMPOSE_TEXT_BYTES))
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            1 => {
                sqlx::query("UPDATE thread_summaries SET compose_images=$2 WHERE thread_id=$1")
                    .bind(thread)
                    .bind(serde_json::json!((0..MAX_COMPOSE_IMAGES)
                        .map(|n| format!("image-{n}"))
                        .collect::<Vec<_>>()))
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            2 => {
                sqlx::query("UPDATE thread_summaries SET state='discarded' WHERE thread_id=$1")
                    .bind(thread)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            _ => {
                sqlx::raw_sql("CREATE FUNCTION refuse_restore() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event_type = 'QueuedMessageRemoved' THEN RAISE EXCEPTION 'synthetic failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER refuse_restore BEFORE INSERT ON events FOR EACH ROW EXECUTE FUNCTION refuse_restore();")
                .execute(&pool).await.unwrap();
            }
        }
        let before = draft(&pool, thread).await;
        let result = take_back_unconsumed(&bus, Some(thread), Some(device_actor()))
            .await
            .unwrap();
        assert_eq!(result.failed_take_back_ids, vec![message], "case {case}");
        assert_eq!(count(&pool, thread, "QueuedMessageRemoved").await, 0);
        assert_eq!(draft(&pool, thread).await, before);
        pool.close().await;
        teardown_test_db(&db).await;
    }
}

#[tokio::test]
async fn registered_unstamped_humans_restore_without_upgrading_attribution() {
    use crate::engine::thread_events::MessageOrigin;
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let thread = seed_chat_thread(&bus, "active").await;
    accept_human_input(&bus, thread, Uuid::new_v4()).await;
    crate::core::DeviceStore::register(&pool, &bus, "registered-browser", None, None, None)
        .await
        .unwrap();
    for (id, mode, eligible) in [
        (None, ActorMode::Human, false),
        (Some("unknown"), ActorMode::Human, false),
        (Some("registered-browser"), ActorMode::Agent, false),
        (Some("registered-browser"), ActorMode::Human, true),
    ] {
        let actor = MessageOrigin::UnattestedCaller {
            mode,
            user_agent: None,
            claimed_device_id: id.map(String::from),
            claimed_local_token: false,
            claimed_workspace: None,
            claimed_thread_id: None,
        };
        let result = take_back_unconsumed(&bus, Some(thread), Some(actor.clone()))
            .await
            .unwrap();
        assert_eq!(result.taken_back_messages.len(), usize::from(eligible));
        if eligible {
            let payload: serde_json::Value = sqlx::query_scalar("SELECT payload FROM events WHERE thread_id=$1 AND event_type='QueuedMessageRemoved'")
                .bind(thread).fetch_one(&pool).await.unwrap();
            assert_eq!(payload["actor"], serde_json::to_value(actor).unwrap());
        }
    }
    pool.close().await;
    teardown_test_db(&db).await;
}
