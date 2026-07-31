use super::*;
use crate::engine::git_ops::git_cmd;

/// Build a fresh temp git repo with one commit on `main`. Returns the
/// tempdir guard (dropped → cleanup) and the repo path.
async fn make_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let _ = git_cmd(&["init"], &repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
    let _ = git_cmd(&["config", "user.email", "test@test.test"], &repo).await;
    let _ = git_cmd(&["config", "user.name", "test"], &repo).await;
    tokio::fs::write(repo.join("a.txt"), "first").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "initial"], &repo).await;
    (tmp, repo)
}

async fn current_head(repo: &std::path::Path) -> String {
    let out = git_cmd(&["rev-parse", "HEAD"], repo).await.unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// -------------------- git_head_sha --------------------

#[tokio::test]
async fn git_head_sha_returns_full_sha_for_real_repo() {
    let (_tmp, repo) = make_repo().await;
    let sha = git_head_sha(&repo).await.expect("HEAD should be readable");
    assert_eq!(sha.len(), 40, "expected 40-char SHA, got: {}", sha);
}

#[tokio::test]
async fn git_head_sha_returns_none_for_missing_dir() {
    let missing = std::path::PathBuf::from("/nonexistent/path/to/wt");
    assert!(git_head_sha(&missing).await.is_none());
}

#[tokio::test]
async fn git_head_sha_returns_none_for_non_git_dir() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(git_head_sha(tmp.path()).await.is_none());
}

// -------------------- compute_external_edit_note --------------------

#[tokio::test]
async fn compute_returns_none_when_last_sha_is_none() {
    // Truly-first turn: no prior idle to compare against → no note.
    let (_tmp, repo) = make_repo().await;
    assert!(compute_external_edit_note(&repo, None).await.is_none());
}

#[tokio::test]
async fn compute_returns_none_when_worktree_unchanged() {
    let (_tmp, repo) = make_repo().await;
    let head = current_head(&repo).await;
    assert!(
        compute_external_edit_note(&repo, Some(&head))
            .await
            .is_none(),
        "no edits since last idle → no note"
    );
}

#[tokio::test]
async fn compute_returns_none_when_worktree_path_missing() {
    let missing = std::path::PathBuf::from("/nonexistent/wt");
    assert!(compute_external_edit_note(&missing, Some("abcd"))
        .await
        .is_none());
}

#[tokio::test]
async fn compute_detects_uncommitted_changes() {
    let (_tmp, repo) = make_repo().await;
    let head = current_head(&repo).await;

    // User edits a file but doesn't commit.
    tokio::fs::write(repo.join("a.txt"), "user edit")
        .await
        .unwrap();

    let note = compute_external_edit_note(&repo, Some(&head))
        .await
        .expect("dirty worktree should produce a note");
    assert!(note.contains("Uncommitted changes:"), "note: {}", note);
    assert!(
        note.contains("a.txt"),
        "note should list the file: {}",
        note
    );
    assert!(
        !note.contains("Committed changes"),
        "no commits to report: {}",
        note
    );
    assert!(note.starts_with("[Note from engine"));
    assert!(note.ends_with(']'));
}

#[tokio::test]
async fn compute_detects_new_commits_since_last_idle() {
    let (_tmp, repo) = make_repo().await;
    let last_head = current_head(&repo).await;

    // User commits something between turns.
    tokio::fs::write(repo.join("user_change.txt"), "user work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "user added a thing"], &repo).await;

    let note = compute_external_edit_note(&repo, Some(&last_head))
        .await
        .expect("HEAD moved → note");
    assert!(
        note.contains("Committed changes since your last action:"),
        "note: {}",
        note
    );
    assert!(
        note.contains("user added a thing"),
        "log subject must appear: {}",
        note
    );
    assert!(
        !note.contains("Uncommitted changes"),
        "tree is clean: {}",
        note
    );
}

#[tokio::test]
async fn compute_detects_both_commits_and_uncommitted() {
    let (_tmp, repo) = make_repo().await;
    let last_head = current_head(&repo).await;

    // Commit one change …
    tokio::fs::write(repo.join("committed.txt"), "yes")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "commit between turns"], &repo).await;

    // … and leave another uncommitted.
    tokio::fs::write(repo.join("dirty.txt"), "wip")
        .await
        .unwrap();

    let note = compute_external_edit_note(&repo, Some(&last_head))
        .await
        .expect("two changes → note");
    assert!(note.contains("Committed changes since your last action:"));
    assert!(note.contains("commit between turns"));
    assert!(note.contains("Uncommitted changes:"));
    assert!(note.contains("dirty.txt"));
}

#[tokio::test]
async fn compute_truncates_huge_dirty_lists() {
    let (_tmp, repo) = make_repo().await;
    let last_head = current_head(&repo).await;
    // Create 60 untracked files; helper caps at 50.
    for i in 0..60 {
        tokio::fs::write(repo.join(format!("f{:02}.txt", i)), "x")
            .await
            .unwrap();
    }
    let note = compute_external_edit_note(&repo, Some(&last_head))
        .await
        .expect("many dirty → note");
    assert!(note.contains("… and 10 more file(s)"), "note: {}", note);
}

// -------------------- verify_branch --------------------

#[tokio::test]
async fn verify_branch_ok_when_matching() {
    let (_tmp, repo) = make_repo().await;
    assert!(verify_branch(&repo, "main").await.is_ok());
}

#[tokio::test]
async fn verify_branch_errors_when_user_checked_out_different_branch() {
    let (_tmp, repo) = make_repo().await;
    let _ = git_cmd(&["checkout", "-b", "user-feature"], &repo).await;

    let err = verify_branch(&repo, "main")
        .await
        .expect_err("branch mismatch should produce error");
    assert_eq!(err.expected, "main");
    assert_eq!(err.found.as_deref(), Some("user-feature"));
    let msg = format!("{}", err);
    assert!(msg.contains("user-feature"));
    assert!(msg.contains("main"));
    assert!(msg.contains("Resolve manually"));
}

#[tokio::test]
async fn verify_branch_errors_on_detached_head() {
    let (_tmp, repo) = make_repo().await;
    // Detach HEAD by checking out the SHA directly.
    let head = current_head(&repo).await;
    let out = git_cmd(&["checkout", "--detach", &head], &repo)
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "detach failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let err = verify_branch(&repo, "main")
        .await
        .expect_err("detached HEAD should not match");
    assert_eq!(err.expected, "main");
    assert_eq!(err.found, None);
    let msg = format!("{}", err);
    assert!(msg.contains("detached HEAD"), "msg: {}", msg);
}

#[tokio::test]
async fn verify_branch_ok_when_worktree_missing() {
    let missing = std::path::PathBuf::from("/nonexistent/wt-for-verify");
    // Nothing to verify → no error (let downstream handle the missing dir).
    assert!(verify_branch(&missing, "main").await.is_ok());
}

#[tokio::test]
async fn adopt_returns_none_when_no_last_sha() {
    let (_tmp, repo) = make_repo().await;
    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    assert!(
        try_adopt_renegade_branch(&repo, None).await.is_none(),
        "without a last-known SHA there's no ancestry check to make"
    );
}

#[tokio::test]
async fn adopt_returns_some_when_branch_descends_from_last_sha() {
    let (_tmp, repo) = make_repo().await;
    let initial = current_head(&repo).await;
    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("b.txt"), "x").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "feature commit"], &repo).await;

    let (new_branch, note) = try_adopt_renegade_branch(&repo, Some(&initial))
        .await
        .expect("feature contains the initial commit → safe to adopt");
    assert_eq!(new_branch, "feature");
    // Note mentions the new branch in two places (first to introduce the
    // rename, then to confirm the new tracked branch). Lock both in so a
    // regression that drops one substitution would be caught.
    assert_eq!(
        note.matches("'feature'").count(),
        2,
        "note must name the branch at both substitution sites: {}",
        note
    );
    assert!(note.starts_with("[Note from engine"));
    assert!(note.ends_with(']'));
}

#[tokio::test]
async fn adopt_returns_none_when_branch_does_not_descend() {
    let (_tmp, repo) = make_repo().await;
    let initial = current_head(&repo).await;
    // Move main forward; renegade branches off the OLD initial commit.
    tokio::fs::write(repo.join("main_only.txt"), "main")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "main moves on"], &repo).await;
    let main_after = current_head(&repo).await;

    let _ = git_cmd(&["checkout", "-b", "renegade", &initial], &repo).await;
    tokio::fs::write(repo.join("b.txt"), "f").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "renegade off old initial"], &repo).await;

    assert!(
        try_adopt_renegade_branch(&repo, Some(&main_after))
            .await
            .is_none(),
        "renegade does not contain main_after → unsafe to adopt"
    );
}

#[tokio::test]
async fn is_ancestor_true_when_sha_reachable_from_ref() {
    let (_tmp, repo) = make_repo().await;
    let initial = current_head(&repo).await;
    // Create a feature branch off main with one extra commit.
    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("b.txt"), "more").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "feature commit"], &repo).await;

    assert!(
        is_ancestor(&repo, &initial, "feature").await,
        "main's tip should be an ancestor of feature"
    );
}

#[tokio::test]
async fn is_ancestor_false_when_sha_unreachable_from_ref() {
    let (_tmp, repo) = make_repo().await;
    let initial = current_head(&repo).await;
    // Move main forward; feature branches from the OLD initial commit.
    tokio::fs::write(repo.join("main_only.txt"), "main")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "main moves on"], &repo).await;
    let main_after = current_head(&repo).await;

    // Branch the feature from the original initial commit, not main_after.
    let _ = git_cmd(&["checkout", "-b", "feature", &initial], &repo).await;
    tokio::fs::write(repo.join("b.txt"), "f").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "feature off old initial"], &repo).await;

    assert!(
        !is_ancestor(&repo, &main_after, "feature").await,
        "main's newer commit is NOT reachable from feature"
    );
}

#[tokio::test]
async fn is_ancestor_false_for_unknown_sha() {
    let (_tmp, repo) = make_repo().await;
    assert!(
        !is_ancestor(&repo, "0000000000000000000000000000000000000000", "main").await,
        "unknown SHA must not be reported as ancestor"
    );
}
