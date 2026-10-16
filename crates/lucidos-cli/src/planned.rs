//! `lucidos planned` — record/query the durable Planned marker that enforces
//! the `implementation-plan` skill. Mirrors `hardened.rs` (same `git_context`
//! and HTTP-to-parent-engine pattern). The marker is set by the skill
//! (`mark --plan <docs/plans/file>` → records the awaiting-approval `proposed`
//! state), approved by the agent after the user's chat approval
//! (`approve` → flips `proposed` to gate-satisfying `planned`), set directly
//! for a local fix (`mark --simple "<reason>"` → `acknowledged_simple`, no
//! approval needed), or claimed by an unattended run for a scoped security fix
//! (`mark --security-fix "<reason>" --files <csv>` → `bounded_security_fix`).
//! Those last three satisfy every gate; `proposed` and the absence of a marker
//! both block.

use crate::hardened::{git_context, run_git};
use crate::http::client as http_client;
use crate::workspace::{BoxError, Workspace};

/// What picking an `Implement in a new thread` option does. Printed only
/// when the card may offer the option ([`card_shape`]).
pub(crate) const NEW_THREAD_HANDOFF: &str = "Picking an `Implement in a new thread` option \
    approves the plan: run `lucidos planned approve` here, then hand the build to a successor \
    thread with `lucidos successor spawn --narrative <file>`. Add only the flags that differ \
    from this thread's implementer: `--coding-agent <backend>`, `--coding-agent-model <model id>`, \
    `--reasoning-effort <level>`. An omitted flag keeps this thread's, so picking this thread's \
    implementer adds none. Pass model ids, not picker labels (`claude-sonnet-5-5`, not `Sonnet 5.5`). A typed \
    reply naming another implementer works the same way. Lucidos refuses a model or effort the \
    backend does not offer, and lists the ones it does. `lucidos successor --help` has the \
    narrative format. The narrative names the plan path and says the user approved it here, so \
    the builder builds it phase by phase without asking again. The successor's branch starts at \
    this branch's last commit, so it already holds the plan, and the approved marker carries \
    over. End your turn with the link it prints. The pick is also the owner's consent to the \
    handoff, so a handoff after the pick needs no approval card. After a typed reply, a handoff \
    refused for authority needs the `lucidos ask-owner-approval` it names, then a retry. \
    `Approve here` builds in this thread instead.";

/// How the planning agent picks the successor's implementer. The backend and
/// model are the agent's, because only the agent has read the plan. The effort
/// follows the model and the plan (ADR 0486).
const IMPLEMENTER_CHOICE: &str = "Recommend the successor's implementer by your judgement of \
    the plan. A fully specified plan with settled decisions suits a cheaper model, such as \
    Sonnet. A plan that leaves design judgement to the builder keeps this thread's model. The \
    effort follows from that. A cheaper model on a fully specified plan runs at `medium`, or at \
    `high` when the plan is hard or long. This thread's model runs at `high`. Never recommend \
    `xhigh` or `max` for a builder. Effort sets how many calls a turn makes, and every call \
    reads the whole context again. Say why in one line of your message. Name an implementer by \
    the model's picker label and the effort, for example `Sonnet 5.5, high`. Name the backend \
    only when it is not this thread's, for example `Codex GPT-5.6 Sol, high`.";

/// The large card's options: the context alone calls for a new thread.
const LARGE_OPTIONS: &str = "Offer, in order: `Implement in a new thread: <implementer> \
    (Recommended)`; `Implement in a new thread: <this thread's implementer>`, only when it \
    differs; `Approve here`; then a real fork of the plan if it has one. That is never more \
    than 4 options, and never `Request changes`.";

/// The small card's options. On a small context a fresh thread saves only
/// through a cheaper implementer, so it never offers this thread's own.
const SMALL_OPTIONS: &str = "A cheaper pick is a cheaper model or a lower effort. If your \
    pick is cheaper than this thread's implementer, offer, in order: `Implement in a new \
    thread: <implementer> (Recommended)`; `Approve here`; then a real fork of the plan if it \
    has one. That is never `Request changes`. Otherwise offer no new thread:";

/// The plan-approval card's options under the floor: today's shape.
const FLOOR_SHAPE: &str = "Offer `Approve` / `Request changes`. The option pair is a floor: if \
    the plan offers a real fork, that fork takes the second slot and `Request changes` is \
    dropped rather than carried as a third.";

/// The card's options. Two reasons offer a new thread: a context the engine
/// calls large, or, at any size, a cheaper implementer than this thread's,
/// as the agent picks it (ADR 0497). The engine owns the threshold, so this never
/// compares a number itself. An unreadable reply offers today's shape and says so.
pub(crate) fn card_shape(reading: crate::successor::ContextReading) -> String {
    use crate::successor::{ContextReading, Implementer};
    let runs_on = |implementer: Option<Implementer>| {
        implementer
            .map(|i| format!("This thread runs on {}.", i.describe()))
            .unwrap_or_else(|| {
                "Lucidos could not read this thread's implementer, so name the model you run on."
                    .to_string()
            })
    };
    match reading {
        ContextReading::Read {
            offer: true,
            input_tokens,
            implementer,
            ..
        } => format!(
            "This thread's context is {}, large enough that the build belongs in a fresh \
             thread. {} {IMPLEMENTER_CHOICE} {LARGE_OPTIONS} {NEW_THREAD_HANDOFF}",
            input_tokens
                .map(|t| format!("{}k tokens", t / 1000))
                .unwrap_or_else(|| "large".to_string()),
            runs_on(implementer)
        ),
        // An unmeasured size lands here too: a cheaper implementer pays off
        // at any size, so only the lead sentence differs.
        ContextReading::Read {
            can_hand_off: true,
            input_tokens,
            implementer,
            ..
        } => format!(
            "{} {} {IMPLEMENTER_CHOICE} {SMALL_OPTIONS} {FLOOR_SHAPE} {NEW_THREAD_HANDOFF}",
            match input_tokens {
                Some(t) => format!(
                    "This thread's context is {}k tokens, small enough that a new thread pays \
                     off only on a cheaper implementer.",
                    t / 1000
                ),
                None => "Lucidos has no measure of this thread's context, so offer a new thread \
                         only for a cheaper implementer."
                    .to_string(),
            },
            runs_on(implementer)
        ),
        ContextReading::Read { .. } => FLOOR_SHAPE.to_string(),
        ContextReading::Unknown => format!(
            "{FLOOR_SHAPE} Lucidos could not read this thread's context size, so it offers no \
             new thread."
        ),
    }
}

/// Which kind of planning decision is being recorded.
#[derive(Debug, Clone)]
pub(crate) enum MarkKind<'a> {
    /// A real plan was written to `docs/plans/`; carries the relative path.
    Plan(&'a str),
    /// The agent declared a local fix; carries the one-line reason.
    Simple(&'a str),
    /// An unattended run is committing a security fix confined to `files`.
    /// Carries the one-line reason and the repo-relative paths it is bounded
    /// to. The engine caps the list and the Apply floor enforces it.
    SecurityFix { reason: &'a str, files: Vec<String> },
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
    let (state, plan_path, reason, files) = match &kind {
        // A written plan starts AWAITING APPROVAL — the skill records `proposed`,
        // the user approves in chat, then the agent runs `planned approve`.
        MarkKind::Plan(p) => ("proposed", Some(*p), None, Vec::new()),
        MarkKind::Simple(r) => ("acknowledged_simple", None, Some(*r), Vec::new()),
        MarkKind::SecurityFix { reason, files } => {
            ("bounded_security_fix", None, Some(*reason), files.clone())
        }
    };
    let body = serde_json::json!({
        "repo_root": repo_root.to_string_lossy(),
        "branch_name": branch,
        "head_sha": head_sha,
        "state": state,
        "plan_path": plan_path,
        "reason": reason,
        "files": files,
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
            "Plan recorded (awaiting approval): {} ({}). Summarize it in your message, then ask for approval with your question tool (`AskUserQuestion` on Claude Code, `ask_user_question` on Codex), not in prose. The card's question is one short line that never repeats the summary. {} Once approved, run `lucidos planned approve`.",
            branch,
            p,
            card_shape(crate::successor::read_own_context(ws))
        ),
        MarkKind::Simple(r) => println!("Simple change acknowledged: {} ({})", branch, r),
        // The reminder is the only enforcement of the lane's other two
        // preconditions. Apply checks the file bound. The unattended run and
        // the regression test are claims the agent makes, so state them where
        // the agent reads the result.
        MarkKind::SecurityFix { reason, files } => println!(
            "Bounded security fix recorded: {} ({}). Confined to {} file(s): {}. Apply refuses this branch if it touches anything else, so re-run this command with the full list if the fix has to grow. The lane is for an UNATTENDED run only, and it requires a regression test that fails without your fix: if either is untrue, write a plan instead and get it approved.",
            branch,
            reason,
            files.len(),
            files.join(", ")
        ),
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
        if let Some(plan_path) = body.get("plan_path").and_then(|v| v.as_str()) {
            // The marker is already approved. A failed file edit must not
            // read as a failed approval, so it only warns.
            match mark_plan_file_approved(&cwd, plan_path) {
                Ok(true) => println!("Marked {} as approved and committed it.", plan_path),
                Ok(false) => {}
                Err(e) => eprintln!(
                    "Could not mark {} as approved ({}). Change its status line to \
                     `{}` and commit it.",
                    plan_path, e, APPROVED_STATUS
                ),
            }
        }
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

/// The status line the `implementation-plan` skill writes into a new plan.
pub(crate) const PROPOSED_STATUS: &str = "- **Status**: Proposed";
/// What `planned approve` rewrites it to.
pub(crate) const APPROVED_STATUS: &str = "- **Status**: Approved";

/// The plan text with its status line flipped to approved. `None` when it has
/// no proposed status line, as in a plan written before the line existed.
pub(crate) fn approved_plan_text(text: &str) -> Option<String> {
    let mut flipped = false;
    let out: String = text
        .split_inclusive('\n')
        .map(|line| {
            if !flipped && line.trim_end() == PROPOSED_STATUS {
                flipped = true;
                line.replacen(PROPOSED_STATUS, APPROVED_STATUS, 1)
            } else {
                line.to_string()
            }
        })
        .collect();
    flipped.then_some(out)
}

/// Flip the plan file's status line and commit it, so the file on main says
/// what the marker says. Returns whether it committed anything.
fn mark_plan_file_approved(cwd: &std::path::Path, plan_path: &str) -> Result<bool, BoxError> {
    let toplevel = run_git(cwd, &["rev-parse", "--show-toplevel"])?;
    let file = std::path::Path::new(&toplevel).join(plan_path);
    let text =
        std::fs::read_to_string(&file).map_err(|e| format!("read {}: {}", file.display(), e))?;
    let Some(approved) = approved_plan_text(&text) else {
        return Ok(false);
    };
    std::fs::write(&file, approved).map_err(|e| format!("write {}: {}", file.display(), e))?;
    let file_arg = file.to_string_lossy();
    run_git(cwd, &["add", "--", &file_arg])?;
    let message = format!("docs(plans): approve {}", plan_path);
    run_git(cwd, &["commit", "-m", &message, "--", &file_arg])?;
    Ok(true)
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
    fn approval_flips_only_the_proposed_status_line() {
        let plan = "## Plan\n\nQuotes `- **Status**: Proposed` inline.\n- **Status**: Proposed\n";
        assert_eq!(
            approved_plan_text(plan).as_deref(),
            Some("## Plan\n\nQuotes `- **Status**: Proposed` inline.\n- **Status**: Approved\n")
        );
        assert_eq!(
            approved_plan_text("## Old plan\n\nNo status line.\n"),
            None,
            "a plan written before the status line existed is left alone"
        );
    }

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
