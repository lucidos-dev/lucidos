use crate::support::{
    base_url, db_url, git, git_in, http_client, register_repo, seed_app_cc_thread_summary,
    unique_marker, workspace_path,
};
use serde_json::json;
use uuid::Uuid;

/// External-repo CC threads idled before the May-2026 cleanup consolidation
/// kept their branch + `coding_agent_proposed=true` after `agent_recovery` removed
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
            "{}/api/v1/threads/{}/cc-diff",
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

/// The deterministic worktree directory for a thread, mirroring the engine's
/// `agent_session::resume::deterministic_worktree_path`: the first 8 hex chars
/// of the thread id under `<ws>/.lucidos/worktrees/`.
fn deterministic_worktree(thread_id: Uuid) -> std::path::PathBuf {
    let short: String = thread_id.as_simple().to_string().chars().take(8).collect();
    workspace_path()
        .join(".lucidos/worktrees")
        .join(format!("thread-{}", short))
}

/// An app coding-agent thread commits during its FIRST turn. The post-commit
/// hook lights the Diff button up, and the click used to answer 404: no
/// `CodingAgentIdled` has recorded a worktree yet, and an app thread carries no
/// `SessionStarted.repo_id` for the branch-ref fallback to work from. The
/// worktree is at its deterministic path throughout.
#[tokio::test]
async fn cc_diff_finds_the_first_turn_worktree_of_an_app_thread() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    let app_id = unique_marker("cc-diff-first-turn");
    let branch = format!("lucidos-claude-code-app-{}", app_id);
    seed_app_cc_thread_summary(&pool, thread_id, &app_id, "running").await;

    // An app spawn's SessionStarted: a branch, and deliberately no `repo_id`.
    // Its presence is what makes the assertion below meaningful. The handler
    // has a session to read and still cannot reach a repo from it.
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES ($1, 'SessionStarted', $2::jsonb, $3, 'thread', $3::text)",
    )
    .bind(Uuid::new_v4())
    .bind(
        json!({
            "branch": branch,
            "channel": "claude_code",
            "session_id": "",
            "coding_agent_kind": "app",
            "app_id": app_id,
        })
        .to_string(),
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("insert SessionStarted failed");

    // The worktree the running turn is writing in. `.lucidos/` is gitignored,
    // and the commit lands on the branch only, so the workspace tree is
    // untouched and no `workspace_tree_lock` guard is owed.
    let wt = deterministic_worktree(thread_id);
    git(&[
        "worktree",
        "add",
        wt.to_str().unwrap(),
        "-b",
        &branch,
        "main",
    ]);
    let app_dir = wt.join("data/apps").join(&app_id);
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(app_dir.join("index.html"), "<!doctype html>\n").unwrap();
    std::fs::create_dir_all(wt.join("data/artifacts")).unwrap();
    std::fs::write(wt.join("data/artifacts/stray.md"), "out of scope\n").unwrap();
    git_in(&wt, &["add", "-A"]);
    git_in(&wt, &["commit", "-q", "-m", "first turn, mid-turn commit"]);

    let resp = client
        .get(format!(
            "{}/api/v1/threads/{}/cc-diff",
            base_url(),
            thread_id
        ))
        .send()
        .await
        .expect("cc-diff request failed");
    let status = resp.status().as_u16();
    let body_text = resp.text().await.expect("read body failed");

    // Clean up before asserting, so a failure doesn't strand the worktree.
    let ws = workspace_path();
    let _ = std::process::Command::new("git")
        .args(["worktree", "remove", "--force", wt.to_str().unwrap()])
        .current_dir(&ws)
        .output();
    let _ = std::fs::remove_dir_all(&wt);
    let _ = std::process::Command::new("git")
        .args(["branch", "-D", &branch])
        .current_dir(&ws)
        .output();
    let _ = sqlx::query("DELETE FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await;

    assert_eq!(status, 200, "expected 200, got {}: {}", status, body_text);
    let body: serde_json::Value =
        serde_json::from_str(&body_text).expect("cc-diff response was not JSON");
    assert_eq!(
        body["branch_name"].as_str(),
        Some(branch.as_str()),
        "branch_name should be the worktree's own branch: {}",
        body
    );
    let paths: Vec<&str> = body["files"]
        .as_array()
        .expect("files array missing")
        .iter()
        .filter_map(|f| f["path"].as_str())
        .collect();
    let in_scope = format!("data/apps/{}/index.html", app_id);
    assert_eq!(
        paths,
        vec![in_scope.as_str()],
        "expected the in-scope app file only, got {:?}: {}",
        paths,
        body
    );
}
