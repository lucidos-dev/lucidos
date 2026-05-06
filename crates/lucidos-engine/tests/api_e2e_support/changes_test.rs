// NOTE (Phase 10.1 Step B): these api_e2e tests seed via raw `INSERT INTO changes`,
// which used to be the source of truth. Now reads come from the in-memory
// `ChangesProjection` populated from the events table, so these inserts no
// longer surface to the apply endpoint. The tests need a test-only seed
// endpoint that emits a `ChangeProposed` via the live EventBus (which the
// projection subscribes to). Until that lands they remain `#[ignore]`'d
// and out of the regular suite. Tracked in docs/plans/2026-04-24-cc-resume-architecture.md.
use crate::support::{base_url, db_url, git, git_in, http_client, workspace_path};
use uuid::Uuid;

/// Regression: the frontend had `MissingHardeningDetected` wired as an
/// exchange-start event but the engine never emitted it, so hardening
/// collapsed into the prior CC response. Applying an unhardened change
/// must emit it before any further work, ahead of `ChangeApplyFailed`.
#[tokio::test]
#[ignore]
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

    // Skeleton thread_summaries row — keeps the lifecycle projection happy
    // when the event lands. Source matches what a real CC thread would have.
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, source, is_cc, created_at, last_activity, message_count, status) \
         VALUES ($1, 'claude_code', TRUE, NOW(), NOW(), 0, 'idle')"
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("failed to insert thread_summaries");

    sqlx::query(
        "INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened, thread_id) \
         VALUES ($1, $2, $3, $4, $5, 1, ARRAY[$6], false, false, $7)"
    )
    .bind(change_id)
    .bind(Uuid::new_v4())
    .bind(&branch)
    .bind(repo_root)
    .bind("E2E test missing hardening")
    .bind("e2e-missing-harden.txt")
    .bind(thread_id)
    .execute(&pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to insert change: {}", e));

    let url = format!("{}/api/changes/{}/apply", base_url(), change_id);
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
#[ignore]
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

    // Insert two change records in the DB (hardened = true to skip hardening flow)
    let change1_id = Uuid::new_v4();
    let change2_id = Uuid::new_v4();

    for (id, branch, desc) in [
        (change1_id, &branch1, "E2E test change 1"),
        (change2_id, &branch2, "E2E test change 2"),
    ] {
        sqlx::query(
            "INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened) \
             VALUES ($1, $2, $3, $4, $5, 1, ARRAY[$6], false, true)"
        )
        .bind(id)
        .bind(Uuid::new_v4()) // each change gets its own request_id
        .bind(branch)
        .bind(repo_root)
        .bind(desc)
        .bind(if id == change1_id { &file1 } else { &file2 })
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("Failed to insert change {}: {}", id, e));
    }

    // Apply change 1
    let url1 = format!("{}/api/changes/{}/apply", base_url(), change1_id);
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
    let url2 = format!("{}/api/changes/{}/apply", base_url(), change2_id);
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

    pool.close().await;
}
