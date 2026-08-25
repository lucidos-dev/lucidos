use super::test_helpers::*;
use super::*;

#[tokio::test]
async fn recent_thread_messages_for_extraction_returns_oldest_first() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread = Uuid::new_v4();
    insert_thread(&pool, thread, "Test").await;
    insert_message(&pool, thread, "MessageReceived", "regnr bil").await;
    insert_message(&pool, thread, "ResponseGenerated", "Example Owner (eier)").await;
    insert_message(&pool, thread, "MessageReceived", "tlf til verkstedet").await;

    let ctx = store
        .recent_thread_messages_for_extraction(thread, 5, None)
        .await
        .expect("get context");

    assert!(ctx.contains("regnr bil"), "ctx={}", ctx);
    assert!(ctx.contains("Example Owner (eier)"), "ctx={}", ctx);
    let first_pos = ctx.find("regnr bil").unwrap();
    let second_pos = ctx.find("Example Owner").unwrap();
    assert!(first_pos < second_pos, "oldest first; ctx={}", ctx);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn recent_thread_messages_for_extraction_empty_thread() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread = Uuid::new_v4();
    insert_thread(&pool, thread, "Empty").await;

    let ctx = store
        .recent_thread_messages_for_extraction(thread, 5, None)
        .await
        .expect("get context");
    assert_eq!(ctx, "");

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn recent_thread_messages_for_extraction_excludes_event() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread = Uuid::new_v4();
    insert_thread(&pool, thread, "Test").await;
    insert_message(&pool, thread, "MessageReceived", "first message").await;

    let target_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES ($1, 'MessageReceived', $2, $3, 'thread', $3::text)",
    )
    .bind(target_id)
    .bind(serde_json::json!({ "text": "EXCLUDE_ME" }))
    .bind(thread)
    .execute(&pool)
    .await
    .expect("insert event");

    let ctx = store
        .recent_thread_messages_for_extraction(thread, 5, Some(target_id))
        .await
        .expect("get context");

    assert!(ctx.contains("first message"), "ctx={}", ctx);
    assert!(
        !ctx.contains("EXCLUDE_ME"),
        "should exclude target; ctx={}",
        ctx
    );

    teardown_test_db(&db).await;
}
