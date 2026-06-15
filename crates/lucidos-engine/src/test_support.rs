//! Shared helpers for unit tests that need a real Postgres connection.
//! Each call to `setup_test_db` creates a fresh database, runs migrations,
//! and returns the pool plus the database name to pass back to `teardown_test_db`.

#![cfg(test)]

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::path::PathBuf;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};

fn admin_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://lucidos:lucidos@localhost:5432/postgres".into())
}

pub async fn setup_test_db() -> (PgPool, String) {
    let base_url = admin_url();
    let db_name = format!(
        "lucidos_test_{}",
        Uuid::new_v4().to_string().replace('-', "")
    );
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base_url)
        .await
        .expect("admin connect");
    sqlx::query(&format!("CREATE DATABASE \"{}\"", db_name))
        .execute(&admin_pool)
        .await
        .expect("create db");
    admin_pool.close().await;
    let test_url = base_url.replace("/postgres", &format!("/{}", db_name));
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&test_url)
        .await
        .expect("connect test db");
    sqlx::migrate!().run(&pool).await.expect("migrations");
    (pool, db_name)
}

/// Build a tiny git repo with `main` + initial commit and a worktree on
/// `branch`. Returns `(tmpdir, repo_root, worktree_path)`. Caller must keep
/// `tmpdir` in scope for the test duration — dropping it `rm -rf`'s the repo.
///
/// Shared by tests that need a real on-disk git repo + worktree pair (the
/// `seed_coding_agent_has_diff` tests in `session_seed_tests.rs` and the startup
/// sweep tests in `agent_recovery_tests.rs`). The same boilerplate also lives
/// in git_ops_tests.rs / worktree_cleanup_tests.rs — those older copies are out
/// of scope for this helper, follow-up DRY sweep.
pub async fn make_repo_and_worktree(branch: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();

    use crate::engine::git_ops::git_cmd;
    git_cmd(&["init", "-b", "main"], &repo).await.unwrap();
    git_cmd(&["config", "user.email", "test@example.com"], &repo)
        .await
        .unwrap();
    git_cmd(&["config", "user.name", "Test"], &repo)
        .await
        .unwrap();
    std::fs::write(repo.join("seed.txt"), "x").unwrap();
    git_cmd(&["add", "."], &repo).await.unwrap();
    git_cmd(&["commit", "-m", "init"], &repo).await.unwrap();

    let wt = tmp.path().join("wt");
    git_cmd(
        &["worktree", "add", wt.to_str().unwrap(), "-b", branch],
        &repo,
    )
    .await
    .unwrap();

    (tmp, repo, wt)
}

/// Read the `coding_agent_has_diff` projection column for a single thread.
/// Panics if the row is missing — every caller seeds a `SessionStarted` first,
/// so a missing row means the projection write didn't land and the test should
/// fail loudly rather than silently observe `false`.
pub async fn read_coding_agent_has_diff(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Emit `SessionStarted` on the `CodingAgent` channel so the projection
/// upserts an `is_coding_agent=true, state='active'` row in `thread_summaries`. Pass
/// `repo_id = Some(...)` to stamp `cc_repo_id` on the projection row (the
/// external-repo branch); `None` covers the in-workspace common case.
///
/// `session_id` is a fixed sentinel — no test reads it back. The branch and
/// repo_id are the per-test inputs that actually matter.
pub async fn start_cc_session(
    bus: &EventBus,
    thread_id: Uuid,
    branch: &str,
    repo_id: Option<String>,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
            branch: branch.into(),
            repo_id,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

pub async fn teardown_test_db(db_name: &str) {
    let base_url = admin_url();
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base_url)
        .await
        .expect("admin connect");
    let _ = sqlx::query(&format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
        db_name
    ))
    .execute(&admin_pool)
    .await;
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", db_name))
        .execute(&admin_pool)
        .await;
    admin_pool.close().await;
}
