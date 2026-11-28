use crate::core::event_subscription::{condition, matchable_system_payload};
use crate::engine::event_bus::{EventBus, SystemEvent};
use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;

fn matches_device(payload: &serde_json::Value) -> bool {
    condition::evaluate(Some(&json!({"actor.kind": "device"})), payload)
}

fn forged_device_payload() -> serde_json::Value {
    json!({
        "summary": "stamp the film script",
        "actor": {"kind": "device", "device_id": "forged", "label": "My iPhone"},
    })
}

#[test]
fn a_caller_written_actor_never_reaches_the_matcher() {
    let event = SystemEvent::DomainEvent {
        event_type: "FilmScriptStampRequested".to_string(),
        payload: forged_device_payload(),
        depth: 0,
        transient: false,
        actor: None,
    };
    let matchable = matchable_system_payload(&event);
    assert!(
        !matches_device(&matchable),
        "a forged actor matched: {matchable}"
    );
    assert!(matchable.get("actor").is_none(), "{matchable}");
    assert_eq!(matchable["summary"], "stamp the film script");
}

#[test]
fn the_engine_actor_replaces_a_caller_written_one() {
    let event = SystemEvent::DomainEvent {
        event_type: "FilmScriptStampRequested".to_string(),
        payload: forged_device_payload(),
        depth: 0,
        transient: true,
        actor: Some(MessageOrigin::Api {
            user_agent: None,
            mode: ActorMode::Agent,
            source_thread_id: None,
            emitting_trigger_id: None,
        }),
    };
    let payload = event.to_payload();
    assert_eq!(payload["actor"], json!({"kind": "api", "mode": "agent"}));
    assert!(!matches_device(&matchable_system_payload(&event)));
}

#[tokio::test]
async fn the_emit_event_tool_cannot_forge_a_device_actor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let args = json!({
        "event_type": "FilmScriptStampRequested",
        "payload": forged_device_payload(),
    });
    let text = crate::engine::tools::emit_event_impl(&bus, &args, None)
        .await
        .expect("the emit succeeds");
    assert!(text.contains("`actor` was dropped"), "{text}");

    let stored: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE event_type = 'FilmScriptStampRequested'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        stored["actor"],
        json!({"kind": "agent", "agent": {"kind": "lucidos_agent"}})
    );
    assert_eq!(stored["summary"], "stamp the film script");
    assert!(!matches_device(&stored), "the stored row matched: {stored}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_emit_event_tool_stamps_the_agent_without_a_warning() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let args = json!({"event_type": "HabitCompleted", "payload": {"summary": "ran"}});
    let text = crate::engine::tools::emit_event_impl(&bus, &args, None)
        .await
        .expect("the emit succeeds");
    assert!(!text.contains("dropped"), "{text}");

    let kind: String = sqlx::query_scalar(
        "SELECT payload->'actor'->>'kind' FROM events WHERE event_type = 'HabitCompleted'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(kind, "agent");

    pool.close().await;
    teardown_test_db(&db_name).await;
}
