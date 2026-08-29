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
    if !status.is_success() {
        let text = resp
            .text()
            .map_err(|e| format!("POST {} returned {}, body read failed: {}", url, status, e))?;
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
    Ok(())
}

/// GET the hardening state of the current branch from the parent engine.
/// Used by `cmd_query` (printing) and `cc_stop_reminder` (deciding).
pub(crate) fn query_state(ws: &Workspace) -> Result<HardenedState, BoxError> {
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
    let state = body
        .get("state")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("GET {} response missing `state`: {}", url, body))?;
    Ok(HardenedState::parse(state))
}

/// Print `FRESH`, `STALE`, or `MISSING` for the current branch to stdout.
/// Transport / git-context errors go to stderr with exit 1.
pub(crate) fn cmd_query(ws: &Workspace) -> Result<(), BoxError> {
    let state = query_state(ws)?;
    println!("{}", state.as_str());
    Ok(())
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
}
