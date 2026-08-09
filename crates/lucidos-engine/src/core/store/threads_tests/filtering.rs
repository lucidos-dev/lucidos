use super::test_helpers::*;
use super::*;

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
        .get_older_threads(
            cutoff,
            10,
            None,
            Some(&["trig-orphan".to_string()]),
            None,
            None,
        )
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

/// When a repo's name was never recorded anywhere — not in the live
/// `repositories` registry and not in the durable `repo_names` projection —
/// `cc_repo_name` resolves to NULL. The frontend's deleted-vs-live decision is
/// keyed off the live registry, not this absence, so a NULL name just means
/// "truly unknown" (an orphan UUID with no provenance).
#[tokio::test]
async fn get_older_threads_returns_null_repo_name_for_unknown_repo() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let orphan_repo = Uuid::new_v4();
    insert_cc_repo_thread(&pool, &orphan_repo.to_string(), 60).await;

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let hits = store
        .get_older_threads(
            cutoff,
            10,
            None,
            None,
            Some(&[orphan_repo.to_string()]),
            None,
        )
        .await
        .expect("get_older_threads filtered");

    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].cc_repo_id.as_deref(),
        Some(orphan_repo.to_string().as_str())
    );
    assert_eq!(
        hits[0].cc_repo_name, None,
        "repo with no recorded name yields NULL"
    );

    teardown_test_db(&db).await;
}

/// The bug fix: a repo that WAS registered (so its name lives in the durable
/// `repo_names` projection) and is later removed from the live `repositories`
/// registry still resolves its historical `cc_repo_name` — the filter and
/// thread rows show "Apple", not the raw UUID.
#[tokio::test]
async fn get_older_threads_resolves_deleted_repo_name_from_projection() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let repo = Uuid::new_v4();
    insert_repository(&pool, repo, "Apple", "/tmp/apple").await;
    insert_repo_name(&pool, repo, "Apple").await; // what RepositoryAdded persists
    insert_cc_repo_thread(&pool, &repo.to_string(), 60).await;
    delete_repository(&pool, repo).await; // RepositoryRemoved — projection retained

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let hits = store
        .get_older_threads(cutoff, 10, None, None, Some(&[repo.to_string()]), None)
        .await
        .expect("get_older_threads filtered");

    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].cc_repo_name.as_deref(),
        Some("Apple"),
        "deleted repo keeps its historical name via repo_names"
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

/// Pagination companion to `count_archived_threads_channels_plus_facet_counts_union`:
/// the rows `get_older_threads` returns must be the SAME union the badge counts.
/// With whole channels active (`sources = None`) plus one trigger sub-selected,
/// the pre-fix facet branch ignored the channel selection and returned ONLY the
/// trigger's threads — so the (fixed) badge total and the scrolled rows would
/// disagree, and the user could never scroll to the chat / claude_code archive.
/// The unified predicate mirrors `threadPassesChannelFilter`: chat + claude_code
/// + the selected trigger, minus the unselected trigger.
#[tokio::test]
async fn get_older_threads_channels_plus_facet_returns_union() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let chat = Uuid::new_v4();
    insert_thread(&pool, chat, "Chat").await;
    let repo = Uuid::new_v4();
    let cc = insert_cc_repo_thread(&pool, &repo.to_string(), 30).await;
    let trig_keep = insert_trigger_thread(&pool, "trig-keep", "Keep", 60).await;
    insert_trigger_thread(&pool, "trig-drop", "Drop", 90).await;

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let hits = store
        .get_older_threads(
            cutoff,
            10,
            None,
            Some(&["trig-keep".to_string()]),
            None,
            None,
        )
        .await
        .expect("get_older_threads channels + facet");

    let returned: std::collections::HashSet<&str> =
        hits.iter().map(|t| t.thread_id.as_str()).collect();
    assert_eq!(
        hits.len(),
        3,
        "chat + claude_code + trig-keep — NOT just the trigger (pre-fix bug)"
    );
    assert!(
        returned.contains(chat.to_string().as_str()),
        "chat thread must page in"
    );
    assert!(
        returned.contains(cc.to_string().as_str()),
        "claude_code thread must page in"
    );
    assert!(
        returned.contains(trig_keep.to_string().as_str()),
        "selected trigger pages in"
    );

    teardown_test_db(&db).await;
}

/// `app_ids` narrows `get_older_threads` to app coding-agent threads whose
/// `data/apps/<id>` folder matches — including archived ones (archived threads
/// are real sessions; the filter must fetch them, not hide them).
#[tokio::test]
async fn get_older_threads_filters_by_app_ids() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let habit_active = insert_app_thread(&pool, "habit", 60, false).await;
    let habit_archived = insert_app_thread(&pool, "habit", 90, true).await;
    let _other = insert_app_thread(&pool, "habit-tracker", 30, false).await;

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let hits = store
        .get_older_threads(cutoff, 10, None, None, None, Some(&["habit".to_string()]))
        .await
        .expect("get_older_threads app filtered");

    let returned: std::collections::HashSet<&str> =
        hits.iter().map(|t| t.thread_id.as_str()).collect();
    assert_eq!(
        hits.len(),
        2,
        "both habit threads (active + archived) match"
    );
    assert!(returned.contains(habit_active.to_string().as_str()));
    assert!(
        returned.contains(habit_archived.to_string().as_str()),
        "archived app thread must be fetched, not excluded"
    );

    teardown_test_db(&db).await;
}

/// The app-id predicate must not prefix-match: selecting "habit" must NOT
/// pull in "habit-tracker".
#[tokio::test]
async fn get_older_threads_app_ids_exact_segment_match() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    insert_app_thread(&pool, "habit-tracker", 30, false).await;

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let hits = store
        .get_older_threads(cutoff, 10, None, None, None, Some(&["habit".to_string()]))
        .await
        .expect("get_older_threads app filtered");

    assert_eq!(hits.len(), 0, "'habit' must not match 'habit-tracker'");

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
    insert_app_thread(&pool, "habit", 90, true).await; // archived-only → still a facet
    insert_app_thread(&pool, "habit-tracker", 30, false).await;

    let facets = store.get_filter_facets().await.expect("get_filter_facets");

    let repo_ids: std::collections::HashSet<&str> = facets
        .repos
        .iter()
        .filter_map(|f| f.id.as_deref())
        .collect();
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
        app_ids.contains("habit"),
        "archived-only app must still be a facet"
    );
    assert!(app_ids.contains("habit-tracker"));

    teardown_test_db(&db).await;
}

/// The repos facet resolves a `name`: the live registry for a registered repo,
/// the durable `repo_names` projection for a removed one — so the dropdown lists
/// a deleted repo under its historical name instead of its UUID. Trigger/app
/// facets carry NULL names (resolved client-side).
#[tokio::test]
async fn get_filter_facets_resolves_repo_names_including_deleted() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let live_repo = Uuid::new_v4();
    insert_repository(&pool, live_repo, "Live Repo", "/tmp/live").await;
    insert_repo_name(&pool, live_repo, "Live Repo").await;
    insert_cc_repo_thread(&pool, &live_repo.to_string(), 30).await;

    let dead_repo = Uuid::new_v4();
    insert_repo_name(&pool, dead_repo, "Dead Repo").await; // registered then removed
    insert_cc_repo_thread(&pool, &dead_repo.to_string(), 60).await;

    let facets = store.get_filter_facets().await.expect("get_filter_facets");

    let by_id: std::collections::HashMap<&str, Option<&str>> = facets
        .repos
        .iter()
        .filter_map(|f| f.id.as_deref().map(|id| (id, f.name.as_deref())))
        .collect();
    assert_eq!(
        by_id.get(live_repo.to_string().as_str()),
        Some(&Some("Live Repo")),
        "live repo resolves from the registry"
    );
    assert_eq!(
        by_id.get(dead_repo.to_string().as_str()),
        Some(&Some("Dead Repo")),
        "removed repo resolves its historical name from repo_names"
    );

    teardown_test_db(&db).await;
}

// ---------------------------------------------------------------------------
// Status filter. `active` groups `running` with `waiting_for_user_answer`,
// which are opposite answers to "is the workspace busy?": one is working, the
// other is stopped and waiting on a person. These cover the precise form.
// ---------------------------------------------------------------------------

#[test]
fn parse_status_filter_accepts_the_values_rows_actually_carry() {
    let parsed = parse_status_filter_csv("running, waiting_for_user_answer ")
        .expect("the wire spelling is what rows print");
    assert_eq!(
        parsed,
        [ThreadStatus::Running, ThreadStatus::WaitingForUserAnswer]
    );
}

/// The repo spells public parameter values in kebab-case, while the `status`
/// field on every returned row is snake_case. Accepting both means a caller
/// following either convention is right.
#[test]
fn parse_status_filter_accepts_the_kebab_spelling_too() {
    assert_eq!(
        parse_status_filter_csv("waiting-for-user-answer").expect("kebab is accepted"),
        parse_status_filter_csv("waiting_for_user_answer").expect("snake is accepted")
    );
    assert_eq!(
        parse_status_filter_csv("RUNNING").expect("case is not load-bearing"),
        [ThreadStatus::Running]
    );
}

/// Three shapes that must be errors rather than a quietly different answer: a
/// typo would select nothing, an empty filter would select everything, and a
/// stray comma hides either.
#[test]
fn parse_status_filter_refuses_what_it_cannot_honour() {
    for raw in ["runnign", "", "  ", "running,", "running,,failed"] {
        let err = parse_status_filter_csv(raw)
            .expect_err("must not answer a question the caller did not ask");
        assert!(
            err.contains("running"),
            "every refusal lists the valid values: {err}"
        );
    }
}

#[test]
fn status_filter_binds_collapse_all_three_variants_onto_one_predicate() {
    assert_eq!(StatusFilter::Any.binds(), (None, false));
    assert_eq!(
        StatusFilter::Active(true).binds(),
        (Some(vec!["running", "waiting_for_user_answer"]), false)
    );
    assert_eq!(
        StatusFilter::Active(false).binds(),
        (Some(vec!["running", "waiting_for_user_answer"]), true)
    );
    assert_eq!(
        StatusFilter::OneOf(&[ThreadStatus::Running]).binds(),
        (Some(vec!["running"]), false)
    );
}

async fn insert_thread_with_status(pool: &PgPool, title: &str, status: ThreadStatus) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
            (thread_id, title, source, message_count, last_activity, has_response, status) \
         VALUES ($1, $2, 'chat', 0, NOW(), TRUE, $3)",
    )
    .bind(id)
    .bind(title)
    .bind(status.as_str())
    .execute(pool)
    .await
    .expect("insert thread_summaries");
    id
}

fn filters(status: StatusFilter<'_>) -> ThreadSummaryFilters<'_> {
    ThreadSummaryFilters {
        status,
        sources: None,
        parent: None,
        limit: 1000,
    }
}

/// The whole change, at the layer that decides it. A caller asking "is
/// anything busy?" must not be handed the thread parked on a question: that
/// thread is blocked on a human, and it is the state most likely to coincide
/// with work piling up unannounced.
#[tokio::test]
async fn running_and_awaiting_a_user_are_selectable_apart_and_together() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let running = insert_thread_with_status(&pool, "Working", ThreadStatus::Running).await;
    let parked =
        insert_thread_with_status(&pool, "Parked", ThreadStatus::WaitingForUserAnswer).await;
    let proposed = insert_thread_with_status(&pool, "Proposed", ThreadStatus::Waiting).await;
    insert_thread_with_status(&pool, "Done", ThreadStatus::Idle).await;

    let ids = |summaries: Vec<ThreadSummary>| -> Vec<String> {
        let mut out: Vec<String> = summaries.into_iter().map(|s| s.thread_id).collect();
        out.sort();
        out
    };
    let sorted = |mut v: Vec<Uuid>| -> Vec<String> {
        let mut out: Vec<String> = v.drain(..).map(|id| id.to_string()).collect();
        out.sort();
        out
    };

    let busy = store
        .list_thread_summaries(filters(StatusFilter::OneOf(&[ThreadStatus::Running])))
        .await
        .expect("list running");
    assert_eq!(
        ids(busy),
        sorted(vec![running]),
        "status=running is the workspace working, and nothing else"
    );

    let awaiting = store
        .list_thread_summaries(filters(StatusFilter::OneOf(&[
            ThreadStatus::WaitingForUserAnswer,
        ])))
        .await
        .expect("list awaiting");
    assert_eq!(
        ids(awaiting),
        sorted(vec![parked]),
        "status=waiting_for_user_answer must exclude the thread that is working"
    );

    let union = store
        .list_thread_summaries(filters(StatusFilter::Active(true)))
        .await
        .expect("list active");
    assert_eq!(
        ids(union),
        sorted(vec![running, parked]),
        "active=true is unchanged: still both, and still not the proposed thread"
    );

    let both_named = store
        .list_thread_summaries(filters(StatusFilter::OneOf(&[
            ThreadStatus::Running,
            ThreadStatus::WaitingForUserAnswer,
        ])))
        .await
        .expect("list both statuses");
    assert_eq!(
        ids(both_named),
        sorted(vec![running, parked]),
        "naming both statuses reproduces the union exactly"
    );

    let review = store
        .list_thread_summaries(filters(StatusFilter::OneOf(&[ThreadStatus::Waiting])))
        .await
        .expect("list waiting");
    assert_eq!(
        ids(review),
        sorted(vec![proposed]),
        "`waiting` is reachable on its own, and is in neither activity group"
    );

    teardown_test_db(&db).await;
}

/// A count is what an idle detector calls, and it cannot be corrected
/// client-side, so it has to agree with the list for every filter shape.
#[tokio::test]
async fn count_agrees_with_list_for_every_status_filter_shape() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    insert_thread_with_status(&pool, "Working", ThreadStatus::Running).await;
    insert_thread_with_status(&pool, "Parked", ThreadStatus::WaitingForUserAnswer).await;
    insert_thread_with_status(&pool, "Done", ThreadStatus::Idle).await;

    for status in [
        StatusFilter::Any,
        StatusFilter::Active(true),
        StatusFilter::Active(false),
        StatusFilter::OneOf(&[ThreadStatus::Running]),
        StatusFilter::OneOf(&[ThreadStatus::WaitingForUserAnswer]),
        StatusFilter::OneOf(&[ThreadStatus::Idle, ThreadStatus::Running]),
    ] {
        let counted = store
            .count_thread_summaries(filters(status))
            .await
            .expect("count");
        let listed = store
            .list_thread_summaries(filters(status))
            .await
            .expect("list")
            .len() as i64;
        assert_eq!(
            counted, listed,
            "count and list must apply the same predicate for {status:?}"
        );
    }

    teardown_test_db(&db).await;
}
