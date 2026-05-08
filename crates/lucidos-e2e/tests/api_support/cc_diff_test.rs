use crate::support::{base_url, db_url, git_in, http_client, register_repo, unique_marker};
use serde_json::json;
use uuid::Uuid;

/// External-repo CC threads idled before the May-2026 cleanup consolidation
/// kept their branch + `cc_has_changes=true` after `agent_recovery` removed
/// the worktree dir. The handler used to 404 with "Worktree not found on disk"
/// even though the diff was still recoverable from `SessionStarted.repo_id` +
/// `branch`.
#[tokio::test]
async fn cc_diff_falls_back_to_branch_when_worktree_missing() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    git_in(repo, &["init", "-q", "-b", "main"]);
    git_in(repo, &["config", "user.email", "test@example.com"]);
    git_in(repo, &["config", "user.name", "test"]);
    std::fs::write(repo.join("base.txt"), "base\n").unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-q", "-m", "first"]);
    let branch = format!("claude-code/{}", unique_marker("cc-diff-fallback"));
    git_in(repo, &["checkout", "-q", "-b", &branch]);
    std::fs::write(repo.join("changed.txt"), "added on branch\n").unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-q", "-m", "branch commit"]);
    git_in(repo, &["checkout", "-q", "main"]);

    let repo_id = register_repo(&client, repo, "cc-diff-repo").await;

    // Direct INSERT skips the EventBus precondition that the thread already be
    // CC-classified in `thread_summaries` — the handler reads `events` only, so
    // there's no need to spin up a real CC session for this read-side check.
    let thread_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES ($1, 'SessionStarted', $2::jsonb, $3, 'thread', $3::text)",
    )
    .bind(Uuid::new_v4())
    .bind(
        json!({
            "branch": branch,
            "repo_id": repo_id,
            "channel": "claude_code",
            "session_id": "",
            "request_event_id": Uuid::new_v4().to_string(),
        })
        .to_string(),
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("insert SessionStarted failed");

    let dead_worktree = format!("/tmp/lucidos-test-missing-worktree-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES ($1, 'CodingAgentIdled', $2::jsonb, $3, 'thread', $3::text)",
    )
    .bind(Uuid::new_v4())
    .bind(
        json!({
            "worktree_path": dead_worktree,
            "has_changes": true,
            "is_external_repo": true,
        })
        .to_string(),
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("insert CodingAgentIdled failed");

    let resp = client
        .get(format!(
            "{}/api/threads/{}/cc-diff",
            base_url(),
            thread_id
        ))
        .send()
        .await
        .expect("cc-diff request failed");
    let status = resp.status().as_u16();
    let body_text = resp.text().await.expect("read body failed");
    assert_eq!(status, 200, "expected 200, got {}: {}", status, body_text);
    let body: serde_json::Value =
        serde_json::from_str(&body_text).expect("cc-diff response was not JSON");

    assert_eq!(
        body["branch_name"].as_str(),
        Some(branch.as_str()),
        "branch_name should round-trip from SessionStarted: {}",
        body
    );
    assert_eq!(
        body["repo_root"].as_str(),
        Some(repo.to_str().unwrap()),
        "repo_root should resolve to the registered repo path: {}",
        body
    );
    let files = body["files"].as_array().expect("files array missing");
    assert_eq!(
        files.len(),
        1,
        "expected exactly one changed file in the branch diff: {}",
        body
    );
    assert_eq!(
        files[0]["path"].as_str(),
        Some("changed.txt"),
        "expected the branch's changed.txt to appear: {}",
        body
    );
}
