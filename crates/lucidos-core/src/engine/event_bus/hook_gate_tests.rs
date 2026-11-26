use std::time::Duration;

use serde_json::{json, Map, Value};
use tokio::sync::Notify;

use super::*;
use crate::core::event_hooks::{ReturnedEvent, REFUSAL_AGGREGATE};
use crate::test_support::{
    arm_hook, hook_refuses, message_received, setup_test_db, teardown_test_db, FakeHookAsker,
};

fn allow(event_type: &str) -> HookVerdict {
    let mut payload = Map::new();
    payload.insert("summary".into(), json!("Noted"));
    HookVerdict::Allowed(vec![ReturnedEvent {
        event_type: event_type.into(),
        payload,
    }])
}

async fn count(pool: &sqlx::PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

async fn rows_of(pool: &sqlx::PgPool, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = $1")
        .bind(event_type)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn apply_requested() -> ThreadEvent {
    ThreadEvent::ChangeApplyRequested {
        change_id: Uuid::new_v4().to_string(),
        branch_name: "feature".into(),
        repo_root: "/repo".into(),
    }
}

#[tokio::test]
async fn an_unhooked_ask_is_one_in_memory_lookup() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let asker = FakeHookAsker::new(hook_refuses("never"));
    bus.install_event_hook_asker(asker.clone()).await.unwrap();
    pool.close().await;

    let thread_id = Uuid::new_v4();
    let event = message_received("hello");
    let asked = bus.ask_hooks(thread_id, &event, &EventMeta::NONE).await;
    assert!(asked.is_ok(), "{asked:?}");
    assert!(asker.asked().is_empty());

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_refusal_is_recorded_and_stops_the_event() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    bus.install_event_hook_asker(FakeHookAsker::new(hook_refuses("No plan approved.")))
        .await
        .unwrap();
    arm_hook(&bus, "require-a-plan", "ChangeApplyRequested").await;

    let thread_id = Uuid::new_v4();
    let actor = MessageOrigin::Device {
        device_id: "my-macbook".into(),
    };
    let refusal = bus
        .ask_hooks(
            thread_id,
            &apply_requested(),
            &EventMeta::with_actor(Some(actor.clone())),
        )
        .await
        .unwrap_err();
    assert_eq!(refusal.slug.as_deref(), Some("require-a-plan"));
    assert_eq!(refusal.cause, RefusalCause::Refused);
    assert!(
        refusal.to_string().contains("No plan approved."),
        "{refusal}"
    );

    let (aggregate, payload): (String, Value) = sqlx::query_as(
        "SELECT aggregate, payload FROM events WHERE event_type = 'EventHookRefused'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(aggregate, REFUSAL_AGGREGATE);
    let data = &payload["data"];
    assert_eq!(data["slug"], "require-a-plan");
    assert_eq!(data["event_type"], "ChangeApplyRequested");
    assert_eq!(data["thread_id"], json!(thread_id));
    assert_eq!(data["cause"], "refused");
    assert_eq!(data["actor"]["device_id"], "my-macbook");
    assert_eq!(rows_of(&pool, "ChangeApplyRequested").await, 0);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_refusal_leaves_the_hook_armed() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let asker = FakeHookAsker::new(hook_refuses("no"));
    bus.install_event_hook_asker(asker.clone()).await.unwrap();
    arm_hook(&bus, "gate", "ChangeApplyRequested").await;

    for _ in 0..2 {
        let asked = bus
            .ask_hooks(Uuid::new_v4(), &apply_requested(), &EventMeta::NONE)
            .await;
        assert!(asked.is_err());
    }
    assert_eq!(asker.asked().len(), 2);
    let armed = crate::core::event_hooks::armed_hooks(&pool).await.unwrap();
    assert_eq!(armed.len(), 1);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn returned_events_commit_with_the_asked_event() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    bus.install_event_hook_asker(FakeHookAsker::new(allow("TicketLinked")))
        .await
        .unwrap();
    arm_hook(&bus, "link-ticket", "MessageReceived").await;

    let thread_id = Uuid::new_v4();
    let event = message_received("start the work");
    let asked = bus
        .ask_hooks(thread_id, &event, &EventMeta::NONE)
        .await
        .unwrap();
    let emitted = bus
        .emit_asked(asked, thread_id, event, EventMeta::NONE)
        .await
        .unwrap();
    assert!(emitted.is_some());

    let payload: Value =
        sqlx::query_scalar("SELECT payload FROM events WHERE event_type = 'TicketLinked'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(payload["summary"], "Noted");
    assert_eq!(
        payload["actor"],
        json!({"kind": "event_hook", "slug": "link-ticket"})
    );
    assert_eq!(rows_of(&pool, "MessageReceived").await, 1);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn returned_events_drop_when_validate_drops_the_event() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    bus.install_event_hook_asker(FakeHookAsker::new(allow("TicketLinked")))
        .await
        .unwrap();
    arm_hook(&bus, "link-ticket", "MessageReceived").await;

    let thread_id = Uuid::new_v4();
    let event = message_received("held");
    let meta = EventMeta {
        queued_for_permission: true,
        ..EventMeta::NONE
    };
    let asked = bus.ask_hooks(thread_id, &event, &meta).await.unwrap();
    let emitted = bus.emit_asked(asked, thread_id, event, meta).await.unwrap();
    assert!(emitted.is_none());
    assert_eq!(rows_of(&pool, "TicketLinked").await, 0);
    assert_eq!(rows_of(&pool, "MessageReceived").await, 0);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_hook_returns_domain_events_only() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    bus.install_event_hook_asker(FakeHookAsker::new(allow("ChangeApplied")))
        .await
        .unwrap();
    arm_hook(&bus, "sneaky", "ChangeApplyRequested").await;

    let refusal = bus
        .ask_hooks(Uuid::new_v4(), &apply_requested(), &EventMeta::NONE)
        .await
        .unwrap_err();
    assert_eq!(refusal.cause, RefusalCause::InvalidAnswer);
    assert!(refusal.reason.contains("domain events only"), "{refusal}");
    assert_eq!(rows_of(&pool, "ChangeApplied").await, 0);
    assert_eq!(rows_of(&pool, "EventHookRefused").await, 1);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn no_hook_is_asked_about_an_event_a_hook_caused() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let asker = FakeHookAsker::new(hook_refuses("no"));
    bus.install_event_hook_asker(asker.clone()).await.unwrap();
    arm_hook(&bus, "gate", "MessageReceived").await;

    let hook = MessageOrigin::EventHook {
        slug: "gate".into(),
    };
    let mut by_origin = message_received("from the hook's own process");
    if let ThreadEvent::MessageReceived { origin, .. } = &mut by_origin {
        *origin = Some(hook.clone());
    }
    let asked = bus
        .ask_hooks(Uuid::new_v4(), &by_origin, &EventMeta::NONE)
        .await;
    assert!(asked.is_ok(), "{asked:?}");
    let asked = bus
        .ask_hooks(
            Uuid::new_v4(),
            &message_received("by meta"),
            &EventMeta::with_actor(Some(hook)),
        )
        .await;
    assert!(asked.is_ok(), "{asked:?}");
    assert!(asker.asked().is_empty());

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn only_a_threads_first_message_is_asked() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let asker = FakeHookAsker::new(HookVerdict::Allowed(Vec::new()));
    bus.install_event_hook_asker(asker.clone()).await.unwrap();
    arm_hook(&bus, "gate", "MessageReceived").await;

    let thread_id = Uuid::new_v4();
    let first = message_received("first");
    let asked = bus
        .ask_hooks(thread_id, &first, &EventMeta::NONE)
        .await
        .unwrap();
    bus.emit_asked(asked, thread_id, first, EventMeta::NONE)
        .await
        .unwrap();
    let asked = asker.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].payload[FIRST_MESSAGE_KEY], json!(true));
    assert_eq!(asked[0].payload["text"], "first");

    let follow_up = message_received("second");
    let asked = bus.ask_hooks(thread_id, &follow_up, &EventMeta::NONE).await;
    assert!(asked.is_ok(), "{asked:?}");
    assert_eq!(asker.asked().len(), 1);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_hook_runs_outside_every_transaction() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let hold = Arc::new(Notify::new());
    let asker = FakeHookAsker::held(HookVerdict::Allowed(Vec::new()), Some(hold.clone()));
    bus.install_event_hook_asker(asker.clone()).await.unwrap();
    arm_hook(&bus, "slow", "ChangeApplyRequested").await;

    let hooked_thread = Uuid::new_v4();
    let slow_bus = bus.clone();
    let slow = tokio::spawn(async move {
        let event = apply_requested();
        let asked = slow_bus
            .ask_hooks(hooked_thread, &event, &EventMeta::NONE)
            .await
            .unwrap();
        slow_bus
            .emit_asked(asked, hooked_thread, event, EventMeta::NONE)
            .await
            .unwrap()
    });
    while asker.asked().is_empty() {
        tokio::task::yield_now().await;
    }
    let other = tokio::time::timeout(
        Duration::from_secs(10),
        bus.emit(BusEvent::Thread {
            thread_id: Uuid::new_v4(),
            event: message_received("meanwhile"),
            meta: EventMeta::NONE,
        }),
    )
    .await
    .expect("an unrelated append waited on a hook");
    assert!(other.unwrap().is_some());

    hold.notify_one();
    assert!(slow.await.unwrap().is_some());

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_token_records_only_the_event_it_was_minted_for() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let event = apply_requested();
    let asked = bus
        .ask_hooks(thread_id, &event, &EventMeta::NONE)
        .await
        .unwrap();
    let err = bus
        .emit_asked(asked, Uuid::new_v4(), event, EventMeta::NONE)
        .await
        .err()
        .expect("a token for another event records nothing");
    assert!(err.to_string().contains("Asked token"), "{err}");

    let asked = bus
        .ask_hooks(thread_id, &apply_requested(), &EventMeta::NONE)
        .await
        .unwrap();
    let err = bus
        .emit_asked(asked, thread_id, message_received("x"), EventMeta::NONE)
        .await
        .err()
        .expect("a token for another event records nothing");
    assert!(err.to_string().contains("Asked token"), "{err}");
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM events WHERE aggregate = 'thread'"
        )
        .await,
        0
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_armed_hook_with_no_runner_refuses() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    arm_hook(&bus, "gate", "ChangeApplyRequested").await;

    let refusal = bus
        .ask_hooks(Uuid::new_v4(), &apply_requested(), &EventMeta::NONE)
        .await
        .unwrap_err();
    assert_eq!(refusal.cause, RefusalCause::Unavailable);
    assert_eq!(rows_of(&pool, "EventHookRefused").await, 1);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_arming_event_refreshes_the_armed_hooks() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    bus.install_event_hook_asker(FakeHookAsker::new(hook_refuses("no")))
        .await
        .unwrap();
    arm_hook(&bus, "gate", "ChangeApplyRequested").await;
    assert!(bus.hooks_armed_on("ChangeApplyRequested").await.unwrap());

    bus.emit(BusEvent::System(SystemEvent::EventHookDisarmed {
        slug: "gate".into(),
        actor: None,
    }))
    .await
    .unwrap();
    assert!(!bus.hooks_armed_on("ChangeApplyRequested").await.unwrap());
    let asked = bus
        .ask_hooks(Uuid::new_v4(), &apply_requested(), &EventMeta::NONE)
        .await;
    assert!(asked.is_ok(), "{asked:?}");

    pool.close().await;
    teardown_test_db(&db).await;
}
