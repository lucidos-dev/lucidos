//! `lucidos planned` — record/query the durable Planned marker that enforces
//! the `implementation-plan` skill. Mirrors `hardened.rs` (same `git_context`
//! + HTTP-to-parent-engine pattern). The marker is set either by the skill
//! (`mark --plan <docs/plans/file>`) or by the agent acknowledging a local fix
//! (`mark --simple "<reason>"`). Both states satisfy every gate; only the
//! absence of a marker blocks.

use crate::hardened::git_context;
use crate::http::client as http_client;
use crate::workspace::{BoxError, Workspace};

/// Which kind of planning decision is being recorded.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MarkKind<'a> {
    /// A real plan was written to `docs/plans/`; carries the relative path.
    Plan(&'a str),
    /// The agent declared a local fix; carries the one-line reason.
    Simple(&'a str),
}

/// Planned-marker presence, mirroring the engine's `PlanMarkerState`. The wire
/// `state` field is the literal `"PRESENT"` / `"MISSING"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlannedState {
    Present,
    Missing,
}

impl PlannedState {
    pub(crate) fn parse(raw: &str) -> Self {
        match raw.trim() {
            "PRESENT" => PlannedState::Present,
            // Unknown / unreachable engine / empty body => Missing so a
            // transient error doesn't silently let an unplanned edit through.
            _ => PlannedState::Missing,
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            PlannedState::Present => "PRESENT",
            PlannedState::Missing => "MISSING",
        }
    }
}

pub(crate) fn cmd_mark(ws: &Workspace, kind: MarkKind<'_>) -> Result<(), BoxError> {
    let cwd = std::env::current_dir().map_err(|e| format!("Failed to read cwd: {}", e))?;
    let (repo_root, branch, head_sha) = git_context(&cwd)?;
    let url = format!("{}/api/v1/internal/mark-planned", ws.base_url());
    let (state, plan_path, reason) = match kind {
        MarkKind::Plan(p) => ("planned", Some(p), None),
        MarkKind::Simple(r) => ("acknowledged_simple", None, Some(r)),
    };
    let body = serde_json::json!({
        "repo_root": repo_root.to_string_lossy(),
        "branch_name": branch,
        "head_sha": head_sha,
        "state": state,
        "plan_path": plan_path,
        "reason": reason,
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
    match kind {
        MarkKind::Plan(p) => println!("Plan recorded: {} ({})", branch, p),
        MarkKind::Simple(r) => println!("Simple change acknowledged: {} ({})", branch, r),
    }
    Ok(())
}

/// GET the Planned-marker state of the current branch from the parent engine.
/// Used by `cmd_state` (printing) and `cc_plan_gate` (deciding).
pub(crate) fn query_state(ws: &Workspace) -> Result<PlannedState, BoxError> {
    let cwd = std::env::current_dir().map_err(|e| format!("Failed to read cwd: {}", e))?;
    let (repo_root, branch, _head_sha) = git_context(&cwd)?;
    let url = format!("{}/api/v1/internal/planned-state", ws.base_url());
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
    Ok(PlannedState::parse(state))
}

/// Print `PRESENT` or `MISSING` for the current branch to stdout.
pub(crate) fn cmd_state(ws: &Workspace) -> Result<(), BoxError> {
    let state = query_state(ws)?;
    println!("{}", state.as_str());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_known_states() {
        assert_eq!(PlannedState::parse("PRESENT"), PlannedState::Present);
        assert_eq!(PlannedState::parse("MISSING"), PlannedState::Missing);
    }

    #[test]
    fn parse_falls_back_to_missing_for_unknown_or_empty() {
        // Empty body / unreachable engine must not silently mask an unplanned
        // branch — treat as Missing so the gate still fires.
        assert_eq!(PlannedState::parse(""), PlannedState::Missing);
        assert_eq!(PlannedState::parse("???"), PlannedState::Missing);
    }

    #[test]
    fn parse_strips_trailing_whitespace() {
        assert_eq!(PlannedState::parse("PRESENT\n"), PlannedState::Present);
    }
}
