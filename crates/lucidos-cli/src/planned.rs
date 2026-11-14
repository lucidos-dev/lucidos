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

const IMPLEMENTER_CHOICE: &str = "Recommend the successor's implementer by your judgement of \
    the work left, such as a plan, as a rung on the intelligence ladder above. A higher rung is \
    more intelligent, costs more and runs longer. Turn intelligence down for settled work: a \
    fully specified plan with settled decisions suits a lower rung. Turn it up for hard or open \
    work: work that leaves design judgement to the builder keeps this thread's rung or takes a \
    higher one. Every call reads the whole context again, so a rung above what the work needs \
    costs on every turn. Say why in one line of your message. Name a rung by its picker label \
    and effort, as the ladder prints them. Name the backend only when it is not this thread's.";

/// Allocate scarce card slots before putting them in presentation order.
fn card_slots(
    plan: bool,
    other: bool,
    recommended: bool,
    on_other: bool,
    paying: bool,
    fork: bool,
) -> String {
    let candidates = [
        ('A', true),
        ('P', plan),
        ('X', other && !(recommended && on_other)),
        ('R', recommended),
        ('F', fork),
        ('T', paying && recommended),
    ];
    let selected: Vec<char> = candidates
        .into_iter()
        .filter(|(_, present)| *present)
        .take(4)
        .map(|(code, _)| code)
        .collect();
    ['R', 'X', 'T', 'A', 'P', 'F']
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
            out.push_str(&format!(
                "\n- {} recommendation, {} real fork: {}.",
                if !recommended {
                    "No"
                } else if on_other {
                    "Other-backend"
                } else {
                    "Same-backend"
                },
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
    } else {
        "The handoff estimate says a fresh session does not pay here. Recommend one only for a lower rung that fits the work."
    };
    let raise = if !paying && ladder.has_higher_intelligence_rung() {
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
    let alternative = other.map(|l| format!(" X = `Implement in a fresh session: <rung> on {}`. Always offer X when R does not already name this backend. Choose its rung from the other ladder above.", l.surface)).unwrap_or_default();
    let question_flow = "Use your normal user question tool. Offer a short set of relevant choices, not every rung. You may ask multiple questions, such as backend first and rung next. Each question has at most four options. The following shapes are single-question examples, not an exhaustive questionnaire.";
    let mut out = format!("{question_flow}\n\n{lead}{raise} {size}{runs}\n\n{}\n\n{IMPLEMENTER_CHOICE}\n\nFor choices sharing a question: A = `{}`.{} R = `Implement in a fresh session: <rung> (Recommended)`; when R uses the other backend, name it in the label and merge X into R.{} F = a real fork, only if there is one.{} Fill slots in priority A, P, X, R, F, T, omitting ineligible options; display them in order R, X, T, A, P, F. A typed reply requests changes; do not spend a slot on it.",
        ladder.render(), if plan { "Approve here" } else { "Continue here" }, plan_only, alternative,
        if paying { " T = `Implement in a fresh session: <this thread's rung>`, only when it differs from R." } else { "" });
    if plan {
        out.push_str(" If no fresh-session option remains, A reads `Approve`.");
    }
    out.push_str("\nThe resulting option codes are:");
    out.push_str(&card_examples(plan, other.is_some(), true, paying));
    if !paying {
        out.push_str(&card_examples(plan, other.is_some(), false, false));
    }
    out.push_str(&format!("\n{}", fresh_session_handoff(flags)));
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
            println!("Plan approved: {}. Implementation is unblocked.", branch);
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
        assert_eq!(card_slots(true, true, false, false, false, true), "XAPF");
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
                                assert_eq!(slots.contains('P'), plan);
                                assert_eq!(
                                    slots.contains('X'),
                                    other && !(recommended && on_other)
                                );
                            }
                        }
                    }
                }
            }
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
    fn other_backend_is_always_an_option_when_its_ladder_was_read() {
        let mut own = ladder(Some(1));
        let mut other = ladder(None);
        other.surface = "codex".into();
        own.other_backend_ladders.push(other);
        for pays in [false, true] {
            let text = card_shape(reading(Some(pays), Some(own.clone())), FLAGS);
            assert!(text.contains("on codex"));
            assert!(text.contains("A, P, X, R, F, T"));
            assert!(text.contains("R, X, T, A, P, F"));
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
        for word in IMPLEMENTER_CHOICE.split(|c: char| !c.is_ascii_alphanumeric()) {
            assert!(!crate::spawn_thread::EFFORT_LEVELS.contains(&word));
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
