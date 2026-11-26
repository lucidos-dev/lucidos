use super::*;
use crate::engine::git_ops::{git_cmd, worktree_current_branch};
use std::path::PathBuf;

const BRANCH: &str = "claude-code/20260803-045152-ec0012";

async fn repo_with_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    tokio::fs::create_dir_all(&repo).await.unwrap();
    let _ = git_cmd(&["init"], &repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
    tokio::fs::write(repo.join("tracked.txt"), "initial")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "initial commit"], &repo).await;
    let wt = tmp.path().join("wt");
    let _ = git_cmd(
        &["worktree", "add", "-b", BRANCH, wt.to_str().unwrap()],
        &repo,
    )
    .await;
    (tmp, repo, wt)
}

#[test]
fn discard_removes_the_tree_it_can_name() {
    assert_eq!(
        discarded_worktree_removal(Some(BRANCH), BRANCH),
        WorktreeRemoval::Remove,
        "Discard is the one session end that reclaims, or the button does nothing"
    );
}

#[test]
fn discard_keeps_a_worktree_on_another_branch() {
    assert_eq!(
        discarded_worktree_removal(Some("claude-code/somebody-elses-work"), BRANCH),
        WorktreeRemoval::Keep("worktree is checked out on a different branch")
    );
}

#[test]
fn discard_keeps_a_worktree_whose_branch_cannot_be_read() {
    assert!(matches!(
        discarded_worktree_removal(None, BRANCH),
        WorktreeRemoval::Keep(_)
    ));
}

#[tokio::test]
async fn a_real_discarded_worktree_is_recognized_as_ours() {
    let (_tmp, _repo, wt) = repo_with_worktree().await;
    assert_eq!(
        discarded_worktree_removal(worktree_current_branch(&wt).await.as_deref(), BRANCH),
        WorktreeRemoval::Remove
    );
}

#[tokio::test]
async fn a_real_worktree_on_another_branch_is_kept() {
    let (_tmp, _repo, wt) = repo_with_worktree().await;
    assert!(matches!(
        discarded_worktree_removal(
            worktree_current_branch(&wt).await.as_deref(),
            "claude-code/a-branch-this-session-invented",
        ),
        WorktreeRemoval::Keep(_)
    ));
}

#[test]
fn the_completion_path_removes_only_the_two_worktrees_it_is_allowed_to() {
    const COMPLETION_SRC: &str = include_str!("../run_session/completion.rs");
    const REMOVE_CALL: &str = r#"&["worktree", "remove", "--force","#;

    let calls: Vec<usize> = COMPLETION_SRC
        .match_indices(REMOVE_CALL)
        .map(|m| m.0)
        .collect();
    assert_eq!(
        calls.len(),
        2,
        "completion.rs may remove exactly two worktrees, found {}. The allowed pair is (1) the \
         session's own tree on an explicit user Discard, and (2) the Tier-3 temp tree the merge \
         attempt itself created, gated by conflict_abort_deletes_temp_state. Anything else is a \
         session teardown reclaiming a worktree, which it must never do: reclamation has one \
         owner, the WorktreeCleanup worker (ADR 0035). A teardown runs precisely when something \
         went wrong, and it can be unwinding while the safety net relaunches a session into the \
         very tree it would delete.",
        calls.len()
    );

    let helper = COMPLETION_SRC
        .find("async fn remove_discarded_worktree(")
        .expect("the Discard removal helper must still be named remove_discarded_worktree");
    let impl_block = COMPLETION_SRC
        .find("impl LucidosEngine {")
        .expect("completion.rs must still carry its impl block");
    assert!(
        calls[0] > helper && calls[0] < impl_block,
        "the Discard removal must sit inside remove_discarded_worktree, above the impl block"
    );

    let temp_guard = COMPLETION_SRC
        .find("if conflict_abort_deletes_temp_state(")
        .expect("the merge temp-tree removal must stay behind conflict_abort_deletes_temp_state");
    assert!(
        calls[1] > temp_guard,
        "the second removal must be the guarded merge temp tree, not a new unguarded one"
    );
}
