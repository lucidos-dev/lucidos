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

    // User edits a file but doesn't commit.
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

    // User commits something between turns.
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
    // Create 60 untracked files; helper caps at 50.
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

/// The note must not assert a cause it cannot know: it sees a SHA and a
/// `git status`, never who moved them. Blaming the user for an engine reset is
/// the misattribution this wording fix exists to stop.
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

/// Build the Discard signature: commit on top of `main`, then reset the branch
/// back. HEAD has moved relative to the recorded SHA, but `<last>..HEAD` is
/// empty because HEAD went BACKWARDS. Returns the pre-reset SHA (what the last
/// `CodingAgentIdled` would have recorded).
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

/// The regression: after a Discard, the whole note was "the user edited files
/// … HEAD moved (no log available)". With the cause explained by the turn-gap
/// note and a clean tree, there is nothing left to report.
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

    // Unexplained, the same state still reports the move: the suppression is
    // scoped to the caller knowing the cause, not to the shape of the reset.
    let note = compute_external_edit_note(&repo, Some(&last_sha), false)
        .await
        .expect("an unexplained HEAD move must still be reported");
    assert!(note.contains("HEAD moved (no log available)"), "{}", note);
}

/// Suppressing the HEAD-move line must not swallow a real edit the user made
/// after the reset.
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

/// A non-empty log is factual and names no cause, so an explained HEAD move
/// (e.g. an Apply, which moves the worktree FORWARD onto merged commits) keeps
/// reporting it.
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

// -------------------- try_adopt_branch_at_idle --------------------

/// Put the repo on an engine-named branch with one commit, mimicking a
/// coding-agent session that has done work. Returns the branch's start SHA
/// (the anchor a first idle would carry) and the branch name.
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
async fn idle_adopts_a_branch_renamed_in_place() {
    // The reported bug: a repo skill runs `git branch -m` mid-session, so the
    // tracked ref is gone by the time the idle computes the diff.
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
    // `git checkout -b` mid-turn: the tracked ref still exists and is an
    // ancestor of HEAD, so the new branch genuinely continues our work.
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
    // The dangerous shape gate 2 exists for: the worktree was manually checked
    // out onto a sibling branch that forks from the SAME base, so the anchor
    // ancestry check alone would pass. The tracked ref is still alive and is
    // NOT in that branch's history, so this is not our work.
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
    // Gate 1: the tracked ref is gone AND the worktree sits on something that
    // predates where this session started.
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
    // The absence of the tracked ref is NOT evidence of a rename: checking out
    // a sibling branch and then deleting the tracked one leaves exactly the
    // same absence. On a first idle the anchor is only the shared base, so the
    // ancestry gate alone would pass and the thread would adopt work that was
    // never its own (and delete it on a later Discard).
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
    // Reflogs off means no evidence, and no evidence means no adoption. The
    // thread keeps its tracked branch and the next spawn re-derives.
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
