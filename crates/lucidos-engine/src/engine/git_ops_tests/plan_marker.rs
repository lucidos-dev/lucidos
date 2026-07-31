use super::common::make_test_repo;
use super::*;

#[test]
fn plan_marker_kind_round_trips_known_states() {
    assert_eq!(
        PlanMarkerKind::parse("planned").as_db(),
        PlanMarkerKind::Planned.as_db()
    );
    assert_eq!(
        PlanMarkerKind::parse("acknowledged_simple"),
        PlanMarkerKind::AcknowledgedSimple
    );
    assert_eq!(PlanMarkerKind::parse("proposed"), PlanMarkerKind::Proposed);
    assert_eq!(PlanMarkerKind::Proposed.as_db(), "proposed");
}

#[test]
fn plan_marker_kind_unknown_defaults_to_planned() {
    // An unknown/drifted value must not turn into a gate-blocking Missing, so it
    // parses as the settled (satisfying) Planned state. `proposed` is matched
    // explicitly, so a drift never masquerades as awaiting-approval.
    assert_eq!(PlanMarkerKind::parse("???"), PlanMarkerKind::Planned);
    assert_eq!(PlanMarkerKind::parse(""), PlanMarkerKind::Planned);
}

#[test]
fn only_planned_and_simple_satisfy_the_gate() {
    assert!(PlanMarkerKind::Planned.satisfies_gate());
    assert!(PlanMarkerKind::AcknowledgedSimple.satisfies_gate());
    assert!(
        !PlanMarkerKind::Proposed.satisfies_gate(),
        "a proposed plan awaits approval and must NOT satisfy the gate"
    );
    // State-level mirror.
    assert!(PlanMarkerState::Present(PlanMarkerKind::Planned).satisfies_gate());
    assert!(!PlanMarkerState::Present(PlanMarkerKind::Proposed).satisfies_gate());
    assert!(!PlanMarkerState::Missing.satisfies_gate());
}

#[tokio::test]
async fn plan_marker_missing_when_no_db_row() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "any-branch").await,
        PlanMarkerState::Missing,
        "No DB row should report Missing"
    );
    assert!(!plan_marker_state(&pool, &repo_path, "any-branch")
        .await
        .satisfies_gate());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn plan_marker_present_after_record_planned() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo_path).await;
    tokio::fs::write(repo_path.join("a.txt"), "a")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "feature work"], &repo_path).await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Planned,
        Some("docs/plans/2026-06-18-x.md"),
        None,
        &head_sha,
    )
    .await
    .unwrap();

    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::Planned),
    );
    assert!(plan_marker_state(&pool, &repo_path, "feature")
        .await
        .satisfies_gate());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn plan_marker_present_after_acknowledged_simple() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::AcknowledgedSimple,
        None,
        Some("one-line typo fix"),
        &head_sha,
    )
    .await
    .unwrap();

    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::AcknowledgedSimple),
        "an acknowledged-simple marker must satisfy the gate just like a full plan",
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Unlike the harden marker, the plan gate is binary present/absent — a new
/// commit after the planning decision must NOT invalidate it.
#[tokio::test]
async fn plan_marker_stays_present_after_new_commit() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo_path).await;
    tokio::fs::write(repo_path.join("a.txt"), "a")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "plan-time commit"], &repo_path).await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();
    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Planned,
        Some("docs/plans/x.md"),
        None,
        &head_sha,
    )
    .await
    .unwrap();

    tokio::fs::write(repo_path.join("b.txt"), "b")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "later work"], &repo_path).await;

    assert!(
        plan_marker_state(&pool, &repo_path, "feature")
            .await
            .satisfies_gate(),
        "a follow-up commit must not invalidate a settled plan decision",
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn plan_marker_consumed_returns_to_missing() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Planned,
        Some("docs/plans/x.md"),
        None,
        &head_sha,
    )
    .await
    .unwrap();
    assert!(plan_marker_state(&pool, &repo_path, "feature")
        .await
        .satisfies_gate());

    consume_plan_marker(&pool, &repo_path, "feature").await;
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Missing,
        "consume must delete the row so a later branch reuse starts fresh",
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The upsert path: re-marking the same branch flips the state (e.g. an
/// acknowledged-simple branch that later grows into a real plan).
#[tokio::test]
async fn plan_marker_record_upserts_state() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::AcknowledgedSimple,
        None,
        Some("started small"),
        &head_sha,
    )
    .await
    .unwrap();
    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Planned,
        Some("docs/plans/grew.md"),
        None,
        &head_sha,
    )
    .await
    .unwrap();

    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::Planned),
        "second mark must upsert, not duplicate or error on the PK",
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The skill records `proposed`, which is present-but-not-satisfying: the gate
/// stays closed until the human approves and the agent flips it to `planned`.
#[tokio::test]
async fn proposed_plan_does_not_satisfy_gate_until_approved() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Proposed,
        Some("docs/plans/2026-06-19-x.md"),
        None,
        &head_sha,
    )
    .await
    .unwrap();

    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::Proposed),
    );
    assert!(
        !plan_marker_state(&pool, &repo_path, "feature")
            .await
            .satisfies_gate(),
        "a proposed (unapproved) plan must NOT satisfy the gate",
    );

    // Approve flips proposed -> planned and now satisfies the gate.
    let approved = approve_plan(&pool, &repo_path, "feature").await.unwrap();
    assert!(approved, "approving a proposed plan must report a flip");
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::Planned),
    );
    assert!(plan_marker_state(&pool, &repo_path, "feature")
        .await
        .satisfies_gate());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Approve preserves `plan_path` and only ever touches a `proposed` row — it
/// never fabricates a marker on an unplanned branch nor clobbers a `--simple`
/// acknowledgment.
#[tokio::test]
async fn approve_plan_is_targeted_and_preserves_plan_path() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    // No row: approve is a no-op and must not fabricate a marker.
    assert!(!approve_plan(&pool, &repo_path, "feature").await.unwrap());
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Missing,
    );

    // acknowledged_simple: approve must not touch it.
    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::AcknowledgedSimple,
        None,
        Some("typo"),
        &head_sha,
    )
    .await
    .unwrap();
    assert!(
        !approve_plan(&pool, &repo_path, "feature").await.unwrap(),
        "approve must only flip a proposed row, never a simple ack",
    );
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::AcknowledgedSimple),
    );

    // proposed with a plan_path: approve flips and keeps the path.
    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Proposed,
        Some("docs/plans/keep-me.md"),
        None,
        &head_sha,
    )
    .await
    .unwrap();
    assert!(approve_plan(&pool, &repo_path, "feature").await.unwrap());
    let stored_path: Option<String> =
        sqlx::query_scalar("SELECT plan_path FROM planned_branches WHERE branch_name = $1")
            .bind("feature")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored_path.as_deref(),
        Some("docs/plans/keep-me.md"),
        "approve must preserve plan_path when flipping proposed -> planned",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
