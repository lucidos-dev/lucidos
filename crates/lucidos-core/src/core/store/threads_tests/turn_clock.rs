//! `thread_latest_event_created`, the read behind a chat turn's clock.
//! Rendering lives in `engine::chat::process::turn_clock` (ADR 0084).

use super::test_helpers::*;
use super::*;

/// Insert one event at a chosen age, so a test can order rows by `created`
/// independently of insertion order or `sequence`.
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

/// The turn's clock is the NEWEST event, not the one that opened the exchange.
///
/// This is the answer-driven resume. A thread parked on `ask_user_question`
/// survives a restart, and the user answers the next day.
/// `ChatResumeAnchor::ExistingTurn` re-uses the interrupted turn's
/// `request_event_id`, so the resumed events group under the question card.
/// Reading that anchor's own `created` would tell the agent it is yesterday.
#[tokio::test]
async fn the_clock_follows_the_newest_event_not_the_turn_anchor() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread = Uuid::new_v4();
    insert_thread(&pool, thread, "Parked on a question").await;
    // Yesterday's MessageReceived, which stays the turn anchor across the
    // resume, then this morning's answer.
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

/// A backdated row cannot drag the clock backwards.
///
/// `replay_historical_event` stamps a caller-supplied `created` on a row that
/// still takes the next `sequence`. So "newest by sequence" would hand a turn
/// a clock from whenever the replayed event originally happened.
#[tokio::test]
async fn a_backdated_replay_does_not_move_the_clock_backwards() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread = Uuid::new_v4();
    insert_thread(&pool, thread, "Backfilled").await;
    insert_event_aged(&pool, thread, "MessageReceived", 0).await;
    // Inserted last, so it holds the highest sequence, but dated a week back.
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

/// A thread with no events yet reports no clock, so the caller can say which
/// happened rather than silently rendering the epoch.
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
