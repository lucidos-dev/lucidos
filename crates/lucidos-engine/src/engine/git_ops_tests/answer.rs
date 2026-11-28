use super::common::make_test_repo;
use super::*;
use std::time::Duration;

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

#[test]
fn or_unknown_only_moves_the_unknown_arm() {
    for on_unknown in [true, false] {
        assert!(GitAnswer::Yes.or_unknown(on_unknown));
        assert!(!GitAnswer::No.or_unknown(on_unknown));
        assert_eq!(GitAnswer::Unknown.or_unknown(on_unknown), on_unknown);
    }
}

#[tokio::test]
async fn unmeetable_timeout_errors_and_classifies_unknown() {
    let (_tmp, repo) = make_test_repo().await;
    let result = git_cmd_env_timeout(
        &["-c", "alias.hang=!sleep 5", "hang"],
        &repo,
        &[],
        Duration::from_millis(10),
    )
    .await;
    let err = result.expect_err("a 10ms ceiling cannot be met by a 5s sleep");
    assert!(err.contains("timed out"), "unexpected error: {}", err);
    assert_eq!(classify(Err(err)), GitAnswer::Unknown);
}

#[tokio::test]
async fn real_spawn_failure_is_unknown() {
    let gone = std::path::Path::new("/definitely/not/a/directory/on/this/host");
    assert_eq!(
        git_answer(&["rev-parse", "--verify", "main"], gone).await,
        GitAnswer::Unknown
    );
}

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
