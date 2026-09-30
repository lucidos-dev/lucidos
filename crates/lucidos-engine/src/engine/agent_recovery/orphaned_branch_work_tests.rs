use super::*;
use crate::core::changes::ChangeStatus;
use crate::engine::event_bus::EmittedEvent;
use crate::test_support::{
    make_repo_and_worktree, setup_test_db, start_cc_session, teardown_test_db,
};
use tokio::sync::broadcast;

/// How many `ChangeProposed` the trigger matcher was handed since the last
/// call: each one fires every trigger subscribed to `ChangeProposed`.
fn change_proposed_dispatches(rx: &mut broadcast::Receiver<EmittedEvent>) -> usize {
    let mut count = 0;
    while let Ok(emitted) = rx.try_recv() {
        if crate::scheduler::trigger_dispatch(&emitted)
            .is_some_and(|d| d.event_type == "ChangeProposed")
        {
            count += 1;
        }
    }
    count
}

/// The upgrade burst: the boot pass records an archived thread's unproposed
/// branch work once. Every later pass, and every later archive, finds the
/// change row and stays silent, so a trigger never hears a re-sync.
#[tokio::test]
async fn a_second_pass_never_re_announces_set_aside_branch_work() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = "claude-code/archived-branch-work";
    let (_tmp, repo, wt) = make_repo_and_worktree(branch).await;
    std::fs::write(wt.join("a.txt"), "archived work").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "archived work"], &wt)
        .await
        .unwrap();

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, branch, None).await;
    sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .expect("archive the thread");

    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };
    let mut rx = bus.subscribe();

    assert_eq!(scope.set_aside_archived_branch_work_on_startup().await, 1);
    assert_eq!(
        change_proposed_dispatches(&mut rx),
        1,
        "the first pass records the work once"
    );

    assert_eq!(scope.set_aside_archived_branch_work_on_startup().await, 0);
    assert!(!scope
        .set_aside_archived_branch_work(thread_id)
        .await
        .expect("archive half runs"));
    assert_eq!(
        change_proposed_dispatches(&mut rx),
        0,
        "a second boot pass or archive gives a trigger nothing"
    );

    let persisted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'ChangeProposed'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted, 1, "one ChangeProposed in the timeline");
    let change = bus
        .changes_projection()
        .get_open_by_branch(branch)
        .await
        .unwrap()
        .expect("the Changes panel has the set-aside change");
    assert_eq!(change.status(), ChangeStatus::SetAside);
    assert_eq!(change.files, vec!["a.txt".to_string()]);

    pool.close().await;
    teardown_test_db(&db).await;
}

fn change_proposed(set_aside: bool) -> ThreadEvent {
    ThreadEvent::ChangeProposed {
        change_id: Uuid::new_v4().to_string(),
        description: Some("work".into()),
        requires_restart: false,
        files: vec!["a.txt".into()],
        origin: None,
        commit_sha: None,
        branch_name: "claude-code/any".into(),
        repo_root: "/repo".into(),
        hardened: false,
        incomplete: set_aside,
        set_aside,
        path: String::new(),
        diff: String::new(),
    }
}

/// The filter `triggers.md` gives a "changes to apply" trigger. A pending
/// proposal omits `set_aside`, so a bare `false` would match nothing.
#[test]
fn the_documented_changes_to_apply_filter_skips_only_set_aside_work() {
    use crate::core::event_subscription::{condition, matchable_thread_payload};
    let filter = serde_json::json!({ "set_aside": { "$ne": true } });
    let thread_id = Uuid::new_v4();
    let pending = matchable_thread_payload(&change_proposed(false), thread_id);
    let parked = matchable_thread_payload(&change_proposed(true), thread_id);
    assert!(condition::evaluate(Some(&filter), &pending));
    assert!(!condition::evaluate(Some(&filter), &parked));
    assert!(
        !condition::evaluate(Some(&serde_json::json!({ "set_aside": false })), &pending),
        "a bare false misses the pending proposal, which is why the doc says $ne"
    );
}
