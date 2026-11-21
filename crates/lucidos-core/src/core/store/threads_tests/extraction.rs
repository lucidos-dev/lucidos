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

#[tokio::test]
async fn held_delivery_extraction_context_waits_for_the_same_thread_receipt() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let thread = Uuid::new_v4();
    let other_thread = Uuid::new_v4();
    let held = Uuid::new_v4();
    let withdrawn = Uuid::new_v4();

    for (id, thread_id, event_type, payload) in [
        (
            held,
            thread,
            "MessageReceived",
            serde_json::json!({
                "text": "held question", "queued_for_permission": true
            }),
        ),
        (
            withdrawn,
            thread,
            "MessageReceived",
            serde_json::json!({
                "text": "withdrawn question", "queued_for_permission": true
            }),
        ),
        (
            Uuid::new_v4(),
            other_thread,
            "ChatTurnSettingsResolved",
            serde_json::json!({"request_event_id": held}),
        ),
        (
            Uuid::new_v4(),
            thread,
            "QueuedMessageRemoved",
            serde_json::json!({"removed_message_id": withdrawn}),
        ),
    ] {
        sqlx::query(
            "INSERT INTO events (id, thread_id, event_type, payload, aggregate, aggregate_id)              VALUES ($1, $2, $3, $4, 'thread', $2::text)",
        )
        .bind(id)
        .bind(thread_id)
        .bind(event_type)
        .bind(payload)
        .execute(&pool)
        .await
        .unwrap();
    }
    insert_message(&pool, thread, "ResponseGenerated", "previous reply").await;
    let before = store
        .recent_thread_messages_for_extraction(thread, 5, None)
        .await
        .unwrap();
    assert_eq!(before, "Assistant: previous reply");
    let mut classic = store.get_thread_events(&thread.to_string()).await.unwrap();
    store
        .project_permission_held_inputs(thread, &mut classic)
        .await
        .unwrap();
    assert!(classic.iter().all(|e| e.id != held && e.id != withdrawn));
    let rebuild = store
        .memory_index_events(&["MessageReceived"])
        .await
        .unwrap();
    assert!(rebuild.iter().all(|e| e.id != held && e.id != withdrawn));

    sqlx::query(
        "INSERT INTO events (id, thread_id, event_type, payload, aggregate, aggregate_id)          VALUES (gen_random_uuid(), $1, 'ChatTurnSettingsResolved', $2, 'thread', $1::text)",
    )
    .bind(thread)
    .bind(serde_json::json!({"request_event_id": held}))
    .execute(&pool)
    .await
    .unwrap();
    let after = store
        .recent_thread_messages_for_extraction(thread, 5, None)
        .await
        .unwrap();
    assert_eq!(after, "Assistant: previous reply\nUser: held question");
    let mut classic = store.get_thread_events(&thread.to_string()).await.unwrap();
    store
        .project_permission_held_inputs(thread, &mut classic)
        .await
        .unwrap();
    let messages = crate::core::store::messages::build_session_messages(&classic);
    let contents: Vec<_> = messages.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(contents, ["previous reply", "held question"]);
    let rebuild = store
        .memory_index_events(&["MessageReceived"])
        .await
        .unwrap();
    assert_eq!(rebuild.iter().map(|e| e.id).collect::<Vec<_>>(), [held]);

    teardown_test_db(&db).await;
}
