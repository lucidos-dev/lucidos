use super::seed_branch_work;
use crate::engine::event_bus::EventBus;
use crate::engine::git_ops::git_cmd;
use crate::engine::thread_lifecycle::ChangeStateKind;
use crate::test_support::{
    make_repo_and_worktree, read_change_state_kind, setup_test_db, start_cc_session,
    teardown_test_db,
};
use uuid::Uuid;

#[tokio::test]
async fn session_bootstrap_seeds_unproposed_when_branch_has_commits() {
    let branch = "claude-code/seed-true";
    let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;

    std::fs::write(wt.join("a.txt"), "hello").unwrap();
    git_cmd(&["add", "a.txt"], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "feat: add a"], &wt)
        .await
        .unwrap();

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, branch, None).await;

    assert_eq!(
        read_change_state_kind(&pool, thread_id).await,
        ChangeStateKind::None,
        "precondition: the SessionStarted upsert leaves the column default"
    );

    seed_branch_work(&pool, thread_id, &repo_root, branch).await;

    assert_eq!(
        read_change_state_kind(&pool, thread_id).await,
        ChangeStateKind::Unproposed,
        "seed must read commits beyond main as unproposed work"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn session_bootstrap_leaves_change_state_none_when_branch_is_fresh() {
    let branch = "claude-code/seed-fresh";
    let (_tmp, repo_root, _wt) = make_repo_and_worktree(branch).await;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, branch, None).await;

    seed_branch_work(&pool, thread_id, &repo_root, branch).await;

    assert_eq!(
        read_change_state_kind(&pool, thread_id).await,
        ChangeStateKind::None,
        "seed must read a branch with no net diff against its diff base as none"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
