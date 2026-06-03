use super::*;
use super::common::make_test_repo;

#[tokio::test]
async fn harden_marker_missing_when_no_db_row() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    assert_eq!(
        harden_marker_state(&pool, &repo_path, "any-branch").await,
        HardenMarkerState::Missing,
        "No DB row should report Missing"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn harden_marker_with_matching_head_sha_is_fresh() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "test-branch"], &repo_path).await;
    tokio::fs::write(repo_path.join("test.txt"), "hello")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "test.txt"], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "add test file"], &repo_path).await;

    let head_sha = current_head_sha(&repo_path).await.unwrap();
    record_hardened(&pool, &repo_path, "test-branch", &head_sha)
        .await
        .unwrap();

    assert!(
        is_harden_marker_fresh(&pool, &repo_path, "test-branch").await,
        "Marker with matching HEAD SHA should be fresh"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn harden_marker_becomes_stale_after_new_commit() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "test-branch"], &repo_path).await;
    tokio::fs::write(repo_path.join("file1.txt"), "first")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "file1.txt"], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "first commit"], &repo_path).await;

    let head_sha = current_head_sha(&repo_path).await.unwrap();
    record_hardened(&pool, &repo_path, "test-branch", &head_sha)
        .await
        .unwrap();
    assert!(
        is_harden_marker_fresh(&pool, &repo_path, "test-branch").await,
        "Should be fresh right after hardening"
    );

    tokio::fs::write(repo_path.join("file2.txt"), "second")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "file2.txt"], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "fix harden finding"], &repo_path).await;

    assert!(
        !is_harden_marker_fresh(&pool, &repo_path, "test-branch").await,
        "Should be STALE after new commit changes HEAD"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression for the badge-thread scenario: branch HEAD `X` is hardened,
/// then `X` is fast-forwarded into main. The marker stays fresh because HEAD
/// itself didn't change.
#[tokio::test]
async fn harden_marker_stays_fresh_when_branch_merged_to_main() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo_path).await;
    tokio::fs::write(repo_path.join("badge.tsx"), "<Badge/>")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "add badge"], &repo_path).await;

    let head_sha = current_head_sha(&repo_path).await.unwrap();
    record_hardened(&pool, &repo_path, "feature", &head_sha)
        .await
        .unwrap();
    assert!(is_harden_marker_fresh(&pool, &repo_path, "feature").await);

    let _ = git_cmd(&["checkout", "main"], &repo_path).await;
    let _ = git_cmd(&["merge", "--ff-only", "feature"], &repo_path).await;
    let _ = git_cmd(&["checkout", "feature"], &repo_path).await;

    assert!(
        is_harden_marker_fresh(&pool, &repo_path, "feature").await,
        "Marker must stay fresh when branch is merged to main without HEAD changing"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Main advancing with unrelated commits must not invalidate a fresh marker
/// when HEAD on the branch is unchanged.
#[tokio::test]
async fn harden_marker_stays_fresh_when_main_advances() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo_path).await;
    tokio::fs::write(repo_path.join("feature.txt"), "f")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "feature work"], &repo_path).await;

    let head_sha = current_head_sha(&repo_path).await.unwrap();
    record_hardened(&pool, &repo_path, "feature", &head_sha)
        .await
        .unwrap();

    let _ = git_cmd(&["checkout", "main"], &repo_path).await;
    tokio::fs::write(repo_path.join("other.txt"), "o")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "main work"], &repo_path).await;
    let _ = git_cmd(&["checkout", "feature"], &repo_path).await;

    assert!(
        is_harden_marker_fresh(&pool, &repo_path, "feature").await,
        "Marker must stay fresh when main advances without touching branch HEAD"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn harden_marker_state_distinguishes_missing_stale_fresh() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    assert_eq!(
        harden_marker_state(&pool, &repo_path, "feature").await,
        HardenMarkerState::Missing,
        "No DB row should report Missing"
    );

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo_path).await;
    tokio::fs::write(repo_path.join("a.txt"), "a").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "initial feature"], &repo_path).await;

    let head_sha = current_head_sha(&repo_path).await.unwrap();
    record_hardened(&pool, &repo_path, "feature", &head_sha)
        .await
        .unwrap();
    assert_eq!(
        harden_marker_state(&pool, &repo_path, "feature").await,
        HardenMarkerState::Fresh,
    );

    tokio::fs::write(repo_path.join("b.txt"), "b").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "new commit after harden"], &repo_path).await;
    assert_eq!(
        harden_marker_state(&pool, &repo_path, "feature").await,
        HardenMarkerState::Stale,
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: marker must be findable via (repo_root, branch_name) even
/// after the worktree directory is gone. Stale-session recovery removes the
/// worktree before propose-change runs, and apply needs to trust the marker
/// without re-running `/harden` on already-hardened work.
#[tokio::test]
async fn harden_marker_survives_worktree_removal() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    let branch_name = "claude-code/survives-removal";
    let o = git_cmd(
        &[
            "worktree",
            "add",
            "-b",
            branch_name,
            wt_path.to_str().unwrap(),
            "main",
        ],
        &repo_path,
    )
    .await
    .unwrap();
    assert!(
        o.status.success(),
        "worktree add failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    tokio::fs::write(wt_path.join("file.txt"), "work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt_path).await;
    let _ = git_cmd(&["commit", "-m", "work"], &wt_path).await;

    let head_sha = current_head_sha(&wt_path).await.unwrap();
    record_hardened(&pool, &repo_path, branch_name, &head_sha)
        .await
        .unwrap();
    assert_eq!(
        harden_marker_state(&pool, &repo_path, branch_name).await,
        HardenMarkerState::Fresh,
    );

    let o = git_cmd(
        &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
        &repo_path,
    )
    .await
    .unwrap();
    assert!(o.status.success());

    assert_eq!(
        harden_marker_state(&pool, &repo_path, branch_name).await,
        HardenMarkerState::Fresh,
        "DB-backed marker must survive worktree removal",
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn floor_char_boundary_truncation_handles_multibyte() {
    // Regression: &s[..200] panics when byte 200 falls inside a multi-byte
    // character like em dash (U+2014, 3 bytes). floor_char_boundary avoids this.
    let mut s = String::new();
    // Fill with 198 ASCII chars, then an em dash (3 bytes = positions 198..201)
    for _ in 0..198 {
        s.push('a');
    }
    s.push('\u{2014}'); // U+2014, 3 bytes
    s.push_str("after");
    assert_eq!(s.len(), 206); // 198 + 3 + 5

    // Old code: &s[..200] would panic here (byte 200 is inside the em dash)
    // New code: floor_char_boundary(200) -> 198 (before the em dash)
    let safe_end = s.floor_char_boundary(200.min(s.len()));
    let truncated = &s[..safe_end];
    assert_eq!(truncated.len(), 198);
    assert!(truncated.is_char_boundary(truncated.len()));

    // Also verify min(200) with string shorter than 200
    let short = "h\u{00e9}llo w\u{00f6}rld";
    let safe_end = short.floor_char_boundary(200.min(short.len()));
    let truncated = &short[..safe_end];
    assert_eq!(truncated, short);
}

