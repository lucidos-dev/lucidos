use super::*;
use crate::engine::thread_lifecycle::LifecycleViolation;
use crate::test_support::{setup_test_db, teardown_test_db};

async fn home_rows(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM thread_summaries WHERE is_home")
        .fetch_one(pool)
        .await
        .unwrap()
}

const RETIRED_SWITCH: &str = "home_thread_enabled";

const RETIRE_SWITCH_MIGRATION: &str =
    include_str!("../../../lucidos-core/migrations/20261010082810_retire_home_thread_switch.sql");

#[tokio::test]
async fn boot_creates_the_home_thread_in_a_fresh_workspace() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let home = ensure_home_thread(&bus, &pool).await.unwrap();

    assert_eq!(home_rows(&pool).await, 1);
    assert_eq!(home_thread_id(&pool).await.unwrap(), Some(home));
    assert!(is_home_thread(&pool, home).await.unwrap());

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_migration_keeps_an_upgraded_home_and_its_history() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    crate::engine::AuxCapture::for_thread_or_home(
        &bus,
        None,
        crate::engine::ContextPurpose::Memory,
    )
    .record_usage("gemini-3-flash-preview", 120, None)
    .await;
    let hidden = home_thread_id(&pool)
        .await
        .unwrap()
        .expect("the call made Home");
    for device in [None, Some("dev-1")] {
        sqlx::query("INSERT INTO preferences (key, value, device_id) VALUES ($1, 'false', $2)")
            .bind(RETIRED_SWITCH)
            .bind(device)
            .execute(&pool)
            .await
            .unwrap();
    }
    let events_before = events_on(&pool, hidden).await;

    sqlx::raw_sql(RETIRE_SWITCH_MIGRATION)
        .execute(&pool)
        .await
        .expect("the migration runs");
    let booted = ensure_home_thread(&bus, &pool).await.unwrap();

    assert_eq!(booted, hidden, "boot adopts the same Home");
    assert_eq!(home_rows(&pool).await, 1);
    assert_eq!(home_thread_id(&pool).await.unwrap(), Some(hidden));
    assert_eq!(
        events_on(&pool, hidden).await,
        events_before,
        "history kept"
    );
    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM preferences WHERE key = $1")
        .bind(RETIRED_SWITCH)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, 0, "no switch row survives, global or per device");

    teardown_test_db(&db).await;
}

async fn events_on(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<(Uuid, String)> {
    sqlx::query_as("SELECT id, event_type FROM events WHERE thread_id = $1 ORDER BY sequence")
        .bind(thread_id)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn summary_of(pool: &sqlx::PgPool, thread_id: Uuid) -> (String, String, i64, bool, String) {
    sqlx::query_as(
        "SELECT status, archive_state, message_count::bigint, has_response, \
                last_activity::text \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn an_unused_home_costs_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let home = ensure_home_thread(&bus, &pool).await.unwrap();
    let before = summary_of(&pool, home).await;

    for purpose in [
        crate::engine::ContextPurpose::Memory,
        crate::engine::ContextPurpose::Title,
    ] {
        crate::engine::AuxCapture::for_thread_or_home(&bus, None, purpose)
            .record_usage("gemini-3-flash-preview", 50, None)
            .await;
    }

    assert_eq!(summary_of(&pool, home).await, before, "no badge, no move");
    let types: Vec<String> = events_on(&pool, home)
        .await
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert_eq!(
        types,
        ["HomeThreadCreated", "ContextCaptured", "ContextCaptured"],
        "no turn ever started"
    );
    let log = crate::engine::summary_tree::store::load_thread_log_now(
        &pool,
        home,
        crate::engine::summary_tree::log::ThreadKind::LucidosAgent,
    )
    .await
    .unwrap();
    assert!(
        log.entries.is_empty() && log.turns.is_empty(),
        "nothing to summarize"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn ensuring_the_home_thread_twice_creates_one() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let first = ensure_home_thread(&bus, &pool).await.unwrap();
    let second = ensure_home_thread(&bus, &pool).await.unwrap();

    assert_eq!(first, second);
    assert_eq!(home_rows(&pool).await, 1);
    assert!(is_home_thread(&pool, first).await.unwrap());
    assert!(!is_home_thread(&pool, Uuid::new_v4()).await.unwrap());
    let (title, state, source): (String, String, String) =
        sqlx::query_as("SELECT title, state, source FROM thread_summaries WHERE thread_id = $1")
            .bind(first)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (title.as_str(), state.as_str(), source.as_str()),
        (HOME_THREAD_TITLE, "active", "chat")
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_second_home_thread_is_refused_and_leaves_no_event() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    ensure_home_thread(&bus, &pool).await.unwrap();

    let impostor = Uuid::new_v4();
    let emitted = bus
        .emit(BusEvent::Thread {
            thread_id: impostor,
            event: ThreadEvent::HomeThreadCreated,
            meta: EventMeta::NONE,
        })
        .await;

    assert!(
        emitted.is_err(),
        "the unique marker must refuse a second home"
    );
    assert_eq!(home_rows(&pool).await, 1);
    let events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = 'HomeThreadCreated'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(events, 1);
    assert!(!is_home_thread(&pool, impostor).await.unwrap());

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_bus_refuses_to_archive_the_home_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let home = ensure_home_thread(&bus, &pool).await.unwrap();

    let refused = bus
        .emit(BusEvent::Thread {
            thread_id: home,
            event: ThreadEvent::ThreadArchived,
            meta: EventMeta::NONE,
        })
        .await
        .err()
        .expect("the bus must refuse to archive the home thread");

    assert!(
        refused.downcast_ref::<LifecycleViolation>().is_some(),
        "a typed lifecycle refusal, so a cascade reports it as left open: {refused}"
    );
    let state: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(home)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "inbox");

    teardown_test_db(&db).await;
}

async fn title_of(pool: &sqlx::PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT title FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_home_thread_takes_a_rename_and_refuses_a_generated_title() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let home = ensure_home_thread(&bus, &pool).await.unwrap();
    let generated = |title: &str| BusEvent::Thread {
        thread_id: home,
        event: ThreadEvent::ThreadTitleGenerated {
            title: title.to_string(),
        },
        meta: EventMeta::NONE,
    };

    let refused = bus
        .emit(generated("Fixing Thinking Indicator Bug"))
        .await
        .err()
        .expect("the bus must refuse a generated title on the home thread");
    assert!(
        refused.downcast_ref::<LifecycleViolation>().is_some(),
        "a typed lifecycle refusal: {refused}"
    );
    assert_eq!(title_of(&pool, home).await, HOME_THREAD_TITLE);

    bus.emit(BusEvent::Thread {
        thread_id: home,
        event: ThreadEvent::ThreadTitleRenamed {
            title: "Kitchen table".to_string(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("a rename by hand lands on the home thread");
    assert_eq!(title_of(&pool, home).await, "Kitchen table");

    assert!(bus.emit(generated("Another topic")).await.is_err());
    assert_eq!(title_of(&pool, home).await, "Kitchen table");

    teardown_test_db(&db).await;
}
