//! A thread's own events project its enabled MCP servers. A stop or a remove
//! switches a server off in every thread (ADR 0430).

use super::*;
use crate::engine::thread_events::McpEnablementSource;

async fn enabled(pool: &PgPool, thread_id: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT enabled_mcp_servers FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

fn enable(server_id: &str) -> ThreadEvent {
    ThreadEvent::ThreadMcpServerEnabled {
        server_id: server_id.into(),
        by: McpEnablementSource::Agent,
    }
}

async fn chat_thread(bus: &EventBus) -> Uuid {
    let thread_id = Uuid::new_v4();
    emit_thread_message(bus, thread_id, None, "check slack for me").await;
    thread_id
}

#[tokio::test]
async fn enabling_adds_the_id_once_and_disabling_removes_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = chat_thread(&bus).await;
    assert!(enabled(&pool, thread_id).await.is_empty());

    emit(&bus, thread_id, enable("slack")).await;
    emit(&bus, thread_id, enable("slack")).await;
    emit(&bus, thread_id, enable("jira")).await;
    assert_eq!(enabled(&pool, thread_id).await, ["slack", "jira"]);

    emit(
        &bus,
        thread_id,
        ThreadEvent::ThreadMcpServerDisabled {
            server_id: "slack".into(),
            by: McpEnablementSource::User,
        },
    )
    .await;
    assert_eq!(enabled(&pool, thread_id).await, ["jira"]);

    teardown_test_db(&db_name).await;
}

/// The invariant "stop reaches every thread": two threads enabled one server,
/// and one stop empties both, while a third server stays where it was.
#[tokio::test]
async fn a_stop_or_a_remove_switches_the_server_off_in_every_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let first = chat_thread(&bus).await;
    let second = chat_thread(&bus).await;
    for thread_id in [first, second] {
        emit(&bus, thread_id, enable("slack")).await;
        emit(&bus, thread_id, enable("jira")).await;
    }

    bus.emit(BusEvent::System(SystemEvent::McpServerStopped {
        server_id: "slack".into(),
        actor: None,
    }))
    .await
    .unwrap();
    assert_eq!(enabled(&pool, first).await, ["jira"]);
    assert_eq!(enabled(&pool, second).await, ["jira"]);

    bus.emit(BusEvent::System(SystemEvent::McpServerRemoved {
        server_id: "jira".into(),
        actor: None,
    }))
    .await
    .unwrap();
    assert!(enabled(&pool, first).await.is_empty());
    assert!(enabled(&pool, second).await.is_empty());

    teardown_test_db(&db_name).await;
}

/// Enabling is bookkeeping: it moves no status, section or recency.
#[tokio::test]
async fn enabling_leaves_the_thread_where_it_was() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = chat_thread(&bus).await;
    let read = || async {
        sqlx::query_as::<_, (String, String, chrono::DateTime<chrono::Utc>)>(
            "SELECT status, archive_state, last_activity FROM thread_summaries \
             WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let before = read().await;

    emit(&bus, thread_id, enable("slack")).await;

    assert_eq!(before, read().await);

    teardown_test_db(&db_name).await;
}
