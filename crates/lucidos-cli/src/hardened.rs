use std::path::PathBuf;
use std::process::Command;

use crate::http::client as http_client;
use crate::workspace::{BoxError, Workspace};

/// Hardening marker state for a branch, mirroring the engine's
/// `HardenMarkerState`. Wire format on the HTTP API is the literal string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HardenedState {
    Fresh,
    Stale,
    Missing,
}

impl HardenedState {
    pub(crate) fn parse(raw: &str) -> Self {
        match raw.trim() {
            "FRESH" => HardenedState::Fresh,
            "STALE" => HardenedState::Stale,
            // Unknown / unreachable engine / empty body => treat like Missing
            // so transient errors don't silently mask the reminder.
            _ => HardenedState::Missing,
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            HardenedState::Fresh => "FRESH",
            HardenedState::Stale => "STALE",
            HardenedState::Missing => "MISSING",
        }
    }
}

/// Resolve `(repo_root, branch, head_sha)` for the worktree at `cwd`.
pub(crate) fn git_context(cwd: &std::path::Path) -> Result<(PathBuf, String, String), BoxError> {
    let common = run_git(cwd, &["rev-parse", "--git-common-dir"])?;
    // git-common-dir may be relative (`.git`) or absolute (`/repo/.git`).
    let common_path = if std::path::Path::new(&common).is_absolute() {
        PathBuf::from(common)
    } else {
        std::fs::canonicalize(cwd.join(&common))
            .map_err(|e| format!("canonicalize git-common-dir: {}", e))?
    };
    let repo_root = common_path
        .parent()
        .ok_or("git-common-dir has no parent")?
        .to_path_buf();

    let branch = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch.is_empty() || branch == "HEAD" {
        return Err(format!("Could not resolve current branch (got {:?})", branch).into());
    }

    let head_sha = run_git(cwd, &["rev-parse", "HEAD"])?;
    if head_sha.is_empty() {
        return Err("Could not resolve HEAD".into());
    }

    Ok((repo_root, branch, head_sha))
}

pub(crate) fn run_git(cwd: &std::path::Path, args: &[&str]) -> Result<String, BoxError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("git {}: {}", args.join(" "), e))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) fn cmd_mark(ws: &Workspace) -> Result<(), BoxError> {
    let cwd = std::env::current_dir().map_err(|e| format!("Failed to read cwd: {}", e))?;
    let (repo_root, branch, head_sha) = git_context(&cwd)?;
    let url = format!("{}/api/v1/internal/mark-hardened", ws.base_url());
    let body = serde_json::json!({
        "repo_root": repo_root.to_string_lossy(),
        "branch_name": branch,
        "head_sha": head_sha,
    });
    let resp = http_client()?
        .post(&url)
        .json(&body)
        .send()
        .map_err(|e| format!("POST {} failed: {}", url, e))?;
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| format!("POST {} returned {}, body read failed: {}", url, status, e))?;
    if !status.is_success() {
        return Err(format!("POST {} returned {}: {}", url, status, text).into());
    }
    // `floor_char_boundary` rather than a byte index: `head_sha` is whatever
    // `git rev-parse` printed, and slicing subprocess output by byte panics on
    // anything multi-byte.
    println!(
        "Hardening recorded: {} {}",
        branch,
        &head_sha[..head_sha.floor_char_boundary(12)]
    );
    // An engine predating the report answers 204 with no body.
    let report: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    for line in mark_report_lines(&report) {
        println!("{line}");
    }
    Ok(())
}

/// What the engine did to the calling thread when it recorded the marker, as
/// lines for the agent: the leftover background tasks it stopped, and every
/// wait still live. Nothing for a caller with no thread.
pub(crate) fn mark_report_lines(report: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    let stopped = report
        .get("stopped_background_tasks")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();
    for task in stopped {
        let text = |key: &str| task.get(key).and_then(|v| v.as_str()).unwrap_or_default();
        lines.push(format!(
            "Stopped background task {} ({}): this hardening supersedes it, and its \
             completion will not re-open this thread.",
            text("task_id"),
            text("label"),
        ));
        let ended: Vec<&str> = task
            .get("ended_with_others")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|r| r.as_str()).collect())
            .unwrap_or_default();
        if !ended.is_empty() {
            lines.push(format!(
                "  That also ended your wait for {}, which watched more than this task.",
                ended.join("; ")
            ));
        }
    }
    let waiting: Vec<&str> = report
        .get("still_waiting_on")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|r| r.as_str()).collect())
        .unwrap_or_default();
    if !waiting.is_empty() {
        lines.push(format!(
            "Still waiting on: {}. Ending your turn leaves this thread waiting, and Apply \
             stays withheld until that resolves. Say so in your summary, or stand the wait \
             down with `lucidos event-waits cancel` if you no longer need it.",
            waiting.join("; ")
        ));
    }
    lines
}

/// GET the hardening state of the current branch from the parent engine.
/// Used by `cmd_query` (printing) and `cc_stop_reminder` (deciding).
pub(crate) fn query_state(ws: &Workspace) -> Result<HardenedState, BoxError> {
    let (url, _branch, body) = fetch_marker(ws)?;
    let state = body
        .get("state")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("GET {} response missing `state`: {}", url, body))?;
    Ok(HardenedState::parse(state))
}

/// GET `/api/v1/internal/hardened-state` for the branch in `$PWD`. Returns the
/// URL (for error messages), the branch, and the JSON body.
fn fetch_marker(ws: &Workspace) -> Result<(String, String, serde_json::Value), BoxError> {
    let cwd = std::env::current_dir().map_err(|e| format!("Failed to read cwd: {}", e))?;
    let (repo_root, branch, _head_sha) = git_context(&cwd)?;
    let url = format!("{}/api/v1/internal/hardened-state", ws.base_url());
    let resp = http_client()?
        .get(&url)
        .query(&[
            ("repo_root", repo_root.to_string_lossy().as_ref()),
            ("branch_name", branch.as_str()),
        ])
        .send()
        .map_err(|e| format!("GET {} failed: {}", url, e))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp
            .text()
            .map_err(|e| format!("GET {} returned {}, body read failed: {}", url, status, e))?;
        return Err(format!("GET {} returned {}: {}", url, status, text).into());
    }
    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("GET {} returned non-JSON body: {}", url, e))?;
    Ok((url, branch, body))
}

/// Print `FRESH`, `STALE`, or `MISSING` for the current branch to stdout.
/// Transport / git-context errors go to stderr with exit 1.
pub(crate) fn cmd_query(ws: &Workspace) -> Result<(), BoxError> {
    let state = query_state(ws)?;
    println!("{}", state.as_str());
    Ok(())
}

/// Print the HEAD SHA the last `/harden` recorded for the current branch.
/// No marker is an error (exit 1), so a caller cannot mistake it for a SHA.
pub(crate) fn cmd_sha(ws: &Workspace) -> Result<(), BoxError> {
    let (_url, branch, body) = fetch_marker(ws)?;
    let sha = hardened_sha(&body).ok_or_else(|| format!("No harden marker for {}", branch))?;
    println!("{}", sha);
    Ok(())
}

fn hardened_sha(body: &serde_json::Value) -> Option<&str> {
    body.get("head_sha")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_known_states() {
        assert_eq!(HardenedState::parse("FRESH"), HardenedState::Fresh);
        assert_eq!(HardenedState::parse("STALE"), HardenedState::Stale);
        assert_eq!(HardenedState::parse("MISSING"), HardenedState::Missing);
    }

    #[test]
    fn parse_falls_back_to_missing_for_unknown_or_empty() {
        // Empty body / unreachable engine must not silently mask a real
        // unhardened branch — treat as Missing so the reminder still fires.
        assert_eq!(HardenedState::parse(""), HardenedState::Missing);
        assert_eq!(HardenedState::parse("???"), HardenedState::Missing);
    }

    #[test]
    fn parse_strips_trailing_whitespace() {
        // The HTTP body is JSON-extracted so trim is belt-and-braces, but
        // covers the case where someone pipes `lucidos hardened query` output.
        assert_eq!(HardenedState::parse("FRESH\n"), HardenedState::Fresh);
    }

    #[test]
    fn hardened_sha_reads_the_recorded_head() {
        let body = serde_json::json!({ "state": "STALE", "head_sha": "abc123" });
        assert_eq!(hardened_sha(&body), Some("abc123"));
    }

    #[test]
    fn hardened_sha_is_none_without_a_marker() {
        assert_eq!(
            hardened_sha(&serde_json::json!({ "state": "MISSING" })),
            None
        );
        // An empty SHA must not print as one: a blank line reads as a revision.
        assert_eq!(
            hardened_sha(&serde_json::json!({ "state": "FRESH", "head_sha": "" })),
            None
        );
    }

    /// The evidence case: a leftover `make lint` the marker stopped. The agent
    /// must read that it was stopped and that it will not wake the thread.
    #[test]
    fn a_stopped_leftover_task_is_named_with_its_consequence() {
        let lines = mark_report_lines(&serde_json::json!({
            "stopped_background_tasks": [{
                "task_id": "t1",
                "label": "make lint",
                "ended_with_others": [],
            }],
            "still_waiting_on": [],
        }));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("Stopped background task t1 (make lint)"));
        assert!(lines[0].contains("will not re-open this thread"));
    }

    /// A wait the marker cannot judge stays, so the agent must hear that its
    /// turn will not end finished: the thread keeps waiting and Apply is held.
    #[test]
    fn a_wait_still_live_is_named_with_what_it_withholds() {
        let lines = mark_report_lines(&serde_json::json!({
            "stopped_background_tasks": [],
            "still_waiting_on": ["the release build to finish"],
        }));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("Still waiting on: the release build to finish."));
        assert!(lines[0].contains("Apply stays withheld"));
    }

    #[test]
    fn a_model_wait_ended_whole_is_named_under_its_task() {
        let lines = mark_report_lines(&serde_json::json!({
            "stopped_background_tasks": [{
                "task_id": "t1",
                "label": "make lint",
                "ended_with_others": ["the lint or the review"],
            }],
        }));
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[1].contains("the lint or the review"));
    }

    /// No thread (the user's shell), or an engine predating the report.
    #[test]
    fn an_empty_or_missing_report_prints_nothing() {
        assert!(mark_report_lines(&serde_json::Value::Null).is_empty());
        assert!(mark_report_lines(&serde_json::json!({
            "stopped_background_tasks": [],
            "still_waiting_on": [],
        }))
        .is_empty());
    }
}
