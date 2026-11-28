use super::common::make_test_repo;
use super::*;

#[tokio::test]
async fn root_commit_sha_is_the_initial_commit_and_stable() {
    let (_tmp, repo) = make_test_repo().await;
    let first = root_commit_sha(&repo)
        .await
        .expect("root commit resolvable");
    assert_eq!(first.len(), 40, "full SHA-1 hex");

    tokio::fs::write(repo.join("more.txt"), "more")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "second"], &repo).await;
    let after = root_commit_sha(&repo)
        .await
        .expect("root commit still resolvable");
    assert_eq!(first, after, "root commit unaffected by later commits");
}

#[tokio::test]
async fn root_commit_sha_is_none_without_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let _ = git_cmd(&["init"], &repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
    assert!(root_commit_sha(&repo).await.is_none());
}

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
    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("stray.txt"), "").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add stray"], &repo).await;
    tokio::fs::remove_file(repo.join("stray.txt"))
        .await
        .unwrap();
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
    let err = branch_changed_files_checked(&repo, "no-such-branch")
        .await
        .expect_err("a missing ref is a git failure, not an empty diff");
    assert!(
        err.contains("no-such-branch"),
        "error should name the failing range, got: {err}"
    );
    assert!(branch_changed_files(&repo, "no-such-branch")
        .await
        .is_empty());
}

#[tokio::test]
async fn local_branch_exists_answers_yes_and_no() {
    let (_tmp, repo) = make_test_repo().await;
    commit_on_branch(&repo, "feature", "added.txt", "body").await;

    assert!(local_branch_exists(&repo, "feature")
        .await
        .or_unknown(false));
    assert!(!local_branch_exists(&repo, "no-such-branch")
        .await
        .or_unknown(true));
}

#[tokio::test]
async fn sole_branch_containing_finds_the_renamed_branch() {
    let (_tmp, repo) = make_test_repo().await;
    commit_on_branch(&repo, "tracked", "added.txt", "body").await;
    let work = {
        let out = git_cmd(&["rev-parse", "tracked"], &repo).await.unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let _ = git_cmd(&["branch", "-m", "tracked", "renamed"], &repo).await;

    assert_eq!(
        sole_branch_containing(&repo, &work, &["main"])
            .await
            .as_deref(),
        Some("renamed")
    );
}

#[tokio::test]
async fn sole_branch_containing_ignores_the_base_branch() {
    let (_tmp, repo) = make_test_repo().await;
    commit_on_branch(&repo, "feature", "added.txt", "body").await;
    let work = {
        let out = git_cmd(&["rev-parse", "feature"], &repo).await.unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let _ = git_cmd(&["merge", "--no-ff", "--no-edit", "feature"], &repo).await;

    assert_eq!(
        sole_branch_containing(&repo, &work, &["main"])
            .await
            .as_deref(),
        Some("feature"),
        "main contains the commit after the merge, but it is the diff base"
    );
}

#[tokio::test]
async fn sole_branch_containing_refuses_when_several_branches_qualify() {
    let (_tmp, repo) = make_test_repo().await;
    commit_on_branch(&repo, "feature", "added.txt", "body").await;
    let work = {
        let out = git_cmd(&["rev-parse", "feature"], &repo).await.unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let _ = git_cmd(&["branch", "spin-off", "feature"], &repo).await;

    assert!(
        sole_branch_containing(&repo, &work, &["main"])
            .await
            .is_none(),
        "two candidates must not be guessed between"
    );
}

#[tokio::test]
async fn sole_branch_containing_is_none_when_the_work_was_deleted() {
    let (_tmp, repo) = make_test_repo().await;
    commit_on_branch(&repo, "feature", "added.txt", "body").await;
    let work = {
        let out = git_cmd(&["rev-parse", "feature"], &repo).await.unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let _ = git_cmd(&["branch", "-D", "feature"], &repo).await;

    assert!(sole_branch_containing(&repo, &work, &["main"])
        .await
        .is_none());
}

#[tokio::test]
async fn sole_branch_containing_ignores_the_local_default_too() {
    let (_tmp, repo) = make_test_repo().await;
    commit_on_branch(&repo, "feature", "added.txt", "body").await;
    let work = {
        let out = git_cmd(&["rev-parse", "feature"], &repo).await.unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let _ = git_cmd(&["merge", "--no-ff", "--no-edit", "feature"], &repo).await;
    let _ = git_cmd(&["branch", "-D", "feature"], &repo).await;

    assert!(
        sole_branch_containing(&repo, &work, &["origin/main", "main"])
            .await
            .is_none(),
        "merged and deleted work must not resolve to the default branch"
    );
}

#[tokio::test]
async fn worktree_dirty_files_reports_every_shape_that_would_be_committed() {
    let (_tmp, repo) = make_test_repo().await;
    assert!(
        worktree_dirty_files(&repo)
            .await
            .expect("clean tree answers")
            .is_empty(),
        "a clean tree has nothing that would land",
    );

    tokio::fs::write(repo.join("untracked.rs"), "u")
        .await
        .unwrap();
    tokio::fs::write(repo.join("with space.rs"), "s")
        .await
        .unwrap();
    tokio::fs::write(repo.join("init.txt"), "changed")
        .await
        .unwrap();
    tokio::fs::write(repo.join("staged.rs"), "st")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "staged.rs"], &repo).await;

    let mut dirty = worktree_dirty_files(&repo).await.expect("dirty tree");
    dirty.sort();
    assert_eq!(
        dirty,
        vec![
            "init.txt".to_string(),
            "staged.rs".to_string(),
            "untracked.rs".to_string(),
            "with space.rs".to_string(),
        ],
    );
}

#[tokio::test]
async fn worktree_dirty_files_reads_a_rename_origin_whole() {
    let (_tmp, repo) = make_test_repo().await;
    tokio::fs::create_dir_all(repo.join("src")).await.unwrap();
    tokio::fs::write(repo.join("src/alpha.rs"), "a")
        .await
        .unwrap();
    tokio::fs::write(repo.join("ünïcödé.rs"), "u")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "-A"], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add the files to rename"], &repo).await;

    let _ = git_cmd(&["mv", "src/alpha.rs", "src/beta.rs"], &repo).await;
    let _ = git_cmd(&["mv", "ünïcödé.rs", "renamed.rs"], &repo).await;

    let mut dirty = worktree_dirty_files(&repo).await.expect("rename listed");
    dirty.sort();
    assert_eq!(
        dirty,
        vec![
            "renamed.rs".to_string(),
            "src/alpha.rs".to_string(),
            "src/beta.rs".to_string(),
            "ünïcödé.rs".to_string(),
        ],
        "both halves of each rename, with the origin path intact",
    );
}

#[tokio::test]
async fn landing_content_uses_the_landed_trees_not_the_proposal_files() {
    use crate::engine::thread_events::ChangeContent;
    let (_tmp, repo) = make_test_repo().await;
    tokio::fs::create_dir_all(repo.join("docs/plans"))
        .await
        .unwrap();
    commit_on_branch(&repo, "plan", "docs/plans/example.md", "plan").await;
    let files = changed_files_in_range(&repo, "main..plan").await.unwrap();
    assert_eq!(ChangeContent::from_files(&files), ChangeContent::PlanOnly);
    git_cmd(&["checkout", "plan"], &repo).await.unwrap();
    tokio::fs::write(repo.join("code.rs"), "fn example() {}")
        .await
        .unwrap();
    git_cmd(&["add", "."], &repo).await.unwrap();
    git_cmd(&["commit", "-m", "add code"], &repo).await.unwrap();
    let files = changed_files_in_range(&repo, "main..plan").await.unwrap();
    assert_eq!(ChangeContent::from_files(&files), ChangeContent::Code);
    assert!(changed_files_in_range(&repo, "missing..plan")
        .await
        .is_err());
    let files = changed_files_in_range(&repo, "plan..plan").await.unwrap();
    assert_eq!(ChangeContent::from_files(&files), ChangeContent::Code);
}
