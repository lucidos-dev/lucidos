use super::test_helpers::*;
use super::*;

/// Multi-token text queries must match threads where every token appears
/// somewhere — title or any event payload — even if no single string contains
/// the full phrase.
#[tokio::test]
async fn search_threads_by_text_matches_per_token_across_events() {
    let (pool, db) = setup_test_db().await;
    ensure_memory_entries_table(&pool).await;
    let store = EventStore::new(pool.clone());

    let phrase_in_title = Uuid::new_v4();
    insert_thread(&pool, phrase_in_title, "Bil reparasjon").await;

    let split_across_events = Uuid::new_v4();
    insert_thread(&pool, split_across_events, "Verkstedet timeavtale").await;
    insert_message(
        &pool,
        split_across_events,
        "MessageReceived",
        "min bil til service",
    )
    .await;
    insert_message(
        &pool,
        split_across_events,
        "MessageReceived",
        "reparasjonen er ferdig om to dager",
    )
    .await;

    let only_one_token = Uuid::new_v4();
    insert_thread(&pool, only_one_token, "Bil mappa dokumenter").await;
    insert_message(
        &pool,
        only_one_token,
        "MessageReceived",
        "fant fram dokumentene",
    )
    .await;

    let irrelevant = Uuid::new_v4();
    insert_thread(&pool, irrelevant, "Varmepumpe logging").await;
    insert_message(&pool, irrelevant, "MessageReceived", "varmepumpe styring").await;

    let results = store
        .search_threads_by_text("bil reparasjon", 20)
        .await
        .expect("search");

    let ids: Vec<&str> = results.iter().map(|r| r.info.thread_id.as_str()).collect();
    let phrase = phrase_in_title.to_string();
    let split = split_across_events.to_string();
    let one = only_one_token.to_string();
    let bad = irrelevant.to_string();

    assert!(
        ids.contains(&phrase.as_str()),
        "thread with both tokens in title must match. ids={:?}",
        ids
    );
    assert!(
        ids.contains(&split.as_str()),
        "thread with tokens split across separate events must match. ids={:?}",
        ids
    );
    assert!(
        !ids.contains(&one.as_str()),
        "thread missing the 'reparasjon' token must NOT match. ids={:?}",
        ids
    );
    assert!(
        !ids.contains(&bad.as_str()),
        "thread with neither token must NOT match. ids={:?}",
        ids
    );

    let phrase_pos = ids.iter().position(|id| *id == phrase.as_str()).unwrap();
    let split_pos = ids.iter().position(|id| *id == split.as_str()).unwrap();
    assert!(
        phrase_pos < split_pos,
        "title-token match must rank above content-token match. ids={:?}",
        ids
    );

    teardown_test_db(&db).await;
}

/// LIKE metacharacters in the query must be escaped, otherwise a token like
/// `foo_bar` matches `fooXbar` and `50%` matches everything starting with 50.
#[tokio::test]
async fn search_threads_by_text_treats_wildcards_as_literals() {
    let (pool, db) = setup_test_db().await;
    ensure_memory_entries_table(&pool).await;
    let store = EventStore::new(pool.clone());

    let literal_match = Uuid::new_v4();
    insert_thread(&pool, literal_match, "foo_bar exact title").await;

    let wildcard_trap = Uuid::new_v4();
    insert_thread(&pool, wildcard_trap, "fooXbar should not match").await;

    let results = store
        .search_threads_by_text("foo_bar", 20)
        .await
        .expect("search");
    let ids: Vec<&str> = results.iter().map(|r| r.info.thread_id.as_str()).collect();
    let literal = literal_match.to_string();
    let trap = wildcard_trap.to_string();
    assert!(ids.contains(&literal.as_str()), "literal match required");
    assert!(
        !ids.contains(&trap.as_str()),
        "underscore must not act as a wildcard. ids={:?}",
        ids
    );

    teardown_test_db(&db).await;
}

/// A thread should match a multi-token query when its `memory_entries.entities`
/// cover all tokens, even when no event payload text contains them — entity
/// extraction adds linkages the raw payload doesn't have.
#[tokio::test]
async fn search_threads_by_text_matches_via_entities() {
    let (pool, db) = setup_test_db().await;
    ensure_memory_entries_table(&pool).await;
    let store = EventStore::new(pool.clone());

    let thread = Uuid::new_v4();
    insert_thread(&pool, thread, "Some title").await;
    let event_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES ($1, 'MessageReceived', $2, $3, 'thread', $3::text)",
    )
    .bind(event_id)
    .bind(serde_json::json!({"text": "ringer verkstedet om servicen i morgen"}))
    .bind(thread)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
            "INSERT INTO memory_entries (id, source, topic, summary, importance, entities, embedding, embedding_model, src_created_at, created_at) \
             VALUES ($1, $2::jsonb, 'vehicle service', 'Bilen trenger service', 0.9, $3::jsonb, $4::vector, 'multilingual-e5-small', NOW(), NOW())"
        )
        .bind(Uuid::new_v4())
        .bind(serde_json::json!({"type": "event", "id": event_id}))
        .bind(serde_json::json!(["bil", "service", "verksted"]))
        .bind(format!("[{}]", vec!["0"; 384].join(",")))
        .execute(&pool).await.unwrap();

    let results = store
        .search_threads_by_text("bil service", 20)
        .await
        .unwrap();
    let ids: Vec<&str> = results.iter().map(|r| r.info.thread_id.as_str()).collect();
    let target = thread.to_string();
    assert!(
        ids.contains(&target.as_str()),
        "thread should match via entity tokens. ids={:?}",
        ids
    );

    teardown_test_db(&db).await;
}
