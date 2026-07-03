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
