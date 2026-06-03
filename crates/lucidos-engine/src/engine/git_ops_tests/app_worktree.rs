use super::*;
use super::common::make_test_repo;

#[test]
fn generate_app_branch_name_embeds_app_id() {
    let b = generate_app_branch_name("momentum");
    assert!(b.starts_with("claude-code/app/momentum/"));
    // Branch has the shape `claude-code/app/<id>/<ts>-<uuid>` — three slashes
    // before the timestamp + random suffix.
    assert_eq!(b.matches('/').count(), 3);
}

#[test]
fn any_iframe_bundled_file_changed_picks_up_app_files() {
    let files: Vec<String> = vec![
        "data/apps/momentum/index.html".into(),
        "data/apps/momentum/styles.css".into(),
        "data/apps/momentum/knowhow/intent.md".into(),
        "data/apps/other/index.html".into(),
        "scripts/build.sh".into(),
    ];
    assert!(any_iframe_bundled_file_changed(&files, "momentum"));
    // .md inside the app folder doesn't reload the iframe (not a bundled
    // asset; markdown is engine-side context).
    let files: Vec<String> = vec!["data/apps/momentum/knowhow/intent.md".into()];
    assert!(!any_iframe_bundled_file_changed(&files, "momentum"));
    // Files in OTHER apps don't trigger refresh for this app.
    let files: Vec<String> = vec!["data/apps/other/index.html".into()];
    assert!(!any_iframe_bundled_file_changed(&files, "momentum"));
}

#[test]
fn any_iframe_bundled_file_changed_recognises_static_assets() {
    let files: Vec<String> = vec!["data/apps/momentum/assets/logo.svg".into()];
    assert!(any_iframe_bundled_file_changed(&files, "momentum"));
    let files: Vec<String> = vec!["data/apps/momentum/fonts/x.woff2".into()];
    assert!(any_iframe_bundled_file_changed(&files, "momentum"));
}

#[test]
fn any_iframe_bundled_file_changed_recognises_manifest() {
    let files: Vec<String> = vec!["data/apps/momentum/manifest.json".into()];
    assert!(any_iframe_bundled_file_changed(&files, "momentum"));
}

#[tokio::test]
async fn create_sparse_app_worktree_materialises_only_app_folder() {
    // Build a "workspace git" with two apps and a knowhow file. The sparse
    // worktree narrowed to `data/apps/momentum/` must contain only that
    // folder plus the workspace's top-level files (root .gitignore here).
    let (_tmp, ws) = make_test_repo().await;
    tokio::fs::create_dir_all(ws.join("data/apps/momentum")).await.unwrap();
    tokio::fs::create_dir_all(ws.join("data/apps/other")).await.unwrap();
    tokio::fs::create_dir_all(ws.join("data/knowhow")).await.unwrap();
    tokio::fs::write(ws.join("data/apps/momentum/index.html"), "<h1>m</h1>").await.unwrap();
    tokio::fs::write(ws.join("data/apps/other/index.html"), "<h1>o</h1>").await.unwrap();
    tokio::fs::write(ws.join("data/knowhow/x.md"), "k").await.unwrap();
    tokio::fs::write(ws.join(".gitignore"), ".lucidos/\n").await.unwrap();
    let _ = git_cmd(&["add", "."], &ws).await;
    let _ = git_cmd(&["commit", "-m", "scaffold"], &ws).await;

    let wt_tmp = tempfile::tempdir().unwrap();
    let wt = wt_tmp.path().join("thread-test");
    let branch = generate_app_branch_name("momentum");

    create_sparse_app_worktree(&ws, "momentum", &branch, &wt)
        .await
        .expect("sparse worktree should succeed");

    assert!(
        wt.join("data/apps/momentum/index.html").exists(),
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

/// App spawns reuse the same deterministic `thread-<id>` worktree path as chat
/// CC spawns, so they hit the identical "missing but already registered"
/// residue when a prior worktree dir was wiped without `git worktree remove`.
/// `create_sparse_app_worktree` must self-heal via the shared prune-before-add.
#[tokio::test]
async fn create_sparse_app_worktree_recovers_from_missing_but_registered_path() {
    let (_tmp, ws) = make_test_repo().await;
    tokio::fs::create_dir_all(ws.join("data/apps/momentum")).await.unwrap();
    tokio::fs::write(ws.join("data/apps/momentum/index.html"), "<h1>m</h1>").await.unwrap();
    tokio::fs::write(ws.join(".gitignore"), ".lucidos/\n").await.unwrap();
    let _ = git_cmd(&["add", "."], &ws).await;
    let _ = git_cmd(&["commit", "-m", "scaffold"], &ws).await;

    let wt_tmp = tempfile::tempdir().unwrap();
    let wt = wt_tmp.path().join("thread-stale");

    // First spawn creates the worktree.
    create_sparse_app_worktree(&ws, "momentum", &generate_app_branch_name("momentum"), &wt)
        .await
        .expect("first sparse worktree should succeed");

    // Residue: dir wiped, git registration kept.
    tokio::fs::remove_dir_all(&wt).await.unwrap();
    assert!(!wt.exists(), "precondition: worktree dir should be gone");

    // Second spawn reuses the same path with a fresh branch — must recover.
    let branch = generate_app_branch_name("momentum");
    create_sparse_app_worktree(&ws, "momentum", &branch, &wt)
        .await
        .expect("re-spawn over stale registration should recover");
    assert!(
        wt.join("data/apps/momentum/index.html").exists(),
        "app folder not materialised on re-spawn",
    );
}

