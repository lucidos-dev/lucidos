use super::*;
use crate::engine::git_ops::{git_cmd, git_cmd_env};
use std::ffi::OsStr;
use std::path::PathBuf;

/// Initialise a git repo at `repo` with one commit on `main`.
async fn init_repo(repo: &std::path::Path) {
    let _ = git_cmd(&["init"], repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
    let _ = git_cmd(&["config", "user.email", "test@test.test"], repo).await;
    let _ = git_cmd(&["config", "user.name", "test"], repo).await;
    tokio::fs::write(repo.join("a.txt"), "first").await.unwrap();
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "initial"], repo).await;
}

/// Build a fresh temp git repo with one commit on `main`. Returns the
/// tempdir guard (dropped → cleanup) and the repo path.
async fn make_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    init_repo(&repo).await;
    (tmp, repo)
}

/// A repo and a linked worktree at SEPARATE paths, as production always has
/// them. The worktree does not exist yet: [`spawn_worktree_at_base`] adds it.
/// Only here does the per-worktree HEAD reflog differ from the main checkout's,
/// which is the log the provenance proof reads.
async fn make_repo_with_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let worktree = tmp.path().join("wt");
    tokio::fs::create_dir(&repo).await.unwrap();
    init_repo(&repo).await;
    (tmp, repo, worktree)
}

/// Add the session's worktree on a tracked branch cut from `base` with NO
/// commit of its own, the way the engine spawns one. Every branch forked from
/// that base then contains the tracked ref, so containment proves nothing and
/// only provenance is left. Returns the tracked branch name.
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

/// Somebody else's branch, cut from `base` in the MAIN checkout and carrying
/// its own commit. Created there rather than in the session's worktree, so that
/// worktree's HEAD reflog knows nothing about it, exactly as in production.
/// `created_at` pins the creation entry's timestamp.
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
    // Nothing to verify → no error (let downstream handle the missing dir).
    assert!(verify_branch(&missing, "main").await.is_ok());
}

/// A directory that exists but is not a git work tree makes `rev-parse` exit
/// non-zero. That is the shape a 30s timeout also takes, and the gate decides
/// where the agent's next commits land, so an unanswered probe must refuse.
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
    // Proof 1 on its own. The tracked ref is deleted, so proof 2 has nothing to
    // say, and only the anchor's ancestry can carry the adoption.
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
        try_adopt_renegade_branch(&repo, &repo, "main", &IdleAnchor::Found(main_after.clone()))
            .await
            .is_none(),
        "renegade does not contain the anchor, and that veto is the whole test"
    );
}

#[tokio::test]
async fn adopt_without_an_anchor_takes_a_branch_created_off_the_tracked_one() {
    // The reported bug. The agent ran `git checkout -b feature-1234` mid-turn
    // and the engine restarted before the turn idled, so no anchor was ever
    // recorded. The tracked ref still leads into HEAD, which is proof enough.
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
    // `git branch -m` leaves no tracked ref to reach HEAD from, so the proof is
    // git's own reflog entry for the rename.
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
    // A sibling off the shared base does not contain the session's commit.
    // Adopting it would point the thread's Diff at work that was never ours.
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
    // Absence is not evidence of a rename. Checking out a sibling and deleting
    // the tracked branch leaves exactly the same absence as a rename does.
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
    // Nothing was switched, so the agent must not be told that it was. Without
    // this guard the anchor proof passes and we "adopt" the branch we are on.
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
    // A dropped Postgres connection must not read as "this thread never idled".
    // The shape below is the one that pays for it: with a real anchor the
    // adoption is a veto, and only an unanswered lookup would reach the weaker
    // proof and take the branch.
    let (_tmp, repo) = make_repo().await;
    let (_base, tracked) = session_on_tracked_branch(&repo).await;
    let _ = git_cmd(&["checkout", "-b", "feature-1234"], &repo).await;

    assert!(
        try_adopt_renegade_branch(&repo, &repo, &tracked, &IdleAnchor::Unknown)
            .await
            .is_none(),
        "an unanswered anchor lookup must never authorize retargeting the thread"
    );
    // The same worktree, with the lookup answering, is adopted. Without this
    // the test above would pass for the wrong reason.
    assert!(
        try_adopt_renegade_branch(&repo, &repo, &tracked, &IdleAnchor::Absent)
            .await
            .is_some(),
        "a verified absence still falls back to the continuation proof"
    );
}

#[tokio::test]
async fn adopt_refuses_a_stale_rename_that_no_longer_holds_the_anchor() {
    // A rename entry survives a later `git reset --hard`, so the reflog alone
    // says "this branch came from ours" about a tip that dropped our commits.
    // An anchor we have is therefore a veto, never something the rename proof
    // may override.
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
    // The call site routes `Detached` here too, so the refusal must hold here.
    // A detached HEAD names no branch, and "HEAD" is not one to adopt. The
    // branch read short-circuits before any proof runs, and the anchor below
    // would otherwise pass, so that read is the only thing refusing.
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
    // Reflogs off means no evidence, and no evidence means no adoption. The
    // spawn keeps refusing, which costs a resend rather than the user's work.
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

// -------------------- adoption at a base-tip tracked ref --------------------

#[tokio::test]
async fn adopt_refuses_a_sibling_when_the_tracked_ref_sits_at_the_base() {
    // The gap. A tracked branch with no commit of its own is reachable from
    // EVERY branch cut from the same base, so containment says nothing at all.
    // Only provenance separates this from the agent's own `git checkout -b`.
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

    // Without this the test could pass for the wrong reason. The dangerous
    // shape is precisely the one where containment still holds.
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
    // The same shape through the anchor arm. A thread that idled having
    // committed nothing recorded the base as its anchor, so that ancestry is
    // just as vacuous as the tracked ref's.
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
    // A session's first idle anchors on its start HEAD, which for a tracked
    // branch with no commit is the base. Both gates are then vacuous.
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
    // Reflog timestamps have one-second resolution, so a sibling created in the
    // same second as the move onto it matches on time. Both entries are pinned
    // to one instant here, which leaves the sha as the only discriminator: the
    // sibling was created at the base and has moved on, while our HEAD landed
    // on its tip.
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
    // The mirror of the test above. This sibling never moved from where it was
    // created, so its sha matches the one we land on and only the creation TIME
    // refuses it. `GIT_COMMITTER_DATE` backdates that entry, which is how a
    // genuinely older branch is expressible without sleeping through a second.
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
    // The case adoption exists to serve, in the same git state as the refusals
    // above. A ticket-branch convention runs `git checkout -b` BEFORE it
    // commits anything, so the tracked branch is legitimately still at the base.
    //
    // This is also the only shape where the repo root and the worktree are
    // different paths. Reading HEAD's reflog in the repo root would find no
    // move at all and refuse here.
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
    // `git switch -c` is the same operation under a newer name, and git writes
    // it into both reflogs identically.
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
async fn the_provenance_probe_refuses_when_git_cannot_answer() {
    // A directory that is not a work tree makes `git reflog` exit non-zero,
    // which is the shape a 30s timeout also takes. No answer, no proof.
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
