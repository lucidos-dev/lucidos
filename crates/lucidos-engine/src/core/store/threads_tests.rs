use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};
use sqlx::PgPool;
use uuid::Uuid;

/// Insert a parent thread and a child thread referencing it. Returns
/// `(parent_id, child_id)`. `child_saved` controls whether the child
/// is_saved (the parent is never saved). Both have has_response=TRUE
/// so they show up in get_recent_threads / get_older_threads.
async fn insert_parent_child(pool: &PgPool, child_saved: bool) -> (Uuid, Uuid) {
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    sqlx::query(
            "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, is_saved, parent_thread_id) \
             VALUES ($1, 'Parent thread', 'chat', 1, NOW(), TRUE, FALSE, NULL), \
                    ($2, 'Child thread',  'chat', 1, NOW(), TRUE, $3,   $1)"
        )
        .bind(parent)
        .bind(child)
        .bind(child_saved)
        .execute(pool)
        .await
        .expect("insert thread_summaries");
    (parent, child)
}

#[tokio::test]
async fn get_saved_threads_resolves_parent_title() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let (parent, child) = insert_parent_child(&pool, true).await;

    let saved = store.get_saved_threads().await.expect("get_saved_threads");

    let row = saved
        .iter()
        .find(|t| t.thread_id == child.to_string())
        .expect("child thread should appear in saved");
    assert_eq!(
        row.parent_thread_id.as_deref(),
        Some(parent.to_string().as_str())
    );
    assert_eq!(row.parent_thread_title.as_deref(), Some("Parent thread"));

    teardown_test_db(&db).await;
}

/// Regression test for fe5212ea: `get_recent_threads` wraps thread_summaries
/// in a derived table, so the parent_thread_title subquery must reference
/// the outer alias `t`, not the inner table name. Pre-fix code aliased the
/// outer as `ranked` but the subquery hardcoded `thread_summaries`, and
/// /api/threads 500'd with "invalid reference to FROM-clause entry".
#[tokio::test]
async fn get_recent_threads_resolves_parent_title() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let (_parent, child) = insert_parent_child(&pool, false).await;

    let recent = store
        .get_recent_threads(10)
        .await
        .expect("get_recent_threads");

    let row = recent
        .iter()
        .find(|t| t.thread_id == child.to_string())
        .expect("child thread should appear in recent");
    assert_eq!(row.parent_thread_title.as_deref(), Some("Parent thread"));

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn get_older_threads_resolves_parent_title() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let (_parent, child) = insert_parent_child(&pool, false).await;

    let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
    let older = store
        .get_older_threads(cutoff, 10, None, None, None)
        .await
        .expect("get_older_threads");

    let row = older
        .iter()
        .find(|t| t.thread_id == child.to_string())
        .expect("child thread should appear in older");
    assert_eq!(row.parent_thread_title.as_deref(), Some("Parent thread"));

    teardown_test_db(&db).await;
}

/// Insert a trigger-source thread with the given trigger_id/trigger_name and
/// last_activity offset. Returns the new thread id. has_response=TRUE so the
/// thread surfaces in `get_older_threads`.
async fn insert_trigger_thread(
    pool: &PgPool,
    trigger_id: &str,
    trigger_name: &str,
    minutes_ago: i64,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, is_saved, trigger_id, trigger_name) \
             VALUES ($1, 'T', 'trigger', 1, NOW() - ($2 || ' minutes')::interval, TRUE, FALSE, $3, $4)",
        )
        .bind(id)
        .bind(minutes_ago.to_string())
        .bind(trigger_id)
        .bind(trigger_name)
        .execute(pool)
        .await
        .expect("insert trigger thread");
    id
}

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
        .get_older_threads(cutoff, 10, None, Some(&["trig-a".to_string()]), None)
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
        .get_older_threads(cutoff, 10, None, Some(&["trig-orphan".to_string()]), None)
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

/// Insert a CC-source thread bound to the given repo UUID with the given
/// last_activity offset. Returns the new thread id.
async fn insert_cc_repo_thread(pool: &PgPool, repo_id: &str, minutes_ago: i64) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, is_saved, cc_repo_id) \
             VALUES ($1, 'CC', 'claude_code', 1, NOW() - ($2 || ' minutes')::interval, TRUE, FALSE, $3)",
        )
        .bind(id)
        .bind(minutes_ago.to_string())
        .bind(repo_id)
        .execute(pool)
        .await
        .expect("insert cc repo thread");
    id
}

/// Register a repo in the `repositories` table so `cc_repo_name` resolves.
async fn insert_repository(pool: &PgPool, repo_id: Uuid, name: &str, path: &str) {
    sqlx::query("INSERT INTO repositories (id, name, path) VALUES ($1, $2, $3)")
        .bind(repo_id)
        .bind(name)
        .bind(path)
        .execute(pool)
        .await
        .expect("insert repository");
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
        .get_older_threads(cutoff, 10, None, None, Some(&[repo_a.to_string()]))
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
        .get_older_threads(cutoff, 10, None, None, Some(&[orphan_repo.to_string()]))
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
async fn backfill_trigger_id_rewrites_v5_hashes_to_config_ids() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let live_config_id = "5633f3e1-110c-4df4-a6fc-c0df8fd36df4";
    let v5_hash = crate::scheduler::trigger_id_to_uuid(live_config_id).to_string();
    let untouched_config_id = "08f22aed-ab0f-498d-83d7-2d7e420141ff";

    sqlx::query(
        "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id) \
             VALUES ($1, 'TriggerCreated', $2, 'trigger', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(serde_json::json!({"trigger_id": live_config_id, "name": "Job Listing Check"}))
    .bind(live_config_id)
    .execute(&pool)
    .await
    .expect("insert TriggerCreated");

    let legacy = insert_trigger_thread(&pool, &v5_hash, "Job Listing Check", 60).await;
    let already_correct =
        insert_trigger_thread(&pool, untouched_config_id, "Check Bank Balance", 60).await;
    let orphan_v5 = insert_trigger_thread(
        &pool,
        "deadbeef-dead-5eed-dead-deaddeaddead",
        "Some deleted trigger",
        60,
    )
    .await;

    let updated = store
        .backfill_trigger_id_v5_to_config_id()
        .await
        .expect("backfill");
    assert_eq!(updated, 1, "exactly one row had a known v5 hash");

    let legacy_after: String =
        sqlx::query_scalar("SELECT trigger_id FROM thread_summaries WHERE thread_id = $1")
            .bind(legacy)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(legacy_after, live_config_id);

    let untouched_after: String =
        sqlx::query_scalar("SELECT trigger_id FROM thread_summaries WHERE thread_id = $1")
            .bind(already_correct)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(untouched_after, untouched_config_id);

    let orphan_after: String =
        sqlx::query_scalar("SELECT trigger_id FROM thread_summaries WHERE thread_id = $1")
            .bind(orphan_v5)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        orphan_after, "deadbeef-dead-5eed-dead-deaddeaddead",
        "v5 hash with no matching TriggerCreated stays as-is"
    );

    let second = store
        .backfill_trigger_id_v5_to_config_id()
        .await
        .expect("idempotent");
    assert_eq!(second, 0, "second run touches nothing");

    teardown_test_db(&db).await;
}

/// Insert a trigger-source thread row with NULL trigger_id/trigger_name —
/// the state every legacy thread is in after the broken
/// `20260429214800_addtriggeridtothreadsummaries.sql` backfill, which only
/// reads `payload->>'trigger_id'` and skips the legacy `task_id`/`task_name`
/// pair. Returns the new thread id.
async fn insert_null_trigger_thread(pool: &PgPool, minutes_ago: i64) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, is_saved, trigger_id, trigger_name) \
             VALUES ($1, 'T', 'trigger', 1, NOW() - ($2 || ' minutes')::interval, TRUE, FALSE, NULL, NULL)",
        )
        .bind(id)
        .bind(minutes_ago.to_string())
        .execute(pool)
        .await
        .expect("insert null-trigger thread");
    id
}

/// Insert a `TriggerStarted` event for the given thread with a raw payload.
/// Lets the test mimic legacy (`task_id`) vs modern (`trigger_id`) shapes.
async fn insert_trigger_started_event(pool: &PgPool, thread_id: Uuid, payload: serde_json::Value) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id, thread_id) \
             VALUES ($1, 'TriggerStarted', $2, 'thread', $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(payload)
    .bind(thread_id.to_string())
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("insert TriggerStarted");
}

/// Regression for the work-workspace bug where every trigger thread
/// rendered with NULL `trigger_id` because the
/// `20260429214800_addtriggeridtothreadsummaries.sql` backfill only read
/// `payload->>'trigger_id'` and ignored legacy events that stored the id
/// under `task_id`. The runtime backfill below recovers the value from
/// `events`, COALESCEing both shapes.
#[tokio::test]
async fn backfill_trigger_id_from_events_reads_legacy_task_id() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let legacy = insert_null_trigger_thread(&pool, 90).await;
    insert_trigger_started_event(
        &pool,
        legacy,
        serde_json::json!({
            "task_id": "364d689e-0620-5712-9739-c9ceb1d12fe1",
            "task_name": "Legacy Trigger",
            "channel": "trigger",
        }),
    )
    .await;

    let modern = insert_null_trigger_thread(&pool, 60).await;
    insert_trigger_started_event(
        &pool,
        modern,
        serde_json::json!({
            "trigger_id": "a969c963-dbc0-4f5f-8ebb-58c7f2b80c96",
            "trigger_name": "Modern Trigger",
            "channel": "trigger",
        }),
    )
    .await;

    // Already-set trigger_id must NOT be overwritten — the runtime
    // projection populated it, so events would just confirm what's there.
    let preset = insert_trigger_thread(&pool, "preset-id", "Preset Name", 30).await;
    insert_trigger_started_event(
        &pool,
        preset,
        serde_json::json!({
            "trigger_id": "different-id-should-be-ignored",
            "trigger_name": "Different Name",
        }),
    )
    .await;

    // Trigger-source thread with no TriggerStarted event in `events`
    // (corruption / lost event). Must stay NULL — never invent values.
    let orphan = insert_null_trigger_thread(&pool, 20).await;

    let updated = store
        .backfill_trigger_id_from_events()
        .await
        .expect("backfill_trigger_id_from_events");
    assert_eq!(updated, 2, "two NULL-trigger rows had matching events");

    let (legacy_id, legacy_name) = fetch_trigger_pair(&pool, legacy).await;
    assert_eq!(
        legacy_id.as_deref(),
        Some("364d689e-0620-5712-9739-c9ceb1d12fe1"),
        "legacy task_id must be COALESCEd into trigger_id"
    );
    assert_eq!(legacy_name.as_deref(), Some("Legacy Trigger"));

    let (modern_id, modern_name) = fetch_trigger_pair(&pool, modern).await;
    assert_eq!(
        modern_id.as_deref(),
        Some("a969c963-dbc0-4f5f-8ebb-58c7f2b80c96")
    );
    assert_eq!(modern_name.as_deref(), Some("Modern Trigger"));

    let (preset_id, preset_name) = fetch_trigger_pair(&pool, preset).await;
    assert_eq!(
        preset_id.as_deref(),
        Some("preset-id"),
        "row that already had trigger_id must not be overwritten"
    );
    assert_eq!(preset_name.as_deref(), Some("Preset Name"));

    let (orphan_id, orphan_name) = fetch_trigger_pair(&pool, orphan).await;
    assert_eq!(orphan_id, None, "no event = no value invented");
    assert_eq!(orphan_name, None);

    let second = store
        .backfill_trigger_id_from_events()
        .await
        .expect("idempotent");
    assert_eq!(second, 0, "second run touches nothing (marker set)");

    teardown_test_db(&db).await;
}

async fn fetch_trigger_pair(pool: &PgPool, thread_id: Uuid) -> (Option<String>, Option<String>) {
    sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT trigger_id, trigger_name FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .expect("fetch trigger pair")
}

/// Reproduce the original work-workspace bug end-to-end: a legacy event
/// with `task_id` set to the v5 hash of `config.id`, and a NULL row in
/// `thread_summaries`. After both backfills run in startup order the
/// dropdown filter (which sends `config.id`) must match.
#[tokio::test]
async fn both_backfills_compose_legacy_task_id_to_config_id() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let config_id = "a969c963-dbc0-4f5f-8ebb-58c7f2b80c96";
    let v5_hash = crate::scheduler::trigger_id_to_uuid(config_id).to_string();

    sqlx::query(
        "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id) \
             VALUES ($1, 'TriggerCreated', $2, 'trigger', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(serde_json::json!({"trigger_id": config_id, "name": "UA Analysis Runner"}))
    .bind(config_id)
    .execute(&pool)
    .await
    .expect("insert TriggerCreated");

    let thread = insert_null_trigger_thread(&pool, 60).await;
    insert_trigger_started_event(
        &pool,
        thread,
        serde_json::json!({
            "task_id": v5_hash,
            "task_name": "UA Analysis Runner",
        }),
    )
    .await;

    let from_events = store
        .backfill_trigger_id_from_events()
        .await
        .expect("step 1");
    assert_eq!(from_events, 1);
    let v5_to_cfg = store
        .backfill_trigger_id_v5_to_config_id()
        .await
        .expect("step 2");
    assert_eq!(v5_to_cfg, 1);

    let (final_id, _) = fetch_trigger_pair(&pool, thread).await;
    assert_eq!(
        final_id.as_deref(),
        Some(config_id),
        "legacy task_id (v5 hash) must end up as the live config.id so the dropdown filter matches"
    );

    teardown_test_db(&db).await;
}

/// `get_recent_threads` must surface every thread that NEEDS user action
/// (`cc_has_changes=TRUE`, `status='waiting_for_user_answer'`, `status='failed'`)
/// even when the per-source `rn <= per_source` window would otherwise drop it.
///
/// REVIEW is a "needs attention" pile. Without this guarantee, a CC thread
/// pushed past the per-source window vanishes from the drawer entirely —
/// the user has no way to Apply/Discard the changes, no way to see them in
/// REVIEW, no Diff button. The `changes` data still exists in the DB but
/// the thread carrying it is invisible until the user manually scrolls
/// far enough to trigger `get_older_threads`.
///
/// Regression: 2026-04-25 dev workspace had four CC threads with pending
/// changes at rn=17, 18, 19, 40 — all hidden from /api/threads.
#[tokio::test]
async fn get_recent_threads_always_includes_actionable_threads_beyond_window() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    // 18 CC threads with descending last_activity. The three at i=15..17
    // carry actionable signals — each picks an inert status so the only
    // thing that lets it bypass the rn<=15 cap is the predicate under
    // test: cc_has_changes (#15), waiting_for_user_answer (#16),
    // failed (#17). One distinct second per row stabilizes the ranking.
    let now = chrono::Utc::now();
    let mut ids = Vec::with_capacity(18);
    for i in 0..18 {
        let id = Uuid::new_v4();
        ids.push(id);
        let last_activity = now - chrono::Duration::seconds(i as i64);
        let (status, cc_has_changes, section) = match i {
            15 => (ThreadStatus::Idle.as_str(), true, "inbox"),
            16 => (ThreadStatus::WaitingForUserAnswer.as_str(), false, "inbox"),
            17 => (ThreadStatus::Failed.as_str(), false, "inbox"),
            _ => (ThreadStatus::Idle.as_str(), false, "archived"),
        };
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, cc_has_changes, archive_state) \
                 VALUES ($1, $2, 'claude_code', 1, $3, TRUE, $4, $5, $6)",
        )
        .bind(id)
        .bind(format!("Thread {}", i))
        .bind(last_activity)
        .bind(status)
        .bind(cc_has_changes)
        .bind(section)
        .execute(&pool)
        .await
        .expect("insert thread_summaries");
    }
    let pending_changes = ids[15];
    let needs_answer = ids[16];
    let failed = ids[17];

    let recent = store
        .get_recent_threads(15)
        .await
        .expect("get_recent_threads");

    let returned: std::collections::HashSet<&str> =
        recent.iter().map(|t| t.thread_id.as_str()).collect();
    let pending = pending_changes.to_string();
    let answer = needs_answer.to_string();
    let fail = failed.to_string();
    assert!(
            returned.contains(pending.as_str()),
            "thread with cc_has_changes=TRUE at rn>per_source must surface (Apply/Discard buttons live here); returned {} entries",
            recent.len()
        );
    assert!(
            returned.contains(answer.as_str()),
            "thread with status=waiting_for_user_answer at rn>per_source must surface (Question card lives here); returned {} entries",
            recent.len()
        );
    assert!(
            returned.contains(fail.as_str()),
            "thread with status=failed at rn>per_source must surface (error indicator lives here); returned {} entries",
            recent.len()
        );

    teardown_test_db(&db).await;
}

/// REVIEW must contain every inbox thread, not just the top-N per source.
/// An inbox row is one the user hasn't dismissed; capping it would silently
/// hide work — e.g. a CC thread whose subprocess crashed mid-flow without
/// emitting a terminal event keeps `cc_has_changes=false` and would be
/// gated out solely by recency.
#[tokio::test]
async fn get_recent_threads_returns_all_inbox_threads_beyond_window() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    // 20 inert idle inbox CC threads. None carry an actionable signal,
    // so the only thing that can surface row 19 (rn=20, past the window
    // of 15) is the inbox bypass under test.
    let now = chrono::Utc::now();
    let mut ids = Vec::with_capacity(20);
    for i in 0..20 {
        let id = Uuid::new_v4();
        ids.push(id);
        let last_activity = now - chrono::Duration::seconds(i as i64);
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, cc_has_changes, archive_state) \
                 VALUES ($1, $2, 'claude_code', 1, $3, TRUE, 'idle', FALSE, 'inbox')",
        )
        .bind(id)
        .bind(format!("Inbox thread {}", i))
        .bind(last_activity)
        .execute(&pool)
        .await
        .expect("insert thread_summaries");
    }
    let furthest_back = ids[19];

    let recent = store
        .get_recent_threads(15)
        .await
        .expect("get_recent_threads");

    let returned: std::collections::HashSet<&str> =
        recent.iter().map(|t| t.thread_id.as_str()).collect();
    let needed = furthest_back.to_string();
    assert!(
        returned.contains(needed.as_str()),
        "inbox thread at rn>per_source must surface; got {} entries",
        recent.len()
    );
    assert_eq!(
        recent.len(),
        20,
        "all 20 inbox threads must appear; got {}",
        recent.len()
    );

    teardown_test_db(&db).await;
}

/// History (archived threads) stays capped per source so the drawer
/// doesn't load the whole archive on refresh; `get_older_threads` pages
/// backward through what this omits.
#[tokio::test]
async fn get_recent_threads_caps_archived_threads_at_per_source() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    // 20 archived idle chats with no actionable signal — only the top 15
    // per source should come back.
    let now = chrono::Utc::now();
    for i in 0..20 {
        let last_activity = now - chrono::Duration::seconds(i as i64);
        sqlx::query(
            "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, cc_has_changes, archive_state) \
                 VALUES ($1, $2, 'chat', 1, $3, TRUE, 'idle', FALSE, 'archived')",
        )
        .bind(Uuid::new_v4())
        .bind(format!("Archived chat {}", i))
        .bind(last_activity)
        .execute(&pool)
        .await
        .expect("insert thread_summaries");
    }

    let recent = store
        .get_recent_threads(15)
        .await
        .expect("get_recent_threads");
    let chat_count = recent.iter().filter(|t| t.channel == "chat").count();
    assert_eq!(
        chat_count, 15,
        "archived threads must stay capped at per_source; got {}",
        chat_count
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn get_threads_by_ids_resolves_parent_title() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let (_parent, child) = insert_parent_child(&pool, false).await;

    let infos = store
        .get_threads_by_ids(&[child.to_string()])
        .await
        .expect("get_threads_by_ids");

    assert_eq!(infos.len(), 1);
    assert_eq!(
        infos[0].parent_thread_title.as_deref(),
        Some("Parent thread")
    );

    teardown_test_db(&db).await;
}

async fn insert_thread(pool: &PgPool, id: Uuid, title: &str) {
    sqlx::query(
            "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response) \
             VALUES ($1, $2, 'chat', 0, NOW(), TRUE)"
        )
        .bind(id)
        .bind(title)
        .execute(pool)
        .await
        .expect("insert thread_summaries");
}

async fn insert_message(pool: &PgPool, thread_id: Uuid, event_type: &str, text: &str) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES ($1, $2, $3, $4, 'thread', $4::text)",
    )
    .bind(Uuid::new_v4())
    .bind(event_type)
    .bind(serde_json::json!({ "text": text }))
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("insert event");
}

/// `memory_entries` is created by `PgVectorIndex::new` rather than a migration,
/// so any test that joins it must initialize the index first.
async fn ensure_memory_entries_table(pool: &PgPool) {
    crate::memory::pgvector::PgVectorIndex::new(pool.clone())
        .await
        .expect("init pgvector schema");
}

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

#[tokio::test]
async fn recent_thread_messages_for_extraction_returns_oldest_first() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let thread = Uuid::new_v4();
    insert_thread(&pool, thread, "Test").await;
    insert_message(&pool, thread, "MessageReceived", "regnr bil").await;
    insert_message(&pool, thread, "ResponseGenerated", "Ola Hansen (eier)").await;
    insert_message(&pool, thread, "MessageReceived", "tlf til verkstedet").await;

    let ctx = store
        .recent_thread_messages_for_extraction(thread, 5, None)
        .await
        .expect("get context");

    assert!(ctx.contains("regnr bil"), "ctx={}", ctx);
    assert!(ctx.contains("Ola Hansen (eier)"), "ctx={}", ctx);
    let first_pos = ctx.find("regnr bil").unwrap();
    let second_pos = ctx.find("Ola Hansen").unwrap();
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
