//! A composer submission consumes the thread's compose slot once, at accept
//! (ADR 0450). The epoch always rises. The stored draft clears only when it is
//! what was submitted, and the report is read from the same statement.

use super::test_helpers::*;
use super::*;

async fn seed_draft(pool: &PgPool, text: &str, images: serde_json::Value) -> Uuid {
    let id = Uuid::new_v4();
    insert_thread(pool, id, "t").await;
    sqlx::query(
        "UPDATE thread_summaries SET state = 'active', compose_text = $2, compose_images = $3, \
         compose_selection = '{\"model\":\"m\"}'::jsonb WHERE thread_id = $1",
    )
    .bind(id)
    .bind(text)
    .bind(images)
    .execute(pool)
    .await
    .expect("seed draft");
    id
}

async fn stored(pool: &PgPool, id: Uuid) -> ComposeReport {
    let row: ComposeReportRow = sqlx::query_as(&format!(
        "SELECT {COMPOSE_REPORT_COLS} FROM thread_summaries WHERE thread_id = $1"
    ))
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("read compose report");
    ComposeReport::from(row)
}

#[tokio::test]
async fn the_submitted_draft_clears_and_the_epoch_rises() {
    let (pool, db) = setup_test_db().await;
    let id = seed_draft(&pool, "send this\n", serde_json::json!([])).await;

    let report = consume_compose_slot(&pool, id, "send this", &[])
        .await
        .unwrap()
        .expect("a live thread has a compose slot");

    assert_eq!(report.text, "", "trimmed text matches, so the draft clears");
    assert_eq!(report.selection, None, "and its picks clear with it");
    assert_eq!(report.compose_epoch, 1);
    assert_eq!(
        report,
        stored(&pool, id).await,
        "the report is the stored row"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_different_draft_survives_but_the_epoch_still_rises() {
    let (pool, db) = setup_test_db().await;
    let id = seed_draft(&pool, "typed after the send", serde_json::json!([])).await;

    let report = consume_compose_slot(&pool, id, "the sent message", &[])
        .await
        .unwrap()
        .unwrap();

    assert_eq!(report.text, "typed after the send");
    assert_eq!(
        report.selection, None,
        "the send carried the picks, whatever the text"
    );
    assert_eq!(
        report.compose_epoch, 1,
        "a stalled write of the sent text must still be refused"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_image_mismatch_keeps_the_draft() {
    let (pool, db) = setup_test_db().await;
    let hash = "a".repeat(64);
    let id = seed_draft(&pool, "caption", serde_json::json!([hash])).await;

    let report = consume_compose_slot(&pool, id, "caption", &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        report.image_hashes,
        vec![hash.clone()],
        "text alone is not the draft"
    );

    let report = consume_compose_slot(&pool, id, "caption", &[hash])
        .await
        .unwrap()
        .unwrap();
    assert!(report.image_hashes.is_empty() && report.text.is_empty());
    assert_eq!(report.compose_epoch, 2);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_discarded_thread_has_no_slot_to_consume() {
    let (pool, db) = setup_test_db().await;
    let id = seed_draft(&pool, "x", serde_json::json!([])).await;
    sqlx::query("UPDATE thread_summaries SET state = 'discarded' WHERE thread_id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        consume_compose_slot(&pool, id, "x", &[]).await.unwrap(),
        None
    );
    assert_eq!(stored(&pool, id).await.compose_epoch, 0);
    teardown_test_db(&db).await;
}

/// Send beat the last debounced write, so the engine holds an earlier, shorter
/// state of the message. That is still the draft the send consumed.
#[tokio::test]
async fn an_earlier_state_of_the_sent_draft_clears() {
    let (pool, db) = setup_test_db().await;
    let id = seed_draft(&pool, "hello wor", serde_json::json!([])).await;

    let report = consume_compose_slot(&pool, id, "hello world", &[])
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        report.text, "",
        "a stored prefix of the message is the message"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn consumption_rolls_back_with_failed_acceptance() {
    let (pool, db) = setup_test_db().await;
    let hash = "b".repeat(64);
    let id = seed_draft(&pool, "send together", serde_json::json!([hash])).await;
    let before = stored(&pool, id).await;
    let mut tx = pool.begin().await.unwrap();
    let consumed = consume_compose_slot(&mut *tx, id, "send together", &[hash])
        .await
        .unwrap()
        .unwrap();
    assert!(consumed.text.is_empty());
    assert!(consumed.image_hashes.is_empty());
    assert_eq!(consumed.selection, None);
    assert_eq!(consumed.compose_epoch, before.compose_epoch + 1);

    // An acceptance write fails after consumption. PostgreSQL aborts the
    // transaction, so the draft must survive, including its picks and token.
    assert!(sqlx::query("SELECT 1 / 0").execute(&mut *tx).await.is_err());
    tx.rollback().await.unwrap();
    assert_eq!(stored(&pool, id).await, before);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn transactional_consumption_is_visible_only_after_commit() {
    let (pool, db) = setup_test_db().await;
    let id = seed_draft(&pool, "send together", serde_json::json!([])).await;
    let mut tx = pool.begin().await.unwrap();
    let consumed = consume_compose_slot(&mut *tx, id, "send together", &[])
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        stored(&pool, id).await,
        consumed,
        "uncommitted consumption is invisible"
    );
    tx.commit().await.unwrap();
    assert_eq!(stored(&pool, id).await, consumed);
    teardown_test_db(&db).await;
}
