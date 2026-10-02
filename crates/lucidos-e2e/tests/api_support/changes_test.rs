use crate::support::{
    base_url, commit_on_new_branch, db_url, git, git_in, http_client, insert_session_started,
    local_process_client, seed_cc_thread_summary, user_client, workspace_path,
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
    let client = user_client().await;
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
    assert_eq!(
        rows[1].0, "ChangeApplyFailed",
        "second event order: {:?}",
        rows
    );

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

/// The implementation-plan floor: a Lucidos-source change with NO Planned
/// marker must be refused at Apply (hard block, no auto-recovery), even when it
/// IS hardened. Mirrors `apply_unhardened_change_emits_missing_hardening_detected`
/// but opts out of the seed's default marker via `planned: false`.
#[tokio::test]
async fn apply_unplanned_change_is_blocked() {
    let client = user_client().await;
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap();

    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let branch = format!("e2e-test/unplanned-{}", suffix);
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();

    seed_cc_thread_summary(&pool, thread_id, "idle").await;

    // Seed a hardened change but explicitly WITHOUT a plan marker.
    let seed_url = format!("{}/api/v1/internal/seed-change-for-test", base_url());
    let seed = client
        .post(&seed_url)
        .json(&json!({
            "change_id": change_id.to_string(),
            "thread_id": thread_id.to_string(),
            "branch_name": branch,
            "repo_root": repo_root,
            "description": "E2E unplanned change",
            "files": ["e2e-unplanned.txt"],
            "requires_restart": false,
            "hardened": true,
            "planned": false,
        }))
        .send()
        .await
        .expect("seed request failed");
    assert!(seed.status().is_success(), "seed failed: {}", seed.status());

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
        "Apply of an unplanned change must be refused (400), got {}: {:?}",
        status, body
    );
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|s| s.contains("implementation-plan marker")),
        "Error must name the missing plan marker: {:?}",
        body
    );

    // ChangeApplyFailed must be recorded for the timeline.
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1::text AND event_type = 'ChangeApplyFailed'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("count query failed");
    assert_eq!(n, 1, "a refused apply announces itself once, found {n}");

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

/// One refused apply draws ONE "Change failed" card, on the Apply Now route.
///
/// Apply Now with no live session applies the thread's pending changes through
/// `apply_change`, which announces its own refusal. It then emitted a second
/// `ChangeApplyFailed` of its own, so the thread showed the same failure twice.
/// The plan floor is the refusal used here because it needs no git state.
#[tokio::test]
async fn apply_now_announces_a_refused_apply_once() {
    let client = user_client().await;
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap();

    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let branch = format!("e2e-test/apply-now-once-{}", suffix);
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();

    seed_cc_thread_summary(&pool, thread_id, "idle").await;

    let seed_url = format!("{}/api/v1/internal/seed-change-for-test", base_url());
    let seed = client
        .post(&seed_url)
        .json(&json!({
            "change_id": change_id.to_string(),
            "thread_id": thread_id.to_string(),
            "branch_name": branch,
            "repo_root": repo_root,
            "description": "E2E apply-now single announcement",
            "files": ["e2e-apply-now-once.txt"],
            "requires_restart": false,
            "hardened": true,
            "planned": false,
        }))
        .send()
        .await
        .expect("seed request failed");
    assert!(seed.status().is_success(), "seed failed: {}", seed.status());

    // No live session for this thread, so this takes the fast path: apply the
    // pending change directly. It awaits the apply, so both emits of the bug
    // had landed by the time the response came back.
    let url = format!(
        "{}/api/v1/claude-code/apply-now?thread_id={}",
        base_url(),
        thread_id
    );
    let resp = client.post(&url).send().await.expect("apply-now failed");
    assert!(
        resp.status().is_success(),
        "apply-now returned {}: {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1::text AND event_type = 'ChangeApplyFailed'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("count query failed");
    assert_eq!(
        n, 1,
        "Apply Now must announce a refused apply once, found {n} ChangeApplyFailed events"
    );

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

/// The plan-approval round-trip over the internal HTTP surface: recording a
/// plan lands the awaiting-approval `proposed` state (gate NOT satisfied), and
/// only `approve-plan` flips it to `SATISFIED`. Exercises the three-way
/// `planned-state` wire contract the cc-plan-gate hook depends on.
#[tokio::test]
async fn plan_marker_proposed_then_approved_round_trip() {
    let client = local_process_client();
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap();

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let branch = format!("e2e-test/plan-approve-{}", suffix);

    let state_url = format!("{}/api/v1/internal/planned-state", base_url());
    let read_state = |branch: String| {
        let client = client.clone();
        let state_url = state_url.clone();
        let repo_root = repo_root.to_string();
        async move {
            let resp = client
                .get(&state_url)
                .query(&[
                    ("repo_root", repo_root.as_str()),
                    ("branch_name", branch.as_str()),
                ])
                .send()
                .await
                .expect("planned-state GET failed");
            let body: serde_json::Value = resp.json().await.expect("planned-state non-JSON");
            body["state"].as_str().unwrap_or_default().to_string()
        }
    };

    // No marker yet.
    assert_eq!(read_state(branch.clone()).await, "MISSING");

    // The skill records `proposed` — present but NOT gate-satisfying.
    let mark = client
        .post(format!("{}/api/v1/internal/mark-planned", base_url()))
        .json(&json!({
            "repo_root": repo_root,
            "branch_name": branch,
            "head_sha": "deadbeef",
            "state": "proposed",
            "plan_path": "docs/plans/2026-06-19-e2e.md",
        }))
        .send()
        .await
        .expect("mark-planned POST failed");
    assert_eq!(mark.status(), 204, "mark-planned must return 204");
    assert_eq!(read_state(branch.clone()).await, "PROPOSED");

    // Approve flips proposed -> planned (SATISFIED).
    let approve = client
        .post(format!("{}/api/v1/internal/approve-plan", base_url()))
        .json(&json!({ "repo_root": repo_root, "branch_name": branch }))
        .send()
        .await
        .expect("approve-plan POST failed");
    assert!(approve.status().is_success());
    let approve_body: serde_json::Value = approve.json().await.expect("approve-plan non-JSON");
    assert_eq!(approve_body["approved"], json!(true));
    assert_eq!(read_state(branch.clone()).await, "SATISFIED");

    // Re-approving an already-planned branch is a no-op (approved: false).
    let reapprove = client
        .post(format!("{}/api/v1/internal/approve-plan", base_url()))
        .json(&json!({ "repo_root": repo_root, "branch_name": branch }))
        .send()
        .await
        .expect("approve-plan POST failed");
    let reapprove_body: serde_json::Value = reapprove.json().await.expect("non-JSON");
    assert_eq!(reapprove_body["approved"], json!(false));

    // Cleanup the marker row (keyed on canonical repo_root).
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let canonical = std::fs::canonicalize(repo_root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| repo_root.to_string());
    let _ = sqlx::query("DELETE FROM planned_branches WHERE repo_root = $1 AND branch_name = $2")
        .bind(&canonical)
        .bind(&branch)
        .execute(&pool)
        .await;
    pool.close().await;
}

/// The bounded security-fix lane over the internal HTTP surface. It satisfies
/// the gate with no approval step, which is the whole point, and the engine
/// refuses a mark whose bound could not be enforced.
///
/// The refusal must be a 400 carrying the reason. The caller is an unattended
/// agent that can correct the call, and a 500 reads as an engine fault it would
/// retry unchanged.
#[tokio::test]
async fn bounded_security_fix_satisfies_the_gate_and_refuses_an_unenforceable_bound() {
    let client = local_process_client();
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap();

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let branch = format!("e2e-test/bounded-security-{}", suffix);
    let mark_url = format!("{}/api/v1/internal/mark-planned", base_url());
    let state_url = format!("{}/api/v1/internal/planned-state", base_url());

    let read_state = |branch: String| {
        let client = client.clone();
        let state_url = state_url.clone();
        let repo_root = repo_root.to_string();
        async move {
            let resp = client
                .get(&state_url)
                .query(&[
                    ("repo_root", repo_root.as_str()),
                    ("branch_name", branch.as_str()),
                ])
                .send()
                .await
                .expect("planned-state GET failed");
            let body: serde_json::Value = resp.json().await.expect("planned-state non-JSON");
            (
                body["state"].as_str().unwrap_or_default().to_string(),
                body["kind"].as_str().unwrap_or_default().to_string(),
            )
        }
    };

    // A bounded fix with no bound is refused, and leaves no marker behind.
    let no_bound = client
        .post(&mark_url)
        .json(&json!({
            "repo_root": repo_root,
            "branch_name": branch,
            "head_sha": "deadbeef",
            "state": "bounded_security_fix",
            "reason": "no files named",
        }))
        .send()
        .await
        .expect("mark-planned POST failed");
    assert_eq!(
        no_bound.status(),
        400,
        "an unenforceable bound is the caller's mistake, not an engine fault",
    );
    let body = no_bound.text().await.unwrap_or_default();
    assert!(
        body.contains("--files"),
        "the 400 body must tell the agent how to fix the call: {body}",
    );
    assert_eq!(read_state(branch.clone()).await.0, "MISSING");

    // With a bound it satisfies the gate immediately: no approval step, which
    // is the deadlock this lane exists to break.
    let marked = client
        .post(&mark_url)
        .json(&json!({
            "repo_root": repo_root,
            "branch_name": branch,
            "head_sha": "deadbeef",
            "state": "bounded_security_fix",
            "reason": "unscoped proxy key; covered by proxy_tests::refuses_foreign_host",
            "files": ["crates/lucidos-engine/src/api/proxy.rs"],
        }))
        .send()
        .await
        .expect("mark-planned POST failed");
    assert_eq!(marked.status(), 204, "mark-planned must return 204");
    assert_eq!(
        read_state(branch.clone()).await,
        ("SATISFIED".to_string(), "bounded_security_fix".to_string()),
        "the lane satisfies the gate, and reports its own kind so a reviewer \
         can tell it from an approved plan",
    );

    // Cleanup the marker row (keyed on canonical repo_root).
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let canonical = std::fs::canonicalize(repo_root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| repo_root.to_string());
    let _ = sqlx::query("DELETE FROM planned_branches WHERE repo_root = $1 AND branch_name = $2")
        .bind(&canonical)
        .bind(&branch)
        .execute(&pool)
        .await;
    pool.close().await;
}

/// The Harden and Apply gates read these markers. So a caller that merely
/// reaches the engine port must not write them. A device id is no credential
/// here: any loopback caller can register one, as `user_client` does. Nor is
/// the gateway's proxied shape, which carries the machine-local token and a
/// forwarded prefix.
#[tokio::test]
async fn a_plain_loopback_caller_cannot_write_a_marker() {
    let device_only = user_client().await;
    let local = local_process_client();
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap();
    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let branch = format!("e2e-test/marker-guard-{}", suffix);

    let writes = [
        (
            "mark-hardened",
            json!({ "repo_root": repo_root, "branch_name": branch, "head_sha": "deadbeef" }),
        ),
        (
            "mark-planned",
            json!({
                "repo_root": repo_root,
                "branch_name": branch,
                "head_sha": "deadbeef",
                "state": "acknowledged_simple",
                "reason": "forged from the loopback port",
            }),
        ),
        (
            "approve-plan",
            json!({ "repo_root": repo_root, "branch_name": branch }),
        ),
    ];
    for (route, body) in &writes {
        let url = format!("{}/api/v1/internal/{}", base_url(), route);
        let plain = device_only
            .post(&url)
            .json(body)
            .send()
            .await
            .unwrap_or_else(|e| panic!("{route} POST failed: {e}"));
        assert_eq!(
            plain.status(),
            403,
            "{route} must refuse a device-only caller"
        );
        let proxied = local
            .post(&url)
            .header("x-forwarded-prefix", "/e2e-test/")
            .json(body)
            .send()
            .await
            .unwrap_or_else(|e| panic!("{route} POST failed: {e}"));
        assert_eq!(
            proxied.status(),
            403,
            "{route} must refuse a request that came through the gateway proxy"
        );
    }

    assert_eq!(
        marker_state(&local, "planned-state", repo_root, &branch).await,
        "MISSING"
    );
    assert_eq!(
        marker_state(&local, "hardened-state", repo_root, &branch).await,
        "MISSING"
    );
}

/// Sequential apply of two changes must both succeed.
/// Regression test: after applying the first change, the working tree was left
/// dirty (detached HEAD caused `reset --hard HEAD` to target the wrong commit),
/// causing the second apply to fail with "uncommitted changes".
#[tokio::test]
async fn sequential_apply_two_changes_succeeds() {
    let client = user_client().await;
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
        &client,
        change1_id,
        thread1_id,
        &branch1,
        repo_root,
        "E2E test change 1",
        &[&file1],
        false,
        true,
    )
    .await;
    seed_change_for_test(
        &client,
        change2_id,
        thread2_id,
        &branch2,
        repo_root,
        "E2E test change 2",
        &[&file2],
        false,
        true,
    )
    .await;

    // Both applies below put files into the shared workspace working tree,
    // which the command-checkpoint test snapshots whole; see
    // `workspace_tree_lock`. A WRITE guard, held across the pair. The point
    // here is that the SECOND apply follows the first on a clean tree. A read
    // guard leaves other writers free to dirty it under both.
    let _tree = crate::support::workspace_tree_lock().write().await;

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

/// Read a branch's plan or harden marker state over the internal HTTP surface.
async fn marker_state(
    client: &reqwest::Client,
    route: &str,
    repo_root: &str,
    branch: &str,
) -> String {
    let resp = client
        .get(format!("{}/api/v1/internal/{}", base_url(), route))
        .query(&[("repo_root", repo_root), ("branch_name", branch)])
        .send()
        .await
        .unwrap_or_else(|e| panic!("{route} GET failed: {e}"));
    let body: serde_json::Value = resp.json().await.expect("marker state non-JSON");
    body["state"].as_str().unwrap_or_default().to_string()
}

/// POST one change's Apply and return the JSON body, asserting a 200.
async fn apply_ok(client: &reqwest::Client, change_id: Uuid) -> serde_json::Value {
    let resp = client
        .post(format!("{}/api/v1/changes/{}/apply", base_url(), change_id))
        .send()
        .await
        .expect("apply request failed");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("apply non-JSON");
    assert_eq!(
        status, 200,
        "apply of {change_id} returned {status}: {body:?}"
    );
    assert_eq!(body["status"], "applied", "apply of {change_id}: {body:?}");
    body
}

/// **The case ADR 0106 feared, end to end (ADR 0249).** A delegating parent,
/// idle apart from a running child, has its change applied. It then commits
/// again on the same branch, as it would after the child's completion wakes
/// it.
///
/// That later work must come back as a NEW pending change that applies on its
/// own: not lost, not folded into the applied one, and not refused as already
/// merged. The worktree is kept, so the apply takes Tier 2, the path an idle
/// thread's apply takes. The apply must also clear the branch's plan and
/// harden markers, so the next change has to earn both gates again.
#[tokio::test]
async fn a_parents_commits_after_an_apply_come_back_as_a_new_change() {
    let client = user_client().await;
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let branch = format!("e2e-test/parent-{}", suffix);
    let file1 = format!("e2e-parent-first-{}.txt", suffix);
    let file2 = format!("e2e-parent-second-{}.txt", suffix);
    let wt_dir = std::env::temp_dir().join(format!("e2e-wt-parent-{}", suffix));
    let wt = wt_dir.to_str().unwrap();

    let _tree = crate::support::workspace_tree_lock().write().await;

    git(&["worktree", "add", wt, "-b", &branch, "main"]);
    std::fs::write(wt_dir.join(&file1), "first round").unwrap();
    git_in(&wt_dir, &["add", &file1]);
    git_in(&wt_dir, &["commit", "-m", "e2e parent: first round"]);

    // A delegating parent: idle, with a sub-thread still running.
    let parent = Uuid::new_v4();
    seed_cc_thread_summary(&pool, parent, "idle").await;
    sqlx::query("UPDATE thread_summaries SET active_children_count = 1 WHERE thread_id = $1")
        .bind(parent)
        .execute(&pool)
        .await
        .expect("give the parent a running child");

    let first = Uuid::new_v4();
    seed_change_for_test(
        &client,
        first,
        parent,
        &branch,
        repo_root,
        "E2E parent first round",
        &[&file1],
        false,
        true,
    )
    .await;
    let head = String::from_utf8(git_in(&wt_dir, &["rev-parse", "HEAD"]).stdout).unwrap();
    let mark = local_process_client()
        .post(format!("{}/api/v1/internal/mark-hardened", base_url()))
        .json(&json!({ "repo_root": repo_root, "branch_name": branch, "head_sha": head.trim() }))
        .send()
        .await
        .expect("mark-hardened POST failed");
    assert!(
        mark.status().is_success(),
        "mark-hardened returned {}",
        mark.status()
    );
    assert_eq!(
        marker_state(&client, "planned-state", repo_root, &branch).await,
        "SATISFIED"
    );
    assert_ne!(
        marker_state(&client, "hardened-state", repo_root, &branch).await,
        "MISSING"
    );
    // `lucidos hardened sha` prints this field for `/harden`'s merge-only mode.
    let hardened: serde_json::Value = client
        .get(format!("{}/api/v1/internal/hardened-state", base_url()))
        .query(&[("repo_root", repo_root), ("branch_name", branch.as_str())])
        .send()
        .await
        .expect("hardened-state GET failed")
        .json()
        .await
        .expect("hardened-state non-JSON");
    assert_eq!(hardened["head_sha"].as_str(), Some(head.trim()));

    // The running child does not withhold the parent's Apply.
    apply_ok(&client, first).await;

    assert_eq!(
        marker_state(&client, "planned-state", repo_root, &branch).await,
        "MISSING",
        "an apply must clear the branch's plan marker"
    );
    assert_eq!(
        marker_state(&client, "hardened-state", repo_root, &branch).await,
        "MISSING",
        "an apply must clear the branch's harden marker"
    );

    // The parent wakes and commits again on the same branch, in the same
    // worktree the apply kept and reset to main.
    std::fs::write(wt_dir.join(&file2), "second round").unwrap();
    git_in(&wt_dir, &["add", &file2]);
    git_in(&wt_dir, &["commit", "-m", "e2e parent: second round"]);

    // The proposal path mints a new id, because no pending change is left on
    // the branch. The one-pending-per-branch index must not trip on the
    // applied row.
    let second = Uuid::new_v4();
    seed_change_for_test(
        &client,
        second,
        parent,
        &branch,
        repo_root,
        "E2E parent second round",
        &[&file2],
        false,
        true,
    )
    .await;

    let body2 = apply_ok(&client, second).await;
    assert_eq!(body2["files_changed"], 1, "second apply: {body2:?}");
    // What the second apply landed on main. A catchup merge from a concurrent
    // test can add a merge commit here, so read subjects, not a count.
    let range = format!(
        "{}..{}",
        body2["previous_commit"].as_str().expect("previous_commit"),
        body2["applied_commit"].as_str().expect("applied_commit"),
    );
    let landed = String::from_utf8(git(&["log", "--format=%s", &range]).stdout).unwrap();
    assert!(
        landed.contains("e2e parent: second round"),
        "the second apply lands the new commit: {landed}"
    );
    assert!(
        !landed.contains("e2e parent: first round"),
        "the second apply must not re-land the applied commit: {landed}"
    );
    assert!(ws.join(&file1).exists() && ws.join(&file2).exists());

    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, status FROM changes WHERE id = ANY($1) ORDER BY created_at")
            .bind(&[first, second][..])
            .fetch_all(&pool)
            .await
            .expect("read the change rows");
    assert_eq!(
        rows,
        vec![
            (first, "applied".to_string()),
            (second, "applied".to_string())
        ],
        "two changes, each applied on its own"
    );

    // Clean up: files, worktree, branch, rows, markers.
    std::fs::remove_file(ws.join(&file1)).unwrap();
    std::fs::remove_file(ws.join(&file2)).unwrap();
    git(&["add", &file1, &file2]);
    git(&[
        "commit",
        "-m",
        &format!("chore: clean up e2e parent files ({})", suffix),
    ]);
    let _ = std::process::Command::new("git")
        .args(["worktree", "remove", "--force", wt])
        .current_dir(&ws)
        .output();
    let _ = std::process::Command::new("git")
        .args(["branch", "-D", &branch])
        .current_dir(&ws)
        .output();
    let _ = sqlx::query("DELETE FROM changes WHERE id = ANY($1)")
        .bind(&[first, second][..])
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(parent)
        .execute(&pool)
        .await;
    for table in ["planned_branches", "hardened_branches"] {
        let _ = sqlx::query(&format!("DELETE FROM {table} WHERE branch_name = $1"))
            .bind(&branch)
            .execute(&pool)
            .await;
    }
    pool.close().await;
}

/// An in-workspace CC thread with a pending change is NOT archivable — the
/// archive endpoint returns 409 `parent_has_pending_changes` and emits
/// nothing. The user must Apply or Discard the change first. Without this
/// gate, the change row is left pending in the `changes` table while the
/// thread sits in Archive. Aligns with `resolve_actions`, which already returns [Discard, Apply]
/// (never Archive) in this state.
#[tokio::test]
async fn archive_with_pending_change_is_rejected_409() {
    let client = user_client().await;
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

/// Canceling Apply All with no batch running is a clean 400 (nothing to cancel),
/// not a 500 or a silent success — the endpoint reports there's nothing running.
#[tokio::test]
async fn cancel_apply_all_with_no_batch_returns_bad_request() {
    let client = user_client().await;
    let url = format!("{}/api/v1/changes/apply-all/cancel", base_url());
    let resp = client
        .post(&url)
        .send()
        .await
        .expect("cancel apply-all request failed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::BAD_REQUEST,
        "cancel with no running batch must be 400"
    );
    let body: serde_json::Value = resp.json().await.expect("response body");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("No Apply All batch"),
        "error must explain nothing is running: {body:?}"
    );
}

/// A pending change whose branch diff has gone empty (its commits cancelled
/// out, so `reconcile_emptied_pending_change` re-synced the row to zero files)
/// cannot be applied: merging it only pushes no-op commits onto main, and for
/// an unhardened Lucidos-source change it would spend a whole harden-at-apply
/// session on an empty diff. `Discard` is the resolution, so the row must
/// survive the refusal untouched — the engine never resolves a change on the
/// user's behalf.
#[tokio::test]
async fn apply_change_with_no_files_is_rejected_409() {
    let client = user_client().await;
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap();

    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let branch = format!("e2e-test/empty-change-{}", suffix);
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();

    seed_cc_thread_summary(&pool, thread_id, "idle").await;
    seed_change_for_test(
        &client,
        change_id,
        thread_id,
        &branch,
        repo_root,
        "E2E reconciled-to-empty change",
        &[],
        false,
        true,
    )
    .await;

    let url = format!("{}/api/v1/changes/{}/apply", base_url(), change_id);
    let resp = client
        .post(&url)
        .send()
        .await
        .expect("apply request failed");
    assert_eq!(
        resp.status().as_u16(),
        409,
        "applying a change with no file changes must be refused"
    );
    let body: serde_json::Value = resp.json().await.expect("response body");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no file changes"),
        "the 409 must say why and point at Discard: {body:?}"
    );

    let row: Option<(String, i32)> =
        sqlx::query_as("SELECT status, file_count FROM changes WHERE id = $1")
            .bind(change_id)
            .fetch_optional(&pool)
            .await
            .expect("changes lookup");
    assert_eq!(
        row,
        Some(("pending".to_string(), 0)),
        "the refused change must stay pending for the user to discard"
    );

    // Discard is still available — it is how an empty change is resolved.
    let discard_url = format!("{}/api/v1/changes/{}/discard", base_url(), change_id);
    let discard = client
        .post(&discard_url)
        .send()
        .await
        .expect("discard request failed");
    assert!(
        discard.status().is_success(),
        "discard must stay available on an empty change, got {}",
        discard.status()
    );

    let _ = sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await;
    pool.close().await;
}

/// Poll a change row's status until it reads `want`, or fail after 10s. The
/// archive net runs off the request, so its row lands a moment later.
async fn await_change_status_for_branch(
    pool: &sqlx::PgPool,
    branch: &str,
    want: &str,
) -> (Uuid, bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let row: Option<(Uuid, String, bool)> =
            sqlx::query_as("SELECT id, status, incomplete FROM changes WHERE branch_name = $1")
                .bind(branch)
                .fetch_optional(pool)
                .await
                .expect("changes lookup");
        if let Some((id, status, incomplete)) = &row {
            if status == want {
                return (*id, *incomplete);
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "branch {branch} never reached {want}: {row:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
}

async fn post_change_verb(
    client: &reqwest::Client,
    change_id: Uuid,
    verb: &str,
) -> reqwest::Response {
    client
        .post(format!(
            "{}/api/v1/changes/{}/{}",
            base_url(),
            change_id,
            verb
        ))
        .send()
        .await
        .unwrap_or_else(|e| panic!("{verb} request failed: {e}"))
}

async fn listed_change_ids(client: &reqwest::Client, list: &str) -> Vec<String> {
    let body: serde_json::Value = client
        .get(format!("{}/api/v1/changes", base_url()))
        .send()
        .await
        .expect("list changes")
        .json()
        .await
        .expect("changes JSON");
    body[list]
        .as_array()
        .unwrap_or_else(|| panic!("no {list} list in {body}"))
        .iter()
        .filter_map(|c| c["id"].as_str().map(String::from))
        .collect()
}

/// A set-aside change leaves Review and does not block Archive. Apply refuses
/// it until it is brought back, and then it merges what was proposed, though
/// its worktree is long gone (ADR 0328).
#[tokio::test]
async fn a_set_aside_change_waits_out_of_the_way_until_brought_back() {
    let client = user_client().await;
    let ws = workspace_path();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let branch = format!("e2e-test/set-aside-{suffix}");
    let file = format!("e2e-set-aside-{suffix}.txt");
    commit_on_new_branch(&branch, &file, "kept for later");
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "idle").await;
    seed_change_for_test(
        &client,
        change_id,
        thread_id,
        &branch,
        ws.to_str().unwrap(),
        "E2E set aside",
        &[&file],
        false,
        true,
    )
    .await;

    let anon = post_change_verb(&http_client(), change_id, "set-aside").await;
    assert_eq!(anon.status().as_u16(), 401, "no credential, no move");

    let resp = post_change_verb(&client, change_id, "set-aside").await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "set aside: {:?}",
        resp.text().await
    );
    assert!(listed_change_ids(&client, "set_aside")
        .await
        .contains(&change_id.to_string()));
    assert!(!listed_change_ids(&client, "pending")
        .await
        .contains(&change_id.to_string()));
    let proposed: bool = sqlx::query_scalar(
        "SELECT coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("thread row");
    assert!(!proposed, "a set-aside change leaves Review and attention");

    let refused = post_change_verb(&client, change_id, "apply").await;
    assert_eq!(refused.status().as_u16(), 409);
    let body: serde_json::Value = refused.json().await.expect("refusal JSON");
    assert_eq!(body["reason"], "change_set_aside", "{body}");

    let archived = client
        .post(format!("{}/api/v1/threads/archive", base_url()))
        .json(&json!({ "thread_id": thread_id.to_string() }))
        .send()
        .await
        .expect("archive request");
    assert_eq!(
        archived.status().as_u16(),
        200,
        "a set-aside change never blocks Archive"
    );

    let resp = post_change_verb(&client, change_id, "bring-back").await;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "bring back: {:?}",
        resp.text().await
    );
    assert!(listed_change_ids(&client, "pending")
        .await
        .contains(&change_id.to_string()));

    let _tree = crate::support::workspace_tree_lock().write().await;
    let applied = apply_ok(&client, change_id).await;
    assert_eq!(applied["status"], "applied", "{applied}");
    assert!(ws.join(&file).exists(), "the set-aside work merged intact");

    std::fs::remove_file(ws.join(&file)).unwrap();
    git(&["add", &file]);
    git(&[
        "commit",
        "-m",
        &format!("chore: clean up e2e set-aside file ({suffix})"),
    ]);
    let _ = std::process::Command::new("git")
        .args(["branch", "-D", &branch])
        .current_dir(&ws)
        .output();
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

/// Archiving a thread whose branch holds unproposed work sets that work aside
/// instead of losing it. A branch the user already decided on is left alone,
/// so a discarded change never comes back (ADR 0328).
#[tokio::test]
async fn archive_sets_aside_unproposed_branch_work_but_never_a_decided_one() {
    let client = user_client().await;
    let ws = workspace_path();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let orphan_branch = format!("e2e-test/orphan-{suffix}");
    let decided_branch = format!("e2e-test/decided-{suffix}");
    commit_on_new_branch(
        &orphan_branch,
        &format!("e2e-orphan-{suffix}.txt"),
        "unproposed",
    );
    commit_on_new_branch(
        &decided_branch,
        &format!("e2e-decided-{suffix}.txt"),
        "discarded",
    );

    let orphan_thread = Uuid::new_v4();
    let decided_thread = Uuid::new_v4();
    for (thread_id, branch) in [
        (orphan_thread, &orphan_branch),
        (decided_thread, &decided_branch),
    ] {
        seed_cc_thread_summary(&pool, thread_id, "idle").await;
        insert_session_started(&pool, thread_id, branch).await;
    }
    sqlx::query(
        "INSERT INTO changes (request_id, branch_name, repo_root, thread_id, status)          VALUES ($1, $2, $3, $4, 'discarded')",
    )
    .bind(Uuid::new_v4())
    .bind(&decided_branch)
    .bind(ws.to_str().unwrap())
    .bind(decided_thread)
    .execute(&pool)
    .await
    .expect("seed the discarded change");

    // The decided thread goes first, so its net has run by the time the
    // orphan's row appears.
    for thread_id in [decided_thread, orphan_thread] {
        let resp = client
            .post(format!("{}/api/v1/threads/archive", base_url()))
            .json(&json!({ "thread_id": thread_id.to_string() }))
            .send()
            .await
            .expect("archive request");
        assert_eq!(resp.status().as_u16(), 200, "archive {thread_id}");
    }

    let (_, incomplete) = await_change_status_for_branch(&pool, &orphan_branch, "set_aside").await;
    assert!(
        incomplete,
        "work nobody finished reviewing is marked incomplete"
    );
    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(orphan_thread)
            .fetch_one(&pool)
            .await
            .expect("thread row");
    assert_eq!(
        section, "archived",
        "setting work aside leaves the thread archived"
    );

    let decided_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM changes WHERE branch_name = $1")
            .bind(&decided_branch)
            .fetch_one(&pool)
            .await
            .expect("count rows");
    assert_eq!(decided_rows, 1, "a discarded change is never resurrected");

    for branch in [&orphan_branch, &decided_branch] {
        let _ = std::process::Command::new("git")
            .args(["branch", "-D", branch])
            .current_dir(&ws)
            .output();
        let _ = sqlx::query("DELETE FROM changes WHERE branch_name = $1")
            .bind(branch)
            .execute(&pool)
            .await;
    }
    for thread_id in [orphan_thread, decided_thread] {
        let _ = sqlx::query("DELETE FROM events WHERE thread_id = $1")
            .bind(thread_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .execute(&pool)
            .await;
    }
    pool.close().await;
}
