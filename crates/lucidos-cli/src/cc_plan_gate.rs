//! PreToolUse hook for Claude Code's `Edit` and `Write` tools — the early
//! enforcement of the `implementation-plan` skill. If the branch has no
//! Planned marker yet, a source edit is denied with a reason telling the model
//! to either run the `implementation-plan` skill (complex work) or acknowledge
//! a local fix via `lucidos planned mark --simple "<reason>"`. Once a marker
//! exists (either kind), every edit passes.
//!
//! Two deliberate exemptions keep the gate from deadlocking or misfiring:
//!   * **`docs/plans/` writes are allowed.** Writing the plan file is itself
//!     part of planning and happens BEFORE the marker is recorded — gating it
//!     would make the skill impossible to run.
//!   * **No skill, no gate.** External repos (and app worktrees) don't ship
//!     `.claude/skills/implementation-plan/SKILL.md`; there the hook is a
//!     silent no-op, mirroring how `cc-stop-reminder` skips when
//!     `.claude/commands/harden.md` is absent.
//!
//! Wired into `<workspace>/.lucidos/cc-settings.json` via `cc_settings.rs`,
//! on the Edit/Write matchers. Fails OPEN on parse / I/O / engine-unreachable
//! errors so a hook bug can't brick every Edit/Write call. Claude-Code only —
//! Codex has no PreToolUse hook and is covered by the prompt rule + Apply floor.

use serde::Deserialize;
use std::io::Read;
use std::path::Path;

use crate::planned::{query_state, PlannedState};
use crate::workspace::{resolve_from_env, BoxError};

#[derive(Debug, Deserialize)]
struct HookPayload {
    #[serde(default)]
    tool_name: String,
    tool_input: ToolInput,
}

#[derive(Debug, Deserialize)]
struct ToolInput {
    #[serde(default)]
    file_path: String,
}

/// Tools we gate. Both need a Planned marker before they touch source.
pub(crate) fn is_gated_tool(name: &str) -> bool {
    matches!(name, "Edit" | "Write")
}

/// The skill ships at this path in Lucidos-source worktrees; absent in external
/// repos and app worktrees, where the gate must be a silent no-op.
fn plan_skill_available(cwd: &Path) -> bool {
    cwd.join(".claude/skills/implementation-plan/SKILL.md")
        .is_file()
}

/// Writing the plan file itself must never be blocked by the missing-plan gate
/// — it happens before the marker is recorded. `docs/plans/` is the repo
/// convention (also the auto-commit allow-list in `git_ops::commits`).
pub(crate) fn is_plan_artifact(file_path: &str) -> bool {
    file_path.contains("docs/plans/")
}

/// Deny reason for a `Missing` marker — no plan recorded yet.
pub(crate) fn build_deny_json(file_path: &str) -> String {
    let reason = format!(
        "Edit blocked: this branch has no implementation-plan marker yet. Before editing source, \
         decide: if this is complex work (cross-layer, or any routing / topology / storage / \
         security / migration / process change, ADR- or design-backed, or anything beyond a local \
         bug fix), run the `implementation-plan` skill first — it writes docs/plans/<date>-<slug>.md \
         and records a `proposed` marker for the user to approve. If this is a genuinely local fix, \
         acknowledge it with `lucidos planned mark --simple \"<one-line reason>\"`. Then retry your \
         edit to `{path}`.",
        path = file_path,
    );
    deny_envelope(&reason)
}

/// Deny reason for a `Proposed` marker — a plan exists but the user hasn't
/// approved it. The path forward is approval, NOT re-running the skill.
///
/// It names the `AskUserQuestion` tool and its option pair because prose alone
/// is what failed: told only to "present the plan and wait", the agent wrote a
/// summary and ended the turn, and the thread sat idle until the user typed
/// "approve" by hand (2026-08-02). Approval is a DECISION question, and this
/// hook firing is the proof: source edits are blocked until it is answered.
pub(crate) fn build_awaiting_approval_json(file_path: &str) -> String {
    let reason = format!(
        "Edit blocked: the implementation plan on this branch is awaiting the user's approval. \
         Do NOT re-run the `implementation-plan` skill — the plan is already recorded. Present the \
         plan, then ASK FOR APPROVAL WITH THE `AskUserQuestion` TOOL, offering `Approve` and \
         `Request changes`. Approval is a DECISION question, not a post-work confirmation: this \
         very hook is blocking your edits until it is answered, so asking in plain prose leaves \
         the thread idle. Once the user approves, run `lucidos planned approve` to unblock \
         implementation, then retry your edit to `{path}`. If the user requests changes, revise \
         the plan file under docs/plans/, re-commit, and ask again the same way (the marker stays \
         `proposed` until approved).",
        path = file_path,
    );
    deny_envelope(&reason)
}

fn deny_envelope(reason: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

pub(crate) fn run() -> Result<(), BoxError> {
    let mut buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
        eprintln!("cc-plan-gate: stdin read failed, allowing: {}", e);
        return Ok(());
    }
    let payload: HookPayload = match serde_json::from_str(&buf) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cc-plan-gate: payload parse failed, allowing: {}", e);
            return Ok(());
        }
    };
    if !is_gated_tool(&payload.tool_name) {
        // settings.json only routes Edit/Write here; an unknown matcher means
        // CC's contract changed — fall through rather than denying every call.
        return Ok(());
    }
    if payload.tool_input.file_path.is_empty() {
        return Ok(());
    }
    if is_plan_artifact(&payload.tool_input.file_path) {
        // Writing the plan itself is part of planning — never gate it.
        return Ok(());
    }

    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cc-plan-gate: cwd failed, allowing: {}", e);
            return Ok(());
        }
    };
    if !plan_skill_available(&cwd) {
        // External repo / app worktree — the skill isn't shipped here.
        return Ok(());
    }

    let ws = match resolve_from_env() {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("cc-plan-gate: workspace resolve failed, allowing: {}", e);
            return Ok(());
        }
    };

    match query_state(&ws) {
        Ok(PlannedState::Satisfied) => Ok(()),
        Ok(PlannedState::Proposed) => {
            println!(
                "{}",
                build_awaiting_approval_json(&payload.tool_input.file_path)
            );
            Ok(())
        }
        Ok(PlannedState::Missing) => {
            println!("{}", build_deny_json(&payload.tool_input.file_path));
            Ok(())
        }
        Err(e) => {
            // Engine unreachable / git-context / parse failure — fail OPEN so a
            // transient hiccup doesn't block every edit. The Apply-time floor
            // is the durable backstop if a marker-less change still slips through.
            eprintln!("cc-plan-gate: engine check failed, allowing: {}", e);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn is_gated_tool_recognises_edit_and_write_only() {
        assert!(is_gated_tool("Edit"));
        assert!(is_gated_tool("Write"));
        assert!(!is_gated_tool("Read"));
        assert!(!is_gated_tool("Bash"));
        assert!(!is_gated_tool(""));
    }

    #[test]
    fn plan_artifact_exempts_docs_plans_writes() {
        // The plan file is written before the marker exists; gating it would
        // deadlock the skill.
        assert!(is_plan_artifact(
            "/wt/docs/plans/2026-06-18-enforce-plan.md"
        ));
        assert!(is_plan_artifact("docs/plans/x.md"));
        assert!(!is_plan_artifact("/wt/crates/lucidos-engine/src/lib.rs"));
        assert!(!is_plan_artifact("/wt/docs/adr/0001.md"));
    }

    #[test]
    fn deny_json_uses_documented_envelope_and_names_recovery() {
        let out = build_deny_json("/tmp/x.rs");
        let parsed: Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "deny");
        let reason = parsed["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("reason must be a string");
        assert!(
            reason.contains("/tmp/x.rs"),
            "reason must name the file so the retry targets the same path: {reason}"
        );
        assert!(
            reason.contains("implementation-plan"),
            "reason must name the skill: {reason}"
        );
        assert!(
            reason.contains("lucidos planned mark --simple"),
            "reason must offer the local-fix escape: {reason}"
        );
    }

    #[test]
    fn awaiting_approval_json_directs_to_approve_not_replan() {
        let out = build_awaiting_approval_json("/tmp/x.rs");
        let parsed: Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "deny");
        let reason = parsed["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("reason must be a string");
        assert!(
            reason.contains("/tmp/x.rs"),
            "reason must name the file: {reason}"
        );
        assert!(
            reason.contains("lucidos planned approve"),
            "awaiting-approval reason must name the approve command: {reason}"
        );
        assert!(
            reason.contains("approval"),
            "awaiting-approval reason must mention approval: {reason}"
        );
        // It must NOT push the agent to re-run the skill — the plan exists.
        assert!(
            reason.contains("Do NOT re-run"),
            "awaiting-approval reason must tell the agent not to re-plan: {reason}"
        );
        // "Present the plan and wait" is what failed: the agent wrote prose and
        // the thread sat idle until the user typed "approve" by hand
        // (2026-08-02). This hook firing IS the proof that approval is a
        // decision the agent is blocked on, so it must say how to ask. Keep the
        // option pair identical to the prompt rules and the skill.
        for needle in [
            "`AskUserQuestion`",
            "`Approve` and `Request changes`",
            "DECISION question",
        ] {
            assert!(
                reason.contains(needle),
                "awaiting-approval reason must route approval through the question tool \
                 (missing {needle:?}): {reason}"
            );
        }
    }

    #[test]
    fn parse_hook_payload_extracts_file_path_and_tool_name() {
        let raw = r#"{ "tool_name": "Edit", "tool_input": { "file_path": "/a/b.rs" } }"#;
        let payload: HookPayload = serde_json::from_str(raw).expect("valid payload");
        assert_eq!(payload.tool_name, "Edit");
        assert_eq!(payload.tool_input.file_path, "/a/b.rs");
    }

    #[test]
    fn parse_hook_payload_tolerates_missing_fields() {
        let raw = r#"{ "tool_input": { } }"#;
        let payload: HookPayload = serde_json::from_str(raw).expect("valid payload");
        assert_eq!(payload.tool_name, "");
        assert_eq!(payload.tool_input.file_path, "");
    }
}
