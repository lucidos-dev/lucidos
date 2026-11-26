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

pub(crate) fn is_gated_tool(name: &str) -> bool {
    matches!(name, "Edit" | "Write")
}

fn plan_skill_available(cwd: &Path) -> bool {
    cwd.join(".claude/skills/implementation-plan/SKILL.md")
        .is_file()
}

pub(crate) fn is_plan_artifact(file_path: &str) -> bool {
    file_path.contains("docs/plans/")
}

pub(crate) fn build_deny_json(file_path: &str) -> String {
    let reason = format!(
        "Edit blocked: this branch has no implementation-plan marker yet. Before editing source, \
         decide: if this is complex work (cross-layer, or any routing / topology / storage / \
         security / migration / process change, ADR- or design-backed, or anything beyond a local \
         bug fix), run the `implementation-plan` skill first — it writes docs/plans/<date>-<slug>.md \
         and records a `proposed` marker for the user to approve. If this is a genuinely local fix, \
         acknowledge it with `lucidos planned mark --simple \"<one-line reason>\"`. ONLY IF NOBODY \
         CAN BE ASKED (an unattended run, such as the nightly security pass) and this is a security \
         fix confined to a few named files with a regression test, take the bounded lane instead: \
         `lucidos planned mark --security-fix \"<finding + the test that proves the fix>\" --files \
         <comma-separated repo-relative paths>`. Apply then refuses the branch if it touches \
         anything outside that list, so anything wider stays gated: write the plan, leave the \
         marker `proposed`, and report that you are blocked on a decision. Then retry your edit to \
         `{path}`.",
        path = file_path,
    );
    deny_envelope(&reason)
}

pub(crate) fn build_awaiting_approval_json(file_path: &str) -> String {
    let reason = format!(
        "Edit blocked: the implementation plan on this branch is awaiting the user's approval. \
         Do NOT re-run the `implementation-plan` skill: the plan is already recorded. ASK FOR \
         APPROVAL WITH THE `AskUserQuestion` TOOL. Put the plan summary on the card's question \
         itself, in short markdown with the ask on its last line: the user never sees a message \
         that also calls a tool. Run the approval checkpoint in the `implementation-plan` skill \
         (§ Asking for approval) and offer the options it prints. With no fresh session to offer, \
         that is `Approve` / `Request changes`, a floor: if the plan offers a real fork, that \
         fork takes the second slot and `Request changes` is dropped rather than carried as a \
         third. Approval is a DECISION question, not a post-work confirmation: \
         this very hook is blocking your edits until it is answered, so asking in plain prose \
         leaves the thread idle. Once the user taps an approving option, run `lucidos planned \
         approve` to unblock implementation, then retry your edit to `{path}`. A tapped fork is \
         an approval: revise the plan file to that variant, re-commit, then run `lucidos planned \
         approve`. A fork pick does not re-mark. A typed reply that asks, suggests or questions \
         is a request for changes, not an approval: the engine refuses `lucidos planned approve` \
         until the user taps an approving option or types a plain approval. For changes, revise \
         the plan file under docs/plans/, run `lucidos planned mark --plan <path>` again, \
         re-commit, and ask again: the card carries what changed, and the plan file holds the \
         rest (the marker stays `proposed` until approved).",
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
        return Ok(());
    }
    if payload.tool_input.file_path.is_empty() {
        return Ok(());
    }
    if is_plan_artifact(&payload.tool_input.file_path) {
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
        Ok(PlannedState::Changed) => {
            println!(
                "{}",
                deny_envelope(
                    "Edit blocked: the plan changed after approval. Restore and commit the \
                     approved plan text to reuse the approval. If the plan needs to change, \
                     mark the revision and ask for approval."
                )
            );
            Ok(())
        }
        Ok(PlannedState::Unreadable) => {
            println!(
                "{}",
                deny_envelope(
                    "Edit blocked: Lucidos could not verify the implementation plan. \
                     Check the branch and committed plan file, then retry `lucidos planned state`."
                )
            );
            Ok(())
        }
        Err(e) => {
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
        for needle in [
            "ONLY IF NOBODY CAN BE ASKED",
            "lucidos planned mark --security-fix",
            "--files",
            "blocked on a decision",
        ] {
            assert!(
                reason.contains(needle),
                "reason must name the bounded security-fix lane and its bound \
                 (missing {needle:?}): {reason}"
            );
        }
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
        assert!(
            reason.contains("Do NOT re-run"),
            "awaiting-approval reason must tell the agent not to re-plan: {reason}"
        );
        for needle in [
            "`AskUserQuestion`",
            "`Approve` / `Request changes`",
            "DECISION question",
            "that fork takes the second slot",
            "A tapped fork is an approval",
            "A fork pick does not re-mark",
            "A typed reply that asks, suggests or questions is a request for changes",
            "Put the plan summary on the card's question itself",
        ] {
            assert!(
                reason.contains(needle),
                "awaiting-approval reason must route approval through the question tool \
                 (missing {needle:?}): {reason}"
            );
        }
    }

    #[test]
    fn awaiting_approval_points_at_the_skills_checkpoint() {
        let out = build_awaiting_approval_json("/tmp/x.rs");
        assert!(out.contains("approval checkpoint in the `implementation-plan` skill"));
        assert!(!out.contains("lucidos successor"), "{out}");
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
