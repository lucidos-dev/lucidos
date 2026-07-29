use super::*;
use super::common::make_test_repo;

#[test]
fn generate_app_branch_name_embeds_app_id() {
    let b = generate_app_branch_name("habit-tracker");
    assert!(b.starts_with("claude-code/app/habit-tracker/"));
    // Branch has the shape `claude-code/app/<id>/<ts>-<uuid>` — three slashes
    // before the timestamp + random suffix.
    assert_eq!(b.matches('/').count(), 3);
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
    // .md inside the app folder doesn't reload the iframe (not a bundled
    // asset; markdown is engine-side context).
    let files: Vec<String> = vec!["data/apps/habit-tracker/knowhow/intent.md".into()];
    assert!(!any_iframe_bundled_file_changed(&files, "habit-tracker"));
    // Files in OTHER apps don't trigger refresh for this app.
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
    // Build a "workspace git" with two apps and a knowhow file. The sparse
    // worktree narrowed to `data/apps/habit-tracker/` must contain only that
    // folder plus the workspace's top-level files (root .gitignore here).
    let (_tmp, ws) = make_test_repo().await;
    tokio::fs::create_dir_all(ws.join("data/apps/habit-tracker")).await.unwrap();
    tokio::fs::create_dir_all(ws.join("data/apps/other")).await.unwrap();
    tokio::fs::create_dir_all(ws.join("data/knowhow")).await.unwrap();
    tokio::fs::write(ws.join("data/apps/habit-tracker/index.html"), "<h1>m</h1>").await.unwrap();
    tokio::fs::write(ws.join("data/apps/other/index.html"), "<h1>o</h1>").await.unwrap();
    tokio::fs::write(ws.join("data/knowhow/x.md"), "k").await.unwrap();
    tokio::fs::write(ws.join(".gitignore"), ".lucidos/\n").await.unwrap();
    let _ = git_cmd(&["add", "."], &ws).await;
    let _ = git_cmd(&["commit", "-m", "scaffold"], &ws).await;

    let wt_tmp = tempfile::tempdir().unwrap();
    let wt = wt_tmp.path().join("thread-test");
    let branch = generate_app_branch_name("habit-tracker");

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

    // Confirm the branch was actually checked out.
    let head = git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"], &wt).await.unwrap();
    let head_str = String::from_utf8_lossy(&head.stdout);
    assert_eq!(head_str.trim(), branch);
}

/// The resume path: an app coding-agent thread's worktree dir can be torn
/// down (cleanup, reclaim) while its branch survives with the session's
/// committed work. Re-spawning must reuse that branch — an unconditional
/// `-b` create fails with "a branch named ... already exists" and strands
/// the thread (every "continue" hits the same wall).
#[tokio::test]
async fn create_sparse_app_worktree_reuses_surviving_branch_on_resume() {
    let (_tmp, ws) = make_test_repo().await;
    tokio::fs::create_dir_all(ws.join("data/apps/habit-tracker")).await.unwrap();
    tokio::fs::write(ws.join("data/apps/habit-tracker/index.html"), "<h1>v1</h1>").await.unwrap();
    let _ = git_cmd(&["add", "."], &ws).await;
    let _ = git_cmd(&["commit", "-m", "scaffold"], &ws).await;

    let wt_tmp = tempfile::tempdir().unwrap();
    let wt = wt_tmp.path().join("thread-resume");
    let branch = generate_app_branch_name("habit-tracker");

    let created = create_sparse_app_worktree(&ws, "habit-tracker", &branch, &wt)
        .await
        .expect("first sparse worktree should succeed");
    assert!(created, "first spawn must report the branch as created");

    // The session commits work on the branch.
    tokio::fs::write(wt.join("data/apps/habit-tracker/index.html"), "<h1>v2</h1>").await.unwrap();
    let _ = git_cmd(&["add", "."], &wt).await;
    let _ = git_cmd(&["commit", "-m", "session work"], &wt).await;

    // Worktree torn down; branch survives.
    let _ = git_cmd(&["worktree", "remove", "--force", wt.to_str().unwrap()], &ws).await;
    assert!(!wt.exists(), "precondition: worktree dir removed");

    // Resume: recreate the worktree on the SAME branch.
    let created = create_sparse_app_worktree(&ws, "habit-tracker", &branch, &wt)
        .await
        .expect("resume must reuse the surviving branch");
    assert!(!created, "resume must report the branch as reused, not created");

    let html = tokio::fs::read_to_string(wt.join("data/apps/habit-tracker/index.html"))
        .await
        .unwrap();
    assert_eq!(
        html, "<h1>v2</h1>",
        "resumed worktree must carry the branch's committed work",
    );
    let head = git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"], &wt).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), branch);
}

/// The dirtier variant of the resume case: the worktree dir was wiped with a
/// bare rm-rf (no `git worktree remove`), leaving a stale registration AND
/// the surviving branch. Reuse then depends on the prune-before-add inside
/// `worktree_add_pruning_stale` — without it, git refuses with "missing but
/// already registered worktree". Pins that load-bearing interaction for the
/// reuse arm specifically (the fresh-branch arm is pinned by the
/// `recovers_from_missing_but_registered_path` test below).
#[tokio::test]
async fn create_sparse_app_worktree_reuses_branch_after_dirty_teardown() {
    let (_tmp, ws) = make_test_repo().await;
    tokio::fs::create_dir_all(ws.join("data/apps/habit-tracker")).await.unwrap();
    tokio::fs::write(ws.join("data/apps/habit-tracker/index.html"), "<h1>v1</h1>").await.unwrap();
    let _ = git_cmd(&["add", "."], &ws).await;
    let _ = git_cmd(&["commit", "-m", "scaffold"], &ws).await;

    let wt_tmp = tempfile::tempdir().unwrap();
    let wt = wt_tmp.path().join("thread-dirty");
    let branch = generate_app_branch_name("habit-tracker");

    create_sparse_app_worktree(&ws, "habit-tracker", &branch, &wt)
        .await
        .expect("first sparse worktree should succeed");
    tokio::fs::write(wt.join("data/apps/habit-tracker/index.html"), "<h1>v2</h1>").await.unwrap();
    let _ = git_cmd(&["add", "."], &wt).await;
    let _ = git_cmd(&["commit", "-m", "session work"], &wt).await;

    // Dirty teardown: dir wiped, registration kept, branch survives.
    tokio::fs::remove_dir_all(&wt).await.unwrap();

    let created = create_sparse_app_worktree(&ws, "habit-tracker", &branch, &wt)
        .await
        .expect("reuse over stale registration must recover via prune-before-add");
    assert!(!created, "branch must be reused, not recreated");
    let html = tokio::fs::read_to_string(wt.join("data/apps/habit-tracker/index.html"))
        .await
        .unwrap();
    assert_eq!(html, "<h1>v2</h1>", "committed work must survive the dirty resume");
}

/// App spawns reuse the same deterministic `thread-<id>` worktree path as chat
/// CC spawns, so they hit the identical "missing but already registered"
/// residue when a prior worktree dir was wiped without `git worktree remove`.
/// `create_sparse_app_worktree` must self-heal via the shared prune-before-add.
#[tokio::test]
async fn create_sparse_app_worktree_recovers_from_missing_but_registered_path() {
    let (_tmp, ws) = make_test_repo().await;
    tokio::fs::create_dir_all(ws.join("data/apps/habit-tracker")).await.unwrap();
    tokio::fs::write(ws.join("data/apps/habit-tracker/index.html"), "<h1>m</h1>").await.unwrap();
    tokio::fs::write(ws.join(".gitignore"), ".lucidos/\n").await.unwrap();
    let _ = git_cmd(&["add", "."], &ws).await;
    let _ = git_cmd(&["commit", "-m", "scaffold"], &ws).await;

    let wt_tmp = tempfile::tempdir().unwrap();
    let wt = wt_tmp.path().join("thread-stale");

    // First spawn creates the worktree.
    create_sparse_app_worktree(&ws, "habit-tracker", &generate_app_branch_name("habit-tracker"), &wt)
        .await
        .expect("first sparse worktree should succeed");

    // Residue: dir wiped, git registration kept.
    tokio::fs::remove_dir_all(&wt).await.unwrap();
    assert!(!wt.exists(), "precondition: worktree dir should be gone");

    // Second spawn reuses the same path with a fresh branch — must recover.
    let branch = generate_app_branch_name("habit-tracker");
    create_sparse_app_worktree(&ws, "habit-tracker", &branch, &wt)
        .await
        .expect("re-spawn over stale registration should recover");
    assert!(
        wt.join("data/apps/habit-tracker/index.html").exists(),
        "app folder not materialised on re-spawn",
    );
}

