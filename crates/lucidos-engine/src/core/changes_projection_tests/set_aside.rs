use super::cp_helpers::*;
use super::*;
use crate::core::changes::ChangeStatus;

fn set_aside_event(change_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChangeSetAside {
        change_id: change_id.to_string(),
    }
}

fn brought_back_event(change_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChangeBroughtBack {
        change_id: change_id.to_string(),
    }
}

async fn coding_agent_proposed(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT coding_agent_proposed FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn archive_state(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// S1 and S4: a set-aside change leaves every pending read and clears the
/// thread's proposal flag, which Review, attention and the archive gate read.
/// Bringing it back restores both.
#[tokio::test]
async fn set_aside_leaves_pending_and_bring_back_returns_it() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-s", "/r"),
    )
    .await;
    assert!(coding_agent_proposed(&pool, thread).await);

    emit(&bus, thread, set_aside_event(change_id)).await;
    let proj = ChangesProjection::new(pool.clone());
    let row = proj.get_by_id(change_id).await.unwrap().expect("row");
    assert_eq!(row.status(), ChangeStatus::SetAside);
    assert!(
        row.resolved_at.is_none(),
        "a set-aside change is open, not resolved"
    );
    assert!(!coding_agent_proposed(&pool, thread).await);
    assert!(proj.list_pending().await.unwrap().is_empty());
    assert!(proj.pending_for_thread(thread).await.unwrap().is_empty());
    assert!(proj
        .get_pending_by_branch("branch-s")
        .await
        .unwrap()
        .is_none());
    assert_eq!(proj.list_set_aside().await.unwrap().len(), 1);
    assert_eq!(proj.open_for_thread(thread).await.unwrap().len(), 1);
    assert_eq!(
        proj.get_open_by_branch("branch-s")
            .await
            .unwrap()
            .map(|c| c.id),
        Some(change_id)
    );

    emit(&bus, thread, brought_back_event(change_id)).await;
    let row = proj.get_by_id(change_id).await.unwrap().expect("row");
    assert_eq!(row.status(), ChangeStatus::Pending);
    assert!(row.resolved_at.is_none());
    assert!(coding_agent_proposed(&pool, thread).await);
    assert!(proj.list_set_aside().await.unwrap().is_empty());

    teardown_test_db(&db).await;
}

/// S3: the database refuses a second open row on one branch, whatever mix of
/// pending and set aside the two would be.
#[tokio::test]
async fn a_branch_holds_one_open_change() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-one", "/r"),
    )
    .await;
    emit(&bus, thread, set_aside_event(change_id)).await;

    let second = sqlx::query(
        "INSERT INTO changes (request_id, branch_name, repo_root, status) \
         VALUES ($1, 'branch-one', '/r', 'pending')",
    )
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await;
    assert!(
        second.is_err(),
        "a pending row beside a set-aside one on the same branch must not store"
    );

    teardown_test_db(&db).await;
}

/// O2 at the projection: work proposed straight into set-aside on an archived
/// thread stays out of the way. The thread stays archived and asks nothing.
#[tokio::test]
async fn a_proposal_made_set_aside_leaves_an_archived_thread_archived() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    emit(&bus, thread, ThreadEvent::ThreadArchived).await;
    assert_eq!(archive_state(&pool, thread).await, "archived");

    let mut proposed = aggregate_proposed(change_id, "branch-orphan", "/r");
    if let ThreadEvent::ChangeProposed {
        set_aside,
        incomplete,
        ..
    } = &mut proposed
    {
        *set_aside = true;
        *incomplete = true;
    }
    emit(&bus, thread, proposed).await;

    assert_eq!(archive_state(&pool, thread).await, "archived");
    assert!(!coding_agent_proposed(&pool, thread).await);
    let row = ChangesProjection::new(pool.clone())
        .get_by_id(change_id)
        .await
        .unwrap()
        .expect("row");
    assert_eq!(row.status(), ChangeStatus::SetAside);
    assert!(row.incomplete);

    teardown_test_db(&db).await;
}

/// S4: the rebuild replays both status events, and a proposal made straight
/// into set-aside, to the status the live projection wrote.
#[tokio::test]
async fn rebuild_restores_each_set_aside_shape() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let set_aside = Uuid::new_v4();
    let brought_back = Uuid::new_v4();
    let proposed_set_aside = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(&bus, thread, aggregate_proposed(set_aside, "b-1", "/r")).await;
    emit(&bus, thread, set_aside_event(set_aside)).await;
    emit(&bus, thread, aggregate_proposed(brought_back, "b-2", "/r")).await;
    emit(&bus, thread, set_aside_event(brought_back)).await;
    emit(&bus, thread, brought_back_event(brought_back)).await;
    let mut proposed = aggregate_proposed(proposed_set_aside, "b-3", "/r");
    if let ThreadEvent::ChangeProposed { set_aside, .. } = &mut proposed {
        *set_aside = true;
    }
    emit(&bus, thread, proposed).await;

    sqlx::query("DELETE FROM changes WHERE thread_id = $1")
        .bind(thread)
        .execute(&pool)
        .await
        .unwrap();
    let proj = ChangesProjection::new(pool.clone());
    assert_eq!(proj.rebuild_missing_from_events().await.unwrap(), 3);

    for (id, want) in [
        (set_aside, ChangeStatus::SetAside),
        (brought_back, ChangeStatus::Pending),
        (proposed_set_aside, ChangeStatus::SetAside),
    ] {
        let row = proj.get_by_id(id).await.unwrap().expect("rebuilt row");
        assert_eq!(row.status(), want, "change {id}");
        assert!(row.resolved_at.is_none(), "change {id} is still open");
    }

    teardown_test_db(&db).await;
}

/// Bringing a change back on an archived thread surfaces the thread, as a
/// proposal does, so an archived thread never holds a pending change.
#[tokio::test]
async fn bringing_a_change_back_surfaces_an_archived_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-back", "/r"),
    )
    .await;
    emit(&bus, thread, set_aside_event(change_id)).await;
    emit(&bus, thread, ThreadEvent::ThreadArchived).await;
    assert_eq!(archive_state(&pool, thread).await, "archived");

    emit(&bus, thread, brought_back_event(change_id)).await;
    assert_eq!(archive_state(&pool, thread).await, "inbox");
    assert!(coding_agent_proposed(&pool, thread).await);

    teardown_test_db(&db).await;
}
