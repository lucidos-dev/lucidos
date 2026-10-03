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

/// The `limit` cut keeps the tiers ranking puts first: an exact title, then a
/// title holding the query as a phrase. Newer threads in a lower tier must not
/// push an old one out, or the hit the user most likely meant never shows.
#[tokio::test]
async fn search_threads_by_text_cuts_by_title_tier_before_recency() {
    let (pool, db) = setup_test_db().await;
    ensure_memory_entries_table(&pool).await;
    let store = EventStore::new(pool.clone());

    let age = |id: Uuid, days: i32| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "UPDATE thread_summaries SET last_activity = NOW() - make_interval(days => $2) \
                 WHERE thread_id = $1",
            )
            .bind(id)
            .bind(days)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    let exact = Uuid::new_v4();
    insert_thread(&pool, exact, "Search  Ranking").await;
    age(exact, 90).await;
    let phrase = Uuid::new_v4();
    insert_thread(&pool, phrase, "Notes on search ranking").await;
    age(phrase, 60).await;
    for title in [
        "Search ranking v2",
        "Ranking the search box",
        "Ranking of search",
        "Search: the ranking",
    ] {
        insert_thread(&pool, Uuid::new_v4(), title).await;
    }

    let hits = store
        .search_threads_by_text("search ranking", 3)
        .await
        .unwrap();
    let titles: Vec<&str> = hits.iter().map(|h| h.info.title.as_str()).collect();
    assert_eq!(
        hits[0].info.thread_id,
        exact.to_string(),
        "titles={titles:?}"
    );
    assert!(
        hits.iter().any(|h| h.info.thread_id == phrase.to_string()),
        "an old phrase title must outlast newer scattered-token titles. titles={titles:?}"
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

/// A short token is checked only inside threads a long token matched. It must
/// still be found in a different message from the long one.
#[tokio::test]
async fn a_short_token_matches_in_another_message_of_the_long_tokens_thread() {
    let (pool, db) = setup_test_db().await;
    ensure_memory_entries_table(&pool).await;
    let store = EventStore::new(pool.clone());

    let both = Uuid::new_v4();
    insert_thread(&pool, both, "Kitchen").await;
    insert_message(&pool, both, "MessageReceived", "the dishwasher leaks").await;
    insert_message(&pool, both, "ResponseGenerated", "call them at 9 am").await;
    let long_only = Uuid::new_v4();
    insert_thread(&pool, long_only, "Laundry").await;
    insert_message(&pool, long_only, "MessageReceived", "dishwasher broken").await;

    let hits = store
        .search_threads_by_text("dishwasher am", 20)
        .await
        .unwrap();
    let ids: Vec<String> = hits.into_iter().map(|h| h.info.thread_id).collect();
    assert_eq!(ids, vec![both.to_string()]);

    teardown_test_db(&db).await;
}

/// Each arm counts distinct patterns, so a repeated token must not raise the
/// count a thread has to reach.
#[tokio::test]
async fn a_repeated_token_still_matches_message_text() {
    let (pool, db) = setup_test_db().await;
    ensure_memory_entries_table(&pool).await;
    let store = EventStore::new(pool.clone());

    let thread = Uuid::new_v4();
    insert_thread(&pool, thread, "Garden").await;
    insert_message(&pool, thread, "MessageReceived", "prune the roses").await;

    let hits = store
        .search_threads_by_text("roses roses", 20)
        .await
        .unwrap();
    let ids: Vec<String> = hits.into_iter().map(|h| h.info.thread_id).collect();
    assert_eq!(ids, vec![thread.to_string()]);

    teardown_test_db(&db).await;
}

/// No index can serve a 1 or 2 character token over every message, so a query
/// made only of them skips message bodies. Titles still match it.
#[tokio::test]
async fn a_query_of_only_short_tokens_matches_titles_but_not_message_bodies() {
    let (pool, db) = setup_test_db().await;
    ensure_memory_entries_table(&pool).await;
    let store = EventStore::new(pool.clone());

    let titled = Uuid::new_v4();
    insert_thread(&pool, titled, "QA notes").await;
    let body_only = Uuid::new_v4();
    insert_thread(&pool, body_only, "Release").await;
    insert_message(&pool, body_only, "MessageReceived", "ask qa first").await;

    let hits = store.search_threads_by_text("qa", 20).await.unwrap();
    let ids: Vec<String> = hits.into_iter().map(|h| h.info.thread_id).collect();
    assert_eq!(ids, vec![titled.to_string()]);

    teardown_test_db(&db).await;
}

/// The message-text arm must reach `events` through the trigram index and the
/// entity arm through the primary key. Either falling back to a scan of every
/// message is what timed Search Everywhere out on a large workspace. Checked
/// on the generic plan, since sqlx reuses prepared statements.
#[tokio::test]
async fn text_search_reaches_events_through_its_indexes() {
    let (pool, db) = setup_test_db().await;
    ensure_memory_entries_table(&pool).await;

    // Enough messages, of a real length, that a pass over all of them costs
    // more than the index, as on a real workspace.
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         SELECT gen_random_uuid(), 'MessageReceived', \
                jsonb_build_object('text', repeat(md5(i::text), 10)), t, 'thread', t::text \
         FROM generate_series(1, 20000) AS i, LATERAL (SELECT gen_random_uuid() AS t) g",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE events").execute(&pool).await.unwrap();

    let explain = format!(
        "EXPLAIN (GENERIC_PLAN) {}",
        super::search::text_search_sql()
    );
    // The simple protocol, because a generic plan leaves `$n` unbound.
    let plan = sqlx::raw_sql(&explain)
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|row| sqlx::Row::get::<String, _>(row, 0))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("idx_events_message_text_trgm"),
        "message text is not searched through its trigram index:\n{plan}"
    );
    assert!(
        plan.contains("events_pkey"),
        "the entity arm does not join events by primary key:\n{plan}"
    );

    teardown_test_db(&db).await;
}

// ── Archive stayed searchable ─────────────────────────────────────────
//
// Decision 1 of `docs/plans/2026-09-15-deleting-a-thread.md`: archive is "put
// down for now" and delete is "gone", and only delete changed. An archive
// filter appearing in either search arm would quietly turn the softer action
// into the harsher one.

/// The tester's report that started the delete work said archived threads felt
/// retrievable. They ARE, and that is the decision: this pins it, so the
/// obvious follow-up ("while we are here, hide archived from search") cannot
/// land by accident.
#[tokio::test]
async fn an_archived_thread_is_still_returned_by_text_search() {
    let (pool, db) = setup_test_db().await;
    ensure_memory_entries_table(&pool).await;
    let store = EventStore::new(pool.clone());

    let archived = Uuid::new_v4();
    insert_thread(&pool, archived, "Heat pump experiments").await;
    insert_message(&pool, archived, "MessageReceived", "the defrost cycle").await;
    sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
        .bind(archived)
        .execute(&pool)
        .await
        .unwrap();

    let hits = store.search_threads_by_text("defrost", 10).await.unwrap();
    assert!(
        hits.iter()
            .any(|h| h.info.thread_id == archived.to_string()),
        "an archived thread must rank like a live one; only delete removes it"
    );

    teardown_test_db(&db).await;
}

/// The memory arm needs an embedder, so it is asserted at the source. Both
/// queries read `thread_summaries`, so either could grow the filter, and only
/// one of the two is reachable from a test without a model.
#[test]
fn neither_search_arm_filters_on_archive_state() {
    const SRC: &str = include_str!("../threads/search.rs");
    assert!(
        !SRC.contains("archive_state"),
        "a search arm grew an archive filter. Archive is 'put down for now' and \
         stays fully searchable; delete is the action that removes a thread."
    );
}
