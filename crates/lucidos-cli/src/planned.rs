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

/// The option that keeps the work in this thread, which the flow names.
const STAY_OPTION: &str = "your flow's option to stay in this session (`Approve here` for a plan, \
    else `Continue here`)";

/// How the agent picks the successor's implementer. The rung is the agent's
/// judgement, because only the agent has read the work left (ADR 0486). It
/// names no model and no effort: those live on the ladder (ADR 0500).
const IMPLEMENTER_CHOICE: &str = "Recommend the successor's implementer by your judgement of \
    the work left, such as a plan, as a rung on the intelligence ladder above. A higher rung is \
    more intelligent, costs more and runs longer. Turn intelligence down for settled work: a \
    fully specified plan with settled decisions suits a lower rung. Turn it up for hard or open \
    work: work that leaves design judgement to the builder keeps this thread's rung or takes a \
    higher one. Every call reads the whole context again, so a rung above what the work needs \
    costs on every turn. Say why in one line of your message. Name a rung by its picker label \
    and effort, as the ladder prints them. Name the backend only when it is not this thread's.";

/// The paying card's options: the estimate alone calls for a fresh session.
fn paying_options() -> String {
    format!(
        "Offer, in order: `Implement in a fresh session: <implementer> (Recommended)`; `Implement \
         in a fresh session: <this thread's implementer>`, only when it differs; {STAY_OPTION}; \
         then a real fork of the decision if it has one. That is never more than 4 options, and \
         never `Request changes`."
    )
}

/// The card's options when the handoff does not pay. A fresh session then
/// saves only through a lower rung, so it never offers this thread's own
/// (ADR 0497). A thread with a rung above it may also raise (ADR 0499).
fn lower_options(raise: bool) -> String {
    let lower = format!(
        "If your pick is a lower rung than this thread's, offer, in order: `Implement in a fresh \
         session: <implementer> (Recommended)`; {STAY_OPTION}; then a real fork of the decision \
         if it has one. That is never `Request changes`."
    );
    if !raise {
        return format!("{lower} Otherwise:");
    }
    format!(
        "{lower} Else, if the work is hard or long, a higher rung builds it better: offer \
         `Implement in a fresh session: <a higher rung> (Recommended)`; {STAY_OPTION}; then a real \
         fork of the decision if it has one. That is never more than 3 options, and never \
         `Request changes`. Raise one or two rungs, never to the top for a builder. Otherwise:"
    )
}

/// The card with no fresh session to offer.
const FLOOR: &str = "Ask the question your flow asks here, with its own options, and offer no \
    fresh session. A plan's is `Approve` / `Request changes`, where a real fork takes the second \
    slot. If your flow asks no question here, carry on and ask nothing.";

/// The offered card's shape at the checkpoint run named by `flags`. Two
/// reasons offer a fresh session: the engine's *handoff estimate* says it pays,
/// or, at any size, a lower rung than this thread's, as the agent picks it
/// (ADR 0497). A thread with a rung above its own may also raise (ADR 0499).
/// The engine decides whether it pays and owns the ladder, so this compares
/// neither itself. An unreadable reply offers the floor and says so.
pub(crate) fn card_shape(reading: crate::successor::ContextReading, flags: &str) -> String {
    use crate::intelligence_ladder::Ladder;
    use crate::successor::{ContextReading, Implementer};
    let runs_on = |implementer: Option<&Implementer>, ladder: Option<&Ladder>| {
        let runs = implementer
            .map(|i| format!("This thread runs on {}.", i.describe()))
            .unwrap_or_else(|| {
                "Lucidos could not read this thread's implementer, so name the model you run on."
                    .to_string()
            });
        let ladder = ladder.map(Ladder::render).unwrap_or_else(|| {
            "Lucidos could not read the intelligence ladder: run `lucidos intelligence-ladder` \
             before you recommend."
                .to_string()
        });
        format!("{runs}\n\n{ladder}\n\n")
    };
    let size = |tokens: Option<u64>| {
        tokens
            .map(|t| format!("{}k tokens", t / 1000))
            .unwrap_or_else(|| "unmeasured".to_string())
    };
    match reading {
        ContextReading::Read {
            can_hand_off: true,
            pays: true,
            input_tokens,
            implementer,
            ladder,
        } => format!(
            "The handoff estimate says a fresh session pays here: this thread's context is {}. \
             {}{IMPLEMENTER_CHOICE} {} {}",
            size(input_tokens),
            runs_on(implementer.as_ref(), ladder.as_ref()),
            paying_options(),
            fresh_session_handoff(flags)
        ),
        ContextReading::Read {
            can_hand_off: true,
            input_tokens,
            implementer,
            ladder,
            ..
        } => format!(
            "This thread's context is {}, where the handoff estimate says a fresh session does \
             not pay, so a fresh session pays off only through its implementer. {}\
             {IMPLEMENTER_CHOICE} {} {FLOOR} {}",
            size(input_tokens),
            runs_on(implementer.as_ref(), ladder.as_ref()),
            lower_options(
                ladder
                    .as_ref()
                    .is_some_and(Ladder::has_higher_intelligence_rung)
            ),
            fresh_session_handoff(flags)
        ),
        ContextReading::Read { .. } => FLOOR.to_string(),
        ContextReading::Unknown => format!(
            "{FLOOR} Lucidos could not read this thread's context, so it offers no fresh session."
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

/// The engine's own words when it refuses a marker write the agent can fix:
/// an approval the user has not given, or a branch the thread's Apply never
/// lands. The agent then reads what to do, not an HTTP status. `None` for any
/// other error body.
fn marker_refusal(body: &str) -> Option<String> {
    let body: serde_json::Value = serde_json::from_str(body).ok()?;
    let reason = body.get("reason")?.as_str()?;
    if !matches!(reason, "no-plan-approval" | "marker-off-tracked-branch") {
        return None;
    }
    Some(body.get("error")?.as_str()?.to_string())
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

/// Print `SATISFIED`, `PROPOSED`, or `MISSING` for the current branch to stdout.
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
            reference: None,
        }
    }

    fn reading(pays: Option<bool>, ladder: Option<Ladder>) -> ContextReading {
        match pays {
            None => ContextReading::Unknown,
            Some(pays) => ContextReading::Read {
                input_tokens: Some(240_000),
                can_hand_off: true,
                pays,
                implementer: Some(Implementer {
                    coding_agent: "claude-code".into(),
                    model: Some("m-b".into()),
                    reasoning_effort: Some("medium".into()),
                }),
                ladder,
            },
        }
    }

    /// I8: the raise clause needs a readable verdict that does not pay and a
    /// rung above the thread's own. The lower pick (ADR 0497) stays on every
    /// card that does not pay.
    #[test]
    fn the_raise_clause_needs_a_non_paying_thread_with_a_higher_rung() {
        for (pays, ladder, raise) in [
            (Some(false), Some(ladder(Some(0))), true),
            (Some(false), Some(ladder(Some(1))), false),
            (Some(false), Some(ladder(Some(2))), false),
            (Some(false), Some(ladder(None)), false),
            (Some(false), None, false),
            (Some(true), Some(ladder(Some(0))), false),
            (None, Some(ladder(Some(0))), false),
        ] {
            let shape = card_shape(reading(pays, ladder.clone()), FLAGS);
            assert_eq!(
                shape.contains("`Implement in a fresh session: <a higher rung> (Recommended)`"),
                raise,
                "{pays:?} {ladder:?}: {shape}"
            );
            if raise {
                assert!(shape.contains("never to the top for a builder"), "{shape}");
                assert!(shape.contains("never `Request changes`"), "{shape}");
            }
            assert_eq!(
                shape.contains("If your pick is a lower rung than this thread's"),
                pays == Some(false),
                "{pays:?} {ladder:?}: {shape}"
            );
        }
    }

    /// The model and effort names live on the ladder, never in the guidance
    /// (ADR 0500). Scans the principle and both cards with no ladder
    /// rendered, for every model the menus list and every effort word.
    #[test]
    fn the_implementer_guidance_names_no_model_and_no_effort() {
        let mut texts = vec![
            IMPLEMENTER_CHOICE.to_string(),
            lower_options(true),
            paying_options(),
        ];
        for pays in [true, false] {
            texts.push(card_shape(reading(Some(pays), None), FLAGS));
        }
        let mut banned: Vec<String> = Vec::new();
        for menu in [
            crate::spawn_thread::CC_MENU,
            crate::spawn_thread::CODEX_MENU,
        ] {
            let parsed: serde_json::Value = serde_json::from_str(menu).expect("menu parses");
            for row in parsed["models"].as_array().expect("models") {
                let value = row["value"].as_str().expect("value");
                if value == "default" {
                    continue;
                }
                let label = row["label"].as_str().expect("label");
                banned.push(value.to_string());
                banned.push(label.to_string());
                banned.push(label.split(' ').next().unwrap_or(label).to_string());
            }
        }
        let words = |text: &str| -> Vec<String> {
            text.split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '.')
                .map(|w| w.trim_matches('.').to_string())
                .collect()
        };
        for text in &texts {
            // The thread's own implementer line quotes what it runs on.
            let text = text.replace("claude-code with model m-b at medium effort", "");
            for name in &banned {
                assert!(!text.contains(name.as_str()), "names {name:?}: {text}");
            }
            for word in words(&text) {
                assert!(
                    !crate::spawn_thread::EFFORT_LEVELS.contains(&word.as_str()),
                    "names the effort {word:?}: {text}"
                );
            }
        }
    }

    /// I8: only a paying verdict recommends a fresh session on this thread's own
    /// implementer, whatever the size.
    #[test]
    fn a_fresh_session_on_this_implementer_needs_a_paying_verdict() {
        let paying = card_shape(reading(Some(true), Some(ladder(Some(1)))), FLAGS);
        for needle in [
            "The handoff estimate says a fresh session pays here",
            "240k tokens",
            "This thread runs on claude-code with model m-b at medium effort.",
            "intelligence ladder, least to most intelligent",
            "[this thread]",
            "`Implement in a fresh session: <implementer> (Recommended)`",
            "`Implement in a fresh session: <this thread's implementer>`, only when it differs",
            "`Approve here` for a plan",
            "never more than 4 options, and never `Request changes`",
            "by your judgement of the work left",
            "a fully specified plan with settled decisions suits a lower rung",
            "lucidos successor start --narrative <file> --at plan-approval --progress 0/4`",
            "the successor session starts in this thread once it ends",
            "a plan flow approves the plan",
            "remaining checkpoint instructions",
            "picking this thread's implementer adds none",
        ] {
            assert!(paying.contains(needle), "missing {needle:?}: {paying}");
        }
        let large_but_not_paying = card_shape(reading(Some(false), None), FLAGS);
        assert!(
            !large_but_not_paying.contains("<this thread's implementer>"),
            "{large_but_not_paying}"
        );
        assert!(large_but_not_paying.contains("does not pay"));
    }

    #[test]
    fn a_thread_that_cannot_hand_off_gets_the_floor() {
        let closed = ContextReading::Read {
            input_tokens: Some(400_000),
            can_hand_off: false,
            pays: true,
            implementer: None,
            ladder: None,
        };
        let shape = card_shape(closed, FLAGS);
        assert_eq!(shape, FLOOR);
        assert!(shape.contains("`Approve` / `Request changes`"));
        assert!(!shape.contains("Implement in a fresh session"), "{shape}");
        let unknown = card_shape(ContextReading::Unknown, FLAGS);
        assert!(unknown.starts_with(FLOOR));
        assert!(unknown.contains("could not read this thread's context"));
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
