//! A card nobody waits for closes as `Superseded` (ADR 0803).

use super::*;
use crate::engine::agent_host::NoAgentHost;
use crate::engine::event_bus::BusEvent;
use crate::engine::permission::{menu, raise_request, raise_turn_holding_request, RequestCard};
use crate::engine::thread_events::{EventChannel, EventMeta};
use crate::test_support::{
    boot_mock_engine_with_host, end_coding_turn, message_received, seed_coding_thread_as,
    setup_test_db, teardown_test_db,
};

async fn boot(db: &str) -> (Arc<LucidosEngine>, tempfile::TempDir, Uuid) {
    let thread_id = Uuid::new_v4();
    let host = NoAgentHost {
        held_message_release_thread: Some(thread_id),
    };
    let (engine, workspace) = boot_mock_engine_with_host(db, Arc::new(host)).await;
    seed_coding_thread_as(&engine.event_bus, thread_id, None).await;
    (engine, workspace, thread_id)
}

fn card(thread_id: Uuid) -> RequestCard {
    let action = crate::engine::permission::Action::CreateWorkspace;
    RequestCard {
        question: "Create the workspace?".into(),
        header: "Permission",
        action,
        breadth: Some("demo".into()),
        options: menu::options(action, thread_id, "Nothing was created."),
        batch: None,
    }
}

async fn emit(engine: &LucidosEngine, thread_id: Uuid, event: ThreadEvent) {
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
}

async fn last_event(engine: &LucidosEngine, thread_id: Uuid) -> ThreadEvent {
    let (event_type, payload): (String, serde_json::Value) = sqlx::query_as(
        "SELECT event_type, payload FROM events WHERE thread_id = $1 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(engine.pool())
    .await
    .unwrap();
    ThreadEvent::from_stored(&event_type, payload).unwrap()
}

async fn superseded(engine: &LucidosEngine, thread_id: Uuid, card: &str) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events WHERE thread_id = $1 \
           AND event_type = 'UserQuestionAnswered' AND payload->>'tool_use_id' = $2 \
           AND payload->'answer'->>'kind' = 'Superseded')",
    )
    .bind(thread_id)
    .bind(card)
    .fetch_one(engine.pool())
    .await
    .unwrap()
}

/// An archive retires the open card and the one queued behind it.
#[tokio::test]
async fn archiving_retires_every_card() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot(&db).await;
    end_coding_turn(&engine.event_bus, thread_id).await;
    let open = raise_request(
        &engine.event_bus,
        thread_id,
        card(thread_id),
        EventChannel::ClaudeCode,
    )
    .await
    .unwrap();
    let queued = raise_request(
        &engine.event_bus,
        thread_id,
        card(thread_id),
        EventChannel::ClaudeCode,
    )
    .await
    .unwrap();
    // Lifecycle refuses to archive a thread waiting on its owner, so the test
    // hands the event in directly.
    retire_unwanted_cards(&engine, thread_id, &ThreadEvent::ThreadArchived)
        .await
        .unwrap();
    assert!(superseded(&engine, thread_id, &queued).await);
    assert!(superseded(&engine, thread_id, &open).await);
    teardown_test_db(&db).await;
}

/// A turn-holding card retires when its turn ends. A card that waits for the
/// owner past a turn's end stays.
#[tokio::test]
async fn a_turn_end_retires_only_the_card_it_held() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot(&db).await;
    let held = raise_turn_holding_request(
        &engine.event_bus,
        thread_id,
        card(thread_id),
        EventChannel::ClaudeCode,
    )
    .await
    .unwrap();
    let waiting = raise_request(
        &engine.event_bus,
        thread_id,
        card(thread_id),
        EventChannel::ClaudeCode,
    )
    .await
    .unwrap();
    end_coding_turn(&engine.event_bus, thread_id).await;
    let ended = last_event(&engine, thread_id).await;
    retire_unwanted_cards(&engine, thread_id, &ended)
        .await
        .unwrap();
    assert!(superseded(&engine, thread_id, &held).await);
    assert!(!superseded(&engine, thread_id, &waiting).await);

    emit(&engine, thread_id, message_received("next")).await;
    assert!(!superseded(&engine, thread_id, &waiting).await);
    teardown_test_db(&db).await;
}

/// The boot sweep retires a stored step card and the step card queued behind
/// it. An ordinary card on the same thread stays.
#[tokio::test]
async fn the_boot_sweep_retires_only_step_cards() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot(&db).await;
    end_coding_turn(&engine.event_bus, thread_id).await;
    let step_card = || RequestCard {
        action: crate::engine::permission::Action::Harden,
        ..card(thread_id)
    };
    let open_step = raise_request(
        &engine.event_bus,
        thread_id,
        step_card(),
        EventChannel::ClaudeCode,
    )
    .await
    .unwrap();
    let queued_step = raise_request(
        &engine.event_bus,
        thread_id,
        step_card(),
        EventChannel::ClaudeCode,
    )
    .await
    .unwrap();
    let ordinary = raise_request(
        &engine.event_bus,
        thread_id,
        card(thread_id),
        EventChannel::ClaudeCode,
    )
    .await
    .unwrap();
    retire_step_cards(&engine).await.unwrap();
    assert!(superseded(&engine, thread_id, &open_step).await);
    assert!(superseded(&engine, thread_id, &queued_step).await);
    assert!(!superseded(&engine, thread_id, &ordinary).await);
    teardown_test_db(&db).await;
}
