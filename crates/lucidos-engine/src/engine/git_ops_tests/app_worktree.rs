use super::common::make_test_repo;
use super::*;

fn app_branch(app_id: &str) -> String {
    coding_agent_branch_base(
        crate::runtime::CodingAgent::ClaudeCode,
        &BranchScope::App(app_id.to_string()),
        "add-streaks",
        uuid::Uuid::new_v4(),
    )
}

#[test]
fn any_iframe_bundled_file_changed_picks_up_app_files() {
    let files: Vec<String> = vec![
        "data/apps/habit-tracker/index.html".into(),
        "data/apps/habit-tracker/styles.css".into(),
        "data/apps/habit-tracker/knowhow/intent.md".into(),
        "data/apps/other/index.html".into(),
        "scripts/build.sh".into(),
    ];
    assert!(any_iframe_bundled_file_changed(&files, "habit-tracker"));
    let files: Vec<String> = vec!["data/apps/habit-tracker/knowhow/intent.md".into()];
    assert!(!any_iframe_bundled_file_changed(&files, "habit-tracker"));
    let files: Vec<String> = vec!["data/apps/other/index.html".into()];
    assert!(!any_iframe_bundled_file_changed(&files, "habit-tracker"));
}

#[test]
fn any_iframe_bundled_file_changed_recognises_static_assets() {
    let files: Vec<String> = vec!["data/apps/habit-tracker/assets/logo.svg".into()];
    assert!(any_iframe_bundled_file_changed(&files, "habit-tracker"));
    let files: Vec<String> = vec!["data/apps/habit-tracker/fonts/x.woff2".into()];
    assert!(any_iframe_bundled_file_changed(&files, "habit-tracker"));
}

#[test]
fn any_iframe_bundled_file_changed_recognises_manifest() {
    let files: Vec<String> = vec!["data/apps/habit-tracker/manifest.json".into()];
    assert!(any_iframe_bundled_file_changed(&files, "habit-tracker"));
}

#[tokio::test]
async fn create_sparse_app_worktree_materialises_only_app_folder() {
    let (_tmp, ws) = make_test_repo().await;
    tokio::fs::create_dir_all(ws.join("data/apps/habit-tracker"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(ws.join("data/apps/other"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(ws.join("data/knowhow"))
        .await
        .unwrap();
    tokio::fs::write(ws.join("data/apps/habit-tracker/index.html"), "<h1>m</h1>")
        .await
        .unwrap();
    tokio::fs::write(ws.join("data/apps/other/index.html"), "<h1>o</h1>")
        .await
        .unwrap();
    tokio::fs::write(ws.join("data/knowhow/x.md"), "k")
        .await
        .unwrap();
    tokio::fs::write(ws.join(".gitignore"), ".lucidos/\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &ws).await;
    let _ = git_cmd(&["commit", "-m", "scaffold"], &ws).await;

    let wt_tmp = tempfile::tempdir().unwrap();
    let wt = wt_tmp.path().join("thread-test");
    let branch = app_branch("habit-tracker");

    create_sparse_app_worktree(&ws, "habit-tracker", &branch, &wt)
        .await
        .expect("sparse worktree should succeed");

    assert!(
        wt.join("data/apps/habit-tracker/index.html").exists(),
        "target app folder must be materialised",
    );
    assert!(
        wt.join(".gitignore").exists(),
        "top-level files always materialise in cone mode",
    );
    assert!(
        !wt.join("data/apps/other/index.html").exists(),
        "sibling apps must NOT be materialised in the sparse cone",
    );
    assert!(
        !wt.join("data/knowhow/x.md").exists(),
        "non-app data subtree must NOT be materialised in the sparse cone",
    );

    let head = git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"], &wt)
        .await
        .unwrap();
    let head_str = String::from_utf8_lossy(&head.stdout);
    assert_eq!(head_str.trim(), branch);
}

#[tokio::test]
async fn create_sparse_app_worktree_reuses_surviving_branch_on_resume() {
    let (_tmp, ws) = make_test_repo().await;
    tokio::fs::create_dir_all(ws.join("data/apps/habit-tracker"))
        .await
        .unwrap();
    tokio::fs::write(ws.join("data/apps/habit-tracker/index.html"), "<h1>v1</h1>")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &ws).await;
    let _ = git_cmd(&["commit", "-m", "scaffold"], &ws).await;

    let wt_tmp = tempfile::tempdir().unwrap();
    let wt = wt_tmp.path().join("thread-resume");
    let branch = app_branch("habit-tracker");

    let created = create_sparse_app_worktree(&ws, "habit-tracker", &branch, &wt)
        .await
        .expect("first sparse worktree should succeed");
    assert!(created, "first spawn must report the branch as created");

    tokio::fs::write(wt.join("data/apps/habit-tracker/index.html"), "<h1>v2</h1>")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt).await;
    let _ = git_cmd(&["commit", "-m", "session work"], &wt).await;

    let _ = git_cmd(
        &["worktree", "remove", "--force", wt.to_str().unwrap()],
        &ws,
    )
    .await;
    assert!(!wt.exists(), "precondition: worktree dir removed");

    let created = create_sparse_app_worktree(&ws, "habit-tracker", &branch, &wt)
        .await
        .expect("resume must reuse the surviving branch");
    assert!(
        !created,
        "resume must report the branch as reused, not created"
    );

    let html = tokio::fs::read_to_string(wt.join("data/apps/habit-tracker/index.html"))
        .await
        .unwrap();
    assert_eq!(
        html, "<h1>v2</h1>",
        "resumed worktree must carry the branch's committed work",
    );
    let head = git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"], &wt)
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), branch);
}

#[tokio::test]
async fn create_sparse_app_worktree_reuses_branch_after_dirty_teardown() {
    let (_tmp, ws) = make_test_repo().await;
    tokio::fs::create_dir_all(ws.join("data/apps/habit-tracker"))
        .await
        .unwrap();
    tokio::fs::write(ws.join("data/apps/habit-tracker/index.html"), "<h1>v1</h1>")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &ws).await;
    let _ = git_cmd(&["commit", "-m", "scaffold"], &ws).await;

    let wt_tmp = tempfile::tempdir().unwrap();
    let wt = wt_tmp.path().join("thread-dirty");
    let branch = app_branch("habit-tracker");

    create_sparse_app_worktree(&ws, "habit-tracker", &branch, &wt)
        .await
        .expect("first sparse worktree should succeed");
    tokio::fs::write(wt.join("data/apps/habit-tracker/index.html"), "<h1>v2</h1>")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt).await;
    let _ = git_cmd(&["commit", "-m", "session work"], &wt).await;

    tokio::fs::remove_dir_all(&wt).await.unwrap();

    let created = create_sparse_app_worktree(&ws, "habit-tracker", &branch, &wt)
        .await
        .expect("reuse over stale registration must recover via prune-before-add");
    assert!(!created, "branch must be reused, not recreated");
    let html = tokio::fs::read_to_string(wt.join("data/apps/habit-tracker/index.html"))
        .await
        .unwrap();
    assert_eq!(
        html, "<h1>v2</h1>",
        "committed work must survive the dirty resume"
    );
}

#[tokio::test]
async fn create_sparse_app_worktree_recovers_from_missing_but_registered_path() {
    let (_tmp, ws) = make_test_repo().await;
    tokio::fs::create_dir_all(ws.join("data/apps/habit-tracker"))
        .await
        .unwrap();
    tokio::fs::write(ws.join("data/apps/habit-tracker/index.html"), "<h1>m</h1>")
        .await
        .unwrap();
    tokio::fs::write(ws.join(".gitignore"), ".lucidos/\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &ws).await;
    let _ = git_cmd(&["commit", "-m", "scaffold"], &ws).await;

    let wt_tmp = tempfile::tempdir().unwrap();
    let wt = wt_tmp.path().join("thread-stale");

    create_sparse_app_worktree(&ws, "habit-tracker", &app_branch("habit-tracker"), &wt)
        .await
        .expect("first sparse worktree should succeed");

    tokio::fs::remove_dir_all(&wt).await.unwrap();
    assert!(!wt.exists(), "precondition: worktree dir should be gone");

    let branch = app_branch("habit-tracker");
    create_sparse_app_worktree(&ws, "habit-tracker", &branch, &wt)
        .await
        .expect("re-spawn over stale registration should recover");
    assert!(
        wt.join("data/apps/habit-tracker/index.html").exists(),
        "app folder not materialised on re-spawn",
    );
}

#[tokio::test]
async fn create_sparse_app_worktree_removes_its_worktree_when_a_later_step_errors() {
    let (_tmp, ws) = make_test_repo().await;
    tokio::fs::create_dir_all(ws.join("data/apps/habit-tracker"))
        .await
        .unwrap();
    tokio::fs::write(ws.join("data/apps/habit-tracker/index.html"), "<h1>m</h1>")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &ws).await;
    let _ = git_cmd(&["commit", "-m", "scaffold"], &ws).await;

    let rel = PathBuf::from(format!("sparse-wt-{}", uuid::Uuid::new_v4()));
    assert!(
        !rel.exists(),
        "precondition: path must not resolve from cwd"
    );

    let err = create_sparse_app_worktree(&ws, "habit-tracker", &app_branch("habit-tracker"), &rel)
        .await
        .expect_err("step 2 cannot spawn, so the call must fail");
    assert!(
        err.contains("sparse-checkout init"),
        "unexpected error: {err}"
    );

    let list = git_cmd(&["worktree", "list", "--porcelain"], &ws)
        .await
        .unwrap();
    let registered = String::from_utf8_lossy(&list.stdout);
    let rel_str = rel.to_str().unwrap();
    assert!(
        !registered.contains(rel_str),
        "failed spawn left its worktree registered:\n{registered}",
    );
    assert!(
        !ws.join(&rel).exists(),
        "failed spawn left its worktree dir"
    );
}
