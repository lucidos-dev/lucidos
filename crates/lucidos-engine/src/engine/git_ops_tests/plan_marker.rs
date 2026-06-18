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
}

#[test]
fn plan_marker_kind_unknown_defaults_to_planned() {
    // Any stored row means a planning decision was made — a value drift must
    // not turn into a gate-blocking Missing, so unknown parses as Planned.
    assert_eq!(PlanMarkerKind::parse("???"), PlanMarkerKind::Planned);
    assert_eq!(PlanMarkerKind::parse(""), PlanMarkerKind::Planned);
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
    assert!(!is_plan_marker_present(&pool, &repo_path, "any-branch").await);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn plan_marker_present_after_record_planned() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo_path).await;
    tokio::fs::write(repo_path.join("a.txt"), "a").await.unwrap();
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
    assert!(is_plan_marker_present(&pool, &repo_path, "feature").await);
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
    tokio::fs::write(repo_path.join("a.txt"), "a").await.unwrap();
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

    tokio::fs::write(repo_path.join("b.txt"), "b").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "later work"], &repo_path).await;

    assert!(
        is_plan_marker_present(&pool, &repo_path, "feature").await,
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
    assert!(is_plan_marker_present(&pool, &repo_path, "feature").await);

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
