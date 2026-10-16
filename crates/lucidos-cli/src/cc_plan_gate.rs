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
//!     silent no-op.
//!
//! Wired into `<workspace>/.lucidos/cc-settings.json` via `cc_settings.rs`,
//! on the Edit/Write matchers. Fails OPEN on parse / I/O / engine-unreachable
//! errors so a hook bug can't brick every Edit/Write call. Claude-Code only —
//! Codex has no PreToolUse hook and is covered by the prompt rule + Apply floor.

use serde::Deserialize;
use std::io::Read;
use std::path::Path;

use crate::planned::{card_shape, query_state, Checkpoint, PlannedState};
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
///
/// The third option, the bounded security-fix lane, is named here and nowhere
/// else a session reads by default. This text is the only one guaranteed to
/// arrive when an unattended run needs it, and an unattended run is the one
/// that cannot ask. Its precondition leads the sentence so an attended session
/// cannot skim past it: a session with a user to ask must ask.
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

/// Deny reason for a `Proposed` marker — a plan exists but the user hasn't
/// approved it. The path forward is approval, NOT re-running the skill.
///
/// It names the `AskUserQuestion` tool and its option pair because prose alone
/// is what failed: told only to "present the plan and wait", the agent wrote a
/// summary and ended the turn, and the thread sat idle until the user typed
/// "approve" by hand (2026-08-02). Approval is a DECISION question, and this
/// hook firing is the proof: source edits are blocked until it is answered.
///
/// The pair is stated as a FLOOR, not a fixed shape, matching
/// `ASK_USER_QUESTION_RULE` / `IMPLEMENTATION_PLAN_RULE` in the engine's
/// `agent_session::prompts` and the `implementation-plan` skill. Stated
/// unconditionally (as it was until 2026-08-04), an agent that surfaced a real
/// fork kept `Request changes` as a third button, which then meant only "I will
/// type what I want changed": the dead-end shape the prompts' NEVER AUTHOR AN
/// "OTHER" OPTION rule bans.
///
/// `shape` is [`card_shape`]'s answer for this thread's context reading, the
/// same text `lucidos planned mark` printed.
pub(crate) fn build_awaiting_approval_json(file_path: &str, shape: &str) -> String {
    let reason = format!(
        "Edit blocked: the implementation plan on this branch is awaiting the user's approval. \
         Do NOT re-run the `implementation-plan` skill: the plan is already recorded. Summarize \
         the plan in your message, then ASK FOR APPROVAL WITH THE `AskUserQuestion` TOOL. The \
         card's question is one short line that never repeats the summary. {shape} Approval is a \
         DECISION question, not a post-work confirmation: \
         this very hook is blocking your edits until it is answered, so asking in plain prose \
         leaves the thread idle. Once the user approves, run `lucidos planned approve` to unblock \
         implementation, then retry your edit to `{path}`. Picking a fork is an approval too: \
         revise the plan file to that variant, re-commit, then run `lucidos planned approve`. If \
         the user requests changes instead, revise the plan file under docs/plans/, re-commit, \
         and ask again: the new message names only what changed, and the plan file holds the rest \
         (the marker stays `proposed` until approved).",
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
            let shape = card_shape(
                crate::successor::read_own_context(&ws),
                Checkpoint::PlanApproval,
            );
            println!(
                "{}",
                build_awaiting_approval_json(&payload.tool_input.file_path, &shape)
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
        // The bounded security-fix lane is named nowhere else a session reads
        // by default, and an unattended run is the one that cannot ask. Its
        // precondition must lead, or an attended session reads it as a general
        // way past the gate.
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
        let small = crate::successor::ContextReading::Read {
            input_tokens: Some(40_000),
            can_hand_off: false,
            offer: false,
            implementer: None,
        };
        let out =
            build_awaiting_approval_json("/tmp/x.rs", &card_shape(small, Checkpoint::PlanApproval));
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
            "`Approve` / `Request changes`",
            "DECISION question",
            // The pair is a FLOOR, described the same way here as in the two
            // prompt rules and the skill: a plan that offers a real fork puts
            // the fork in the second slot rather than pushing `Request changes`
            // to a third, where it would mean only "I will type what I want
            // changed" (2026-08-04). A fork answer is an approval, so this text
            // must also say the marker can be flipped after revising the plan.
            "that fork takes the second slot",
            "Picking a fork is an approval too",
            // The card renders under the agent's message. A card restating the
            // summary makes the user read the plan twice.
            "never repeats the summary",
        ] {
            assert!(
                reason.contains(needle),
                "awaiting-approval reason must route approval through the question tool \
                 (missing {needle:?}): {reason}"
            );
        }
    }

    fn opus_high() -> Option<crate::successor::Implementer> {
        Some(crate::successor::Implementer {
            coding_agent: "claude-code".into(),
            model: Some("claude-opus-5-5".into()),
            reasoning_effort: Some("high".into()),
        })
    }

    /// I11: the engine's reading decides whether a new thread may be offered.
    /// A large thread leads with the successor. A large card's implementer is
    /// the agent's pick, its effort following the model, named beside this
    /// thread's own, within the question tool's 4 options.
    #[test]
    fn the_card_shape_follows_the_measured_context() {
        use crate::successor::ContextReading;
        let large = card_shape(
            ContextReading::Read {
                input_tokens: Some(240_000),
                can_hand_off: true,
                offer: true,
                implementer: opus_high(),
            },
            Checkpoint::PlanApproval,
        );
        for needle in [
            "240k tokens",
            "This thread runs on claude-code with model claude-opus-5-5 at high effort.",
            "`Implement in a new thread: <implementer> (Recommended)`",
            "only when it differs; `Approve here`",
            "never more than 4 options, and never `Request changes`",
            "by your judgement of the plan",
            "A cheaper model on a fully specified plan runs at `medium`",
            "Never recommend `xhigh` or `max` for a builder",
            "lucidos successor spawn --narrative <file>`",
            "Add only the flags that differ from this thread's implementer",
            "`--coding-agent-model <model id>`",
            "picking this thread's implementer adds none",
            "without asking again",
        ] {
            assert!(large.contains(needle), "missing {needle:?}: {large}");
        }
        let unread = card_shape(
            ContextReading::Read {
                input_tokens: Some(240_000),
                can_hand_off: true,
                offer: true,
                implementer: None,
            },
            Checkpoint::PlanApproval,
        );
        assert!(
            unread.contains("could not read this thread's implementer"),
            "{unread}"
        );
        let closed = card_shape(
            ContextReading::Read {
                input_tokens: Some(40_000),
                can_hand_off: false,
                offer: false,
                implementer: opus_high(),
            },
            Checkpoint::PlanApproval,
        );
        assert!(closed.contains("`Approve` / `Request changes`"));
        assert!(
            !closed.contains("new thread"),
            "a thread that cannot hand off is offered none: {closed}"
        );
        let unknown = card_shape(ContextReading::Unknown, Checkpoint::PlanApproval);
        assert!(unknown.contains("`Approve` / `Request changes`"));
        assert!(unknown.contains("could not read this thread's context size"));
    }

    /// I1 (ADR 0497): under the threshold a new thread pays off only through
    /// its implementer. The card offers a cheaper one, never this thread's
    /// own. A thread below `high` may also raise its effort (ADR 0499).
    #[test]
    fn a_small_card_offers_a_new_thread_only_for_a_cheaper_implementer() {
        use crate::successor::ContextReading;
        let small = card_shape(
            ContextReading::Read {
                input_tokens: Some(40_000),
                can_hand_off: true,
                offer: false,
                implementer: opus_high(),
            },
            Checkpoint::PlanApproval,
        );
        for needle in [
            "40k tokens",
            "a new thread pays off only through its implementer",
            "This thread runs on claude-code with model claude-opus-5-5 at high effort.",
            "by your judgement of the plan",
            "If your pick is cheaper than this thread's implementer",
            "`Implement in a new thread: <implementer> (Recommended)`; `Approve here`",
            "Otherwise offer no new thread: Offer `Approve` / `Request changes`",
            "`Approve` / `Request changes`",
            "lucidos successor spawn --narrative <file>`",
        ] {
            assert!(small.contains(needle), "missing {needle:?}: {small}");
        }
        assert!(
            !small.contains("<this thread's implementer>"),
            "a small card never offers this thread's own implementer in a new thread: {small}"
        );
        let unmeasured = card_shape(
            ContextReading::Read {
                input_tokens: None,
                can_hand_off: true,
                offer: false,
                implementer: opus_high(),
            },
            Checkpoint::PlanApproval,
        );
        assert!(
            unmeasured.contains("no measure of this thread's context")
                && !unmeasured.contains("small enough"),
            "an unmeasured context is never called small: {unmeasured}"
        );
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
