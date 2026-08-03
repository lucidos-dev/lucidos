//! Tri-state git answers ([`super::GitAnswer`]).
//!
//! The property under test is the one whose absence emptied a live worktree on
//! 2026-08-03: a git call the engine could not run to completion must come back
//! as `Unknown`, distinguishable from a git call that ran and said "no". The
//! timeout arm is driven with a nanosecond ceiling rather than the real
//! [`super::GIT_TIMEOUT`], so it is deterministic and instant.

use super::common::make_test_repo;
use super::*;
use std::time::Duration;

/// Build an `Output` with the given exit code and stdout, without running a
/// process. `ExitStatus::from_raw` takes a wait(2) status word on unix, where
/// the exit code lives in the high byte.
fn output(code: i32, stdout: &str) -> std::process::Output {
    use std::os::unix::process::ExitStatusExt;
    std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

fn classify(result: Result<std::process::Output, String>) -> GitAnswer {
    classify_git_answer(
        result,
        |_| true,
        GitAnswer::No,
        &["rev-parse"],
        std::path::Path::new("/"),
    )
}

#[test]
fn exit_zero_is_yes() {
    assert_eq!(classify(Ok(output(0, ""))), GitAnswer::Yes);
}

#[test]
fn non_zero_exit_is_no() {
    assert_eq!(classify(Ok(output(1, ""))), GitAnswer::No);
}

/// The load-bearing one. A timeout is not an answer.
#[test]
fn timeout_is_unknown_not_no() {
    let answer = classify(Err("git rev-parse timed out after 30s".to_string()));
    assert_eq!(
        answer,
        GitAnswer::Unknown,
        "a timed-out git call must be Unknown; reading it as No is what deletes live worktrees"
    );
    assert!(answer.is_unknown());
}

#[test]
fn spawn_failure_is_unknown_not_no() {
    assert_eq!(
        classify(Err(
            "git rev-parse failed: No such file or directory".to_string()
        )),
        GitAnswer::Unknown
    );
}

/// The predicate decides yes-vs-no only for a command that actually ran; it is
/// never consulted for `Unknown`, so an empty stdout from a dead process can
/// never read as "clean" / "no commits" / "nothing here".
#[test]
fn predicate_splits_only_successful_runs() {
    let dirty = |o: &std::process::Output| !o.stdout.is_empty();
    let dir = std::path::Path::new("/");
    let no = GitAnswer::No;
    assert_eq!(
        classify_git_answer(
            Ok(output(0, " M src/main.rs\n")),
            dirty,
            no,
            &["status"],
            dir
        ),
        GitAnswer::Yes
    );
    assert_eq!(
        classify_git_answer(Ok(output(0, "")), dirty, no, &["status"], dir),
        GitAnswer::No
    );
    assert_eq!(
        classify_git_answer(Err("timed out".to_string()), dirty, no, &["status"], dir),
        GitAnswer::Unknown
    );
}

/// For a command whose failure is not an answer, a non-zero exit is `Unknown`
/// too. `git status` exiting non-zero (not a work tree, or a filter blew up)
/// must never read as "clean, nothing to lose here".
#[test]
fn nonzero_exit_is_unknown_when_the_caller_says_failure_is_not_an_answer() {
    let dirty = |o: &std::process::Output| !o.stdout.is_empty();
    let dir = std::path::Path::new("/");
    assert_eq!(
        classify_git_answer(
            Ok(output(128, "")),
            dirty,
            GitAnswer::Unknown,
            &["status", "--porcelain"],
            dir
        ),
        GitAnswer::Unknown
    );
    // Same input, but for a question where a non-zero exit IS the answer.
    assert_eq!(
        classify_git_answer(
            Ok(output(128, "")),
            dirty,
            GitAnswer::No,
            &["rev-parse", "--verify", "gone"],
            dir
        ),
        GitAnswer::No
    );
}

/// `or_unknown` is the only way to collapse the tri-state, and it collapses to
/// whichever side the caller names, leaving Yes/No untouched.
#[test]
fn or_unknown_only_moves_the_unknown_arm() {
    for on_unknown in [true, false] {
        assert!(GitAnswer::Yes.or_unknown(on_unknown));
        assert!(!GitAnswer::No.or_unknown(on_unknown));
        assert_eq!(GitAnswer::Unknown.or_unknown(on_unknown), on_unknown);
    }
}

/// Ties the pure classifier to the real timeout path: an unmeetable ceiling
/// produces the `Err` that `classify_git_answer` maps to `Unknown`.
#[tokio::test]
async fn unmeetable_timeout_errors_and_classifies_unknown() {
    let (_tmp, repo) = make_test_repo().await;
    let result = git_cmd_env_timeout(
        &["rev-parse", "--verify", "main"],
        &repo,
        &[],
        Duration::from_nanos(1),
    )
    .await;
    let err = result.expect_err("a 1ns ceiling cannot be met by a process spawn");
    assert!(err.contains("timed out"), "unexpected error: {}", err);
    assert_eq!(classify(Err(err)), GitAnswer::Unknown);
}

/// A real (not synthetic) spawn failure: git cannot start in a directory that
/// does not exist, so `Command::output` errors before any exit status exists.
/// That must surface as `Unknown`, not as a `No` about the question asked.
#[tokio::test]
async fn real_spawn_failure_is_unknown() {
    let gone = std::path::Path::new("/definitely/not/a/directory/on/this/host");
    assert_eq!(
        git_answer(&["rev-parse", "--verify", "main"], gone).await,
        GitAnswer::Unknown
    );
}

/// Sanity: against a real repo the helper still answers ordinary questions.
#[tokio::test]
async fn git_answer_reads_a_real_repo() {
    let (_tmp, repo) = make_test_repo().await;
    assert_eq!(
        git_answer(&["rev-parse", "--verify", "main"], &repo).await,
        GitAnswer::Yes
    );
    assert_eq!(
        git_answer(&["rev-parse", "--verify", "no-such-branch"], &repo).await,
        GitAnswer::No
    );
}
