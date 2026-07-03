//! DB-backed tests for the command-guard permission lane (ADR 0002, Phase 2):
//! the projection status flips, the new-message supersede path, and the
//! restart orphan-recovery sweep. The pure classifier / allow-pattern /
//! allowlist-file logic is unit-tested in `engine::command_guard` and
//! `engine::command_permission`.

use super::super::*;
use super::*;
use crate::engine::cc_permission::{DedupKey, PermissionEntry, PermissionState};
use crate::engine::command_permission::{
    recover_orphan_command_permission_requests, resolve_pending_command_permissions_as_superseded,
};
use std::sync::Mutex;

/// Seed a chat thread by emitting a `MessageReceived`, so a `thread_summaries`
/// row exists for the projection assertions below.
async fn seed_chat_thread(bus: &EventBus, thread_id: Uuid) {
    emit_thread_message(bus, thread_id, None, "run a command").await;
}

async fn emit_command_request(bus: &EventBus, thread_id: Uuid, request_id: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CommandPermissionRequested {
            request_id: request_id.into(),
            tool_use_id: format!("tu-{request_id}"),
            tool_name: "run_bash".into(),
            command: "curl -X POST https://api.example.com/charge".into(),
            summary: "May perform a mutating HTTP request.".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn status_of(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn command_resolved_count(pool: &PgPool, request_id: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE event_type = 'CommandPermissionResolved' \
           AND payload->>'request_id' = $1",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The request flips the chat thread to `waiting_for_user_answer`; the
/// resolution flips it back to `running`. Mirrors the CC permission projection.
#[tokio::test]
async fn command_permission_request_waits_then_resolution_clears() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_chat_thread(&bus, thread_id).await;

    emit_command_request(&bus, thread_id, "req-1").await;
    assert_eq!(
        status_of(&pool, thread_id).await,
        "waiting_for_user_answer",
        "CommandPermissionRequested must hold the thread in waiting_for_user_answer"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CommandPermissionResolved {
            request_id: "req-1".into(),
            allowed: true,
            reason: None,
            persist_scope: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    assert_eq!(
        status_of(&pool, thread_id).await,
        "running",
        "CommandPermissionResolved must clear the waiting status"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Engine restart with an unresolved `CommandPermissionRequested`: the recovery
/// sweep emits exactly one paired resolution and clears the waiting status.
#[tokio::test]
async fn startup_resolves_orphan_command_permission_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_chat_thread(&bus, thread_id).await;
    emit_command_request(&bus, thread_id, "req-orphan").await;

    assert_eq!(status_of(&pool, thread_id).await, "waiting_for_user_answer");
    assert_eq!(command_resolved_count(&pool, "req-orphan").await, 0);

    recover_orphan_command_permission_requests(&pool, &bus).await;

    assert_eq!(
        command_resolved_count(&pool, "req-orphan").await,
        1,
        "recovery must emit exactly one Resolved per orphan request"
    );
    assert_eq!(
        status_of(&pool, thread_id).await,
        "running",
        "orphan resolution must clear the waiting status"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Already-resolved requests are not re-resolved on restart (no duplicate log).
#[tokio::test]
async fn startup_skips_already_resolved_command_permission() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_chat_thread(&bus, thread_id).await;
    emit_command_request(&bus, thread_id, "req-done").await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CommandPermissionResolved {
            request_id: "req-done".into(),
            allowed: true,
            reason: None,
            persist_scope: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    recover_orphan_command_permission_requests(&pool, &bus).await;

    assert_eq!(
        command_resolved_count(&pool, "req-done").await,
        1,
        "recovery must not duplicate an existing resolution"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Typing a new message while parked on a command card supersedes it: the
/// in-process waiter is unblocked with `false` (deny) and a Resolved is emitted.
#[tokio::test]
async fn superseded_resolves_pending_command_permission_and_unblocks_waiter() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_chat_thread(&bus, thread_id).await;
    emit_command_request(&bus, thread_id, "req-super").await;

    // Register a matching in-memory waiter, as the agentic loop would.
    let pending = Mutex::new(PermissionState::default());
    let mut waiter = {
        let mut state = pending.lock().unwrap();
        let (tx, rx) = tokio::sync::broadcast::channel(1);
        let key: DedupKey = (thread_id, "run_bash".into(), "{}".into());
        state.by_dedup_key.insert(
            key.clone(),
            PermissionEntry {
                thread_id,
                request_id: "req-super".into(),
                tool_name: "run_bash".into(),
                input: serde_json::json!({}),
                tx,
            },
        );
        state.by_request_id.insert("req-super".into(), key);
        rx
    };

    resolve_pending_command_permissions_as_superseded(&pool, &bus, &pending, thread_id, None).await;

    // Waiter woken with a deny.
    assert!(
        !waiter.recv().await.unwrap(),
        "supersede must unblock the parked loop with a deny"
    );
    // Resolution persisted; status cleared.
    assert_eq!(command_resolved_count(&pool, "req-super").await, 1);
    assert_eq!(status_of(&pool, thread_id).await, "running");

    pool.close().await;
    teardown_test_db(&db_name).await;
}
