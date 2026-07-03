//! Tests for one-off migrations whose correctness depends on data shape
//! and cannot be verified by `cargo check` alone.

use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const FIX_INVOCATION_SQL: &str =
    include_str!("../migrations/20260420195145_fix_trigger_invocation_event_only_backfill.sql");

const RENAME_TRIGGER_RUN_TEXT_TO_INTENT_SQL: &str =
    include_str!("../migrations/20260429213500_rename_trigger_run_text_to_intent.sql");

const MIGRATE_TRIGGER_ON_TO_SUBSCRIPTION_LIST_SQL: &str = include_str!(
    "../migrations/20260516195912_migrate_trigger_on_to_subscription_list.sql"
);

/// Which payload field naming a `TriggerStarted` row uses.
#[derive(Clone, Copy)]
enum StartedShape {
    /// Modern: `trigger_id` + `trigger_name`.
    Modern,
    /// Legacy: `task_id` + `task_name` (a derived hash, not the trigger UUID).
    Legacy,
}

async fn insert_trigger_config(
    pool: &PgPool,
    event_type: &str,
    trigger_id: &str,
    name: &str,
    on_event: Option<&str>,
    schedule: &[&str],
    created_offset_secs: i64,
) {
    let payload = json!({
        "trigger_id": trigger_id,
        "name": name,
        "on": on_event,
        "schedule": schedule,
    });
    sqlx::query(
        r#"INSERT INTO events (id, event_type, payload, created)
           VALUES ($1, $2, $3, NOW() - make_interval(secs => $4))"#,
    )
    .bind(Uuid::new_v4())
    .bind(event_type)
    .bind(payload)
    .bind(created_offset_secs as f64)
    .execute(pool)
    .await
    .expect("insert trigger config");
}

/// Where the row's `created` timestamp falls relative to the bad migration.
/// The corrective migration scopes its update by `created < installed_on`, so
/// `BeforeBadMigration` simulates a legacy row and `AfterBadMigration`
/// simulates a row emitted by the new code path.
enum CreatedAt {
    BeforeBadMigration { offset_secs: i64 },
    AfterBadMigration,
}

async fn insert_trigger_started(
    pool: &PgPool,
    shape: StartedShape,
    trigger_id_or_hash: &str,
    name: &str,
    invocation: Value,
    when: CreatedAt,
) -> Uuid {
    let event_id = Uuid::new_v4();
    let (id_field, name_field) = match shape {
        StartedShape::Modern => ("trigger_id", "trigger_name"),
        StartedShape::Legacy => ("task_id", "task_name"),
    };
    let payload = json!({
        id_field: trigger_id_or_hash,
        name_field: name,
        "invocation": invocation,
    });
    let created_sql = match when {
        CreatedAt::BeforeBadMigration { offset_secs } => format!(
            "(SELECT installed_on FROM _sqlx_migrations WHERE version = 20260420164143) - make_interval(secs => {offset_secs})"
        ),
        CreatedAt::AfterBadMigration => {
            "(SELECT installed_on FROM _sqlx_migrations WHERE version = 20260420164143) + INTERVAL '1 second'".to_string()
        }
    };
    sqlx::query(&format!(
        "INSERT INTO events (id, event_type, payload, created)
         VALUES ($1, 'TriggerStarted', $2, {created_sql})"
    ))
    .bind(event_id)
    .bind(payload)
    .execute(pool)
    .await
    .expect("insert trigger started");
    event_id
}

async fn invocation_of(pool: &PgPool, event_id: Uuid) -> Value {
    sqlx::query_scalar::<_, Value>("SELECT payload->'invocation' FROM events WHERE id = $1")
        .bind(event_id)
        .fetch_one(pool)
        .await
        .expect("read invocation")
}

#[tokio::test]
async fn fix_invocation_promotes_event_only_legacy_runs_to_event_kind() {
    let (pool, db_name) = setup_test_db().await;

    let event_only = "11111111-1111-1111-1111-111111111111";
    let schedule_only = "22222222-2222-2222-2222-222222222222";
    let hybrid = "33333333-3333-3333-3333-333333333333";

    // Configs created 2 hours ago, well before the bad migration ran.
    insert_trigger_config(
        &pool,
        "TriggerCreated",
        event_only,
        "E2E Tests on Project Hardened",
        Some("ProjectHardened"),
        &[],
        7200,
    )
    .await;
    insert_trigger_config(
        &pool,
        "TriggerCreated",
        schedule_only,
        "Midnight Compile & Test Fix",
        None,
        &["0 0 0 * * *"],
        7200,
    )
    .await;
    insert_trigger_config(
        &pool,
        "TriggerCreated",
        hybrid,
        "Hybrid Trigger",
        Some("SomethingHappened"),
        &["0 0 9 * * *"],
        7200,
    )
    .await;

    // Legacy TriggerStarted rows — all wrongly defaulted to Schedule by the
    // bad migration. Created 1 hour ago (before bad migration's installed_on).
    let legacy_event_only = insert_trigger_started(
        &pool,
        StartedShape::Modern,
        event_only,
        "E2E Tests on Project Hardened",
        json!({"kind": "Schedule"}),
        CreatedAt::BeforeBadMigration { offset_secs: 3600 },
    )
    .await;
    let legacy_schedule_only = insert_trigger_started(
        &pool,
        StartedShape::Modern,
        schedule_only,
        "Midnight Compile & Test Fix",
        json!({"kind": "Schedule"}),
        CreatedAt::BeforeBadMigration { offset_secs: 3600 },
    )
    .await;
    let legacy_hybrid = insert_trigger_started(
        &pool,
        StartedShape::Modern,
        hybrid,
        "Hybrid Trigger",
        json!({"kind": "Schedule"}),
        CreatedAt::BeforeBadMigration { offset_secs: 3600 },
    )
    .await;
    let legacy_event_only_taskid = insert_trigger_started(
        &pool,
        StartedShape::Legacy,
        event_only,
        "E2E Tests on Project Hardened",
        json!({"kind": "Schedule"}),
        CreatedAt::BeforeBadMigration { offset_secs: 3600 },
    )
    .await;

    // A NEW TriggerStarted emitted after the bad migration ran. The migration
    // must leave anything created at-or-after `installed_on` alone — that is
    // where the new code's correct invocation values live.
    let new_event_only = insert_trigger_started(
        &pool,
        StartedShape::Modern,
        event_only,
        "E2E Tests on Project Hardened",
        json!({"kind": "Schedule"}),
        CreatedAt::AfterBadMigration,
    )
    .await;

    sqlx::raw_sql(FIX_INVOCATION_SQL)
        .execute(&pool)
        .await
        .expect("run corrective migration");

    assert_eq!(
        invocation_of(&pool, legacy_event_only).await,
        json!({"kind": "Event", "event_type": "ProjectHardened"}),
        "event-only legacy run must be promoted to Event invocation",
    );
    assert_eq!(
        invocation_of(&pool, legacy_event_only_taskid).await,
        json!({"kind": "Event", "event_type": "ProjectHardened"}),
        "event-only legacy run with task_id field must also be promoted",
    );
    assert_eq!(
        invocation_of(&pool, legacy_schedule_only).await,
        json!({"kind": "Schedule"}),
        "schedule-only legacy run must stay as Schedule",
    );
    assert_eq!(
        invocation_of(&pool, legacy_hybrid).await,
        json!({"kind": "Schedule"}),
        "hybrid run is ambiguous — leave the bad-migration default in place",
    );
    assert_eq!(
        invocation_of(&pool, new_event_only).await,
        json!({"kind": "Schedule"}),
        "events created after the bad migration ran must not be touched",
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn fix_invocation_falls_back_to_name_when_legacy_task_id_does_not_match() {
    // Legacy TriggerStarted rows carry `task_id` that was a derived hash of
    // the trigger name, not the trigger config's UUID — so a trigger_id
    // join misses them. The migration must fall back to a name join.
    let (pool, db_name) = setup_test_db().await;

    let real_trigger_id = "55555555-5555-5555-5555-555555555555";
    let unrelated_task_hash = "99999999-9999-9999-9999-999999999999";

    insert_trigger_config(
        &pool,
        "TriggerCreated",
        real_trigger_id,
        "E2E Tests on Project Hardened",
        Some("ProjectHardened"),
        &[],
        7200,
    )
    .await;

    let legacy_started_by_name = insert_trigger_started(
        &pool,
        StartedShape::Legacy,
        unrelated_task_hash,
        "E2E Tests on Project Hardened",
        json!({"kind": "Schedule"}),
        CreatedAt::BeforeBadMigration { offset_secs: 3600 },
    )
    .await;

    sqlx::raw_sql(FIX_INVOCATION_SQL)
        .execute(&pool)
        .await
        .expect("run corrective migration");

    assert_eq!(
        invocation_of(&pool, legacy_started_by_name).await,
        json!({"kind": "Event", "event_type": "ProjectHardened"}),
        "name-based fallback must promote legacy run whose task_id was a derived hash",
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn fix_invocation_uses_latest_trigger_config_at_time_of_run() {
    let (pool, db_name) = setup_test_db().await;

    let trigger_id = "44444444-4444-4444-4444-444444444444";

    // Original config: schedule-only.
    insert_trigger_config(
        &pool,
        "TriggerCreated",
        trigger_id,
        "Was Schedule, Now Event",
        None,
        &["0 0 8 * * *"],
        10_800,
    )
    .await;

    // The historical run happened while the trigger was still schedule-only.
    let started_under_schedule = insert_trigger_started(
        &pool,
        StartedShape::Modern,
        trigger_id,
        "Was Schedule, Now Event",
        json!({"kind": "Schedule"}),
        CreatedAt::BeforeBadMigration { offset_secs: 9000 },
    )
    .await;

    // Later the trigger was repurposed as event-only.
    insert_trigger_config(
        &pool,
        "TriggerUpdated",
        trigger_id,
        "Was Schedule, Now Event",
        Some("UserSignedUp"),
        &[],
        7200,
    )
    .await;

    // A second run after the update — fired by the event.
    let started_under_event = insert_trigger_started(
        &pool,
        StartedShape::Modern,
        trigger_id,
        "Was Schedule, Now Event",
        json!({"kind": "Schedule"}),
        CreatedAt::BeforeBadMigration { offset_secs: 3600 },
    )
    .await;

    sqlx::raw_sql(FIX_INVOCATION_SQL)
        .execute(&pool)
        .await
        .expect("run corrective migration");

    assert_eq!(
        invocation_of(&pool, started_under_schedule).await,
        json!({"kind": "Schedule"}),
        "run that happened while trigger was schedule-only must stay Schedule",
    );
    assert_eq!(
        invocation_of(&pool, started_under_event).await,
        json!({"kind": "Event", "event_type": "UserSignedUp"}),
        "run that happened after trigger became event-only must be Event",
    );

    teardown_test_db(&db_name).await;
}

async fn insert_event_with_payload(pool: &PgPool, event_type: &str, payload: Value) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO events (id, event_type, payload) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(event_type)
        .bind(payload)
        .execute(pool)
        .await
        .expect("insert event");
    id
}

/// `insert_event_with_payload` but with an explicit `created_offset_secs`
/// relative to NOW (positive = older). Used by orphan-condition migration
/// tests that need a TriggerUpdated row to land strictly after a
/// TriggerCreated row regardless of test scheduling jitter.
async fn insert_event_with_created_offset(
    pool: &PgPool,
    event_type: &str,
    payload: Value,
    created_offset_secs: i64,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO events (id, event_type, payload, created)
           VALUES ($1, $2, $3, NOW() - make_interval(secs => $4))"#,
    )
    .bind(id)
    .bind(event_type)
    .bind(payload)
    .bind(created_offset_secs as f64)
    .execute(pool)
    .await
    .expect("insert event");
    id
}

async fn run_payload_of(pool: &PgPool, event_id: Uuid) -> Value {
    sqlx::query_scalar::<_, Value>("SELECT payload->'run' FROM events WHERE id = $1")
        .bind(event_id)
        .fetch_one(pool)
        .await
        .expect("read run payload")
}

#[tokio::test]
async fn rename_trigger_run_text_to_intent_rewrites_intent_variant() {
    let (pool, db_name) = setup_test_db().await;

    let created_id = insert_event_with_payload(
        &pool,
        "TriggerCreated",
        json!({
            "trigger_id": "11111111-1111-1111-1111-111111111111",
            "name": "Old-shape Created",
            "schedule": ["0 0 8 * * *"],
            "timezone": "Europe/Oslo",
            "run": { "type": "intent", "text": "Notify me about X", "knowhow": ["domain"] },
        }),
    )
    .await;
    let updated_id = insert_event_with_payload(
        &pool,
        "TriggerUpdated",
        json!({
            "trigger_id": "11111111-1111-1111-1111-111111111111",
            "run": { "type": "intent", "text": "Updated intent", "knowhow": [] },
        }),
    )
    .await;

    sqlx::raw_sql(RENAME_TRIGGER_RUN_TEXT_TO_INTENT_SQL)
        .execute(&pool)
        .await
        .expect("run migration");

    assert_eq!(
        run_payload_of(&pool, created_id).await,
        json!({ "type": "intent", "intent": "Notify me about X", "knowhow": ["domain"] }),
        "TriggerCreated.run.text must move to run.intent",
    );
    assert_eq!(
        run_payload_of(&pool, updated_id).await,
        json!({ "type": "intent", "intent": "Updated intent", "knowhow": [] }),
        "TriggerUpdated.run.text must move to run.intent",
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn rename_trigger_run_text_to_intent_leaves_other_shapes_alone() {
    let (pool, db_name) = setup_test_db().await;

    let script_id = insert_event_with_payload(
        &pool,
        "TriggerCreated",
        json!({
            "trigger_id": "22222222-2222-2222-2222-222222222222",
            "name": "Script trigger",
            "run": { "type": "script", "path": "oura/run.py" },
        }),
    )
    .await;
    let already_new_id = insert_event_with_payload(
        &pool,
        "TriggerCreated",
        json!({
            "trigger_id": "33333333-3333-3333-3333-333333333333",
            "name": "Already migrated",
            "run": { "type": "intent", "intent": "Already canonical", "knowhow": [] },
        }),
    )
    .await;
    let unrelated_id = insert_event_with_payload(
        &pool,
        "MessageReceived",
        json!({ "text": "this is the chat text field, not a trigger run" }),
    )
    .await;

    sqlx::raw_sql(RENAME_TRIGGER_RUN_TEXT_TO_INTENT_SQL)
        .execute(&pool)
        .await
        .expect("run migration");

    assert_eq!(
        run_payload_of(&pool, script_id).await,
        json!({ "type": "script", "path": "oura/run.py" }),
        "script-variant runs must be untouched",
    );
    assert_eq!(
        run_payload_of(&pool, already_new_id).await,
        json!({ "type": "intent", "intent": "Already canonical", "knowhow": [] }),
        "already-canonical runs must be untouched",
    );
    let chat_text: String =
        sqlx::query_scalar("SELECT payload->>'text' FROM events WHERE id = $1")
            .bind(unrelated_id)
            .fetch_one(&pool)
            .await
            .expect("read chat text");
    assert_eq!(
        chat_text, "this is the chat text field, not a trigger run",
        "non-trigger event types' top-level `text` must be untouched",
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn rename_trigger_run_text_to_intent_strips_stale_text_when_both_keys_present() {
    // Coexistence case: a row carrying both `text` (old) and `intent` (new) gets
    // its stale `text` dropped without overwriting the canonical `intent` value.
    let (pool, db_name) = setup_test_db().await;

    let event_id = insert_event_with_payload(
        &pool,
        "TriggerCreated",
        json!({
            "trigger_id": "55555555-5555-5555-5555-555555555555",
            "name": "Both keys",
            "run": {
                "type": "intent",
                "text": "STALE — must be stripped",
                "intent": "Canonical — must be preserved",
                "knowhow": [],
            },
        }),
    )
    .await;

    sqlx::raw_sql(RENAME_TRIGGER_RUN_TEXT_TO_INTENT_SQL)
        .execute(&pool)
        .await
        .expect("run migration");

    assert_eq!(
        run_payload_of(&pool, event_id).await,
        json!({ "type": "intent", "intent": "Canonical — must be preserved", "knowhow": [] }),
        "stale `text` must be stripped without clobbering existing `intent`",
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn rename_trigger_run_text_to_intent_is_idempotent() {
    let (pool, db_name) = setup_test_db().await;

    let event_id = insert_event_with_payload(
        &pool,
        "TriggerCreated",
        json!({
            "trigger_id": "44444444-4444-4444-4444-444444444444",
            "name": "Old shape",
            "run": { "type": "intent", "text": "Original", "knowhow": [] },
        }),
    )
    .await;

    for _ in 0..3 {
        sqlx::raw_sql(RENAME_TRIGGER_RUN_TEXT_TO_INTENT_SQL)
            .execute(&pool)
            .await
            .expect("run migration");
    }

    assert_eq!(
        run_payload_of(&pool, event_id).await,
        json!({ "type": "intent", "intent": "Original", "knowhow": [] }),
        "running the migration multiple times must converge to the same result",
    );

    teardown_test_db(&db_name).await;
}

async fn on_payload_of(pool: &PgPool, event_id: Uuid) -> Value {
    sqlx::query_scalar::<_, Value>(
        "SELECT COALESCE(payload->'on', 'null'::jsonb) FROM events WHERE id = $1",
    )
    .bind(event_id)
    .fetch_one(pool)
    .await
    .expect("read on payload")
}

async fn has_top_level_condition(pool: &PgPool, event_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT payload ? 'condition' FROM events WHERE id = $1")
        .bind(event_id)
        .fetch_one(pool)
        .await
        .expect("read condition presence")
}

#[tokio::test]
async fn migrate_on_string_with_sibling_condition_becomes_one_entry_array() {
    let (pool, db_name) = setup_test_db().await;

    let event_id = insert_event_with_payload(
        &pool,
        "TriggerCreated",
        json!({
            "trigger_id": "11111111-1111-1111-1111-111111111111",
            "name": "Sleep nudge",
            "on": "OuraSleepImported",
            "condition": { "sleep_score": { "$lt": 70 } },
        }),
    )
    .await;

    sqlx::raw_sql(MIGRATE_TRIGGER_ON_TO_SUBSCRIPTION_LIST_SQL)
        .execute(&pool)
        .await
        .expect("run migration");

    assert_eq!(
        on_payload_of(&pool, event_id).await,
        json!([{
            "event_type": "OuraSleepImported",
            "condition": { "sleep_score": { "$lt": 70 } }
        }]),
    );
    assert!(
        !has_top_level_condition(&pool, event_id).await,
        "top-level condition key must be removed after migration"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn migrate_on_string_without_condition_becomes_one_entry_array_no_condition() {
    let (pool, db_name) = setup_test_db().await;

    let event_id = insert_event_with_payload(
        &pool,
        "TriggerCreated",
        json!({
            "trigger_id": "22222222-2222-2222-2222-222222222222",
            "name": "Simple event trigger",
            "on": "EmailReceived",
        }),
    )
    .await;

    sqlx::raw_sql(MIGRATE_TRIGGER_ON_TO_SUBSCRIPTION_LIST_SQL)
        .execute(&pool)
        .await
        .expect("run migration");

    assert_eq!(
        on_payload_of(&pool, event_id).await,
        json!([{ "event_type": "EmailReceived" }]),
        "no sibling condition → entry omits condition (skip_serializing_if = Option::is_none)"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn migrate_already_array_shape_untouched() {
    let (pool, db_name) = setup_test_db().await;

    let canonical = json!([
        { "event_type": "A", "condition": { "x": 1 } },
        { "event_type": "B" }
    ]);
    let event_id = insert_event_with_payload(
        &pool,
        "TriggerCreated",
        json!({
            "trigger_id": "33333333-3333-3333-3333-333333333333",
            "name": "Already migrated",
            "on": canonical.clone(),
        }),
    )
    .await;

    sqlx::raw_sql(MIGRATE_TRIGGER_ON_TO_SUBSCRIPTION_LIST_SQL)
        .execute(&pool)
        .await
        .expect("run migration");

    assert_eq!(on_payload_of(&pool, event_id).await, canonical);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn migrate_orphan_condition_update_folds_into_prior_single_subscription() {
    let (pool, db_name) = setup_test_db().await;

    let trigger_id = "44444444-4444-4444-4444-444444444444";

    // Prior config in the legacy single-string shape — step 1 of the
    // migration rewrites this into an array first.
    let created_id = insert_event_with_created_offset(
        &pool,
        "TriggerCreated",
        json!({
            "trigger_id": trigger_id,
            "name": "Sleep nudge",
            "on": "OuraSleepImported",
        }),
        60,
    )
    .await;

    let orphan_update_id = insert_event_with_created_offset(
        &pool,
        "TriggerUpdated",
        json!({
            "trigger_id": trigger_id,
            "condition": { "sleep_score": { "$lt": 70 } },
        }),
        30,
    )
    .await;

    sqlx::raw_sql(MIGRATE_TRIGGER_ON_TO_SUBSCRIPTION_LIST_SQL)
        .execute(&pool)
        .await
        .expect("run migration");

    assert_eq!(
        on_payload_of(&pool, created_id).await,
        json!([{ "event_type": "OuraSleepImported" }]),
        "TriggerCreated with bare `on` string becomes a one-entry array",
    );
    assert_eq!(
        on_payload_of(&pool, orphan_update_id).await,
        json!([{
            "event_type": "OuraSleepImported",
            "condition": { "sleep_score": { "$lt": 70 } }
        }]),
        "Orphan condition-only update folds into the prior subscription's entry",
    );
    assert!(
        !has_top_level_condition(&pool, orphan_update_id).await,
        "orphan condition key must be gone after migration"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn migrate_orphan_condition_update_dropped_when_no_single_prior() {
    // Prior config is multi-subscription, so the orphan condition can't be
    // folded into one entry; the legacy reader silently ignored it for this
    // shape too. Migration must drop the key without touching subscriptions.
    let (pool, db_name) = setup_test_db().await;

    let trigger_id = "55555555-5555-5555-5555-555555555555";

    insert_event_with_created_offset(
        &pool,
        "TriggerCreated",
        json!({
            "trigger_id": trigger_id,
            "name": "Multi-sub",
            "on": [
                { "event_type": "A", "condition": { "a": 1 } },
                { "event_type": "B", "condition": { "b": 2 } }
            ],
        }),
        60,
    )
    .await;

    let orphan_update_id = insert_event_with_created_offset(
        &pool,
        "TriggerUpdated",
        json!({
            "trigger_id": trigger_id,
            "condition": { "stale": true },
        }),
        30,
    )
    .await;

    sqlx::raw_sql(MIGRATE_TRIGGER_ON_TO_SUBSCRIPTION_LIST_SQL)
        .execute(&pool)
        .await
        .expect("run migration");

    assert_eq!(
        on_payload_of(&pool, orphan_update_id).await,
        json!(null),
        "orphan update without a foldable prior keeps its absent `on`",
    );
    assert!(
        !has_top_level_condition(&pool, orphan_update_id).await,
        "orphan condition key must be dropped even when not folded"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn migrate_on_to_subscription_list_is_idempotent() {
    let (pool, db_name) = setup_test_db().await;

    let event_id = insert_event_with_payload(
        &pool,
        "TriggerCreated",
        json!({
            "trigger_id": "66666666-6666-6666-6666-666666666666",
            "name": "Sleep nudge",
            "on": "OuraSleepImported",
            "condition": { "sleep_score": { "$lt": 70 } },
        }),
    )
    .await;

    for _ in 0..3 {
        sqlx::raw_sql(MIGRATE_TRIGGER_ON_TO_SUBSCRIPTION_LIST_SQL)
            .execute(&pool)
            .await
            .expect("run migration");
    }

    assert_eq!(
        on_payload_of(&pool, event_id).await,
        json!([{
            "event_type": "OuraSleepImported",
            "condition": { "sleep_score": { "$lt": 70 } }
        }]),
        "running the migration multiple times must converge to the same result",
    );

    teardown_test_db(&db_name).await;
}
