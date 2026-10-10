//! `thread_summaries.summary_version` goes up by one on every change to a row,
//! whoever writes it, and on nothing else. The client orders every summary it
//! receives by this number, so a writer that could skip the bump could put a
//! stale status on screen. Raw SQL is the writer these tests use, because it is
//! the one no Rust helper could ever have covered.

use super::test_helpers::*;
use super::*;

async fn version_of(pool: &PgPool, id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT summary_version FROM thread_summaries WHERE thread_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("read summary_version")
}

async fn run(pool: &PgPool, sql: &str, id: Uuid) {
    sqlx::query(sql)
        .bind(id)
        .execute(pool)
        .await
        .expect("update thread_summaries");
}

#[tokio::test]
async fn a_new_row_starts_at_zero() {
    let (pool, db) = setup_test_db().await;
    let id = Uuid::new_v4();
    insert_thread(&pool, id, "t").await;
    assert_eq!(version_of(&pool, id).await, 0);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_insert_cannot_choose_its_version() {
    let (pool, db) = setup_test_db().await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, summary_version) \
         VALUES ($1, 't', 'chat', 0, NOW(), 99)",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect("insert");
    assert_eq!(version_of(&pool, id).await, 0);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn every_change_bumps_the_version_by_one() {
    let (pool, db) = setup_test_db().await;
    let id = Uuid::new_v4();
    insert_thread(&pool, id, "t").await;

    run(
        &pool,
        "UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1",
        id,
    )
    .await;
    assert_eq!(version_of(&pool, id).await, 1, "a status change");

    run(
        &pool,
        "UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1",
        id,
    )
    .await;
    assert_eq!(version_of(&pool, id).await, 2, "a section change");

    run(
        &pool,
        "UPDATE thread_summaries SET compose_text = 'draft' WHERE thread_id = $1",
        id,
    )
    .await;
    assert_eq!(
        version_of(&pool, id).await,
        3,
        "a column no aggregate carries"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_update_that_changes_nothing_keeps_the_version() {
    let (pool, db) = setup_test_db().await;
    let id = Uuid::new_v4();
    insert_thread(&pool, id, "t").await;
    run(
        &pool,
        "UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1",
        id,
    )
    .await;

    run(
        &pool,
        "UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1",
        id,
    )
    .await;
    assert_eq!(version_of(&pool, id).await, 1);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_update_cannot_choose_its_version() {
    let (pool, db) = setup_test_db().await;
    let id = Uuid::new_v4();
    insert_thread(&pool, id, "t").await;

    run(
        &pool,
        "UPDATE thread_summaries SET summary_version = 1000 WHERE thread_id = $1",
        id,
    )
    .await;
    assert_eq!(
        version_of(&pool, id).await,
        0,
        "a version-only write is a no-op"
    );

    run(
        &pool,
        "UPDATE thread_summaries SET summary_version = 1000, status = 'running' WHERE thread_id = $1",
        id,
    )
    .await;
    assert_eq!(
        version_of(&pool, id).await,
        1,
        "the trigger overrides a written version"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_aggregate_and_the_summary_carry_the_row_version() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let id = Uuid::new_v4();
    insert_thread(&pool, id, "t").await;
    run(
        &pool,
        "UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1",
        id,
    )
    .await;

    let aggregate = fetch_thread_aggregate(&pool, id)
        .await
        .expect("fetch aggregate")
        .expect("row exists");
    assert_eq!(aggregate.summary_version, 1);

    let summary = store
        .get_recent_threads(10)
        .await
        .expect("get_recent_threads")
        .into_iter()
        .find(|t| t.thread_id == id.to_string())
        .expect("thread listed");
    assert_eq!(summary.summary_version, 1);
    teardown_test_db(&db).await;
}
