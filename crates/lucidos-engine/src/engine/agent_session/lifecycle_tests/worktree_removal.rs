//! Worktree removal at session end ([`super::discarded_worktree_removal`]),
//! and the invariant that nothing else on that path removes anything at all.
//!
//! Regression background, 2026-08-03. The session-end removal was
//! unconditional. With the host saturated by an e2e run, every engine git call
//! was blowing the 30s ceiling; the spawn path read those timeouts as "the
//! branch is gone, the worktree is stranded", started over, and this path then
//! ran `git worktree remove --force` over a live worktree holding the user's
//! work. Hours later the same line fired again from the other direction: a
//! session died mid-turn, the safety net relaunched Claude Code into the
//! worktree, and the dead session's teardown deleted the tree out from under
//! the process that had just started in it.
//!
//! A positive-evidence gate fixed both routes. This file covers what replaced
//! it: the call site is gone. Reclamation has exactly one owner, the background
//! `WorktreeCleanup` worker, and the only removal left on the session path is
//! an explicit user Discard. See ADR 0035.

use super::*;
use crate::engine::git_ops::{git_cmd, worktree_current_branch};
use std::path::PathBuf;

const BRANCH: &str = "claude-code/20260803-045152-ec0012";

/// A real repo on `main` with a linked worktree checked out on [`BRANCH`],
/// the way a coding-agent session finds things.
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

/// The branch churn from the incident: the spawn path generated a fresh branch
/// after misreading a timeout, so the tree on disk was no longer the one this
/// session believed it owned. Claude Code can also `git checkout` inside its own
/// worktree. Either way we only delete the tree we can name.
#[test]
fn discard_keeps_a_worktree_on_another_branch() {
    assert_eq!(
        discarded_worktree_removal(Some("claude-code/somebody-elses-work"), BRANCH),
        WorktreeRemoval::Keep("worktree is checked out on a different branch")
    );
}

/// `worktree_current_branch` returns `None` for a detached HEAD and for a git
/// call that could not run. Neither is a positive match, so neither authorizes
/// the delete: unknown never authorizes destruction.
#[test]
fn discard_keeps_a_worktree_whose_branch_cannot_be_read() {
    assert!(matches!(
        discarded_worktree_removal(None, BRANCH),
        WorktreeRemoval::Keep(_)
    ));
}

/// End to end against real git. The unit cases pin the decision; this pins that
/// the probe feeding it reads a real tree correctly, which is the half a pure
/// test cannot cover.
#[tokio::test]
async fn a_real_discarded_worktree_is_recognized_as_ours() {
    let (_tmp, _repo, wt) = repo_with_worktree().await;
    assert_eq!(
        discarded_worktree_removal(worktree_current_branch(&wt).await.as_deref(), BRANCH),
        WorktreeRemoval::Remove
    );
}

/// Also end to end: a tree on a different branch than the session believes is
/// never removed, the shape the 2026-08-03 branch churn produced.
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

/// **The regression test for the incident.** Both wipes came from a session end
/// that was NOT a Discard: one aborted after a stray SIGKILL, one acting on a
/// timed-out probe. Neither had any business deleting a working tree, and the
/// fix is structural rather than conditional: the completion path holds exactly
/// one worktree removal, inside the Discard helper.
///
/// Asserted against the source text because that is where the property lives. A
/// behavioural test can only show that the paths reachable *today* keep the
/// tree; it cannot stop the next reader from adding a removal back onto the
/// abort path, which is precisely what happened here twice in one morning.
///
/// Exactly two removals are legitimate in this file, and both are named below.
/// A third is the regression: someone gave the session teardown a reason to
/// delete a working tree again.
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
