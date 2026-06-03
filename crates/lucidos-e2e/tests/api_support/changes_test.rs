use crate::support::{
    base_url, db_url, git, git_in, http_client, seed_cc_thread_summary, workspace_path,
};
use serde_json::json;
use uuid::Uuid;

// Test helper mirrors the full `seed-change-for-test` endpoint payload one-to-one;
// a struct wrapper would just duplicate the JSON shape with no readability gain.
#[allow(clippy::too_many_arguments)]
async fn seed_change_for_test(
    client: &reqwest::Client,
    change_id: Uuid,
    thread_id: Uuid,
    branch_name: &str,
    repo_root: &str,
    description: &str,
    files: &[&str],
    requires_restart: bool,
    hardened: bool,
) {
    let url = format!("{}/api/v1/internal/seed-change-for-test", base_url());
    let resp = client
        .post(&url)
        .json(&json!({
            "change_id": change_id.to_string(),
            "thread_id": thread_id.to_string(),
            "branch_name": branch_name,
            "repo_root": repo_root,
            "description": description,
            "files": files,
            "requires_restart": requires_restart,
            "hardened": hardened,
        }))
        .send()
        .await
        .expect("seed-change-for-test request failed");
    assert!(
        resp.status().is_success(),
        "seed-change-for-test returned {}: {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );
}

/// Regression: the frontend had `MissingHardeningDetected` wired as an
/// exchange-start event but the engine never emitted it, so hardening
/// collapsed into the prior CC response. Applying an unhardened change
/// must emit it before any further work, ahead of `ChangeApplyFailed`.
#[tokio::test]
async fn apply_unhardened_change_emits_missing_hardening_detected() {
    let client = http_client();
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap();

    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let branch = format!("e2e-test/missing-harden-{}", suffix);
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();

    seed_cc_thread_summary(&pool, thread_id, "idle").await;

    seed_change_for_test(
        &client,
        change_id,
        thread_id,
        &branch,
        repo_root,
        "E2E test missing hardening",
        &["e2e-missing-harden.txt"],
        false,
        false,
    )
    .await;

    let url = format!("{}/api/v1/changes/{}/apply", base_url(), change_id);
    let resp = client
        .post(&url)
        .send()
        .await
        .expect("Apply request failed");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON from apply");

    assert_eq!(
        status, 400,
        "Apply against nonexistent branch should fail (400), got {}: {:?}",
        status, body
    );
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|s| s.contains("worktree")),
        "Error should mention worktree creation: {:?}",
        body
    );

    // Both events must be present, and MissingHardeningDetected must come first
    // (we emit it before attempting the worktree creation that fails).
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT event_type, sequence FROM events \
         WHERE aggregate_id = $1::text \
           AND event_type IN ('MissingHardeningDetected', 'ChangeApplyFailed') \
         ORDER BY sequence ASC",
    )
    .bind(thread_id)
    .fetch_all(&pool)
    .await
    .expect("failed to query events");

    assert_eq!(
        rows.len(),
        2,
        "expected MissingHardeningDetected + ChangeApplyFailed, got: {:?}",
        rows
    );
    assert_eq!(
        rows[0].0, "MissingHardeningDetected",
        "MissingHardeningDetected must be emitted before ChangeApplyFailed: {:?}",
        rows
    );
    assert_eq!(rows[1].0, "ChangeApplyFailed", "second event order: {:?}", rows);

    // Cleanup
    let _ = sqlx::query("DELETE FROM events WHERE aggregate_id = $1::text")
        .bind(thread_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await;

    pool.close().await;
}

/// Sequential apply of two changes must both succeed.
/// Regression test: after applying the first change, the working tree was left
/// dirty (detached HEAD caused `reset --hard HEAD` to target the wrong commit),
/// causing the second apply to fail with "uncommitted changes".
#[tokio::test]
async fn sequential_apply_two_changes_succeeds() {
    let client = http_client();
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap();

    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let branch1 = format!("e2e-test/change1-{}", suffix);
    let branch2 = format!("e2e-test/change2-{}", suffix);
    let file1 = format!("e2e-test1-{}.txt", suffix);
    let file2 = format!("e2e-test2-{}.txt", suffix);
    let wt_dir = std::env::temp_dir().join(format!("e2e-wt-changes-{}", suffix));

    // Use a worktree to create branches without touching the main working tree
    git(&[
        "worktree",
        "add",
        wt_dir.to_str().unwrap(),
        "-b",
        &branch1,
        "main",
    ]);
    std::fs::write(wt_dir.join(&file1), "change 1").unwrap();
    git_in(&wt_dir, &["add", &file1]);
    git_in(&wt_dir, &["commit", "-m", "e2e test change 1"]);

    // Create branch2 from branch1 with another file (so it's ff-able after branch1)
    git_in(&wt_dir, &["checkout", "-b", &branch2]);
    std::fs::write(wt_dir.join(&file2), "change 2").unwrap();
    git_in(&wt_dir, &["add", &file2]);
    git_in(&wt_dir, &["commit", "-m", "e2e test change 2"]);

    // Remove the worktree (branches are kept)
    let _ = std::process::Command::new("git")
        .args(["worktree", "remove", "--force", wt_dir.to_str().unwrap()])
        .current_dir(&ws)
        .output();

    let change1_id = Uuid::new_v4();
    let change2_id = Uuid::new_v4();
    // One pending change per thread is the production invariant: a CC thread
    // owns exactly one branch, and `propose_change` reuses the existing
    // pending change for that branch rather than stacking a second. The
    // single-change apply endpoint is gated by `available_thread_actions_for`,
    // which derives Apply from the thread's per-thread `coding_agent_proposed`
    // flag — applying change 1 clears that flag on its thread. So each change
    // must live on its own synthetic thread, exactly as two real CC threads
    // would. (This test still exercises the regression it was written for:
    // two sequential applies must leave the working tree clean.)
    let thread1_id = Uuid::new_v4();
    let thread2_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread1_id, "idle").await;
    seed_cc_thread_summary(&pool, thread2_id, "idle").await;

    seed_change_for_test(
        &client, change1_id, thread1_id, &branch1, repo_root,
        "E2E test change 1", &[&file1], false, true,
    )
    .await;
    seed_change_for_test(
        &client, change2_id, thread2_id, &branch2, repo_root,
        "E2E test change 2", &[&file2], false, true,
    )
    .await;

    // Apply change 1
    let url1 = format!("{}/api/v1/changes/{}/apply", base_url(), change1_id);
    let resp1 = client
        .post(&url1)
        .send()
        .await
        .expect("Apply change 1 request failed");
    let status1 = resp1.status().as_u16();
    let body1: serde_json::Value = resp1.json().await.expect("Invalid JSON from apply 1");

    assert_eq!(
        status1, 200,
        "First apply should succeed (200), got {}: {:?}",
        status1, body1
    );
    assert!(
        body1.get("error").is_none(),
        "First apply should not have error: {:?}",
        body1
    );
    // The response must make verification self-contained — no thread-state poll needed.
    assert_eq!(
        body1["status"], "applied",
        "first apply should report status=applied: {:?}",
        body1
    );
    assert_eq!(
        body1["change_id"],
        change1_id.to_string(),
        "change_id should echo back: {:?}",
        body1
    );
    assert!(
        body1["applied_commit"]
            .as_str()
            .is_some_and(|s| s.len() == 40),
        "applied_commit must be a 40-char SHA: {:?}",
        body1
    );
    assert!(
        body1["previous_commit"]
            .as_str()
            .is_some_and(|s| s.len() == 40),
        "previous_commit must be a 40-char SHA: {:?}",
        body1
    );
    assert!(
        body1["commits_applied"].as_u64().is_some_and(|n| n >= 1),
        "commits_applied should be >= 1: {:?}",
        body1
    );
    assert_eq!(
        body1["files_changed"], 1,
        "files_changed should be 1: {:?}",
        body1
    );

    // Apply change 2 — this was the failing case before the fix
    let url2 = format!("{}/api/v1/changes/{}/apply", base_url(), change2_id);
    let resp2 = client
        .post(&url2)
        .send()
        .await
        .expect("Apply change 2 request failed");
    let status2 = resp2.status().as_u16();
    let body2: serde_json::Value = resp2.json().await.expect("Invalid JSON from apply 2");

    assert_eq!(
        status2, 200,
        "Second apply should succeed (200), got {}: {:?}",
        status2, body2
    );
    assert!(
        body2.get("error").is_none(),
        "Second apply should not have error (was: 'uncommitted changes'): {:?}",
        body2
    );
    assert_eq!(
        body2["status"], "applied",
        "second apply should report status=applied: {:?}",
        body2
    );
    assert!(
        body2["applied_commit"]
            .as_str()
            .is_some_and(|s| s.len() == 40),
        "second apply must surface applied_commit: {:?}",
        body2
    );

    // Idempotent re-apply — must report status=noop, not silently 200 with empty body.
    let resp_repeat = client
        .post(&url1)
        .send()
        .await
        .expect("Re-apply change 1 request failed");
    assert_eq!(resp_repeat.status().as_u16(), 200);
    let body_repeat: serde_json::Value = resp_repeat
        .json()
        .await
        .expect("Invalid JSON from re-apply");
    assert_eq!(
        body_repeat["status"], "noop",
        "re-apply must explicitly report noop: {:?}",
        body_repeat
    );
    assert!(
        body_repeat["applied_commit"].as_str().is_some(),
        "re-apply must echo the original applied_commit so callers can still reference it: {:?}",
        body_repeat
    );

    // Verify git status is clean
    let status_out = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&ws)
        .output()
        .unwrap();
    let status_text = String::from_utf8_lossy(&status_out.stdout);
    let dirty: Vec<&str> = status_text
        .lines()
        .filter(|l| !l.starts_with("??"))
        .collect();
    assert!(
        dirty.is_empty(),
        "Working tree should be clean after both applies, got: {:?}",
        dirty
    );

    // Verify both test files exist (merged to main)
    assert!(ws.join(&file1).exists(), "file from change 1 should exist");
    assert!(ws.join(&file2).exists(), "file from change 2 should exist");

    // Clean up: remove test files, branches, and DB records
    std::fs::remove_file(ws.join(&file1)).unwrap();
    std::fs::remove_file(ws.join(&file2)).unwrap();
    git(&["add", &file1, &file2]);
    git(&[
        "commit",
        "-m",
        &format!("chore: clean up e2e test files ({})", suffix),
    ]);

    // Delete merged branches (apply already deletes them, but be safe)
    let _ = std::process::Command::new("git")
        .args(["branch", "-D", &branch1])
        .current_dir(&ws)
        .output();
    let _ = std::process::Command::new("git")
        .args(["branch", "-D", &branch2])
        .current_dir(&ws)
        .output();

    // Clean up DB records
    if let Err(e) = sqlx::query("DELETE FROM changes WHERE id = ANY($1)")
        .bind(&[change1_id, change2_id][..])
        .execute(&pool)
        .await
    {
        eprintln!("Failed to clean up changes: {}", e);
    }
    if let Err(e) = sqlx::query("DELETE FROM thread_summaries WHERE thread_id = ANY($1)")
        .bind(&[thread1_id, thread2_id][..])
        .execute(&pool)
        .await
    {
        eprintln!("Failed to clean up thread_summaries: {}", e);
    }

    pool.close().await;
}

/// An in-workspace CC thread with a pending change is NOT archivable — the
/// archive endpoint returns 409 `parent_has_pending_changes` and emits
/// nothing. The user must Apply or Discard the change first. Without this
/// gate, `ThreadArchived` projects through `CLEAR_CODING_AGENT_FLAGS` and
/// silently clears `coding_agent_proposed`, leaving the change row dangling
/// in the `changes` table while the thread sits in Archive — the original
/// cca058432 "pending changes survive into Review" contract never held
/// because the projection always cleared the column the routing depended on.
/// Aligns with `resolve_actions`, which already returns [Discard, Apply]
/// (never Archive) in this state.
#[tokio::test]
async fn archive_with_pending_change_is_rejected_409() {
    let client = http_client();
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap();

    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let branch = format!("e2e-test/archive-pending-{}", suffix);
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();

    seed_cc_thread_summary(&pool, thread_id, "waiting").await;

    seed_change_for_test(
        &client,
        change_id,
        thread_id,
        &branch,
        repo_root,
        "E2E test archive-with-pending",
        &["e2e-archive-pending.txt"],
        false,
        true,
    )
    .await;

    let url = format!("{}/api/v1/threads/archive", base_url());
    let body = json!({ "thread_id": thread_id.to_string() });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("archive request failed");
    assert_eq!(
        resp.status().as_u16(),
        409,
        "archive must reject when there is a pending change"
    );
    let body: serde_json::Value = resp.json().await.expect("response body");
    assert_eq!(
        body["reason"], "parent_has_pending_changes",
        "rejection reason must be parent_has_pending_changes: {body:?}"
    );

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let archived: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'ThreadArchived'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap_or(99);
    assert_eq!(
        archived, 0,
        "ThreadArchived must NOT fire when archive is rejected"
    );

    let status: Option<String> = sqlx::query_scalar("SELECT status FROM changes WHERE id = $1")
        .bind(change_id)
        .fetch_optional(&pool)
        .await
        .expect("changes lookup");
    assert_eq!(
        status.as_deref(),
        Some("pending"),
        "pending change row must remain pending after the rejected archive"
    );

    let _ = sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await;

    pool.close().await;
}
