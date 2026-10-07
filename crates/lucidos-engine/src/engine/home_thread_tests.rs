//! One home thread per workspace (ADR 0362, invariant I10): created once,
//! unique in the database, and never archived or auto-titled by any emitter.

use super::*;
use crate::engine::thread_lifecycle::LifecycleViolation;
use crate::test_support::{seed_preference, setup_test_db, teardown_test_db};

async fn home_rows(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM thread_summaries WHERE is_home")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Boot creates nothing in a workspace that never turned the switch on.
#[tokio::test]
async fn boot_creates_no_home_thread_while_the_switch_is_off() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let created = ensure_home_thread_if_enabled(&bus, &pool).await.unwrap();

    assert_eq!(created, None);
    assert_eq!(home_rows(&pool).await, 0);

    teardown_test_db(&db).await;
}

/// Off hides the marked thread from every gated reader. On again brings the
/// same thread back, and ensuring never makes a second one.
#[tokio::test]
async fn the_switch_hides_the_home_thread_and_brings_the_same_one_back() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let home = enabled_home_thread(&bus, &pool).await;
    assert_eq!(home_thread_id(&pool).await.unwrap(), Some(home));

    seed_preference(
        &pool,
        crate::core::prefs::HOME_THREAD_ENABLED.key(),
        "false",
    )
    .await
    .unwrap();
    assert_eq!(home_thread_id(&pool).await.unwrap(), None);
    assert!(!is_home_thread(&pool, home).await.unwrap());
    assert_eq!(
        home_rows(&pool).await,
        1,
        "off hides the row, never removes it"
    );

    seed_preference(&pool, crate::core::prefs::HOME_THREAD_ENABLED.key(), "true")
        .await
        .unwrap();
    assert_eq!(
        ensure_home_thread_if_enabled(&bus, &pool).await.unwrap(),
        Some(home)
    );
    assert!(is_home_thread(&pool, home).await.unwrap());
    assert_eq!(home_rows(&pool).await, 1);

    teardown_test_db(&db).await;
}

/// Boot calls this every time. Only the first call creates a thread.
#[tokio::test]
async fn ensuring_the_home_thread_twice_creates_one() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let first = enabled_home_thread(&bus, &pool).await;
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

/// A second `HomeThreadCreated` is refused by the database marker, and the
/// refusal leaves no event behind: the log never records two homes.
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

/// Every archive path ends in a `ThreadArchived` emit, and the bus refuses
/// that emit on the home thread. The row stays in the inbox.
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

/// Only the user names the home thread. The bus refuses a generated title,
/// whoever emits it, and a rename by hand lands and stays. A generated title
/// after the rename is refused too, so the user's name survives.
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
    assert!(is_marked_home_thread(&pool, home).await.unwrap());
    assert!(!is_marked_home_thread(&pool, Uuid::new_v4()).await.unwrap());

    teardown_test_db(&db).await;
}
