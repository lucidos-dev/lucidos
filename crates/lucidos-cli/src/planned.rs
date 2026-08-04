//! `lucidos planned` — record/query the durable Planned marker that enforces
//! the `implementation-plan` skill. Mirrors `hardened.rs` (same `git_context`
//! and HTTP-to-parent-engine pattern). The marker is set by the skill
//! (`mark --plan <docs/plans/file>` → records the awaiting-approval `proposed`
//! state), approved by the agent after the user's chat approval
//! (`approve` → flips `proposed` to gate-satisfying `planned`), or set directly
//! for a local fix (`mark --simple "<reason>"` → `acknowledged_simple`, no
//! approval needed). `planned` and `acknowledged_simple` satisfy every gate;
//! `proposed` and the absence of a marker both block.

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

/// Planned-marker gate state, mirroring the engine's three-way wire `state`
/// field. `Satisfied` = an approved plan or a `--simple` ack (gate passes);
/// `Proposed` = a plan awaiting the user's approval (gate blocks, but the path
/// forward is `approve`, not re-planning); `Missing` = no marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlannedState {
    Satisfied,
    Proposed,
    Missing,
}

impl PlannedState {
    pub(crate) fn parse(raw: &str) -> Self {
        match raw.trim() {
            "SATISFIED" => PlannedState::Satisfied,
            "PROPOSED" => PlannedState::Proposed,
            // Unknown / unreachable engine / empty body => Missing so a
            // transient error doesn't silently let an unplanned edit through.
            _ => PlannedState::Missing,
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            PlannedState::Satisfied => "SATISFIED",
            PlannedState::Proposed => "PROPOSED",
            PlannedState::Missing => "MISSING",
        }
    }
}

pub(crate) fn cmd_mark(ws: &Workspace, kind: MarkKind<'_>) -> Result<(), BoxError> {
    let cwd = std::env::current_dir().map_err(|e| format!("Failed to read cwd: {}", e))?;
    let (repo_root, branch, head_sha) = git_context(&cwd)?;
    let url = format!("{}/api/v1/internal/mark-planned", ws.base_url());
    let (state, plan_path, reason) = match kind {
        // A written plan starts AWAITING APPROVAL — the skill records `proposed`,
        // the user approves in chat, then the agent runs `planned approve`.
        MarkKind::Plan(p) => ("proposed", Some(p), None),
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
        // Naming the question tool here matters: this line is what the agent
        // reads at the exact moment it turns to present the plan. Told only to
        // "present it", it writes prose and the thread sits idle until the user
        // types "approve" by hand (2026-08-02). Both tool names are spelled out
        // because the CLI serves both backends and cannot tell them apart:
        // Claude Code has `AskUserQuestion`, Codex has `ask_user_question` on
        // the Lucidos MCP server, and sending either after the other's tool
        // would strand a proposed plan with no way to get it approved.
        MarkKind::Plan(p) => println!(
            "Plan recorded (awaiting approval): {} ({}). Present it, then ask for approval with your question tool (`AskUserQuestion` on Claude Code, `ask_user_question` on Codex; options `Approve` / `Request changes`), not in prose. That pair is a floor: if the plan offers a real fork, that fork takes the second slot and `Request changes` is dropped rather than carried as a third. Once approved, run `lucidos planned approve`.",
            branch, p
        ),
        MarkKind::Simple(r) => println!("Simple change acknowledged: {} ({})", branch, r),
    }
    Ok(())
}

/// Approve the proposed plan on the current branch (in $PWD), flipping the
/// marker to gate-satisfying `planned` so source edits and Apply unblock. Run
/// by the coding agent AFTER the user approves the plan in chat. POSTs to the
/// parent engine's `/api/v1/internal/approve-plan`.
pub(crate) fn cmd_approve(ws: &Workspace) -> Result<(), BoxError> {
    let cwd = std::env::current_dir().map_err(|e| format!("Failed to read cwd: {}", e))?;
    let (repo_root, branch, _head_sha) = git_context(&cwd)?;
    let url = format!("{}/api/v1/internal/approve-plan", ws.base_url());
    let body = serde_json::json!({
        "repo_root": repo_root.to_string_lossy(),
        "branch_name": branch,
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
    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("POST {} returned non-JSON body: {}", url, e))?;
    if body.get("approved").and_then(|v| v.as_bool()) == Some(true) {
        println!("Plan approved: {}. Implementation is unblocked.", branch);
    } else {
        // Nothing to flip — no proposed plan (already approved, a --simple ack,
        // or no marker at all). Surface it so the agent doesn't assume success.
        println!(
            "No proposed plan to approve on {} (already approved, a simple-fix ack, or no marker). \
             Run `lucidos planned state` to check.",
            branch
        );
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

/// Print `SATISFIED`, `PROPOSED`, or `MISSING` for the current branch to stdout.
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
        assert_eq!(PlannedState::parse("SATISFIED"), PlannedState::Satisfied);
        assert_eq!(PlannedState::parse("PROPOSED"), PlannedState::Proposed);
        assert_eq!(PlannedState::parse("MISSING"), PlannedState::Missing);
    }

    #[test]
    fn parse_falls_back_to_missing_for_unknown_or_empty() {
        // Empty body / unreachable engine must not silently mask an unplanned
        // branch — treat as Missing so the gate still fires. A drifted/unknown
        // value must NOT be read as the satisfying state.
        assert_eq!(PlannedState::parse(""), PlannedState::Missing);
        assert_eq!(PlannedState::parse("???"), PlannedState::Missing);
        assert_eq!(PlannedState::parse("PRESENT"), PlannedState::Missing);
    }

    #[test]
    fn parse_strips_trailing_whitespace() {
        assert_eq!(PlannedState::parse("SATISFIED\n"), PlannedState::Satisfied);
        assert_eq!(PlannedState::parse("PROPOSED\n"), PlannedState::Proposed);
    }
}
