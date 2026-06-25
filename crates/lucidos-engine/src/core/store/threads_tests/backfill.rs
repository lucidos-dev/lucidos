use super::*;
use super::test_helpers::*;

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

/// The bug fix for repos that predate the `RepositoryAdded` event: their name
/// is in neither the live `repositories` registry (once removed) nor the event
/// log, so the filter showed the raw UUID. Their path survives in
/// `changes.repo_root`, and the backfill scavenges its basename into
/// `repo_names` — so the deleted repo lists as `cognos`, not a UUID, AND the
/// projection-backed names already present are left untouched.
#[tokio::test]
async fn backfill_repo_names_from_changes_recovers_basename() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    // A removed pre-event repo: a CC thread references it and a change recorded
    // its path, but it's in neither `repositories` nor `repo_names`.
    let deleted_repo = Uuid::new_v4();
    let cc = insert_cc_repo_thread(&pool, &deleted_repo.to_string(), 60).await;
    insert_change(&pool, cc, "/Users/dev/IdeaProjects/cognos").await;

    // A repo already named in the projection (registry / RepositoryAdded) must
    // NOT be clobbered by the path basename.
    let named_repo = Uuid::new_v4();
    insert_repo_name(&pool, named_repo, "Canonical Name").await;
    let named_cc = insert_cc_repo_thread(&pool, &named_repo.to_string(), 50).await;
    insert_change(&pool, named_cc, "/Users/dev/projects/lowercase").await;

    // A repo with no change: nothing to scavenge — stays absent (NULL name).
    let nameless_repo = Uuid::new_v4();
    insert_cc_repo_thread(&pool, &nameless_repo.to_string(), 40).await;

    // App coding-agent threads record a *workspace* root in repo_root (basename
    // = workspace, not a repo). They have no cc_repo_id today, but the guard is
    // belt-and-suspenders for the documented footgun — pin it with an artificial
    // app row that DOES carry a cc_repo_id.
    let app_repo = Uuid::new_v4();
    let app = insert_cc_repo_thread(&pool, &app_repo.to_string(), 30).await;
    sqlx::query("UPDATE thread_summaries SET coding_agent_kind = 'app' WHERE thread_id = $1")
        .bind(app)
        .execute(&pool)
        .await
        .expect("mark app thread");
    insert_change(&pool, app, "/Users/dev/workspaces/my-workspace").await;

    let inserted = store
        .backfill_repo_names_from_changes()
        .await
        .expect("backfill_repo_names_from_changes");
    assert_eq!(inserted, 1, "only the deleted pre-event repo gets a scavenged name");

    async fn name_of(pool: &PgPool, id: Uuid) -> Option<String> {
        sqlx::query_scalar::<_, String>("SELECT name FROM repo_names WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .expect("fetch repo_name")
    }

    assert_eq!(
        name_of(&pool, deleted_repo).await.as_deref(),
        Some("cognos"),
        "deleted pre-event repo recovers its name from changes.repo_root basename"
    );
    assert_eq!(
        name_of(&pool, named_repo).await.as_deref(),
        Some("Canonical Name"),
        "a repo already in repo_names is never clobbered by the path basename"
    );
    assert_eq!(name_of(&pool, nameless_repo).await, None, "no change = no name invented");
    assert_eq!(
        name_of(&pool, app_repo).await,
        None,
        "app thread's workspace-root basename must not be scavenged as a repo name"
    );

    // The user-visible surface: the repos filter facet now labels the deleted
    // repo with its recovered name instead of the UUID.
    let facets = store.get_filter_facets().await.expect("get_filter_facets");
    let deleted_facet = facets
        .repos
        .iter()
        .find(|f| f.id.as_deref() == Some(deleted_repo.to_string().as_str()))
        .expect("deleted repo is a facet");
    assert_eq!(deleted_facet.name.as_deref(), Some("cognos"));

    let second = store
        .backfill_repo_names_from_changes()
        .await
        .expect("idempotent");
    assert_eq!(second, 0, "second run touches nothing (marker set)");

    teardown_test_db(&db).await;
}

/// Insert a coding-agent thread row with an explicit kind, external flag, and
/// stale `cc_repo_id`, mirroring the orphaned-thread state (a random repo id
/// bound at first `SessionStarted`, the live registry having since moved on).
async fn insert_ca_thread(
    pool: &sqlx::PgPool,
    kind: Option<&str>,
    is_external: bool,
    cc_repo_id: Option<&str>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
         (thread_id, title, source, message_count, last_activity, has_response, is_saved, is_coding_agent, coding_agent_kind, coding_agent_is_external_repo, cc_repo_id) \
         VALUES ($1, 'CC', 'claude_code', 1, NOW(), TRUE, FALSE, TRUE, $2, $3, $4)",
    )
    .bind(id)
    .bind(kind)
    .bind(is_external)
    .bind(cc_repo_id)
    .execute(pool)
    .await
    .expect("insert coding-agent thread");
    id
}

async fn fetch_cc_repo_id(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT cc_repo_id FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("fetch cc_repo_id")
}

#[tokio::test]
async fn backfill_repoints_lucidos_and_legacy_threads_to_deterministic_id() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let stale_a = Uuid::new_v4().to_string();
    let stale_b = Uuid::new_v4().to_string();
    let external_id = Uuid::new_v4().to_string();
    let legacy_external_id = Uuid::new_v4().to_string();
    let live_external_id = Uuid::new_v4();

    // lucidos-kind and legacy-NULL-kind threads BOTH target the Lucidos source
    // by definition → both should be re-pointed.
    let lucidos = insert_ca_thread(&pool, Some("lucidos"), false, Some(&stale_a)).await;
    let legacy = insert_ca_thread(&pool, None, false, Some(&stale_b)).await;
    // Modern external-repo thread (kind='external') — untouched.
    let external = insert_ca_thread(&pool, Some("external"), true, Some(&external_id)).await;
    // LEGACY external-repo thread: created before the kind column, so
    // coding_agent_kind IS NULL but the durable external flag is set. The
    // `kind IS NULL` arm must NOT mis-repoint it (the regression guard).
    let legacy_external =
        insert_ca_thread(&pool, None, true, Some(&legacy_external_id)).await;
    // A NULL-kind thread whose cc_repo_id is still a LIVE repository (not
    // orphaned) — must be left alone even without the external flag.
    insert_repository(&pool, live_external_id, "other", "/tmp/other").await;
    let live_bound =
        insert_ca_thread(&pool, None, false, Some(&live_external_id.to_string())).await;
    // App thread — untouched.
    let app = insert_app_thread(&pool, "demo", 5, false).await;

    let det = Uuid::new_v4();
    let det_s = det.to_string();

    let updated = store
        .backfill_cc_repo_id_to_deterministic(det)
        .await
        .expect("backfill");
    assert_eq!(updated, 2, "exactly the lucidos + legacy Lucidos rows were re-pointed");

    assert_eq!(fetch_cc_repo_id(&pool, lucidos).await.as_deref(), Some(det_s.as_str()));
    assert_eq!(fetch_cc_repo_id(&pool, legacy).await.as_deref(), Some(det_s.as_str()));
    assert_eq!(
        fetch_cc_repo_id(&pool, external).await.as_deref(),
        Some(external_id.as_str()),
        "modern external-repo thread untouched"
    );
    assert_eq!(
        fetch_cc_repo_id(&pool, legacy_external).await.as_deref(),
        Some(legacy_external_id.as_str()),
        "legacy NULL-kind external-repo thread untouched (external flag guard)"
    );
    assert_eq!(
        fetch_cc_repo_id(&pool, live_bound).await.as_deref(),
        Some(live_external_id.to_string().as_str()),
        "thread bound to a live repository untouched (live-binding guard)"
    );
    assert_eq!(fetch_cc_repo_id(&pool, app).await, None, "app thread cc_repo_id stays NULL");

    // Idempotent — a second boot re-points nothing (marker set).
    let again = store
        .backfill_cc_repo_id_to_deterministic(det)
        .await
        .expect("idempotent");
    assert_eq!(again, 0, "second run touches nothing (marker set)");

    teardown_test_db(&db).await;
}
