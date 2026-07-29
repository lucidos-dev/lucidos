use super::common::make_test_repo;
use super::*;

#[tokio::test]
async fn root_commit_sha_is_the_initial_commit_and_stable() {
    let (_tmp, repo) = make_test_repo().await;
    let first = root_commit_sha(&repo).await.expect("root commit resolvable");
    assert_eq!(first.len(), 40, "full SHA-1 hex");

    // A later commit must NOT change the root-commit SHA — repo identity is
    // intrinsic to the FIRST commit, so it survives ongoing history.
    tokio::fs::write(repo.join("more.txt"), "more").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "second"], &repo).await;
    let after = root_commit_sha(&repo).await.expect("root commit still resolvable");
    assert_eq!(first, after, "root commit unaffected by later commits");
}

#[tokio::test]
async fn root_commit_sha_is_none_without_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let _ = git_cmd(&["init"], &repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
    // No commits yet → no root commit → None; callers fall back to a path-derived id.
    assert!(root_commit_sha(&repo).await.is_none());
}

/// Commit a file on a fresh branch cut from main, then return to main.
async fn commit_on_branch(repo: &std::path::Path, branch: &str, file: &str, body: &str) {
    let _ = git_cmd(&["checkout", "-b", branch], repo).await;
    tokio::fs::write(repo.join(file), body).await.unwrap();
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", &format!("add {file}")], repo).await;
    let _ = git_cmd(&["checkout", "main"], repo).await;
}

#[tokio::test]
async fn branch_changed_files_checked_lists_the_branch_diff() {
    let (_tmp, repo) = make_test_repo().await;
    commit_on_branch(&repo, "feature", "added.txt", "body").await;

    let files = branch_changed_files_checked(&repo, "feature")
        .await
        .expect("git can answer for an existing branch");
    assert_eq!(files, vec!["added.txt".to_string()]);
}

#[tokio::test]
async fn branch_changed_files_checked_reports_empty_for_a_commit_and_revert() {
    let (_tmp, repo) = make_test_repo().await;
    // The incident shape: a file is committed on the branch and a later commit
    // on the SAME branch deletes it, so the net diff against main is empty
    // while the branch still carries two commits.
    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("stray.txt"), "").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add stray"], &repo).await;
    tokio::fs::remove_file(repo.join("stray.txt")).await.unwrap();
    let _ = git_cmd(&["add", "-A"], &repo).await;
    let _ = git_cmd(&["commit", "-m", "drop stray"], &repo).await;
    let _ = git_cmd(&["checkout", "main"], &repo).await;

    let files = branch_changed_files_checked(&repo, "feature")
        .await
        .expect("git answers: the diff really is empty");
    assert!(
        files.is_empty(),
        "commit + revert nets out to no changed files, got {files:?}"
    );
}

#[tokio::test]
async fn branch_changed_files_checked_errors_on_a_missing_branch() {
    let (_tmp, repo) = make_test_repo().await;
    // "git could not answer" must NOT be reported as "the diff is empty" —
    // `reconcile_emptied_pending_change` zeroes a change's file list from this
    // answer, and doing that on a git failure would wipe a real file list.
    let err = branch_changed_files_checked(&repo, "no-such-branch")
        .await
        .expect_err("a missing ref is a git failure, not an empty diff");
    assert!(
        err.contains("no-such-branch"),
        "error should name the failing range, got: {err}"
    );
    // The forgiving wrapper still degrades to empty for the "is there anything
    // to propose?" callers.
    assert!(branch_changed_files(&repo, "no-such-branch").await.is_empty());
}
