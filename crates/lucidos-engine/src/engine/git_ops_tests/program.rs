//! The resolved git binary ([`super::git_program`]).
//!
//! Two properties: it runs the same git bare `git` would, and a probe that
//! answers nothing usable falls back to bare `git` rather than breaking calls.

use super::*;

#[tokio::test]
async fn resolved_git_is_the_same_git_as_bare_git() {
    let resolved = std::process::Command::new(git_program().await)
        .arg("--version")
        .output()
        .expect("the resolved git runs");
    let bare = std::process::Command::new("git")
        .arg("--version")
        .output()
        .expect("bare git runs");
    assert!(resolved.status.success());
    assert_eq!(resolved.stdout, bare.stdout);
}

#[test]
fn a_directory_holding_git_resolves_to_that_git() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("git"), "").unwrap();
    let stdout = format!("{}\n", dir.path().display());
    assert_eq!(
        git_program_from_exec_path(Some(stdout.as_bytes())),
        dir.path().join("git")
    );
}

#[test]
fn an_unusable_probe_falls_back_to_bare_git() {
    let empty_dir = tempfile::tempdir().unwrap();
    let no_git = empty_dir.path().display().to_string();
    for stdout in [None, Some(&b""[..]), Some(b"  \n"), Some(no_git.as_bytes())] {
        assert_eq!(git_program_from_exec_path(stdout), PathBuf::from("git"));
    }
}

#[tokio::test]
async fn a_vanished_binary_falls_back_to_bare_git() {
    let gone = tempfile::tempdir().unwrap().path().join("git");
    let output = run_git(&gone, &["--version"], Path::new("."), &[], false)
        .await
        .expect("bare git runs in its place");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("git version"));
}

#[test]
fn only_the_macos_xcrun_shim_is_bypassed() {
    let usr_bin = Some(Path::new("/usr/bin/git"));
    assert!(is_xcrun_git_shim(usr_bin, true));
    assert!(
        !is_xcrun_git_shim(usr_bin, false),
        "Linux /usr/bin/git is no shim"
    );
    let wrapper = Some(Path::new("/home/user/bin/git"));
    assert!(
        !is_xcrun_git_shim(wrapper, true),
        "a PATH wrapper keeps running"
    );
    assert!(!is_xcrun_git_shim(None, true));
}

/// A read-only probe must not take `index.lock`. `git status` and `git diff`
/// refresh a stale index under that lock. A probe racing a `git add` or
/// `git commit` in the same worktree then makes the writer die.
#[tokio::test]
async fn read_only_probes_leave_a_stale_index_alone() {
    let (_tmp, repo) = common::make_test_repo().await;
    let file = repo.join("tracked.txt");
    std::fs::write(&file, "same\n").unwrap();
    common::git_ok(&repo, &["add", "tracked.txt"]).await;
    common::git_ok(&repo, &["commit", "-q", "-m", "track"]).await;
    // Same content, new mtime: only a refresh that rewrites the index notices.
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    let index = repo.join(".git/index");
    let before = std::fs::read(&index).unwrap();

    git_cmd(&["status", "--porcelain"], &repo).await.unwrap();
    assert_eq!(has_unmerged_paths(&repo).await, GitAnswer::No);

    assert_eq!(
        std::fs::read(&index).unwrap(),
        before,
        "the probe rewrote the index, so it took index.lock"
    );
}
