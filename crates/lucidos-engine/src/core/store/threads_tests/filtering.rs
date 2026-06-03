use super::*;
use super::test_helpers::*;

/// `list_historical_triggers` returns one entry per distinct trigger_id with
/// the most-recent thread's snapshot name and last_activity (covers the
/// trigger-rename case and powers the dropdown's `(until <date>)` suffix).
#[tokio::test]
async fn list_historical_triggers_dedupes_and_takes_most_recent_name() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    insert_trigger_thread(&pool, "trig-a", "Apple (old name)", 60).await;
    let trig_a_recent = insert_trigger_thread(&pool, "trig-a", "Apple", 1).await;
    let trig_b_recent = insert_trigger_thread(&pool, "trig-b", "Banana", 30).await;

    let mut historical = store
        .list_historical_triggers()
        .await
        .expect("list_historical_triggers");
    historical.sort_by(|a, b| a.0.cmp(&b.0));

    let names: Vec<_> = historical
        .iter()
        .map(|(id, name, _)| (id.clone(), name.clone()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("trig-a".to_string(), Some("Apple".to_string())),
            ("trig-b".to_string(), Some("Banana".to_string())),
        ]
    );

    let last_a = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
        "SELECT last_activity FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(trig_a_recent)
    .fetch_one(&pool)
    .await
    .expect("fetch last_activity for trig-a");
    let last_b = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
        "SELECT last_activity FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(trig_b_recent)
    .fetch_one(&pool)
    .await
    .expect("fetch last_activity for trig-b");
    assert_eq!(historical[0].2, last_a);
    assert_eq!(historical[1].2, last_b);

    teardown_test_db(&db).await;
}

/// When `trigger_ids` is provided, `get_older_threads` returns only matching threads.
#[tokio::test]
async fn get_older_threads_filters_by_trigger_ids() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let a1 = insert_trigger_thread(&pool, "trig-a", "Apple", 60).await;
    let _a2 = insert_trigger_thread(&pool, "trig-a", "Apple", 30).await;
    let _b1 = insert_trigger_thread(&pool, "trig-b", "Banana", 20).await;

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let only_a = store
        .get_older_threads(cutoff, 10, None, Some(&["trig-a".to_string()]), None, None)
        .await
        .expect("get_older_threads filtered");

    assert_eq!(only_a.len(), 2);
    assert!(only_a
        .iter()
        .all(|t| t.trigger_id.as_deref() == Some("trig-a")));
    assert!(only_a.iter().any(|t| t.thread_id == a1.to_string()));

    teardown_test_db(&db).await;
}

/// Trigger-id filter returns matches regardless of `has_response`. The
/// dropdown advertises every trigger that ever stamped a row, with no
/// `has_response` gate; the filter must honor the same contract.
#[tokio::test]
async fn get_older_threads_returns_trigger_threads_with_no_response() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let id = Uuid::new_v4();
    sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, is_saved, trigger_id, trigger_name) \
             VALUES ($1, 'orphan', 'trigger', 1, NOW() - INTERVAL '60 minutes', FALSE, FALSE, 'trig-orphan', 'Orphan')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .expect("insert no-response trigger thread");

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let hits = store
        .get_older_threads(cutoff, 10, None, Some(&["trig-orphan".to_string()]), None, None)
        .await
        .expect("get_older_threads filtered");

    assert_eq!(
        hits.len(),
        1,
        "dropdown advertised trig-orphan; filter must return its thread regardless of has_response"
    );
    assert_eq!(hits[0].thread_id, id.to_string());

    teardown_test_db(&db).await;
}

/// `repo_ids` narrows `get_older_threads` to CC threads bound to those
/// repos and projects `cc_repo_name` from the `repositories` registry.
#[tokio::test]
async fn get_older_threads_filters_by_repo_ids() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let repo_a = Uuid::new_v4();
    let repo_b = Uuid::new_v4();
    insert_repository(&pool, repo_a, "Apple", "/tmp/apple").await;
    insert_repository(&pool, repo_b, "Banana", "/tmp/banana").await;

    let a1 = insert_cc_repo_thread(&pool, &repo_a.to_string(), 60).await;
    let _a2 = insert_cc_repo_thread(&pool, &repo_a.to_string(), 30).await;
    let _b1 = insert_cc_repo_thread(&pool, &repo_b.to_string(), 20).await;

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let only_a = store
        .get_older_threads(cutoff, 10, None, None, Some(&[repo_a.to_string()]), None)
        .await
        .expect("get_older_threads filtered");

    assert_eq!(only_a.len(), 2);
    assert!(only_a
        .iter()
        .all(|t| t.cc_repo_id.as_deref() == Some(repo_a.to_string().as_str())));
    assert!(only_a
        .iter()
        .all(|t| t.cc_repo_name.as_deref() == Some("Apple")));
    assert!(only_a.iter().any(|t| t.thread_id == a1.to_string()));

    teardown_test_db(&db).await;
}

/// When the registered repo is later deleted, threads bound to its UUID
/// keep `cc_repo_id` but `cc_repo_name` resolves to NULL — the frontend
/// uses that absence to render the row as `(deleted)`.
#[tokio::test]
async fn get_older_threads_returns_null_repo_name_for_deleted_repo() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let orphan_repo = Uuid::new_v4();
    insert_cc_repo_thread(&pool, &orphan_repo.to_string(), 60).await;

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let hits = store
        .get_older_threads(cutoff, 10, None, None, Some(&[orphan_repo.to_string()]), None)
        .await
        .expect("get_older_threads filtered");

    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].cc_repo_id.as_deref(),
        Some(orphan_repo.to_string().as_str())
    );
    assert_eq!(
        hits[0].cc_repo_name, None,
        "deleted repo must yield NULL name"
    );

    teardown_test_db(&db).await;
}

/// `trigger_ids` and `repo_ids` compose with OR — a user with both
/// filters expanded sees the union.
#[tokio::test]
async fn get_older_threads_combines_trigger_and_repo_ids_with_or() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let repo_a = Uuid::new_v4();
    insert_repository(&pool, repo_a, "Apple", "/tmp/apple").await;

    let cc_thread = insert_cc_repo_thread(&pool, &repo_a.to_string(), 30).await;
    let trig_thread = insert_trigger_thread(&pool, "trig-a", "Trig A", 60).await;
    insert_trigger_thread(&pool, "trig-other", "Other", 90).await;

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let hits = store
        .get_older_threads(
            cutoff,
            10,
            None,
            Some(&["trig-a".to_string()]),
            Some(&[repo_a.to_string()]),
            None,
        )
        .await
        .expect("get_older_threads combined");

    assert_eq!(hits.len(), 2);
    let returned: std::collections::HashSet<&str> =
        hits.iter().map(|t| t.thread_id.as_str()).collect();
    let cc = cc_thread.to_string();
    let trig = trig_thread.to_string();
    assert!(returned.contains(cc.as_str()));
    assert!(returned.contains(trig.as_str()));

    teardown_test_db(&db).await;
}

/// `app_ids` narrows `get_older_threads` to app coding-agent threads whose
/// `data/apps/<id>` folder matches — including archived ones (archived threads
/// are real sessions; the filter must fetch them, not hide them).
#[tokio::test]
async fn get_older_threads_filters_by_app_ids() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let momentum_active = insert_app_thread(&pool, "momentum", 60, false).await;
    let momentum_archived = insert_app_thread(&pool, "momentum", 90, true).await;
    let _other = insert_app_thread(&pool, "momentum-autoresearch", 30, false).await;

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let hits = store
        .get_older_threads(cutoff, 10, None, None, None, Some(&["momentum".to_string()]))
        .await
        .expect("get_older_threads app filtered");

    let returned: std::collections::HashSet<&str> =
        hits.iter().map(|t| t.thread_id.as_str()).collect();
    assert_eq!(hits.len(), 2, "both momentum threads (active + archived) match");
    assert!(returned.contains(momentum_active.to_string().as_str()));
    assert!(
        returned.contains(momentum_archived.to_string().as_str()),
        "archived app thread must be fetched, not excluded"
    );

    teardown_test_db(&db).await;
}

/// The app-id predicate must not prefix-match: selecting "momentum" must NOT
/// pull in "momentum-autoresearch".
#[tokio::test]
async fn get_older_threads_app_ids_exact_segment_match() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    insert_app_thread(&pool, "momentum-autoresearch", 30, false).await;

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let hits = store
        .get_older_threads(cutoff, 10, None, None, None, Some(&["momentum".to_string()]))
        .await
        .expect("get_older_threads app filtered");

    assert_eq!(hits.len(), 0, "'momentum' must not match 'momentum-autoresearch'");

    teardown_test_db(&db).await;
}

/// `get_filter_facets` returns the distinct set of triggers / repos / apps that
/// have any thread — including archived-only apps — so the dropdown lists the
/// complete option set.
#[tokio::test]
async fn get_filter_facets_returns_distinct_sessions() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let repo_a = Uuid::new_v4();
    insert_repository(&pool, repo_a, "Apple", "/tmp/apple").await;
    insert_cc_repo_thread(&pool, &repo_a.to_string(), 60).await;
    insert_cc_repo_thread(&pool, &repo_a.to_string(), 30).await; // same repo twice → one facet
    insert_trigger_thread(&pool, "trig-a", "Trig A", 60).await;
    insert_app_thread(&pool, "momentum", 90, true).await; // archived-only → still a facet
    insert_app_thread(&pool, "momentum-autoresearch", 30, false).await;

    let facets = store.get_filter_facets().await.expect("get_filter_facets");

    let repo_ids: std::collections::HashSet<&str> =
        facets.repos.iter().filter_map(|f| f.id.as_deref()).collect();
    assert_eq!(repo_ids.len(), 1, "duplicate repo collapses to one facet");
    assert!(repo_ids.contains(repo_a.to_string().as_str()));

    let trigger_ids: std::collections::HashSet<&str> = facets
        .triggers
        .iter()
        .filter_map(|f| f.id.as_deref())
        .collect();
    assert!(trigger_ids.contains("trig-a"));

    let app_ids: std::collections::HashSet<&str> =
        facets.apps.iter().filter_map(|f| f.id.as_deref()).collect();
    assert!(
        app_ids.contains("momentum"),
        "archived-only app must still be a facet"
    );
    assert!(app_ids.contains("momentum-autoresearch"));

    teardown_test_db(&db).await;
}
