//! Git post-commit hook callback for coding-agent worktrees.
//!
//! The installed shell hook calls this after each commit. It is intentionally
//! silent and safe to run outside Lucidos-spawned processes: without
//! `LUCIDOS_THREAD_ID`, it no-ops so terminal users are not affected.

use std::time::Instant;

use serde::Serialize;

use crate::hardened::git_context;
use crate::http::{client as http_client, format_request_error};
use crate::workspace::{resolve_from_env, BoxError};

const ENV_SOURCE_THREAD_ID: &str = "LUCIDOS_THREAD_ID";

#[derive(Serialize)]
struct DiffRefreshRequest {
    thread_id: String,
    repo_root: String,
    branch_name: String,
}

pub(crate) fn run() -> Result<(), BoxError> {
    let Some(thread_id) = std::env::var(ENV_SOURCE_THREAD_ID)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return Ok(());
    };

    let cwd = std::env::current_dir().map_err(|e| format!("Failed to read cwd: {}", e))?;
    let (repo_root, branch_name, _head_sha) = git_context(&cwd)?;
    let ws = resolve_from_env()?;
    let url = format!(
        "{}/api/v1/internal/coding-agent-diff-refresh",
        ws.base_url()
    );
    let body = DiffRefreshRequest {
        thread_id,
        repo_root: repo_root.to_string_lossy().to_string(),
        branch_name,
    };

    let start = Instant::now();
    let resp = http_client()?
        .post(&url)
        .json(&body)
        .send()
        .map_err(|e| format_request_error("POST", &url, &e, start.elapsed()))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!("POST {} returned {}: {}", url, status, text).into());
    }

    Ok(())
}
