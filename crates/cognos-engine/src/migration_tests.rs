//! Tests for one-off migrations whose correctness depends on data shape
//! and cannot be verified by `cargo check` alone.

use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const FIX_INVOCATION_SQL: &str =
    include_str!("../migrations/20260420195145_fix_trigger_invocation_event_only_backfill.sql");

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
