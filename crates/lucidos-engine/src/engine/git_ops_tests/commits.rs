use super::common::make_test_repo;
use super::*;

#[tokio::test]
async fn root_commit_sha_is_the_initial_commit_and_stable() {
    let (_tmp, repo) = make_test_repo().await;
    let first = root_commit_sha(&repo)
        .await
        .expect("root commit resolvable");
    assert_eq!(first.len(), 40, "full SHA-1 hex");

    // A later commit must NOT change the root-commit SHA — repo identity is
    // intrinsic to the FIRST commit, so it survives ongoing history.
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
    assert!(branch_changed_files(&repo, "no-such-branch")
        .await
        .is_empty());
}

// -------------------- local_branch_exists / sole_branch_containing --------------------

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
    // The reported shape: the branch was renamed in place, so the recorded
    // name is gone but the thread's commit is still on the new one.
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
    // Once the work is merged, the base contains the commit too. The base is
    // never the answer to "where is this thread's branch".
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
    // Somebody branched off the work: which one the thread meant is ambiguous.
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
    // `default_diff_base` resolves to `origin/<default>` when the local default
    // has diverged, and for-each-ref lists local refs only, so excluding the
    // base alone would leave local `main` eligible for merged-and-deleted work.
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

/// The dirty list is what the bounded security-fix bound is checked against,
/// alongside the committed diff, because every apply path `git add -A`s the
/// worktree before merging.
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

    // Untracked, modified and staged all get swept up by `git add -A`, so all
    // three have to show. A quoted path proves the -z parse needs no unquoting.
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

/// A rename emits its ORIGIN as a second `-z` record carrying NO status
/// prefix. Stripping three bytes off that record mangles the path, and on a
/// multi-byte boundary it panics.
///
/// Both halves land on main, so both belong in the answer. Getting the origin
/// wrong is not a cosmetic slip: an unattended bounded fix would be refused
/// against a path that does not exist, with nobody there to re-mark it.
#[tokio::test]
async fn worktree_dirty_files_reads_a_rename_origin_whole() {
    let (_tmp, repo) = make_test_repo().await;
    tokio::fs::create_dir_all(repo.join("src")).await.unwrap();
    tokio::fs::write(repo.join("src/alpha.rs"), "a")
        .await
        .unwrap();
    // A leading multi-byte char, so a three-byte strip would land mid-char.
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
