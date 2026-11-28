use super::*;
use crate::engine::git_ops::{git_cmd, git_cmd_env};
use std::ffi::OsStr;
use std::path::PathBuf;

async fn init_repo(repo: &std::path::Path) {
    let _ = git_cmd(&["init"], repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
    let _ = git_cmd(&["config", "user.email", "test@test.test"], repo).await;
    let _ = git_cmd(&["config", "user.name", "test"], repo).await;
    tokio::fs::write(repo.join("a.txt"), "first").await.unwrap();
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "initial"], repo).await;
}

async fn make_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    init_repo(&repo).await;
    (tmp, repo)
}

async fn make_repo_with_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let worktree = tmp.path().join("wt");
    tokio::fs::create_dir(&repo).await.unwrap();
    init_repo(&repo).await;
    (tmp, repo, worktree)
}

async fn spawn_worktree_at_base(
    repo: &std::path::Path,
    worktree: &std::path::Path,
    base: &str,
) -> String {
    let tracked = "lucidos-claude-code-repo-example-repo-do-the-thing".to_string();
    let wt = worktree.to_str().unwrap();
    let _ = git_cmd(
        &[
            "worktree",
            "add",
            "--no-checkout",
            "--no-track",
            wt,
            "-b",
            &tracked,
            base,
        ],
        repo,
    )
    .await;
    let _ = git_cmd(&["checkout", "HEAD", "--"], worktree).await;
    tracked
}

async fn unrelated_sibling(repo: &std::path::Path, base: &str, created_at: &str) -> String {
    let name = "someone-elses-branch".to_string();
    let _ = git_cmd_env(
        &["branch", &name, base],
        repo,
        &[("GIT_COMMITTER_DATE", OsStr::new(created_at))],
    )
    .await;
    let _ = git_cmd(&["checkout", &name], repo).await;
    tokio::fs::write(repo.join("theirs.txt"), "their work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "unrelated work"], repo).await;
    let _ = git_cmd(&["checkout", "main"], repo).await;
    name
}

async fn current_head(repo: &std::path::Path) -> String {
    let out = git_cmd(&["rev-parse", "HEAD"], repo).await.unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

async fn session_on_tracked_branch(repo: &std::path::Path) -> (String, String) {
    let tracked = "lucidos-claude-code-repo-example-repo-do-the-thing".to_string();
    let anchor = current_head(repo).await;
    let _ = git_cmd(&["checkout", "-b", &tracked], repo).await;
    tokio::fs::write(repo.join("work.txt"), "agent work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "agent work"], repo).await;
    (anchor, tracked)
}

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

#[tokio::test]
async fn compute_returns_none_when_last_sha_is_none() {
    let (_tmp, repo) = make_repo().await;
    assert!(compute_external_edit_note(&repo, None, false)
        .await
        .is_none());
}

#[tokio::test]
async fn compute_returns_none_when_worktree_unchanged() {
    let (_tmp, repo) = make_repo().await;
    let head = current_head(&repo).await;
    assert!(
        compute_external_edit_note(&repo, Some(&head), false)
            .await
            .is_none(),
        "no edits since last idle → no note"
    );
}

#[tokio::test]
async fn compute_returns_none_when_worktree_path_missing() {
    let missing = std::path::PathBuf::from("/nonexistent/wt");
    assert!(compute_external_edit_note(&missing, Some("abcd"), false)
        .await
        .is_none());
}

#[tokio::test]
async fn compute_detects_uncommitted_changes() {
    let (_tmp, repo) = make_repo().await;
    let head = current_head(&repo).await;

    tokio::fs::write(repo.join("a.txt"), "user edit")
        .await
        .unwrap();

    let note = compute_external_edit_note(&repo, Some(&head), false)
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

    tokio::fs::write(repo.join("user_change.txt"), "user work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "user added a thing"], &repo).await;

    let note = compute_external_edit_note(&repo, Some(&last_head), false)
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

    tokio::fs::write(repo.join("committed.txt"), "yes")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "commit between turns"], &repo).await;

    tokio::fs::write(repo.join("dirty.txt"), "wip")
        .await
        .unwrap();

    let note = compute_external_edit_note(&repo, Some(&last_head), false)
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
    for i in 0..60 {
        tokio::fs::write(repo.join(format!("f{:02}.txt", i)), "x")
            .await
            .unwrap();
    }
    let note = compute_external_edit_note(&repo, Some(&last_head), false)
        .await
        .expect("many dirty → note");
    assert!(note.contains("… and 10 more file(s)"), "note: {}", note);
}

#[tokio::test]
async fn note_does_not_blame_the_user() {
    let (_tmp, repo) = make_repo().await;
    let head = current_head(&repo).await;
    tokio::fs::write(repo.join("a.txt"), "edited")
        .await
        .unwrap();

    let note = compute_external_edit_note(&repo, Some(&head), false)
        .await
        .expect("dirty worktree → note");
    assert!(
        !note.contains("the user edited"),
        "note must not assert who changed the worktree: {}",
        note
    );
}

async fn reset_backwards(repo: &std::path::Path) -> String {
    tokio::fs::write(repo.join("agent_work.txt"), "work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "the agent's commit"], repo).await;
    let pre_reset = current_head(repo).await;
    let _ = git_cmd(&["reset", "--hard", "HEAD~1"], repo).await;
    let _ = git_cmd(&["clean", "-fd"], repo).await;
    pre_reset
}

#[tokio::test]
async fn head_move_with_empty_log_suppressed_when_explained() {
    let (_tmp, repo) = make_repo().await;
    let last_sha = reset_backwards(&repo).await;

    assert!(
        compute_external_edit_note(&repo, Some(&last_sha), true)
            .await
            .is_none(),
        "an explained backwards reset with a clean tree has nothing to report"
    );

    let note = compute_external_edit_note(&repo, Some(&last_sha), false)
        .await
        .expect("an unexplained HEAD move must still be reported");
    assert!(note.contains("HEAD moved (no log available)"), "{}", note);
}

#[tokio::test]
async fn dirty_files_still_reported_when_explained() {
    let (_tmp, repo) = make_repo().await;
    let last_sha = reset_backwards(&repo).await;
    tokio::fs::write(repo.join("user_edit.txt"), "by hand")
        .await
        .unwrap();

    let note = compute_external_edit_note(&repo, Some(&last_sha), true)
        .await
        .expect("a real edit after the reset must still be reported");
    assert!(note.contains("Uncommitted changes:"), "{}", note);
    assert!(note.contains("user_edit.txt"), "{}", note);
    assert!(
        !note.contains("HEAD moved (no log available)"),
        "the explained reset line must still be suppressed: {}",
        note
    );
}

#[tokio::test]
async fn real_commits_still_reported_when_explained() {
    let (_tmp, repo) = make_repo().await;
    let last_head = current_head(&repo).await;
    tokio::fs::write(repo.join("landed.txt"), "merged")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "feat: the merged work"], &repo).await;

    let note = compute_external_edit_note(&repo, Some(&last_head), true)
        .await
        .expect("commits ahead of the recorded SHA are still worth reporting");
    assert!(
        note.contains("Committed changes since your last action:"),
        "{}",
        note
    );
    assert!(note.contains("feat: the merged work"), "{}", note);
}

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
    assert_eq!(
        err,
        BranchMismatch::OnOtherBranch {
            expected: "main".to_string(),
            found: "user-feature".to_string(),
        }
    );
    let msg = format!("{}", err);
    assert!(msg.contains("user-feature"));
    assert!(msg.contains("main"));
    assert!(msg.contains("Resolve manually"));
}

#[tokio::test]
async fn verify_branch_errors_on_detached_head() {
    let (_tmp, repo) = make_repo().await;
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
    assert_eq!(
        err,
        BranchMismatch::Detached {
            expected: "main".to_string(),
        }
    );
    let msg = format!("{}", err);
    assert!(msg.contains("detached HEAD"), "msg: {}", msg);
}

#[tokio::test]
async fn verify_branch_ok_when_worktree_missing() {
    let missing = std::path::PathBuf::from("/nonexistent/wt-for-verify");
    assert!(verify_branch(&missing, "main").await.is_ok());
}

#[tokio::test]
async fn verify_branch_refuses_when_git_cannot_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let err = verify_branch(tmp.path(), "main")
        .await
        .expect_err("an unanswerable probe must refuse the spawn");
    assert_eq!(
        err,
        BranchMismatch::Unanswered {
            expected: "main".to_string(),
        }
    );
    assert!(format!("{}", err).contains("could not say"));
}

#[tokio::test]
async fn adopt_returns_some_when_branch_descends_from_the_anchor() {
    let (_tmp, repo) = make_repo().await;
    let (_base, tracked) = session_on_tracked_branch(&repo).await;
    let anchor = current_head(&repo).await;
    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("b.txt"), "x").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "feature commit"], &repo).await;
    let _ = git_cmd(&["branch", "-D", &tracked], &repo).await;

    let (new_branch, note) =
        try_adopt_renegade_branch(&repo, &repo, &tracked, &IdleAnchor::Found(anchor.clone()))
            .await
            .expect("feature contains the anchor commit, so it is safe to adopt");
    assert_eq!(new_branch, "feature");
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
        try_adopt_renegade_branch(&repo, &repo, "main", &IdleAnchor::Found(main_after.clone()))
            .await
            .is_none(),
        "renegade does not contain the anchor, and that veto is the whole test"
    );
}

#[tokio::test]
async fn adopt_without_an_anchor_takes_a_branch_created_off_the_tracked_one() {
    let (_tmp, repo) = make_repo().await;
    let (_base, tracked) = session_on_tracked_branch(&repo).await;
    let _ = git_cmd(&["checkout", "-b", "feature-1234"], &repo).await;
    tokio::fs::write(repo.join("ticket.txt"), "ticket work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "ticket work"], &repo).await;

    let (adopted, note) = try_adopt_renegade_branch(&repo, &repo, &tracked, &IdleAnchor::Absent)
        .await
        .expect("a branch built on the tracked one is safe to adopt with no anchor");
    assert_eq!(adopted, "feature-1234");
    assert!(note.contains("feature-1234"), "note: {}", note);
}

#[tokio::test]
async fn adopt_without_an_anchor_takes_a_branch_renamed_in_place() {
    let (_tmp, repo) = make_repo().await;
    let (_base, tracked) = session_on_tracked_branch(&repo).await;
    let _ = git_cmd(&["branch", "-m", &tracked, "feature-1234-renamed"], &repo).await;

    let (adopted, _note) = try_adopt_renegade_branch(&repo, &repo, &tracked, &IdleAnchor::Absent)
        .await
        .expect("git's own reflog records where the branch came from");
    assert_eq!(adopted, "feature-1234-renamed");
}

#[tokio::test]
async fn adopt_without_an_anchor_refuses_a_sibling_branch() {
    let (_tmp, repo) = make_repo().await;
    let (base, tracked) = session_on_tracked_branch(&repo).await;
    let _ = git_cmd(&["checkout", "-b", "someone-elses-branch", &base], &repo).await;
    tokio::fs::write(repo.join("theirs.txt"), "theirs")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "unrelated work"], &repo).await;

    assert!(
        try_adopt_renegade_branch(&repo, &repo, &tracked, &IdleAnchor::Absent)
            .await
            .is_none(),
        "a sibling branch off the same base is not a continuation of our work"
    );
}

#[tokio::test]
async fn adopt_without_an_anchor_refuses_when_the_tracked_ref_was_merely_deleted() {
    let (_tmp, repo) = make_repo().await;
    let (base, tracked) = session_on_tracked_branch(&repo).await;
    let _ = git_cmd(&["checkout", "-b", "someone-elses-branch", &base], &repo).await;
    tokio::fs::write(repo.join("theirs.txt"), "theirs")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "unrelated work"], &repo).await;
    let out = git_cmd(&["branch", "-D", &tracked], &repo).await.unwrap();
    assert!(
        out.status.success(),
        "the tracked branch should be deletable once it is not checked out: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        try_adopt_renegade_branch(&repo, &repo, &tracked, &IdleAnchor::Absent)
            .await
            .is_none(),
        "a deleted tracked ref is not a rename, so the sibling must not be adopted"
    );
}

#[tokio::test]
async fn adopt_refuses_a_worktree_still_on_the_tracked_branch() {
    let (_tmp, repo) = make_repo().await;
    let (_base, tracked) = session_on_tracked_branch(&repo).await;
    let anchor = current_head(&repo).await;

    assert!(
        try_adopt_renegade_branch(&repo, &repo, &tracked, &IdleAnchor::Found(anchor.clone()))
            .await
            .is_none(),
        "the worktree is still on the tracked branch, nothing to adopt"
    );
}

#[tokio::test]
async fn adopt_refuses_when_the_anchor_lookup_could_not_answer() {
    let (_tmp, repo) = make_repo().await;
    let (_base, tracked) = session_on_tracked_branch(&repo).await;
    let _ = git_cmd(&["checkout", "-b", "feature-1234"], &repo).await;

    assert!(
        try_adopt_renegade_branch(&repo, &repo, &tracked, &IdleAnchor::Unknown)
            .await
            .is_none(),
        "an unanswered anchor lookup must never authorize retargeting the thread"
    );
    assert!(
        try_adopt_renegade_branch(&repo, &repo, &tracked, &IdleAnchor::Absent)
            .await
            .is_some(),
        "a verified absence still falls back to the continuation proof"
    );
}

#[tokio::test]
async fn adopt_refuses_a_stale_rename_that_no_longer_holds_the_anchor() {
    let (_tmp, repo) = make_repo().await;
    let (_base, tracked) = session_on_tracked_branch(&repo).await;
    let anchor = current_head(&repo).await;
    let _ = git_cmd(&["checkout", "main"], &repo).await;
    tokio::fs::write(repo.join("unrelated.txt"), "unrelated")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "unrelated work"], &repo).await;
    let unrelated = current_head(&repo).await;
    let _ = git_cmd(&["branch", "-m", &tracked, "feature-1234"], &repo).await;
    let _ = git_cmd(&["checkout", "feature-1234"], &repo).await;
    let _ = git_cmd(&["reset", "--hard", &unrelated], &repo).await;

    assert!(
        try_adopt_renegade_branch(&repo, &repo, &tracked, &IdleAnchor::Found(anchor.clone()))
            .await
            .is_none(),
        "the rename is real but the reset dropped our commits, so adoption loses work"
    );
}

#[tokio::test]
async fn adopt_refuses_a_detached_head() {
    let (_tmp, repo) = make_repo().await;
    let (_base, tracked) = session_on_tracked_branch(&repo).await;
    let head = current_head(&repo).await;
    let _ = git_cmd(&["checkout", "--detach", &head], &repo).await;

    assert!(
        try_adopt_renegade_branch(&repo, &repo, &tracked, &IdleAnchor::Found(head.clone()))
            .await
            .is_none(),
        "a detached HEAD names no branch to adopt"
    );
}

#[tokio::test]
async fn adopt_without_an_anchor_refuses_when_no_reflog_proves_the_rename() {
    let (_tmp, repo) = make_repo().await;
    let (_base, tracked) = session_on_tracked_branch(&repo).await;
    let _ = git_cmd(&["config", "core.logAllRefUpdates", "false"], &repo).await;
    let _ = tokio::fs::remove_dir_all(repo.join(".git/logs")).await;
    let _ = git_cmd(
        &["branch", "-m", &tracked, "renamed-without-a-reflog"],
        &repo,
    )
    .await;

    assert!(
        try_adopt_renegade_branch(&repo, &repo, &tracked, &IdleAnchor::Absent)
            .await
            .is_none(),
        "without git's own rename record there is nothing linking the branches"
    );
}

#[tokio::test]
async fn adopt_refuses_a_sibling_when_the_tracked_ref_sits_at_the_base() {
    let (_tmp, repo, wt) = make_repo_with_worktree().await;
    let base = current_head(&repo).await;
    let sibling = unrelated_sibling(&repo, &base, "2020-01-02T03:04:05+0000").await;
    let tracked = spawn_worktree_at_base(&repo, &wt, &base).await;
    let out = git_cmd(&["checkout", &sibling], &wt).await.unwrap();
    assert!(
        out.status.success(),
        "moving the worktree onto the sibling failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        is_ancestor(&wt, &format!("refs/heads/{}", tracked), "HEAD").await,
        "the tracked ref must still be reachable from HEAD for the gap to bite"
    );
    assert!(
        try_adopt_renegade_branch(&repo, &wt, &tracked, &IdleAnchor::Absent)
            .await
            .is_none(),
        "this worktree never created that branch, so it is not ours to adopt"
    );
}

#[tokio::test]
async fn adopt_refuses_a_sibling_when_the_anchor_is_only_the_base_tip() {
    let (_tmp, repo, wt) = make_repo_with_worktree().await;
    let base = current_head(&repo).await;
    let sibling = unrelated_sibling(&repo, &base, "2020-01-02T03:04:05+0000").await;
    let tracked = spawn_worktree_at_base(&repo, &wt, &base).await;
    let _ = git_cmd(&["checkout", &sibling], &wt).await;

    assert!(
        is_ancestor(&wt, &base, "HEAD").await,
        "the anchor must still be reachable from HEAD for the gap to bite"
    );
    assert!(
        try_adopt_renegade_branch(&repo, &wt, &tracked, &IdleAnchor::Found(base.clone()))
            .await
            .is_none(),
        "a base-tip anchor proves nothing, so provenance has to carry the refusal"
    );
}

#[tokio::test]
async fn idle_refuses_a_sibling_when_the_tracked_ref_sits_at_the_base() {
    let (_tmp, repo, wt) = make_repo_with_worktree().await;
    let base = current_head(&repo).await;
    let sibling = unrelated_sibling(&repo, &base, "2020-01-02T03:04:05+0000").await;
    let tracked = spawn_worktree_at_base(&repo, &wt, &base).await;
    let _ = git_cmd(&["checkout", &sibling], &wt).await;

    assert!(
        try_adopt_branch_at_idle(&repo, &wt, &tracked, Some(&base))
            .await
            .is_none(),
        "the thread's Diff must not render a branch this worktree never created"
    );
}

#[tokio::test]
async fn adopt_refuses_a_same_second_sibling_that_carries_its_own_commit() {
    const INSTANT: &str = "2026-01-02T03:04:05+0000";
    let (_tmp, repo, wt) = make_repo_with_worktree().await;
    let base = current_head(&repo).await;
    let sibling = unrelated_sibling(&repo, &base, INSTANT).await;
    let tracked = spawn_worktree_at_base(&repo, &wt, &base).await;
    let _ = git_cmd_env(
        &["checkout", &sibling],
        &wt,
        &[("GIT_COMMITTER_DATE", OsStr::new(INSTANT))],
    )
    .await;

    assert!(
        try_adopt_renegade_branch(&repo, &wt, &tracked, &IdleAnchor::Absent)
            .await
            .is_none(),
        "a branch that had moved since its creation was not created by this move"
    );
}

#[tokio::test]
async fn adopt_refuses_a_sibling_created_before_the_worktree_moved_onto_it() {
    let (_tmp, repo, wt) = make_repo_with_worktree().await;
    let base = current_head(&repo).await;
    let _ = git_cmd_env(
        &["branch", "someone-elses-branch", &base],
        &repo,
        &[("GIT_COMMITTER_DATE", OsStr::new("2020-01-02T03:04:05+0000"))],
    )
    .await;
    let tracked = spawn_worktree_at_base(&repo, &wt, &base).await;
    let _ = git_cmd(&["checkout", "someone-elses-branch"], &wt).await;

    assert!(
        try_adopt_renegade_branch(&repo, &wt, &tracked, &IdleAnchor::Absent)
            .await
            .is_none(),
        "a branch that existed before we moved onto it was not created by us"
    );
}

#[tokio::test]
async fn adopt_takes_a_branch_created_off_a_zero_commit_tracked_branch() {
    let (_tmp, repo, wt) = make_repo_with_worktree().await;
    let base = current_head(&repo).await;
    let tracked = spawn_worktree_at_base(&repo, &wt, &base).await;
    let _ = git_cmd(&["checkout", "-b", "feature-1234"], &wt).await;
    tokio::fs::write(wt.join("ticket.txt"), "ticket work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt).await;
    let _ = git_cmd(&["commit", "-m", "ticket work"], &wt).await;

    let (adopted, _note) = try_adopt_renegade_branch(&repo, &wt, &tracked, &IdleAnchor::Absent)
        .await
        .expect("the worktree created this branch as it moved onto it");
    assert_eq!(adopted, "feature-1234");
}

#[tokio::test]
async fn idle_takes_a_branch_created_off_a_zero_commit_tracked_branch() {
    let (_tmp, repo, wt) = make_repo_with_worktree().await;
    let base = current_head(&repo).await;
    let tracked = spawn_worktree_at_base(&repo, &wt, &base).await;
    let _ = git_cmd(&["checkout", "-b", "feature-1234"], &wt).await;
    tokio::fs::write(wt.join("ticket.txt"), "ticket work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt).await;
    let _ = git_cmd(&["commit", "-m", "ticket work"], &wt).await;

    let (adopted, _note) = try_adopt_branch_at_idle(&repo, &wt, &tracked, Some(&base))
        .await
        .expect("a first idle at the base tip must still follow the agent's own branch");
    assert_eq!(adopted, "feature-1234");
}

#[tokio::test]
async fn adopt_takes_a_branch_created_with_git_switch() {
    let (_tmp, repo, wt) = make_repo_with_worktree().await;
    let base = current_head(&repo).await;
    let tracked = spawn_worktree_at_base(&repo, &wt, &base).await;
    let _ = git_cmd(&["switch", "-c", "feature-1234"], &wt).await;

    let (adopted, _note) = try_adopt_renegade_branch(&repo, &wt, &tracked, &IdleAnchor::Absent)
        .await
        .expect("`git switch -c` creates the branch as it moves onto it");
    assert_eq!(adopted, "feature-1234");
}

#[tokio::test]
async fn a_worktree_on_its_tracked_branch_serves_it() {
    let (_tmp, repo, wt) = make_repo_with_worktree().await;
    let base = current_head(&repo).await;
    let tracked = spawn_worktree_at_base(&repo, &wt, &base).await;

    assert!(worktree_serves_branch(&repo, &wt, &tracked, &IdleAnchor::Absent).await);
}

#[tokio::test]
async fn a_worktree_on_the_agents_own_side_branch_still_serves_the_tracked_one() {
    let (_tmp, repo, wt) = make_repo_with_worktree().await;
    let base = current_head(&repo).await;
    let tracked = spawn_worktree_at_base(&repo, &wt, &base).await;
    let _ = git_cmd(&["switch", "-c", "p0-measure"], &wt).await;
    tokio::fs::write(wt.join("measure.txt"), "run A")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt).await;
    let _ = git_cmd(&["commit", "-m", "measuring run"], &wt).await;

    assert!(
        worktree_serves_branch(&repo, &wt, &tracked, &IdleAnchor::Absent).await,
        "the agent cut this branch from its tracked one, so the worktree is still the thread's"
    );
}

#[tokio::test]
async fn a_worktree_moved_onto_someone_elses_branch_does_not_serve_the_tracked_one() {
    let (_tmp, repo, wt) = make_repo_with_worktree().await;
    let base = current_head(&repo).await;
    let sibling = unrelated_sibling(&repo, &base, "2020-01-02T03:04:05+0000").await;
    let tracked = spawn_worktree_at_base(&repo, &wt, &base).await;
    let _ = git_cmd(&["checkout", &sibling], &wt).await;

    assert!(
        !worktree_serves_branch(&repo, &wt, &tracked, &IdleAnchor::Absent).await,
        "resuming here would run the thread against another branch's checkout"
    );
}

#[tokio::test]
async fn a_side_branch_does_not_serve_when_the_anchor_lookup_failed() {
    let (_tmp, repo, wt) = make_repo_with_worktree().await;
    let base = current_head(&repo).await;
    let tracked = spawn_worktree_at_base(&repo, &wt, &base).await;
    let _ = git_cmd(&["switch", "-c", "p0-measure"], &wt).await;

    assert!(
        !worktree_serves_branch(&repo, &wt, &tracked, &IdleAnchor::Unknown).await,
        "an unanswered anchor must never stand in for the proof"
    );
}

#[test]
fn the_side_branch_note_names_both_branches() {
    let note = side_branch_note("p0-measure", "thread-branch");
    assert!(note.contains("'p0-measure'"), "note: {}", note);
    assert!(note.contains("'thread-branch'"), "note: {}", note);
    assert!(note.starts_with("[Note from engine"));
}

#[tokio::test]
async fn the_provenance_probe_refuses_when_git_cannot_answer() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(
        !worktree_created_the_branch(tmp.path(), "main").await,
        "an unanswered probe must never stand in for provenance"
    );
}

#[tokio::test]
async fn is_ancestor_true_when_sha_reachable_from_ref() {
    let (_tmp, repo) = make_repo().await;
    let initial = current_head(&repo).await;
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
    tokio::fs::write(repo.join("main_only.txt"), "main")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "main moves on"], &repo).await;
    let main_after = current_head(&repo).await;

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

#[tokio::test]
async fn idle_adopts_a_branch_renamed_in_place() {
    let (_tmp, repo) = make_repo().await;
    let (anchor, tracked) = session_on_tracked_branch(&repo).await;
    let _ = git_cmd(
        &["branch", "-m", &tracked, "ticket-1234-drop-unused-tables"],
        &repo,
    )
    .await;

    let (adopted, note) = try_adopt_branch_at_idle(&repo, &repo, &tracked, Some(&anchor))
        .await
        .expect("a rename in place must be adopted at idle");
    assert_eq!(adopted, "ticket-1234-drop-unused-tables");
    assert!(
        note.contains("ticket-1234-drop-unused-tables"),
        "note: {}",
        note
    );
}

#[tokio::test]
async fn idle_adopts_a_branch_created_off_our_own_work() {
    let (_tmp, repo) = make_repo().await;
    let (anchor, tracked) = session_on_tracked_branch(&repo).await;
    let _ = git_cmd(&["checkout", "-b", "feature-1234"], &repo).await;
    tokio::fs::write(repo.join("more.txt"), "more")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "more work"], &repo).await;

    let (adopted, _note) = try_adopt_branch_at_idle(&repo, &repo, &tracked, Some(&anchor))
        .await
        .expect("a branch built on top of the tracked one must be adopted");
    assert_eq!(adopted, "feature-1234");
}

#[tokio::test]
async fn idle_keeps_the_tracked_branch_when_nothing_moved() {
    let (_tmp, repo) = make_repo().await;
    let (anchor, tracked) = session_on_tracked_branch(&repo).await;
    assert!(
        try_adopt_branch_at_idle(&repo, &repo, &tracked, Some(&anchor))
            .await
            .is_none(),
        "the worktree is still on the tracked branch, nothing to adopt"
    );
}

#[tokio::test]
async fn idle_refuses_a_branch_that_does_not_contain_the_tracked_ref() {
    let (_tmp, repo) = make_repo().await;
    let (anchor, tracked) = session_on_tracked_branch(&repo).await;
    let _ = git_cmd(&["checkout", "-b", "someone-elses-branch", &anchor], &repo).await;
    tokio::fs::write(repo.join("theirs.txt"), "theirs")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "unrelated work"], &repo).await;

    assert!(
        try_adopt_branch_at_idle(&repo, &repo, &tracked, Some(&anchor))
            .await
            .is_none(),
        "a sibling branch off the same base is not a continuation of our work"
    );
}

#[tokio::test]
async fn idle_refuses_when_head_does_not_descend_from_the_anchor() {
    let (_tmp, repo) = make_repo().await;
    let (_anchor, tracked) = session_on_tracked_branch(&repo).await;
    let head_with_work = current_head(&repo).await;
    let _ = git_cmd(&["checkout", "main"], &repo).await;
    let _ = git_cmd(&["branch", "-m", &tracked, "renamed-away"], &repo).await;

    assert!(
        try_adopt_branch_at_idle(&repo, &repo, &tracked, Some(&head_with_work))
            .await
            .is_none(),
        "main does not contain the session's work, so it must not be adopted"
    );
}

#[tokio::test]
async fn idle_refuses_a_detached_head() {
    let (_tmp, repo) = make_repo().await;
    let (anchor, tracked) = session_on_tracked_branch(&repo).await;
    let head = current_head(&repo).await;
    let _ = git_cmd(&["checkout", "--detach", &head], &repo).await;

    assert!(
        try_adopt_branch_at_idle(&repo, &repo, &tracked, Some(&anchor))
            .await
            .is_none(),
        "a detached HEAD names no branch to adopt"
    );
}

#[tokio::test]
async fn idle_refuses_without_an_anchor_sha() {
    let (_tmp, repo) = make_repo().await;
    let (_anchor, tracked) = session_on_tracked_branch(&repo).await;
    let _ = git_cmd(&["branch", "-m", &tracked, "renamed"], &repo).await;

    assert!(
        try_adopt_branch_at_idle(&repo, &repo, &tracked, None)
            .await
            .is_none(),
        "without an anchor there is no ancestry check to make"
    );
}

#[tokio::test]
async fn idle_refuses_a_sibling_branch_when_the_tracked_ref_was_merely_deleted() {
    let (_tmp, repo) = make_repo().await;
    let (anchor, tracked) = session_on_tracked_branch(&repo).await;
    let _ = git_cmd(&["checkout", "-b", "someone-elses-branch", &anchor], &repo).await;
    tokio::fs::write(repo.join("theirs.txt"), "theirs")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "unrelated work"], &repo).await;
    let out = git_cmd(&["branch", "-D", &tracked], &repo).await.unwrap();
    assert!(
        out.status.success(),
        "the tracked branch should be deletable once it is not checked out: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        try_adopt_branch_at_idle(&repo, &repo, &tracked, Some(&anchor))
            .await
            .is_none(),
        "a deleted tracked ref is not a rename, so the sibling must not be adopted"
    );
}

#[tokio::test]
async fn idle_refuses_when_the_repo_keeps_no_reflog_to_prove_the_rename() {
    let (_tmp, repo) = make_repo().await;
    let (anchor, tracked) = session_on_tracked_branch(&repo).await;
    let _ = git_cmd(&["config", "core.logAllRefUpdates", "false"], &repo).await;
    let _ = tokio::fs::remove_dir_all(repo.join(".git/logs")).await;
    let _ = git_cmd(
        &["branch", "-m", &tracked, "renamed-without-a-reflog"],
        &repo,
    )
    .await;

    assert!(
        try_adopt_branch_at_idle(&repo, &repo, &tracked, Some(&anchor))
            .await
            .is_none(),
        "without git's own rename record there is nothing linking the branches"
    );
}

#[tokio::test]
async fn the_note_names_no_cause_for_any_kind_of_change() {
    const HEADER: &str = "[Note from engine: your worktree changed since you were last active.";
    let (_tmp, repo) = make_repo().await;
    let pre_reset = reset_backwards(&repo).await;
    let reset = compute_external_edit_note(&repo, Some(&pre_reset), false)
        .await
        .expect("a backwards HEAD move is reported when unexplained");

    let head = current_head(&repo).await;
    tokio::fs::write(repo.join("a.txt"), "changed")
        .await
        .unwrap();
    let dirty = compute_external_edit_note(&repo, Some(&head), false)
        .await
        .expect("a dirty tree is reported");
    let _ = git_cmd(&["commit", "-am", "work"], &repo).await;
    let committed = compute_external_edit_note(&repo, Some(&head), false)
        .await
        .expect("a new commit is reported");

    for note in [reset, dirty, committed] {
        assert!(note.starts_with(HEADER), "{note}");
        let lower = note.to_lowercase();
        for cause in ["user", "edited", "reset", "discard", "apply", "applied"] {
            assert!(
                !lower.contains(cause),
                "the note names a cause ({cause}): {note}"
            );
        }
    }
}

#[tokio::test]
async fn is_ancestor_is_false_when_git_cannot_be_asked() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("gone");
    assert!(
        !is_ancestor(&missing, "0123abc", "HEAD").await,
        "a probe that could not run must never claim ancestry"
    );
}

#[tokio::test]
async fn verify_branch_refuses_when_git_cannot_be_spawned() {
    let tmp = tempfile::tempdir().unwrap();
    let not_a_dir = tmp.path().join("file");
    tokio::fs::write(&not_a_dir, "x").await.unwrap();
    let err = verify_branch(&not_a_dir, "main")
        .await
        .expect_err("a probe that never ran must refuse the spawn");
    assert_eq!(
        err,
        BranchMismatch::Unanswered {
            expected: "main".to_string(),
        }
    );
}

#[tokio::test]
async fn adopt_refuses_a_created_branch_when_the_tracked_ref_was_deleted() {
    let (_tmp, repo, wt) = make_repo_with_worktree().await;
    let base = current_head(&repo).await;
    let tracked = spawn_worktree_at_base(&repo, &wt, &base).await;
    let _ = git_cmd(&["checkout", "-b", "feature-1234"], &wt).await;
    assert!(
        try_adopt_renegade_branch(&repo, &wt, &tracked, &IdleAnchor::Absent)
            .await
            .is_some(),
        "control: the worktree created this branch and the tracked ref still leads into HEAD"
    );

    let out = git_cmd(&["branch", "-D", &tracked], &repo).await.unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        try_adopt_renegade_branch(&repo, &wt, &tracked, &IdleAnchor::Absent)
            .await
            .is_none(),
        "a deleted tracked ref proves nothing, even for a branch this worktree created"
    );
}
