use super::common::make_test_repo;
use super::*;

#[tokio::test]
async fn detect_origin_returns_none_for_repo_without_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_path = tmp.path();

    let init = std::process::Command::new("git")
        .args(["init"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    assert!(init.status.success(), "git init failed");

    std::process::Command::new("git")
        .args(["commit", "--allow-empty", "-m", "init"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    let result = detect_origin_default_branch(repo_path).await;
    assert_eq!(
        result, None,
        "Repo without origin should return None, not a fallback ref"
    );
}

fn git_ok(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn clone_with_origin() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    git_ok(
        tmp.path(),
        &["init", "--bare", "--initial-branch=main", "origin.git"],
    );
    git_ok(tmp.path(), &["clone", "-q", "origin.git", "work"]);
    git_ok(&work, &["config", "user.email", "test@example.com"]);
    git_ok(&work, &["config", "user.name", "Test"]);
    git_ok(&work, &["commit", "-q", "--allow-empty", "-m", "base"]);
    git_ok(&work, &["branch", "-M", "main"]);
    git_ok(&work, &["push", "-q", "-u", "origin", "main"]);
    (tmp, work)
}

#[tokio::test]
async fn detect_origin_keeps_unpushed_commits_on_the_local_default_branch() {
    let (_tmp, work) = clone_with_origin();
    git_ok(&work, &["commit", "-q", "--allow-empty", "-m", "unpushed"]);
    let local_main = git_ok(&work, &["rev-parse", "refs/heads/main"]);
    git_ok(&work, &["checkout", "-q", "-b", "feature"]);

    assert_eq!(
        detect_origin_default_branch(&work).await.as_deref(),
        Some("origin/main")
    );
    assert_eq!(
        git_ok(&work, &["rev-parse", "refs/heads/main"]),
        local_main,
        "the unpushed commit must stay on local main"
    );
}

#[tokio::test]
async fn detect_origin_fast_forwards_a_local_default_branch_that_is_behind() {
    let (tmp, work) = clone_with_origin();
    git_ok(tmp.path(), &["clone", "-q", "origin.git", "other"]);
    let other = tmp.path().join("other");
    git_ok(&other, &["config", "user.email", "test@example.com"]);
    git_ok(&other, &["config", "user.name", "Test"]);
    git_ok(&other, &["commit", "-q", "--allow-empty", "-m", "newer"]);
    git_ok(&other, &["push", "-q", "origin", "main"]);
    let origin_main = git_ok(&other, &["rev-parse", "HEAD"]);
    git_ok(&work, &["checkout", "-q", "-b", "feature"]);

    detect_origin_default_branch(&work).await;
    assert_eq!(
        git_ok(&work, &["rev-parse", "refs/heads/main"]),
        origin_main
    );
}

#[tokio::test]
async fn worktree_creation_succeeds_for_repo_without_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_path = tmp.path();

    std::process::Command::new("git")
        .args(["init"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "--allow-empty", "-m", "init"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    let origin_default = detect_origin_default_branch(repo_path).await;
    assert_eq!(origin_default, None);

    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("test-worktree");
    let branch_name = "claude-code/test-branch";

    let mut wt_args = vec![
        "worktree",
        "add",
        wt_path.to_str().unwrap(),
        "-b",
        branch_name,
    ];
    if let Some(ref base_ref) = origin_default {
        wt_args.push(base_ref);
    }

    let result = git_cmd(&wt_args, repo_path).await;
    assert!(
        result.is_ok(),
        "git worktree add failed: {:?}",
        result.err()
    );
    let output = result.unwrap();
    assert!(
        output.status.success(),
        "git worktree add returned non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = git_cmd(
        &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
        repo_path,
    )
    .await;
}

#[tokio::test]
async fn worktree_add_works_without_git_crypt() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");

    let out = worktree_add(&repo, &wt_path, &["-b", "feature/test"])
        .await
        .expect("worktree_add returned Err");
    assert!(
        out.status.success(),
        "checkout step failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        wt_path.join("init.txt").exists(),
        "init.txt not checked out"
    );
    assert!(wt_path.join(".git").exists(), "worktree .git missing");
}

#[tokio::test]
async fn worktree_add_recovers_from_missing_but_registered_path() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("thread-stale");

    let out = worktree_add(&repo, &wt_path, &["-b", "claude-code/old"])
        .await
        .expect("first worktree_add returned Err");
    assert!(
        out.status.success(),
        "first add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    tokio::fs::remove_dir_all(&wt_path).await.unwrap();
    assert!(
        !wt_path.exists(),
        "precondition: worktree dir should be gone"
    );

    let out = worktree_add(&repo, &wt_path, &["-b", "claude-code/new"])
        .await
        .expect("second worktree_add returned Err");
    assert!(
        out.status.success(),
        "re-add over stale registration failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        wt_path.join("init.txt").exists(),
        "init.txt not checked out on re-add"
    );
    assert!(
        wt_path.join(".git").exists(),
        "worktree .git missing on re-add"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn worktree_add_links_git_crypt_dir_when_present() {
    let (_tmp, repo) = make_test_repo().await;

    let parent_gc = repo.join(".git/git-crypt");
    tokio::fs::create_dir_all(&parent_gc).await.unwrap();
    tokio::fs::write(parent_gc.join("keys"), b"stub")
        .await
        .unwrap();

    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");

    let out = worktree_add(&repo, &wt_path, &["-b", "feature/test"])
        .await
        .expect("worktree_add returned Err");
    assert!(
        out.status.success(),
        "checkout step failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let per_wt_git = git_cmd(&["rev-parse", "--absolute-git-dir"], &wt_path)
        .await
        .unwrap();
    let per_wt_git = std::path::PathBuf::from(
        String::from_utf8_lossy(&per_wt_git.stdout)
            .trim()
            .to_string(),
    );
    let link = per_wt_git.join("git-crypt");
    let meta = tokio::fs::symlink_metadata(&link)
        .await
        .expect("git-crypt symlink missing in per-worktree git dir");
    assert!(
        meta.file_type().is_symlink(),
        "git-crypt entry is not a symlink"
    );

    let resolved = std::fs::canonicalize(tokio::fs::read_link(&link).await.unwrap()).unwrap();
    let expected = std::fs::canonicalize(&parent_gc).unwrap();
    assert_eq!(
        resolved, expected,
        "symlink does not point at parent git-crypt"
    );
}

#[tokio::test]
async fn commit_plus_revert_branch_has_no_changed_files() {
    let (_tmp, repo) = make_test_repo().await;

    let o = git_cmd(&["checkout", "-b", "feature"], &repo)
        .await
        .unwrap();
    assert!(o.status.success(), "checkout -b feature failed");
    tokio::fs::write(repo.join("new.txt"), "content")
        .await
        .unwrap();
    let o = git_cmd(&["add", "."], &repo).await.unwrap();
    assert!(o.status.success(), "git add failed");
    let o = git_cmd(&["commit", "-m", "add file"], &repo).await.unwrap();
    assert!(o.status.success(), "git commit failed");
    let o = git_cmd(&["revert", "--no-edit", "HEAD"], &repo)
        .await
        .unwrap();
    assert!(o.status.success(), "git revert failed");

    assert!(
        has_branch_commits(&repo, "feature").await,
        "Branch should have commits even after revert"
    );

    let files = branch_changed_files(&repo, "feature").await;
    assert!(
        files.is_empty(),
        "Branch with commit+revert should have no changed files, got: {:?}",
        files
    );
}

#[tokio::test]
async fn branch_changed_files_uses_origin_base_when_local_default_diverged() {
    let origin_tmp = tempfile::tempdir().unwrap();
    let origin = origin_tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "-q", "--bare", "-b", "main"], &origin).await;

    let (_tmp, repo) = make_test_repo().await;
    let c_root =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "HEAD"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();

    tokio::fs::write(repo.join("fork-point.txt"), "shipped before the branch\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(
        &["commit", "-m", "feature on main (branch forks here)"],
        &repo,
    )
    .await;
    let _ = git_cmd(
        &["remote", "add", "origin", origin.to_str().unwrap()],
        &repo,
    )
    .await;
    let _ = git_cmd(&["push", "-q", "origin", "main"], &repo).await;
    let _ = git_cmd(&["remote", "set-head", "origin", "main"], &repo).await;

    let _ = git_cmd(&["branch", "feature", "main"], &repo).await;

    let _ = git_cmd(&["reset", "-q", "--hard", &c_root], &repo).await;
    tokio::fs::write(repo.join("MIGRATION.md"), "moved to new-org\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(
        &["commit", "-m", "migration: secrets transfer (automated)"],
        &repo,
    )
    .await;

    let files = branch_changed_files(&repo, "feature").await;
    assert!(
        files.is_empty(),
        "branch_changed_files must diff against origin/main when local main diverged, got: {:?}",
        files
    );
    assert_eq!(
        proposal_files_for_branch(&repo, "feature").await,
        None,
        "no net diff against the branch's true fork point must not warrant a Change proposal"
    );
}

#[tokio::test]
async fn proposal_files_for_branch_rejects_commit_plus_revert() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo)
        .await
        .unwrap();
    tokio::fs::write(repo.join("new.txt"), "x").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "add file"], &repo).await.unwrap();
    let _ = git_cmd(&["revert", "--no-edit", "HEAD"], &repo)
        .await
        .unwrap();

    assert_eq!(
        proposal_files_for_branch(&repo, "feature").await,
        None,
        "branch with commit+revert (zero net diff) must not warrant a Change proposal"
    );
}

#[tokio::test]
async fn proposal_files_for_branch_returns_files_for_real_changes() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo)
        .await
        .unwrap();
    tokio::fs::write(repo.join("new.txt"), "x").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "add file"], &repo).await.unwrap();

    assert_eq!(
        proposal_files_for_branch(&repo, "feature").await,
        Some(vec!["new.txt".to_string()]),
        "branch with real changes must yield the changed file list"
    );
}

#[tokio::test]
async fn proposal_files_for_branch_rejects_no_commits() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["branch", "feature"], &repo).await.unwrap();

    assert_eq!(
        proposal_files_for_branch(&repo, "feature").await,
        None,
        "branch with no commits ahead of main must not warrant a Change proposal"
    );
}

#[tokio::test]
async fn proposal_files_for_branch_rejects_already_applied_after_criss_cross_back_merge() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "claude-code/feature"], &repo).await;
    tokio::fs::write(repo.join("work.rs"), "fn work() {}")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "branch authored work"], &repo).await;

    let _ = git_cmd(&["checkout", "main"], &repo).await;
    tokio::fs::write(repo.join("other.rs"), "fn other() {}")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "main independent work"], &repo).await;
    let pre_apply_main =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "HEAD"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();

    let _ = git_cmd(
        &[
            "merge",
            "--no-ff",
            "-m",
            "apply: merge feature into main",
            "claude-code/feature",
        ],
        &repo,
    )
    .await;

    let _ = git_cmd(&["checkout", "claude-code/feature"], &repo).await;
    let _ = git_cmd(
        &[
            "merge",
            "--no-ff",
            "-m",
            "Merge branch 'main' into claude-code/feature",
            &pre_apply_main,
        ],
        &repo,
    )
    .await;

    assert!(
        has_branch_commits(&repo, "claude-code/feature").await,
        "precondition: the back-merge leaves a merge commit ahead of main"
    );
    assert!(
        !branch_changed_files(&repo, "claude-code/feature")
            .await
            .is_empty(),
        "precondition: criss-cross regresses the 3-dot base, re-surfacing the applied file"
    );

    assert_eq!(
        proposal_files_for_branch(&repo, "claude-code/feature").await,
        None,
        "already-applied branch with only a back-merge commit must not be re-proposed"
    );
}

#[test]
fn files_have_client_update_detects_bundle_inputs() {
    for f in [
        "crates/lucidos-app/src/App.tsx",
        "crates/lucidos-app/src/styles/global/base.css",
        "crates/lucidos-app/public/sw.js",
        "crates/lucidos-app/vite/gatewaySession.ts",
        "crates/lucidos-app/index.html",
        "crates/lucidos-app/vite.config.ts",
        "crates/lucidos-app/package.json",
        "packages/lucidos-sdk/src/index.ts",
        "package.json",
        "package-lock.json",
    ] {
        assert!(
            files_have_client_update(&[f.into()]),
            "{f} feeds the bundle"
        );
    }
}

#[test]
fn files_have_client_update_ignores_files_outside_the_bundle() {
    for f in [
        "crates/lucidos-app/e2e/transcript-window-fills-the-pane.spec.ts",
        "crates/lucidos-app/e2e/fixture.html",
        "crates/lucidos-app/dev-build-watch.mjs",
        "crates/lucidos-agents-api/src/api/sdk_iframe_audio.js",
        "packages/lucidos-sdk/tests/preferences.test.ts",
        "scripts/lib/e2e.ts",
        "crates/lucidos-engine/src/main.rs",
        "Cargo.toml",
        "README.md",
    ] {
        assert!(
            !files_have_client_update(&[f.into()]),
            "{f} is not a bundle input"
        );
    }
    assert!(!files_have_client_update(&[]));
}

#[test]
fn files_have_client_update_matches_the_build_watch_inputs() {
    let watcher = include_str!("../../../../lucidos-app/dev-build-watch.mjs");
    let mut watched: Vec<String> = watcher
        .lines()
        .filter_map(|l| l.trim().strip_prefix("resolve("))
        .filter_map(|l| Some((l.split(',').next()?, l.split('\'').nth(1)?)))
        .map(|(base, rel)| match (base, rel.strip_prefix("../../")) {
            ("PROJECT_DIR", _) => rel.to_string(),
            ("APP_DIR", Some(up)) => up.to_string(),
            ("APP_DIR", None) => format!("crates/lucidos-app/{rel}"),
            _ => panic!("unknown base {base} in dev-build-watch.mjs"),
        })
        .collect();
    let mut classified: Vec<String> = restart_detection::CLIENT_BUNDLE_DIRS
        .iter()
        .map(|dir| dir.trim_end_matches('/'))
        .chain(restart_detection::CLIENT_BUNDLE_FILES.iter().copied())
        .map(str::to_string)
        .collect();
    watched.sort();
    classified.sort();
    assert_eq!(
        watched, classified,
        "dev-build-watch.mjs and files_have_client_update disagree"
    );
}

#[tokio::test]
async fn default_local_branch_reads_origin_head_for_non_main_repos() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();

    let remote_tmp = tempfile::tempdir().unwrap();
    let remote = remote_tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "--bare", "-b", "develop"], &remote)
        .await
        .unwrap();

    let _ = git_cmd(&["init", "-b", "develop"], &repo).await.unwrap();
    let _ = git_cmd(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        &repo,
    )
    .await
    .unwrap();
    tokio::fs::write(repo.join("init.txt"), "x").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "initial"], &repo).await.unwrap();
    let _ = git_cmd(&["push", "-u", "origin", "develop"], &repo)
        .await
        .unwrap();
    let o = git_cmd(&["remote", "set-head", "origin", "-a"], &repo)
        .await
        .unwrap();
    assert!(
        o.status.success(),
        "remote set-head -a failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    assert_eq!(
        default_local_branch(&repo).await,
        "develop",
        "default_local_branch must follow origin/HEAD, not assume main/master"
    );
}

#[tokio::test]
async fn default_local_branch_falls_back_to_main_without_origin() {
    let (_tmp, repo) = make_test_repo().await;
    assert_eq!(default_local_branch(&repo).await, "main");
}

#[tokio::test]
async fn default_local_branch_returns_cached_value_within_ttl() {
    let (_tmp, repo) = make_test_repo().await;
    assert_eq!(default_local_branch(&repo).await, "main");

    let o = git_cmd(&["branch", "-m", "main", "renamed-main"], &repo)
        .await
        .unwrap();
    assert!(
        o.status.success(),
        "branch rename failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    assert_eq!(
        default_local_branch(&repo).await,
        "main",
        "second call within TTL must return cached value, not re-resolve"
    );
}

#[tokio::test]
async fn default_local_branch_cache_separates_per_repo_root() {
    let (_tmp_main, repo_main) = make_test_repo().await;

    let tmp_master = tempfile::tempdir().unwrap();
    let repo_master = tmp_master.path().to_path_buf();
    let _ = git_cmd(&["init"], &repo_master).await.unwrap();
    let _ = git_cmd(&["checkout", "-b", "master"], &repo_master)
        .await
        .unwrap();
    tokio::fs::write(repo_master.join("init.txt"), "x")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_master).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "initial"], &repo_master)
        .await
        .unwrap();

    assert_eq!(default_local_branch(&repo_main).await, "main");
    assert_eq!(default_local_branch(&repo_master).await, "master");
    assert_eq!(
        default_local_branch(&repo_main).await,
        "main",
        "main repo cache must not be polluted by master repo lookup"
    );
}

#[tokio::test]
async fn default_diff_base_falls_back_to_origin_when_local_default_branch_missing() {
    let remote_tmp = tempfile::tempdir().unwrap();
    let remote = remote_tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "-q", "--bare", "-b", "develop"], &remote).await;

    let seed_tmp = tempfile::tempdir().unwrap();
    let seed = seed_tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "-q", "-b", "develop"], &seed).await;
    tokio::fs::write(seed.join("base.txt"), "base\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &seed).await;
    let _ = git_cmd(&["commit", "-q", "-m", "base"], &seed).await;
    let _ = git_cmd(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        &seed,
    )
    .await;
    let _ = git_cmd(&["push", "-q", "origin", "develop"], &seed).await;

    let repo_tmp = tempfile::tempdir().unwrap();
    let repo = repo_tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "-q"], &repo).await;
    let _ = git_cmd(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        &repo,
    )
    .await;
    let _ = git_cmd(&["fetch", "-q", "origin"], &repo).await;
    let o = git_cmd(&["remote", "set-head", "origin", "-a"], &repo)
        .await
        .unwrap();
    assert!(
        o.status.success(),
        "remote set-head -a failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    let branch = "claude-code/20260701-083109-27b2c5";
    let _ = git_cmd(&["checkout", "-q", "-b", branch, "origin/develop"], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "feature\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-q", "-m", "cc work"], &repo).await;

    for phantom in ["develop", "main", "master"] {
        assert!(
            !git_cmd(&["rev-parse", "--verify", "--quiet", phantom], &repo)
                .await
                .unwrap()
                .status
                .success(),
            "precondition: local `{phantom}` must not exist"
        );
    }

    let base = default_diff_base(&repo).await;
    assert_eq!(
        base, "origin/develop",
        "must fall back to origin/<default> when the local default branch is absent, got `{base}`"
    );

    let range = format!("{base}...{branch}");
    let diff = git_cmd(&["diff", &range, "--no-color"], &repo)
        .await
        .unwrap();
    assert!(
        diff.status.success(),
        "diff range `{range}` must resolve, got: {}",
        String::from_utf8_lossy(&diff.stderr)
    );
    assert!(
        String::from_utf8_lossy(&diff.stdout).contains("feature.txt"),
        "diff must show the branch's authored file"
    );
}

#[tokio::test]
async fn default_diff_base_falls_back_to_primary_worktree_head_when_no_default_resolves() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "-q", "-b", "trunk"], &repo).await;
    tokio::fs::write(repo.join("base.txt"), "base\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-q", "-m", "base"], &repo).await;

    let branch = "claude-code/20260701-090000-abcdef";
    let _ = git_cmd(&["checkout", "-q", "-b", branch], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "feature\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-q", "-m", "cc work"], &repo).await;
    let _ = git_cmd(&["checkout", "-q", "trunk"], &repo).await;

    let base = default_diff_base(&repo).await;
    assert!(
        git_cmd(&["rev-parse", "--verify", "--quiet", &base], &repo)
            .await
            .unwrap()
            .status
            .success(),
        "default_diff_base must return a ref that resolves, got `{base}`"
    );

    let range = format!("{base}...{branch}");
    let diff = git_cmd(&["diff", &range, "--no-color"], &repo)
        .await
        .unwrap();
    assert!(
        diff.status.success(),
        "diff range `{range}` must resolve, got: {}",
        String::from_utf8_lossy(&diff.stderr)
    );
    assert!(
        String::from_utf8_lossy(&diff.stdout).contains("feature.txt"),
        "diff must show the branch's authored file"
    );
}

#[tokio::test]
async fn default_diff_base_in_linked_worktree_never_uses_own_head() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "-q", "-b", "trunk"], &repo).await;
    tokio::fs::write(repo.join("base.txt"), "base\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-q", "-m", "base"], &repo).await;

    let branch = "claude-code/20260701-091500-fedcba";
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    let out = git_cmd(
        &[
            "worktree",
            "add",
            "-b",
            branch,
            wt_path.to_str().unwrap(),
            "trunk",
        ],
        &repo,
    )
    .await
    .unwrap();
    assert!(
        out.status.success(),
        "worktree add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    tokio::fs::write(wt_path.join("feature.txt"), "feature\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt_path).await;
    let _ = git_cmd(&["commit", "-q", "-m", "cc work"], &wt_path).await;

    let base = default_diff_base(&wt_path).await;
    assert_ne!(
        base, "HEAD",
        "base must not be the worktree's own HEAD (would diff the branch against itself)"
    );

    let range = format!("{base}...HEAD");
    let diff = git_cmd(&["diff", &range, "--no-color"], &wt_path)
        .await
        .unwrap();
    assert!(
        diff.status.success(),
        "diff range `{range}` must resolve, got: {}",
        String::from_utf8_lossy(&diff.stderr)
    );
    assert!(
        String::from_utf8_lossy(&diff.stdout).contains("feature.txt"),
        "the linked worktree's authored file must appear in the diff — an empty \
         diff means the base collapsed onto the branch's own HEAD"
    );
}

#[test]
fn files_have_client_update_mixed_files() {
    assert!(files_have_client_update(&[
        "crates/lucidos-engine/src/main.rs".into(),
        "crates/lucidos-app/src/App.tsx".into()
    ]));
}

#[test]
fn files_require_restart_detects_rust_and_migrations() {
    assert!(files_require_restart(&[
        "crates/lucidos-engine/src/main.rs".into()
    ]));
    assert!(files_require_restart(&[
        "crates/lucidos-core/src/core/widget.json".into()
    ]));
    assert!(files_require_restart(&["Cargo.toml".into()]));
    assert!(files_require_restart(&["Cargo.lock".into()]));
    assert!(files_require_restart(&[
        "crates/lucidos-core/migrations/001.sql".into()
    ]));
}

#[test]
fn files_require_restart_for_engine_binary_inputs() {
    assert!(files_require_restart(&[
        "crates/lucidos-engine-bin/assets/identity.json".into()
    ]));
    assert!(!files_require_restart(&[
        "crates/lucidos-engine-bin/tests/build_id.rs".into()
    ]));
}

#[test]
fn files_require_restart_for_the_desktop_apps_compile_inputs() {
    for input in [
        "crates/lucidos-app/tauri.conf.json",
        "crates/lucidos-app/Info.plist",
        "crates/lucidos-app/capabilities/default.json",
        "crates/lucidos-app/permissions/app-ipc.json",
        "crates/lucidos-app/icons/icon.icns",
    ] {
        assert!(files_require_restart(&[input.into()]), "{input}");
    }
    assert!(!files_require_restart(&[
        "crates/lucidos-app/Entitlements.plist".into()
    ]));
}

#[test]
fn files_require_restart_ignores_tests_and_docs() {
    assert!(!files_require_restart(&[
        "crates/lucidos-e2e/tests/api.rs".into()
    ]));
    assert!(!files_require_restart(&["README.md".into()]));
    assert!(!files_require_restart(&[
        "crates/lucidos-app/src/App.tsx".into()
    ]));
    assert!(!files_require_restart(&[
        "crates/lucidos-app/src/global.css".into()
    ]));
    assert!(!files_require_restart(&[
        "packages/lucidos-sdk/README.md".into()
    ]));
    assert!(!files_require_restart(&[
        "packages/lucidos-sdk/tests/preferences.test.ts".into()
    ]));
    assert!(!files_require_restart(&[]));
}

#[test]
fn files_require_restart_for_sdk_bundle_sources() {
    assert!(files_require_restart(&[
        "packages/lucidos-sdk/src/preferences.ts".into()
    ]));
    assert!(files_require_restart(&[
        "packages/lucidos-sdk/src/index.ts".into()
    ]));
    assert!(files_require_restart(&[
        "packages/lucidos-sdk/build.mjs".into()
    ]));
    assert!(files_require_restart(&[
        "packages/lucidos-sdk/tsconfig.json".into()
    ]));
}

#[test]
fn files_require_restart_for_engine_bundled_iframe_assets() {
    assert!(files_require_restart(&[
        "crates/lucidos-agents-api/src/api/sdk_iframe.css".into()
    ]));
    assert!(files_require_restart(&[
        "crates/lucidos-agents-api/src/api/sdk_iframe_audio.js".into()
    ]));
}

#[test]
fn files_require_restart_for_the_engine_bundled_fonts() {
    assert!(files_require_restart(&[
        "crates/lucidos-app/src/assets/fonts/FiraCode-VF.woff2".into()
    ]));
    assert!(files_require_restart(&[
        "crates/lucidos-app/src/assets/fonts/Geist-latin.woff2".into()
    ]));
    assert!(!files_require_restart(&[
        "crates/lucidos-app/src/assets/fonts/LICENSE-FiraCode.txt".into()
    ]));
}

fn production_source(source: &str) -> String {
    let mut production = String::new();
    let mut rest = source;
    while let Some(at) = rest.find("\n#[cfg(test)]\n") {
        let (before, from) = rest.split_at(at);
        let item = from
            .lines()
            .skip(2)
            .find(|line| !line.starts_with("#["))
            .unwrap_or("");
        let names_a_module = item.starts_with("mod ") || item.starts_with("pub mod ");
        let body_end = from.find("\n}\n").map(|end| end + "\n}\n".len());
        match body_end {
            Some(end) if names_a_module && item.trim_end().ends_with('{') => {
                production.push_str(before);
                rest = from.split_at(end).1;
            }
            _ => {
                let (kept, after) = from.split_at("\n#[cfg(test)]\n".len());
                production.push_str(before);
                production.push_str(kept);
                rest = after;
            }
        }
    }
    production.push_str(rest);
    production
}

#[test]
fn production_source_drops_test_bodies_and_keeps_what_follows() {
    let source = "#[cfg(test)]\nmod a;\n\n#[cfg(test)]\nmod b;\nconst X: &str = \"x\";\n\
                  \n#[cfg(test)]\n#[allow(dead_code)]\nmod tests {\n    fn t() {}\n}\n\
                  const Y: &str = \"y\";\n\n#[cfg(test)]\nmod more {\n    fn u() {}\n}\n";
    let production = production_source(source);
    assert!(production.contains("mod b;"));
    assert!(production.contains("const X"));
    assert!(production.contains("const Y"));
    assert!(!production.contains("fn t()"));
    assert!(!production.contains("fn u()"));
}

#[test]
fn files_require_restart_for_embedded_data_and_the_iframe_stylesheet() {
    assert!(files_require_restart(&[
        "crates/lucidos-core/src/core/themes/builtin/nord.json".into()
    ]));
    assert!(files_require_restart(&[
        "crates/lucidos-app/src/styles/global/shared-components.css".into()
    ]));
    assert!(files_require_restart(&[
        "crates/lucidos-gateway/Cargo.toml".into()
    ]));
    assert!(!files_require_restart(&[
        "crates/lucidos-engine/src/core/themes/README.md".into()
    ]));
}

#[test]
fn files_require_restart_for_the_engine_bundled_changelog() {
    assert!(files_require_restart(&["CHANGELOG.md".into()]));
    assert!(!files_require_restart(&["docs/glossary.md".into()]));
}

#[test]
fn files_require_restart_for_the_engine_bundled_release_notices() {
    assert!(files_require_restart(&["release-notices.toml".into()]));
    assert!(!files_require_restart(&["rust-toolchain.toml".into()]));
}

#[test]
fn files_require_restart_for_the_gateway_bundled_app_document() {
    assert!(files_require_restart(&[
        "crates/lucidos-app/index.html".into()
    ]));
    assert!(!files_require_restart(&[
        "crates/lucidos-app/e2e/fixture.html".into()
    ]));
}

#[tokio::test]
async fn worktree_add_creates_branch_without_upstream_tracking() {
    let (_tmp, repo) = make_test_repo().await;
    let o = git_cmd(&["config", "branch.autoSetupMerge", "always"], &repo)
        .await
        .unwrap();
    assert!(o.status.success(), "set autoSetupMerge failed");

    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    let out = worktree_add(&repo, &wt_path, &["-b", "claude-code/no-track", "main"])
        .await
        .expect("worktree_add returned Err");
    assert!(
        out.status.success(),
        "worktree_add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cfg = git_cmd(
        &["config", "--get-regexp", r"^branch\.claude-code/no-track\."],
        &repo,
    )
    .await
    .unwrap();
    assert!(
        !cfg.status.success() && String::from_utf8_lossy(&cfg.stdout).trim().is_empty(),
        "branch must have no upstream tracking config, got: {}",
        String::from_utf8_lossy(&cfg.stdout)
    );
}

#[tokio::test]
async fn worktree_add_succeeds_while_config_lock_is_held() {
    let (_tmp, repo) = make_test_repo().await;
    let o = git_cmd(&["config", "branch.autoSetupMerge", "always"], &repo)
        .await
        .unwrap();
    assert!(o.status.success(), "set autoSetupMerge failed");

    let lock = repo.join(".git/config.lock");
    tokio::fs::write(&lock, b"").await.unwrap();

    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    let result = worktree_add(&repo, &wt_path, &["-b", "claude-code/locked", "main"]).await;

    let _ = tokio::fs::remove_file(&lock).await;

    let out = result.expect("worktree_add returned Err");
    assert!(
        out.status.success(),
        "worktree_add must not need the shared config lock, got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        wt_path.join("init.txt").exists(),
        "worktree not checked out"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_add_waits_for_a_prune_holding_the_admin_lock() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");

    let guard = super::worktree::WORKTREE_ADMIN_MUTEX.lock().await;
    let add = tokio::spawn(async move {
        worktree_add(&repo, &wt_path, &["-b", "claude-code/serialised", "main"]).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let ran_anyway = add.is_finished();

    drop(guard);
    let out = add
        .await
        .expect("add task panicked")
        .expect("worktree_add returned Err");

    assert!(
        !ran_anyway,
        "worktree add ran while the admin lock was held, so a concurrent prune can still delete its half-built admin dir"
    );
    assert!(
        out.status.success(),
        "worktree_add failed once the lock was free: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn ref_facts(refs: &[(&str, &str, &str)]) -> RefFacts {
    let stdout: String = refs
        .iter()
        .map(|(name, sha, symref)| format!("{name}\0{sha}\0{symref}\n"))
        .collect();
    RefFacts::parse(&stdout)
}

#[test]
fn diff_base_skips_the_ancestry_check_when_local_and_origin_agree() {
    let facts = ref_facts(&[
        ("refs/heads/main", "aaa", ""),
        (
            "refs/remotes/origin/HEAD",
            "aaa",
            "refs/remotes/origin/main",
        ),
        ("refs/remotes/origin/main", "aaa", ""),
    ]);
    assert_eq!(diff_base_from(&facts), DiffBase::Settled("main".into()));
}

#[test]
fn diff_base_asks_about_ancestry_when_local_and_origin_differ() {
    let facts = ref_facts(&[
        ("refs/heads/main", "aaa", ""),
        ("refs/remotes/origin/main", "bbb", ""),
    ]);
    assert_eq!(
        diff_base_from(&facts),
        DiffBase::LocalUnlessDiverged {
            local: "main".into(),
            remote_tracking: "origin/main".into(),
        }
    );
}

#[test]
fn diff_base_follows_origin_head_to_a_local_default() {
    let facts = ref_facts(&[
        ("refs/heads/develop", "aaa", ""),
        ("refs/heads/main", "ccc", ""),
        (
            "refs/remotes/origin/HEAD",
            "aaa",
            "refs/remotes/origin/develop",
        ),
    ]);
    assert_eq!(diff_base_from(&facts), DiffBase::Settled("develop".into()));
}

#[test]
fn diff_base_uses_master_when_there_is_no_main() {
    let facts = ref_facts(&[("refs/heads/master", "aaa", "")]);
    assert_eq!(diff_base_from(&facts), DiffBase::Settled("master".into()));
}

#[test]
fn diff_base_falls_back_to_origin_head_when_no_local_default_exists() {
    let facts = ref_facts(&[
        ("refs/heads/lucidos-thread-branch", "ddd", ""),
        (
            "refs/remotes/origin/HEAD",
            "aaa",
            "refs/remotes/origin/develop",
        ),
        ("refs/remotes/origin/develop", "aaa", ""),
    ]);
    assert_eq!(
        diff_base_from(&facts),
        DiffBase::Settled("origin/develop".into())
    );
}

#[test]
fn diff_base_uses_origin_main_when_local_main_is_missing() {
    let facts = ref_facts(&[("refs/remotes/origin/main", "aaa", "")]);
    assert_eq!(
        diff_base_from(&facts),
        DiffBase::Settled("origin/main".into())
    );
}

#[test]
fn diff_base_with_no_ref_facts_asks_the_primary_worktree() {
    assert_eq!(
        diff_base_from(&RefFacts::default()),
        DiffBase::PrimaryWorktreeHead
    );
}

#[tokio::test]
async fn ref_facts_read_parses_real_for_each_ref_output() {
    let (_tmp, repo) = make_test_repo().await;
    let facts = RefFacts::read(&repo).await;
    assert_eq!(diff_base_from(&facts), DiffBase::Settled("main".into()));
}

#[tokio::test]
async fn a_dangling_origin_head_names_no_default() {
    let (_tmp, repo) = make_test_repo().await;
    for args in [
        &["branch", "develop"][..],
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/develop",
        ],
    ] {
        let o = git_cmd(args, &repo).await.unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    assert_eq!(default_local_branch(&repo).await, "main");
    assert_eq!(default_diff_base(&repo).await, "main");
}

mod guard_tests {
    use super::*;

    #[test]
    fn a_file_a_binary_embeds_requires_a_restart() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        const NOT_IN_A_RESTARTED_BINARY: &[&str] = &["lucidos-app", "lucidos-e2e"];
        let include = regex::Regex::new(r#"include_(?:str|bytes)!\(\s*"([^"]+)"\s*\)"#).unwrap();
        let mut embedded = Vec::new();
        for crate_dir in std::fs::read_dir(root.join("crates")).unwrap() {
            let crate_dir = crate_dir.unwrap().path();
            let name = crate_dir.file_name().unwrap().to_string_lossy().to_string();
            if NOT_IN_A_RESTARTED_BINARY.contains(&name.as_str()) {
                continue;
            }
            let mut dirs = vec![crate_dir.join("src")];
            while let Some(dir) = dirs.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries {
                    let path = entry.unwrap().path();
                    let file_name = path.file_name().unwrap().to_string_lossy().to_string();
                    if file_name.ends_with("tests") || file_name.ends_with("tests.rs") {
                        continue;
                    }
                    if path.is_dir() {
                        dirs.push(path);
                        continue;
                    }
                    if !file_name.ends_with(".rs") {
                        continue;
                    }
                    let source = std::fs::read_to_string(&path).unwrap();
                    for literal in include.captures_iter(&production_source(&source)) {
                        let target = path.parent().unwrap().join(&literal[1]);
                        let target = target.canonicalize().unwrap_or_else(|e| {
                            panic!("{} embeds {target:?}: {e}", path.display())
                        });
                        let relative = target.strip_prefix(&root).unwrap();
                        embedded.push(relative.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }
        assert!(
            embedded.len() > 20,
            "the scan found only {embedded:?}, so it no longer reads the sources"
        );
        let missed: Vec<&String> = embedded
            .iter()
            .filter(|f| !files_require_restart(&[f.to_string()]))
            .collect();
        assert!(
            missed.is_empty(),
            "a binary embeds these, but an Apply that edits one offers no restart: {missed:?}. \
         Add each to EMBEDDED_FILES in restart_detection.rs."
        );
        let generated: Vec<&String> = embedded
            .iter()
            .filter(|f| {
                crate::codegen_inputs::GENERATED_DIRS
                    .iter()
                    .any(|dir| f.starts_with(&format!("{dir}/")))
            })
            .collect();
        assert!(
            generated.is_empty(),
            "a binary embeds these generated files: {generated:?}"
        );
    }
}
