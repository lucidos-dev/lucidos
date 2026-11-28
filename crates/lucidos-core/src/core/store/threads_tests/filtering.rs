use super::test_helpers::*;
use super::*;

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

#[tokio::test]
async fn get_older_threads_resolves_deleted_repo_name_from_projection() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let repo = Uuid::new_v4();
    insert_repository(&pool, repo, "Apple", "/tmp/apple").await;
    insert_repo_name(&pool, repo, "Apple").await;
    insert_cc_repo_thread(&pool, &repo.to_string(), 60).await;
    delete_repository(&pool, repo).await;

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

#[tokio::test]
async fn get_filter_facets_returns_distinct_sessions() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let repo_a = Uuid::new_v4();
    insert_repository(&pool, repo_a, "Apple", "/tmp/apple").await;
    insert_cc_repo_thread(&pool, &repo_a.to_string(), 60).await;
    insert_cc_repo_thread(&pool, &repo_a.to_string(), 30).await;
    insert_trigger_thread(&pool, "trig-a", "Trig A", 60).await;
    insert_app_thread(&pool, "habit", 90, true).await;
    insert_app_thread(&pool, "habit-tracker", 30, false).await;

    let ws = tempfile::tempdir().unwrap();
    let apps = crate::core::AppManager::new(ws.path()).unwrap();
    let facets = store
        .get_filter_facets(&apps)
        .await
        .expect("get_filter_facets");

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
        facets.apps.iter().map(|f| f.id.as_str()).collect();
    assert!(
        app_ids.contains("habit"),
        "archived-only app must still be a facet"
    );
    assert!(app_ids.contains("habit-tracker"));

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn get_filter_facets_resolves_repo_names_including_deleted() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let live_repo = Uuid::new_v4();
    insert_repository(&pool, live_repo, "Live Repo", "/tmp/live").await;
    insert_repo_name(&pool, live_repo, "Live Repo").await;
    insert_cc_repo_thread(&pool, &live_repo.to_string(), 30).await;

    let dead_repo = Uuid::new_v4();
    insert_repo_name(&pool, dead_repo, "Dead Repo").await;
    insert_cc_repo_thread(&pool, &dead_repo.to_string(), 60).await;

    let ws = tempfile::tempdir().unwrap();
    let apps = crate::core::AppManager::new(ws.path()).unwrap();
    let facets = store
        .get_filter_facets(&apps)
        .await
        .expect("get_filter_facets");

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

#[test]
fn parse_status_filter_accepts_the_values_rows_actually_carry() {
    let parsed = parse_status_filter_csv("running, waiting_for_user_answer ")
        .expect("the wire spelling is what rows print");
    assert_eq!(
        parsed,
        [ThreadStatus::Running, ThreadStatus::WaitingForUserAnswer]
    );
}

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

async fn place_hold(pool: &PgPool, thread_id: Uuid) {
    sqlx::query(
        "UPDATE thread_summaries SET hold = jsonb_build_object( \
            'reason', 'Taken over', 'root_thread_id', $1::text, 'placed_at', NOW()) \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("place hold");
}

fn filters(status: StatusFilter<'_>) -> ThreadSummaryFilters<'_> {
    ThreadSummaryFilters {
        status,
        sources: None,
        parent: None,
        has_draft: None,
        change_state: None,
        limit: 1000,
    }
}

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

#[tokio::test]
async fn a_held_thread_never_reads_as_waiting_on_the_owner() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let running = insert_thread_with_status(&pool, "Working", ThreadStatus::Running).await;
    let parked =
        insert_thread_with_status(&pool, "Parked", ThreadStatus::WaitingForUserAnswer).await;
    let held = insert_thread_with_status(&pool, "Held", ThreadStatus::WaitingForUserAnswer).await;
    place_hold(&pool, held).await;

    let ids = |summaries: Vec<ThreadSummary>| -> Vec<String> {
        let mut out: Vec<String> = summaries.into_iter().map(|s| s.thread_id).collect();
        out.sort();
        out
    };
    let sorted = |v: Vec<Uuid>| -> Vec<String> {
        let mut out: Vec<String> = v.into_iter().map(|id| id.to_string()).collect();
        out.sort();
        out
    };
    let list = |status| store.list_thread_summaries(filters(status));

    let awaiting = list(StatusFilter::OneOf(&[ThreadStatus::WaitingForUserAnswer]))
        .await
        .expect("list awaiting");
    assert_eq!(ids(awaiting), sorted(vec![parked]));

    let active = list(StatusFilter::Active(true)).await.expect("list active");
    assert_eq!(ids(active), sorted(vec![running, parked]));

    let inactive = list(StatusFilter::Active(false))
        .await
        .expect("list inactive");
    assert_eq!(ids(inactive), sorted(vec![held]));

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_waiting_filter_selects_exactly_what_awaits_owner_answer_says() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let mut want = Vec::new();
    for status in ThreadStatus::ALL {
        for held in [false, true] {
            let id = insert_thread_with_status(&pool, status.as_str(), status).await;
            if held {
                place_hold(&pool, id).await;
            }
            if crate::engine::thread_lifecycle::awaits_owner_answer(status, held) {
                want.push(id.to_string());
            }
        }
    }
    let mut got: Vec<String> = store
        .list_thread_summaries(filters(StatusFilter::OneOf(&[
            ThreadStatus::WaitingForUserAnswer,
        ])))
        .await
        .expect("list awaiting")
        .into_iter()
        .map(|s| s.thread_id)
        .collect();
    got.sort();
    want.sort();
    assert_eq!(got, want);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn count_agrees_with_list_for_every_status_filter_shape() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    insert_thread_with_status(&pool, "Working", ThreadStatus::Running).await;
    insert_thread_with_status(&pool, "Parked", ThreadStatus::WaitingForUserAnswer).await;
    let held = insert_thread_with_status(&pool, "Held", ThreadStatus::WaitingForUserAnswer).await;
    place_hold(&pool, held).await;
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

#[tokio::test]
async fn get_filter_facets_labels_apps_and_widgets_including_removed_ones() {
    use crate::core::{AppKind, NewAppKind};
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let ws = tempfile::tempdir().unwrap();
    let apps = crate::core::AppManager::new(ws.path()).unwrap();
    crate::core::apps::tests::app_and_widget(&apps, &bus).await;
    for id in ["removed-widget", "owned-widget"] {
        apps.create_app(
            &bus,
            id,
            "Removed",
            "",
            "<h1>w",
            NewAppKind::Widget {
                origin_thread_id: Uuid::from_u128(2),
                sizes: None,
            },
        )
        .await
        .unwrap();
    }
    apps.delete_app(&bus, "removed-widget", None).await.unwrap();
    apps.delete_widgets(&bus, &["owned-widget".to_string()], None)
        .await
        .unwrap();
    for id in [
        "habit-tracker",
        "fare-grid",
        "removed-widget",
        "owned-widget",
        "never-recorded",
    ] {
        insert_app_thread(&pool, id, 30, false).await;
    }

    let facets = store.get_filter_facets(&apps).await.expect("facets");
    let label = |id: &str| {
        let f = facets.apps.iter().find(|f| f.id == id).expect(id);
        (f.name.as_deref(), f.kind, f.deleted)
    };
    assert_eq!(
        label("habit-tracker"),
        (Some("Habit Tracker"), Some(AppKind::App), false)
    );
    assert_eq!(
        label("fare-grid"),
        (Some("Fare grid"), Some(AppKind::Widget), false)
    );
    assert_eq!(label("removed-widget"), (None, Some(AppKind::Widget), true));
    assert_eq!(label("owned-widget"), (None, Some(AppKind::Widget), true));
    assert_eq!(label("never-recorded"), (None, None, true));

    teardown_test_db(&db).await;
}
