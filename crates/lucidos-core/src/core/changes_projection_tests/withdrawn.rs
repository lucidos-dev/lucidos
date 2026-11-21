//! A withdrawn change hands its work back to the branch undecided (ADR 0400).

use super::cp_helpers::*;
use super::*;
use crate::core::changes::ChangeStatus;
use crate::engine::thread_lifecycle::CodingAgentChangeState;
use crate::test_support::read_change_state;

fn withdrawn_event(change_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChangeWithdrawn {
        change_id: change_id.to_string(),
    }
}

async fn archive_state(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// I5: a withdrawn change leaves every open read and the open-branch index,
/// and the thread lands on unproposed work with no reason. The archive net
/// reads the branch as undecided, so it still sets the work aside.
#[tokio::test]
async fn a_withdrawn_change_leaves_the_open_reads_and_hands_back_its_branch() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-w", "/r"),
    )
    .await;

    emit(&bus, thread, withdrawn_event(change_id)).await;

    let proj = ChangesProjection::new(pool.clone());
    let row = proj.get_by_id(change_id).await.unwrap().expect("row");
    assert_eq!(row.status(), ChangeStatus::Withdrawn);
    assert!(proj.pending_for_thread(thread).await.unwrap().is_empty());
    assert!(proj.open_for_thread(thread).await.unwrap().is_empty());
    assert!(proj.get_open_by_branch("branch-w").await.unwrap().is_none());
    assert!(
        !proj.branch_has_decided_change("branch-w").await.unwrap(),
        "nobody decided a withdrawn change, so the archive net may set it aside"
    );
    assert_eq!(
        read_change_state(&pool, thread).await,
        CodingAgentChangeState::Unproposed { reason: None }
    );

    let next = sqlx::query(
        "INSERT INTO changes (request_id, branch_name, repo_root, status) \
         VALUES ($1, 'branch-w', '/r', 'pending')",
    )
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await;
    assert!(
        next.is_ok(),
        "a withdrawn row must not hold the branch's one open slot: {next:?}"
    );

    teardown_test_db(&db).await;
}

/// Withdrawing a set-aside change surfaces its archived thread: the work is
/// back on the branch and somebody has to decide it.
#[tokio::test]
async fn withdrawing_a_set_aside_change_surfaces_an_archived_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-sa", "/r"),
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::ChangeSetAside {
            change_id: change_id.to_string(),
        },
    )
    .await;
    emit(&bus, thread, ThreadEvent::ThreadArchived).await;
    assert_eq!(archive_state(&pool, thread).await, "archived");

    emit(&bus, thread, withdrawn_event(change_id)).await;

    assert_eq!(archive_state(&pool, thread).await, "inbox");
    let proj = ChangesProjection::new(pool.clone());
    assert!(proj.list_set_aside().await.unwrap().is_empty());
    assert_eq!(
        proj.get_by_id(change_id)
            .await
            .unwrap()
            .expect("row")
            .status(),
        ChangeStatus::Withdrawn
    );

    teardown_test_db(&db).await;
}

/// I4 on the rebuild path: a lost withdrawn row comes back withdrawn. Rebuilt
/// as pending, it would offer an Apply on work a turn never finished.
#[tokio::test]
async fn rebuild_restores_a_withdrawn_change_as_withdrawn() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-rw", "/r"),
    )
    .await;
    emit(&bus, thread, withdrawn_event(change_id)).await;

    sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await
        .unwrap();
    let proj = ChangesProjection::new(pool.clone());
    assert_eq!(proj.rebuild_missing_from_events().await.unwrap(), 1);

    let row = proj
        .get_by_id(change_id)
        .await
        .unwrap()
        .expect("rebuilt row");
    assert_eq!(row.status(), ChangeStatus::Withdrawn);
    assert!(row.resolved_at.is_some(), "a withdrawn change is closed");
    assert_eq!(
        read_change_state(&pool, thread).await.kind(),
        crate::engine::thread_lifecycle::ChangeStateKind::Unproposed
    );

    teardown_test_db(&db).await;
}
