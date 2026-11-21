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
use crate::http::{client as http_client, ConnectRetry};
use crate::workspace::{BoxError, Workspace};

/// What picking an `Implement in a fresh session` option does at an offered
/// checkpoint run named by `flags`. Printed only when the card may offer one.
/// The flow's own decision comes first, since only the flow knows what it is.
fn fresh_session_handoff(flags: &str) -> String {
    format!(
        "Picking an `Implement in a fresh session` option is the user's decision here. First do \
         what your flow says that decision does: a plan flow approves the plan. Then hand the \
         work to a successor session in this thread with `lucidos successor start --narrative \
         <file> {flags}`. Add only the flags that differ from this \
         thread's implementer: `--coding-agent <backend>`, `--coding-agent-model <model id>`, \
         `--reasoning-effort <level>`. An omitted flag keeps this thread's, so picking this \
         thread's implementer adds none. Pass the model id the ladder prints, never its picker \
         label. A typed reply naming another implementer works the same way. Lucidos refuses a \
         pick off the intelligence ladder, and lists the rungs. `lucidos successor --help` has \
         the narrative format. The narrative says what the user decided here, and quotes your \
         flow's remaining checkpoint instructions so the successor session keeps running them. \
         End your turn: the successor session starts in this thread once it ends. A handoff \
         needs no approval. Your flow's option to stay carries on in \
         this session."
    )
}

const IMPLEMENTER_CHOICE: &str = "Follow the flow's backend policy configured in the \
    repository's checkpoint instructions, or the user's explicit backend choice. Without a policy, keep this thread's backend. \
    Offer or recommend another backend only when the user requests it or the policy permits it, \
    and its ladder was read. Intelligence fit takes priority over backend preference. \
    An installed backend alone never requires an option. ";

const PLAN_IMPLEMENTER_CHOICE: &str = "At plan approval, always offer a fresh session on each \
    readable backend. Backend policy chooses the recommendation, never which backends are offered.";

const INTELLIGENCE_FIT: &str = "Choose the successor's implementer by your judgement of \
    the work left, such as a plan, as a rung on the intelligence ladder above. A higher rung is \
    more intelligent, costs more and runs longer. Turn intelligence down for settled work: a \
    fully specified plan with settled decisions suits a lower rung. Turn it up for hard or open \
    work: work that leaves design judgement to the builder keeps this thread's rung or takes a \
    higher one. Every call reads the whole context again, so a rung above what the work needs \
    costs on every turn. Say why in one line of your message. Name a rung by its picker label \
    and effort, as the ladder prints them. Name the backend only when it is not this thread's.";

/// At plan approval a fresh session that does not pay leaves staying here as
/// the recommendation, so `Approve here` carries the label and leads.
fn approve_here_recommended(plan: bool, paying: bool) -> bool {
    plan && !paying
}

/// Allocate scarce card slots, then display the recommended option first.
fn card_slots(
    plan: bool,
    other: bool,
    recommended: bool,
    on_other: bool,
    paying: bool,
    fork: bool,
) -> String {
    let has_r = plan || recommended;
    let candidates = [
        ('A', true),
        ('P', plan),
        ('X', other && !(has_r && on_other)),
        ('R', has_r),
        ('F', fork),
        ('T', paying && recommended),
    ];
    let selected: Vec<char> = candidates
        .into_iter()
        .filter(|(_, present)| *present)
        .take(4)
        .map(|(code, _)| code)
        .collect();
    let order = if !has_r || approve_here_recommended(plan, paying) {
        ['A', 'R', 'X', 'T', 'P', 'F']
    } else {
        ['R', 'X', 'T', 'A', 'P', 'F']
    };
    order
        .into_iter()
        .filter(|code| selected.contains(code))
        .collect()
}

fn card_examples(plan: bool, other: bool, recommended: bool, paying: bool) -> String {
    let mut out = String::new();
    for on_other in [false, true] {
        if on_other && (!recommended || !other) {
            continue;
        }
        for fork in [false, true] {
            let slots = card_slots(plan, other, recommended, on_other, paying, fork);
            let backend = if on_other {
                "Other-backend"
            } else {
                "Same-backend"
            };
            let shape = if !recommended {
                "No recommendation".to_string()
            } else if approve_here_recommended(plan, paying) {
                format!("Stay-here recommendation, {backend} R")
            } else {
                format!("{backend} recommendation")
            };
            out.push_str(&format!(
                "\n- {shape}, {} real fork: {}.",
                if fork { "with a" } else { "without a" },
                slots
                    .chars()
                    .map(|c| c.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            if fork && !slots.contains('F') {
                out.push_str(" Name the fork in the question as a typed reply.");
            }
        }
    }
    out
}

pub(crate) fn card_shape(reading: crate::successor::ContextReading, flags: &str) -> String {
    use crate::successor::ContextReading;
    let plan = flags
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|pair| pair == ["--at", "plan-approval"]);
    let plan_only = if plan {
        " P = `Commit plan for later`. That pick runs `lucidos planned approve --plan-only`, commits only the plan, and ends the turn. It never authorizes implementation."
    } else {
        ""
    };
    let floor = || {
        if plan {
            format!("Use your normal user question tool with `Approve`, `Commit plan for later` and a real fork if any. Follow-up questions may choose the successor backend and rung. A typed reply requests changes. Offer no handoff because the engine reading is unavailable or handoff is blocked.{plan_only}")
        } else {
            "Use the flow's own question and options if it asks one here; otherwise carry on. Offer no handoff because the engine reading is unavailable or handoff is blocked.".to_string()
        }
    };
    let ContextReading::Read {
        input_tokens,
        can_hand_off: true,
        pays,
        past_ceiling,
        implementer,
        ladder,
    } = reading
    else {
        return floor();
    };
    let Some(ladder) = ladder else {
        return floor();
    };
    let paying = pays || past_ceiling;
    let other = ladder
        .other_backend_ladders
        .iter()
        .find(|l| !l.intelligence_rungs.is_empty());
    let lead = if past_ceiling {
        "This session is past its context ceiling: recommend a fresh session whatever the estimate says."
    } else if pays {
        "The handoff estimate says a fresh session pays here: recommend a fresh session."
    } else if plan {
        "The handoff estimate says a fresh session does not pay here. Offer a fresh session, but recommend staying here."
    } else {
        "The handoff estimate says a fresh session does not pay here. Recommend one only for a lower rung that fits the work."
    };
    let raise = if !plan && !paying && ladder.has_higher_intelligence_rung() {
        " If no lower rung fits and hard or long work needs more intelligence, recommend one or two rungs above this thread, never the top."
    } else {
        ""
    };
    let runs = implementer
        .map(|i| format!("This thread runs on {}. ", i.describe()))
        .unwrap_or_default();
    let size = input_tokens
        .map(|n| format!("Context: {}k tokens. ", n / 1000))
        .unwrap_or_default();
    let alternative = other.map(|l| format!(" X = `Implement in a fresh session: <rung> on {}`. {} Merge it into R when R names this backend. Choose its rung from the other ladder above.", l.surface, if plan { "Always offer X at plan approval." } else { "X is optional: offer it only when the user requests it or the flow's backend policy permits it, and it is relevant to the work." })).unwrap_or_default();
    let question_flow = "Use your normal user question tool. Offer a short set of relevant choices, not every rung. You may ask multiple questions, such as backend first and rung next. Each question has at most four options. The following shapes are single-question examples, not an exhaustive questionnaire.";
    let choice_policy = if plan {
        PLAN_IMPLEMENTER_CHOICE
    } else {
        IMPLEMENTER_CHOICE
    };
    let (a_label, r_label, order) = if approve_here_recommended(plan, paying) {
        (
            "Approve here (Recommended)",
            "Implement in a fresh session: <rung>",
            "A, R, X, P, F",
        )
    } else {
        (
            if plan {
                "Approve here"
            } else {
                "Continue here"
            },
            "Implement in a fresh session: <rung> (Recommended)",
            "R, X, T, A, P, F, with A first when R is absent",
        )
    };
    let mut out = format!("{question_flow}\n\n{lead}{raise} {size}{runs}\n\n{}\n\n{choice_policy} {INTELLIGENCE_FIT}\n\nFor choices sharing a question: A = `{a_label}`.{} R = `{r_label}`; when R uses the other backend, name it in the label and merge X into R.{} F = a real fork, only if there is one.{} Fill slots in priority A, P, X, R, F, T, omitting ineligible options; display them in order {order}. When an option is recommended, it comes first and carries `(Recommended)`; never move the label to another option. A typed reply requests changes; do not spend a slot on it.",
        ladder.render(), plan_only, alternative,
        if paying { " T = `Implement in a fresh session: <this thread's rung>`, only when it differs from R." } else { "" });
    if plan {
        out.push_str(" Always offer R at plan approval. Choose its rung by intelligence fit for the work left, whether lower, equal or higher. A higher pick takes only one or two rungs above this thread, never the top.");
        out.push_str("\nDefault option codes, with every readable backend:");
        out.push_str(&card_examples(true, other.is_some(), true, paying));
    } else {
        out.push_str("\nDefault option codes, keeping this thread's backend:");
        out.push_str(&card_examples(false, false, true, paying));
        if !paying {
            out.push_str(&card_examples(false, false, false, false));
        }
        if other.is_some() {
            out.push_str("\nOnly when the flow's backend policy or user request permits another backend, relevant alternatives may use these shapes:");
            out.push_str(&card_examples(false, true, true, paying));
            if !paying {
                out.push_str(&card_examples(false, true, false, false));
            }
        }
    }
    out.push_str(&format!("\n{}", fresh_session_handoff(flags)));
    out.push_str(&format!("\n\nVerdict: {lead}{raise}"));
    out
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

/// Planned-marker gate state from the engine. Only `Satisfied` permits edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlannedState {
    Satisfied,
    Proposed,
    Missing,
    Unreadable,
}

impl PlannedState {
    pub(crate) fn parse(raw: &str) -> Self {
        match raw.trim() {
            "SATISFIED" => PlannedState::Satisfied,
            "PROPOSED" => PlannedState::Proposed,
            "UNREADABLE" => PlannedState::Unreadable,
            // Unknown state labels also block edits.
            _ => PlannedState::Missing,
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            PlannedState::Satisfied => "SATISFIED",
            PlannedState::Proposed => "PROPOSED",
            PlannedState::Missing => "MISSING",
            PlannedState::Unreadable => "UNREADABLE",
        }
    }
}

/// Record the beginning of planning on the signed calling thread.
pub(crate) fn cmd_start(ws: &Workspace, kind: &str) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/internal/start-plan", ws.base_url());
    let resp = http_client()?
        .post(&url)
        .json(&serde_json::json!({ "kind": kind }))
        .send_with_connect_retry()?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("planned start returned {}: {}", status, resp.text()?).into());
    }
    println!("Planning started ({kind}). This grants no source edit permission.");
    Ok(())
}

pub(crate) fn cmd_mark(
    ws: &Workspace,
    kind: MarkKind<'_>,
    plan_kind: &str,
) -> Result<(), BoxError> {
    let cwd = std::env::current_dir().map_err(|e| format!("Failed to read cwd: {}", e))?;
    let (repo_root, branch, head_sha) = git_context(&cwd)?;
    let url = format!("{}/api/v1/internal/mark-planned", ws.base_url());
    let (state, plan_path, reason, files) = match &kind {
        // The engine decides whether the committed plan already has approval.
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
        "kind": plan_kind,
        "plan_path": plan_path,
        "reason": reason,
        "files": files,
    });
    let resp = http_client()?
        .post(&url)
        .json(&body)
        .send_with_connect_retry()
        .map_err(|e| format!("POST {} failed: {}", url, e))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp
            .text()
            .map_err(|e| format!("POST {} returned {}, body read failed: {}", url, status, e))?;
        return Err(marker_refusal(&text)
            .unwrap_or_else(|| format!("POST {} returned {}: {}", url, status, text))
            .into());
    }
    match kind {
        MarkKind::Plan(p) if status == reqwest::StatusCode::OK => {
            println!("Plan approval carried forward: {} ({}). Continue implementation.", branch, p);
        }
        // Naming the question tool here matters: this line is what the agent
        // reads at the exact moment it turns to present the plan. Told only to
        // "present it", it writes prose and the thread sits idle until the user
        // types "approve" by hand (2026-08-02). Both tool names are spelled out
        // because the CLI serves both backends and cannot tell them apart:
        // Claude Code has `AskUserQuestion`, Codex has `ask_user_question` on
        // the Lucidos MCP server, and sending either after the other's tool
        // would strand a proposed plan with no way to get it approved.
        MarkKind::Plan(p) => println!(
            "Plan recorded (awaiting approval): {} ({}). Run the approval checkpoint in the `implementation-plan` skill (§ Asking for approval) and offer the options it prints. Ask with your question tool (`AskUserQuestion` on Claude Code, `ask_user_question` on Codex), not in prose. Put the plan summary on the card's question itself, with the ask on its last line: the user never sees a message that also calls a tool. On a re-ask, carry what changed. Run `lucidos planned approve` only after the user taps an approving option or types a plain approval. A typed question or suggestion is a request for changes: revise the plan, run `lucidos planned mark --plan` again, and ask again.",
            branch, p
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

/// What a full approval tells the agent to do next. A fresh-session pick also
/// approves the full scope, so the text names both paths.
fn implementation_approved_text(branch: &str) -> String {
    format!(
        "Plan approved: {branch}. Implementation is unblocked. \
         If the user picked a fresh session, hand off now. \
         Otherwise the user approved building in this session: start the first phase now, in this turn. \
         An earlier instruction to stop for approval is now met."
    )
}

/// Approve the recorded plan for the scope the user chose.
pub(crate) fn cmd_approve(ws: &Workspace, plan_only: bool) -> Result<(), BoxError> {
    let cwd = std::env::current_dir().map_err(|e| format!("Failed to read cwd: {}", e))?;
    let (repo_root, branch, _head_sha) = git_context(&cwd)?;
    let url = format!("{}/api/v1/internal/approve-plan", ws.base_url());
    let body = serde_json::json!({
        "repo_root": repo_root.to_string_lossy(),
        "branch_name": branch,
        "plan_only": plan_only,
    });
    let resp = http_client()?
        .post(&url)
        .json(&body)
        .send_with_connect_retry()
        .map_err(|e| format!("POST {} failed: {}", url, e))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp
            .text()
            .map_err(|e| format!("POST {} returned {}, body read failed: {}", url, status, e))?;
        return Err(marker_refusal(&text)
            .unwrap_or_else(|| format!("POST {} returned {}: {}", url, status, text))
            .into());
    }
    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("POST {} returned non-JSON body: {}", url, e))?;
    if body.get("approved").and_then(|v| v.as_bool()) == Some(true) {
        if body.get("plan_only").and_then(|v| v.as_bool()) == Some(true) {
            println!(
                "Plan approved for later: {}. Only plan files may land.",
                branch
            );
        } else {
            println!("{}", implementation_approved_text(&branch));
        }
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

/// The engine's own words when it refuses a marker write the agent can fix:
/// an approval the user has not given, a batch approval of plan text that has
/// changed since, or a branch the thread's Apply never lands. The agent then reads what to do, not an HTTP status. `None` for any
/// other error body.
fn marker_refusal(body: &str) -> Option<String> {
    let body: serde_json::Value = serde_json::from_str(body).ok()?;
    let reason = body.get("reason")?.as_str()?;
    if !matches!(
        reason,
        "no-plan-approval" | "plan-changed" | "marker-off-tracked-branch"
    ) {
        return None;
    }
    Some(body.get("error")?.as_str()?.to_string())
}

#[path = "../../lucidos-core/src/core/plan_status.rs"]
mod plan_status;
use plan_status::{approved_plan_text, APPROVED_STATUS};

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
        .send_with_connect_retry()
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

/// Print the current branch’s plan marker state to stdout.
pub(crate) fn cmd_state(ws: &Workspace) -> Result<(), BoxError> {
    let state = query_state(ws)?;
    println!("{}", state.as_str());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intelligence_ladder::{IntelligenceRung, Ladder};
    use crate::successor::{ContextReading, Implementer};

    const FLAGS: &str = "--at plan-approval --progress 0/4";

    /// A three-rung ladder with the thread on `current`.
    fn ladder(current: Option<usize>) -> Ladder {
        let step = |model: &str, effort: &str| IntelligenceRung {
            model: model.into(),
            effort: effort.into(),
            label: model.into(),
            intelligence: Some("1.0×".into()),
            cost: Some("1.0×".into()),
            time: None,
            is_default: false,
        };
        Ladder {
            surface: "claude-code".into(),
            intelligence_rungs: vec![
                step("m-a", "low"),
                step("m-b", "medium"),
                step("m-c", "high"),
            ],
            current,
            current_pair: current.map(|i| {
                format!(
                    "m-{} at {}",
                    ["a", "b", "c"][i],
                    ["low", "medium", "high"][i]
                )
            }),
            reference: None,
            other_backend_ladders: Vec::new(),
        }
    }

    fn reading(pays: Option<bool>, ladder: Option<Ladder>) -> ContextReading {
        match pays {
            None => ContextReading::Unknown,
            Some(pays) => ContextReading::Read {
                input_tokens: Some(240_000),
                can_hand_off: true,
                pays,
                past_ceiling: false,
                implementer: Some(Implementer {
                    coding_agent: "claude-code".into(),
                    model: Some("m-b".into()),
                    reasoning_effort: Some("medium".into()),
                }),
                ladder: ladder.map(Box::new),
            },
        }
    }

    #[test]
    fn card_slots_follow_priority_and_display_order() {
        assert_eq!(card_slots(true, true, true, false, true, true), "RXAP");
        assert_eq!(card_slots(true, true, true, true, true, true), "RAPF");
        assert_eq!(card_slots(true, true, false, false, false, true), "ARXP");
        assert_eq!(card_slots(true, false, true, false, false, true), "ARPF");
        assert_eq!(card_slots(false, true, true, false, true, true), "RXAF");
        for plan in [false, true] {
            for other in [false, true] {
                for recommended in [false, true] {
                    for on_other in [false, true] {
                        for paying in [false, true] {
                            for fork in [false, true] {
                                let slots =
                                    card_slots(plan, other, recommended, on_other, paying, fork);
                                assert!(slots.len() <= 4);
                                assert!(slots.contains('A'));
                                let r_leads = (plan || recommended)
                                    && !approve_here_recommended(plan, paying);
                                let lead = if r_leads { 'R' } else { 'A' };
                                assert_eq!(slots.chars().next(), Some(lead), "{slots}");
                                assert_eq!(slots.contains('P'), plan);
                                assert_eq!(
                                    slots.contains('X'),
                                    other && !((plan || recommended) && on_other)
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn full_approval_names_the_next_action_for_both_picks() {
        let text = implementation_approved_text("feature");
        assert!(text.starts_with("Plan approved: feature. Implementation is unblocked."));
        assert!(text.contains("If the user picked a fresh session, hand off now."));
        assert!(text.contains("start the first phase now, in this turn"));
        assert!(text.contains("An earlier instruction to stop for approval is now met."));
    }

    #[test]
    fn plan_and_successor_cards_report_current_outside_an_empty_ladder() {
        let ladder = Ladder::parse(&serde_json::json!({
            "surface": "codex",
            "rungs": [],
            "current": {"model": "custom-model", "effort": "high", "position": null},
        }))
        .unwrap();
        for flags in [FLAGS, "--at phase-commit --progress 1/4"] {
            let text = card_shape(reading(Some(false), Some(ladder.clone())), flags);
            assert!(
                text.contains("This thread runs on custom-model at high (off-ladder)."),
                "{text}"
            );
            assert!(
                !text.contains("recommend one or two rungs above this thread"),
                "{text}"
            );
        }
    }

    #[test]
    fn plan_only_is_only_offered_at_plan_approval_even_when_unreadable() {
        for reading in [
            reading(Some(true), Some(ladder(Some(1)))),
            ContextReading::Unknown,
        ] {
            let plan = card_shape(reading.clone(), FLAGS);
            assert!(plan.contains("Commit plan for later"));
            assert!(plan.contains("planned approve --plan-only"));
            assert!(plan.contains("normal user question tool"));
            assert!(plan.contains("Follow-up questions") || plan.contains("multiple questions"));
            assert!(!plan.contains("Request changes"));
            let repo = card_shape(reading, "--at phase-commit --progress 1/4");
            assert!(!repo.contains("Commit plan for later"));
            assert!(!repo.contains("Request changes"));
        }
    }

    #[test]
    fn unread_or_closed_reading_cannot_offer_a_handoff() {
        let mut closed = reading(Some(true), Some(ladder(Some(1))));
        if let ContextReading::Read { can_hand_off, .. } = &mut closed {
            *can_hand_off = false;
        }
        for reading in [closed, ContextReading::Unknown] {
            let text = card_shape(reading, FLAGS);
            assert!(!text.contains("Implement in a fresh session"));
            assert!(text.contains("`Approve`"));
        }
    }

    #[test]
    fn plan_approval_always_offers_the_other_readable_backend() {
        for (current, alternative) in [("codex", "claude-code"), ("claude-code", "codex")] {
            let mut own = ladder(Some(1));
            own.surface = current.into();
            let mut other = ladder(None);
            other.surface = alternative.into();
            own.other_backend_ladders.push(other);
            for (pays, past_ceiling) in [(false, false), (true, false), (false, true)] {
                let mut context = reading(Some(pays), Some(own.clone()));
                if let ContextReading::Read {
                    implementer,
                    past_ceiling: ceiling,
                    ..
                } = &mut context
                {
                    implementer.as_mut().unwrap().coding_agent = current.into();
                    *ceiling = past_ceiling;
                }
                let text = card_shape(context, FLAGS);
                assert!(text.contains("Always offer X"));
                let examples = text.split("Default option codes,").nth(1).unwrap();
                assert!(examples.contains(if pays || past_ceiling {
                    "R, X, A, P"
                } else {
                    "A, R, X, P"
                }));
                assert!(!examples.contains("Only when the flow's backend policy"));
                assert!(text.contains(&format!("on {alternative}")));
            }
        }
    }

    #[test]
    fn other_checkpoints_keep_backend_alternatives_optional() {
        for flags in ["--at phase-commit --progress 1/4", "--at successor-choice"] {
            for pays in [false, true] {
                let mut own = ladder(Some(1));
                let mut other = ladder(None);
                other.surface = "codex".into();
                own.other_backend_ladders.push(other);
                let text = card_shape(reading(Some(pays), Some(own)), flags);
                assert!(!text.contains("Always offer R"));
                assert!(!text.contains("Always offer X"));
                assert!(text.contains("Without a policy, keep this thread's backend"));
                let examples = text.split("Default option codes,").nth(1).unwrap();
                let (defaults, alternatives) = examples
                    .split_once("Only when the flow's backend policy")
                    .unwrap();
                assert!(!defaults.contains("X"));
                assert!(alternatives.contains("R, X, A"));
                if !pays {
                    assert!(defaults.contains("No recommendation, without a real fork: A."));
                }
            }
        }
    }

    #[test]
    fn plan_approval_always_offers_r_with_an_honest_recommendation() {
        for pays in [false, true] {
            let text = card_shape(reading(Some(pays), Some(ladder(Some(1)))), FLAGS);
            assert!(text.contains("Always offer R"));
            assert_eq!(text.contains("`Approve here (Recommended)`"), !pays);
            assert_eq!(text.contains("<rung> (Recommended)`"), pays);
            assert!(text.contains("When an option is recommended, it comes first"));
            for line in text.lines().filter(|line| line.starts_with("- ")) {
                assert!(line.contains("R,"));
                assert_eq!(line.starts_with("- Stay-here recommendation, "), !pays);
                assert_eq!(line.contains(": A, R"), !pays, "{line}");
            }
            assert!(text.contains("only one or two rungs above this thread, never the top"));
        }
    }

    #[test]
    fn offered_shapes_end_with_the_verdict() {
        for flags in [FLAGS, "--at successor-choice"] {
            for (pays, past) in [(false, false), (true, false), (false, true)] {
                let mut context = reading(Some(pays), Some(ladder(Some(0))));
                if let ContextReading::Read { past_ceiling, .. } = &mut context {
                    *past_ceiling = past;
                }
                let text = card_shape(context, flags);
                let tail = text.lines().rev().take(10).collect::<Vec<_>>().join("\n");
                assert!(tail.contains(if past {
                    "past its context ceiling"
                } else if pays {
                    "fresh session pays here"
                } else {
                    "fresh session does not pay here"
                }));
                if flags != FLAGS && !pays && !past {
                    assert!(text
                        .lines()
                        .last()
                        .unwrap()
                        .contains("recommend one or two rungs above this thread, never the top"));
                }
            }
        }
    }

    #[test]
    fn successor_choices_allow_short_follow_up_questions() {
        let text = card_shape(
            reading(Some(true), Some(ladder(Some(1)))),
            "--at successor-choice",
        );
        assert!(text.contains("normal user question tool"));
        assert!(text.contains("not every rung"));
        assert!(text.contains("multiple questions"));
        assert!(!text.contains("Ask one question"));
    }

    #[test]
    fn static_card_guidance_names_no_model_or_effort() {
        let text = format!(
            "{} {}",
            IMPLEMENTER_CHOICE,
            card_shape(reading(Some(true), Some(ladder(Some(1)))), FLAGS)
        );
        for menu in [
            crate::spawn_thread::CC_MENU,
            crate::spawn_thread::CODEX_MENU,
        ] {
            let menu: serde_json::Value = serde_json::from_str(menu).unwrap();
            for row in menu["models"].as_array().unwrap() {
                if row["value"] == "default" {
                    continue;
                }
                for key in ["value", "label"] {
                    assert!(!text.contains(row[key].as_str().unwrap()));
                }
            }
        }
        // The rendered ladder and implementer may name efforts; the static principle cannot.
        for guidance in [
            IMPLEMENTER_CHOICE,
            PLAN_IMPLEMENTER_CHOICE,
            INTELLIGENCE_FIT,
        ] {
            for word in guidance.split(|c: char| !c.is_ascii_alphanumeric()) {
                assert!(!crate::spawn_thread::EFFORT_LEVELS.contains(&word));
            }
        }
    }

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
        assert_eq!(
            approved_plan_text("- **Status**: Approved\n- **Status**: Proposed\n"),
            None
        );
    }

    #[test]
    fn a_fixable_refusal_shows_the_engines_words_alone() {
        for (refusal, words) in [
            (
                r#"{"error":"Revise the plan.","reason":"no-plan-approval"}"#,
                "Revise the plan.",
            ),
            (
                r#"{"error":"Switch back.","reason":"marker-off-tracked-branch"}"#,
                "Switch back.",
            ),
        ] {
            assert_eq!(marker_refusal(refusal).as_deref(), Some(words));
        }
        for other in [
            r#"{"error":"busy","reason":"apply_in_progress"}"#,
            r#"{"error":"no reason"}"#,
            "approve_plan: pool closed",
        ] {
            assert_eq!(marker_refusal(other), None, "{other}");
        }
    }

    #[test]
    fn parse_round_trips_known_states() {
        assert_eq!(PlannedState::parse("SATISFIED"), PlannedState::Satisfied);
        assert_eq!(PlannedState::parse("PROPOSED"), PlannedState::Proposed);
        assert_eq!(PlannedState::parse("MISSING"), PlannedState::Missing);
        assert_eq!(PlannedState::parse("UNREADABLE"), PlannedState::Unreadable);
        assert_eq!(PlannedState::Unreadable.as_str(), "UNREADABLE");
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
