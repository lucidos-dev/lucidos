use super::test_helpers::*;
use super::*;

async fn insert_event_aged(pool: &PgPool, thread_id: Uuid, event_type: &str, hours_ago: i64) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id, created) \
         VALUES ($1, $2, '{}'::jsonb, $3, 'thread', $3::text, NOW() - make_interval(hours => $4::int))",
    )
    .bind(Uuid::new_v4())
    .bind(event_type)
    .bind(thread_id)
    .bind(hours_ago as i32)
    .execute(pool)
    .await
    .expect("insert aged event");
}

#[tokio::test]
async fn the_clock_follows_the_newest_event_not_the_turn_anchor() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread = Uuid::new_v4();
    insert_thread(&pool, thread, "Parked on a question").await;
    insert_event_aged(&pool, thread, "MessageReceived", 20).await;
    insert_event_aged(&pool, thread, "UserQuestionAnswered", 0).await;

    let clock = store
        .thread_latest_event_created(thread)
        .await
        .expect("read the clock")
        .expect("a thread with events has a clock");

    let age = chrono::Utc::now() - clock;
    assert!(
        age < chrono::Duration::minutes(5),
        "the clock read {age} old, so it followed the anchor rather than the answer"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_backdated_replay_does_not_move_the_clock_backwards() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread = Uuid::new_v4();
    insert_thread(&pool, thread, "Backfilled").await;
    insert_event_aged(&pool, thread, "MessageReceived", 0).await;
    insert_event_aged(&pool, thread, "ImageDescribed", 168).await;

    let clock = store
        .thread_latest_event_created(thread)
        .await
        .expect("read the clock")
        .expect("a thread with events has a clock");

    let age = chrono::Utc::now() - clock;
    assert!(
        age < chrono::Duration::minutes(5),
        "the clock read {age} old, so a backdated replay moved it"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_thread_with_no_events_has_no_clock() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread = Uuid::new_v4();
    insert_thread(&pool, thread, "Fresh").await;

    let clock = store
        .thread_latest_event_created(thread)
        .await
        .expect("read the clock");
    assert!(clock.is_none(), "an eventless thread must report no clock");

    teardown_test_db(&db).await;
}
