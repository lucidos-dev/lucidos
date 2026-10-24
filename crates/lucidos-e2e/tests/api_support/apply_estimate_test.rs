//! The *apply estimate* over HTTP: what the apply toasts read from
//! `GET /api/v1/changes`. The estimator and the probe are unit-tested in the
//! engine; this covers the payload a reloaded page rehydrates from.

use crate::support::{base_url, db_url, seed_cc_thread_summary, user_client};
use serde_json::json;
use uuid::Uuid;

async fn seed_resolving_change(pool: &sqlx::PgPool, change_id: Uuid, thread_id: Uuid) {
    sqlx::query(
        "INSERT INTO changes \
           (id, request_id, thread_id, branch_name, repo_root, description, \
            file_count, files, requires_restart, status) \
         VALUES ($1, $2, $3, $4, '/no-such-repo', 'e2e apply estimate', 1, \
                 ARRAY['a.rs'], false, 'pending')",
    )
    .bind(change_id)
    .bind(Uuid::new_v4())
    .bind(thread_id)
    .bind(format!("e2e-test/estimate-{}", change_id.as_simple()))
    .execute(pool)
    .await
    .expect("seed change");
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, thread_id, aggregate, aggregate_id) \
         VALUES ($1, 'MergeConflictDetected', $2, now() - make_interval(secs => 300), $3, 'thread', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(json!({ "change_id": change_id.to_string() }))
    .bind(thread_id)
    .bind(thread_id.to_string())
    .execute(pool)
    .await
    .expect("seed the conflict event");
}

/// A reload must keep the elapsed time, so the row says when its conflict
/// resolution began. A repo git cannot read predicts `unknown`, never clean.
/// The response always carries both estimate slots.
#[tokio::test]
async fn a_pending_change_carries_its_phase_start_and_conflict_prediction() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let (thread_id, change_id) = (Uuid::new_v4(), Uuid::new_v4());
    seed_cc_thread_summary(&pool, thread_id, "running").await;
    seed_resolving_change(&pool, change_id, thread_id).await;

    let body: serde_json::Value = client
        .get(format!("{}/api/v1/changes", base_url()))
        .send()
        .await
        .expect("changes request failed")
        .json()
        .await
        .expect("changes body");

    let row = body["pending"]
        .as_array()
        .expect("pending must be an array")
        .iter()
        .find(|c| c["id"] == change_id.to_string())
        .expect("the seeded change must be pending");
    assert_eq!(row["resolving_conflict"], true);
    assert!(
        row["apply_phase_started_at"].is_string(),
        "the row must say when the conflict resolution began: {row}"
    );
    assert_eq!(row["predicted_conflict"], "unknown");

    let estimates = &body["apply_estimates"];
    assert!(
        estimates.get("hardening").is_some() && estimates.get("resolving_conflict").is_some(),
        "both estimate slots must be present, null or not: {estimates}"
    );
}
