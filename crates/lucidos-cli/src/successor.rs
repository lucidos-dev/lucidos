//! `lucidos successor`: run a *handoff checkpoint*, and continue this thread
//! in a *successor session*: a fresh session in the same thread, which carries
//! on with a clean context.

use crate::http::client as http_client;
use crate::workspace::{BoxError, Workspace};

/// `lucidos successor --help`: the whole procedure, so the engine prompt can
/// stay one line.
pub(crate) const HELP: &str = "\
Continue this thread in a successor session: a fresh session in this same thread, which carries \
on with a clean context.

When: the user says \"handoff this session\", or picks an `Implement in a fresh session` option \
on a card. In a Lucidos thread, advice to run `/clear` or to write a handoff file means this.

At a handoff checkpoint, run `lucidos successor offer --at <name>` as your flow writes it, and \
follow what it prints. A repository names its checkpoints in its plan skill, review flow or \
agent instructions; the `coding-agent-flows` system knowhow shows how to wire one. \
`--progress <done>/<total>` says how many units of the flow's work are done, and the engine's \
handoff estimate reads the units left. Without `--ask` the checkpoint is automatic: the \
output says to hand off, and on which implementer, or to carry on. `--default-floor` \
declares the default rung the floor for an automatic run: a thread below it hands off onto \
it, and the output names a risk rung for security work or a costly one-way door. With \
`--ask` it is offered: the output says which question card to put to the user.

How:
1. Write a handoff narrative to a file outside the repo, for example under $TMPDIR. It needs \
one markdown heading for each of: Outcomes, Decisions, Gotchas, State, Next step, Files. \
Write it for a reader with none of this session's context. Cover what was learned, what was \
settled and why, what bit you, and where things stand. Then name the first step and the files \
that matter.
2. Run `lucidos successor start --narrative <file>`. At an automatic checkpoint, add the `--at` \
and `--progress` flags `offer` printed.
3. End your turn. The successor session starts in this thread once it ends, and reads your \
narrative first.

The successor session runs on this thread's implementer: its backend, model and reasoning \
effort. `--coding-agent`, `--coding-agent-model` and `--reasoning-effort` name another, as on \
`spawn-thread`. A model carries over only on the same backend. Naming this thread's model \
without a `[1m]` suffix keeps its exact id, `[1m]` included, so the session reads the same \
prompt cache. An effort carries over only on the same backend while the new model accepts it. \
Lucidos refuses a model or effort the backend does not offer, and lists the ones it does. A \
model or effort you name must be a rung on the intelligence ladder (`lucidos \
intelligence-ladder`). Carrying this thread's own over is never refused.

The thread keeps its branch, its worktree and its change, uncommitted work included. A parent \
waiting on this thread keeps waiting, and hears from it once at the real end. A handoff needs \
no approval.";

/// The calling thread, which is the only thread a handoff can start from.
fn own_thread_id() -> Result<String, BoxError> {
    std::env::var("LUCIDOS_THREAD_ID").map_err(|_| {
        "LUCIDOS_THREAD_ID is not set: run this inside the coding-agent thread that continues"
            .into()
    })
}

/// The *implementer* a handoff names for its successor session. An unset
/// field carries over from this thread, as the engine decides.
#[derive(Debug, Default)]
pub(crate) struct ImplementerChoice {
    pub(crate) coding_agent: Option<crate::CliCodingAgent>,
    pub(crate) model: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
}

/// A *handoff checkpoint* run, as `--at` and `--progress` name it. The engine
/// validates both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckpointRun {
    pub(crate) name: String,
    pub(crate) progress: Option<String>,
}

impl CheckpointRun {
    /// The flags that name this run again on `successor start`.
    fn flags(&self) -> String {
        match &self.progress {
            Some(progress) => format!("--at {} --progress {progress}", self.name),
            None => format!("--at {}", self.name),
        }
    }

    /// The `checkpoint` and `progress` fields of the start body.
    fn start_fields(&self) -> Vec<(&'static str, String)> {
        let mut fields = vec![("checkpoint", self.name.clone())];
        if let Some(progress) = &self.progress {
            fields.push(("progress", progress.clone()));
        }
        fields
    }
}

/// How a checkpoint run decides: alone, or by offering the user a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckpointKind {
    /// With `default_floor`, the engine may offer the floor and risk rungs.
    Automatic {
        default_floor: bool,
    },
    Offered,
}

/// A rung on the intelligence ladder, exactly as the engine named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OfferedIntelligenceRung {
    pub(crate) model: String,
    pub(crate) effort: String,
}

impl OfferedIntelligenceRung {
    fn describe(&self) -> String {
        format!("model {} at {} effort", self.model, self.effort)
    }

    /// The `successor start` flags that start a successor session on this rung.
    fn flags(&self) -> String {
        format!(
            "--coding-agent-model {} --reasoning-effort {}",
            self.model, self.effort
        )
    }
}

fn parse_offered_intelligence_rung(
    value: Option<&serde_json::Value>,
) -> Option<OfferedIntelligenceRung> {
    let text = |key: &str| Some(value?.get(key)?.as_str()?.to_string());
    Some(OfferedIntelligenceRung {
        model: text("model")?,
        effort: text("effort")?,
    })
}

/// Which implementer a suggested handoff runs on.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum HandoffOn {
    /// The handoff pays, and no floor rung is offered.
    ThisImplementer {
        saving_usd: Option<f64>,
        cost_usd: Option<f64>,
    },
    /// The run declared the default floor, and this thread sits below it.
    FloorRung(OfferedIntelligenceRung),
}

/// The engine's verdict at an automatic checkpoint, as the CLI reads it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AutomaticVerdict {
    HandOff {
        on: HandoffOn,
        risk_rung: Option<OfferedIntelligenceRung>,
    },
    /// Only the risk rung is offered: carry on unless the change passes one
    /// of its two tests.
    RiskRungOnly {
        risk_rung: OfferedIntelligenceRung,
    },
    DoesNotPay,
    /// The engine could not be asked, or could not make a reading.
    Unknown,
}

pub(crate) fn parse_verdict(body: &serde_json::Value) -> AutomaticVerdict {
    let handoff = &body["handoff"];
    let pays = handoff.get("pays").and_then(|v| v.as_bool());
    let floor_rung = parse_offered_intelligence_rung(handoff.get("floor_rung"));
    let risk_rung = parse_offered_intelligence_rung(handoff.get("risk_rung"));
    if pays != Some(true) && floor_rung.is_none() && risk_rung.is_none() {
        return match pays {
            Some(false) => AutomaticVerdict::DoesNotPay,
            _ => AutomaticVerdict::Unknown,
        };
    }
    let on = match (floor_rung, pays) {
        (Some(floor), _) => HandoffOn::FloorRung(floor),
        (None, Some(true)) => {
            let usd = |key: &str| handoff["estimate"].get(key).and_then(|v| v.as_f64());
            HandoffOn::ThisImplementer {
                saving_usd: usd("saving_usd"),
                cost_usd: usd("cost_usd"),
            }
        }
        (None, _) => {
            return risk_rung.map_or(AutomaticVerdict::Unknown, |risk_rung| {
                AutomaticVerdict::RiskRungOnly { risk_rung }
            })
        }
    };
    AutomaticVerdict::HandOff { on, risk_rung }
}

/// The two tests that send a handoff to the risk rung.
const RISK_TESTS: &str = "security work (a security or authority boundary, secrets, \
     credentials, pairing, sandboxing, or what an agent may do) or a costly one-way door (a \
     migration or new stored format, a published contract, a release or install format, or \
     destroyed data or git work)";

/// Handing off needs no approval, so the user has nothing to decide.
const HAND_OFF_NOW: &str = "Hand off now and ask nothing: the user has no choice to make.";

/// What an automatic checkpoint prints. It names an implementer only for a
/// rung the engine offered, and asks nothing.
pub(crate) fn automatic_text(verdict: &AutomaticVerdict, run: &CheckpointRun) -> String {
    const CARRY_ON: &str = "Carry on in this session and ask nothing:";
    match verdict {
        AutomaticVerdict::DoesNotPay => {
            format!("{CARRY_ON} the handoff estimate says a fresh session does not pay here.")
        }
        AutomaticVerdict::Unknown => {
            format!("{CARRY_ON} Lucidos could not estimate a handoff here.")
        }
        AutomaticVerdict::HandOff { on, risk_rung } => {
            let (lead, floor) = match on {
                HandoffOn::ThisImplementer {
                    saving_usd,
                    cost_usd,
                } => {
                    let figures = match (saving_usd, cost_usd) {
                        (Some(saving), Some(cost)) => {
                            format!(" It saves about ${saving:.2} and costs about ${cost:.2}.")
                        }
                        _ => String::new(),
                    };
                    (
                        format!("The handoff estimate says a fresh session pays here.{figures}"),
                        None,
                    )
                }
                HandoffOn::FloorRung(floor) => (
                    format!(
                        "This checkpoint runs on the default floor, and this thread runs \
                         below it: hand off on the floor rung, {}.",
                        floor.describe()
                    ),
                    Some(floor),
                ),
            };
            let risk = risk_rung.as_ref().map_or(String::new(), |risk| {
                format!(
                    " If the change is {RISK_TESTS}, hand off on the risk rung instead, {}, \
                     and say why in one line.",
                    risk.describe()
                )
            });
            let steps = handoff_steps(run, floor, risk_rung.as_ref());
            format!("{lead}{risk} {HAND_OFF_NOW} {steps}")
        }
        AutomaticVerdict::RiskRungOnly { risk_rung } => {
            let steps = handoff_steps(run, Some(risk_rung), None);
            format!(
                "Carry on in this session and ask nothing, unless the change is {RISK_TESTS}. \
                 Then hand off on the risk rung, {}, and say why in one line. {HAND_OFF_NOW} \
                 {steps}",
                risk_rung.describe()
            )
        }
    }
}

/// The numbered steps of an automatic handoff: on `rung`, else on this
/// thread's implementer. `risk_rung` names the alternative flags.
fn handoff_steps(
    run: &CheckpointRun,
    rung: Option<&OfferedIntelligenceRung>,
    risk_rung: Option<&OfferedIntelligenceRung>,
) -> String {
    let start = match rung {
        None => format!(
            "`lucidos successor start --narrative <file> {}`, with no other flags, so the \
             successor session runs on this thread's implementer.",
            run.flags()
        ),
        Some(rung) => format!(
            "`lucidos successor start --narrative <file> {} {}`.",
            run.flags(),
            rung.flags()
        ),
    };
    let risk = risk_rung.map_or(String::new(), |risk| {
        format!(" On the risk rung, name `{}` instead.", risk.flags())
    });
    format!(
        "1. Write a handoff narrative to a file outside the repo, with the six sections \
         `lucidos successor --help` names. Its next step is the step of your flow that follows \
         this checkpoint. Quote your flow's remaining checkpoint instructions in it, so the \
         successor session keeps running them. 2. Run {start}{risk} 3. End your turn: the \
         successor session starts in this thread once it ends."
    )
}

/// The body of `POST /api/v1/threads/:id/handoff-checkpoints`.
pub(crate) fn checkpoint_body(run: &CheckpointRun, kind: CheckpointKind) -> serde_json::Value {
    let mut body = serde_json::json!({ "name": run.name });
    if let Some(progress) = &run.progress {
        body["progress"] = progress.clone().into();
    }
    match kind {
        CheckpointKind::Offered => body["offered"] = true.into(),
        CheckpointKind::Automatic {
            default_floor: true,
        } => body["default_floor"] = true.into(),
        CheckpointKind::Automatic {
            default_floor: false,
        } => {}
    }
    body
}

/// Run a checkpoint: record it with the engine and print what to do. An
/// offered run prints the card's shape, an automatic one the verdict.
pub(crate) fn cmd_offer(ws: &Workspace, run: CheckpointRun, kind: CheckpointKind) {
    let body = post_checkpoint(ws, &run, kind);
    if kind == CheckpointKind::Offered {
        let reading = match body.map_or(ContextReading::Unknown, |b| parse_reading(&b)) {
            ContextReading::Read {
                input_tokens,
                can_hand_off,
                pays,
                implementer,
                ..
            } => ContextReading::Read {
                input_tokens,
                can_hand_off,
                pays,
                implementer,
                ladder: crate::intelligence_ladder::read_own(ws),
            },
            unknown => unknown,
        };
        println!("{}", crate::planned::card_shape(reading, &run.flags()));
    } else {
        let verdict = body.map_or(AutomaticVerdict::Unknown, |b| parse_verdict(&b));
        println!("{}", automatic_text(&verdict, &run));
    }
}

/// POST the run for the calling thread. `None` when the engine could not be
/// asked or refused, which reads as unknown, never as small.
fn post_checkpoint(
    ws: &Workspace,
    run: &CheckpointRun,
    kind: CheckpointKind,
) -> Option<serde_json::Value> {
    let thread_id = own_thread_id().ok()?;
    let url = format!(
        "{}/api/v1/threads/{thread_id}/handoff-checkpoints",
        ws.base_url()
    );
    let resp = http_client()
        .ok()?
        .post(&url)
        .json(&checkpoint_body(run, kind))
        .send()
        .ok()?;
    if !resp.status().is_success() {
        let status = resp.status();
        let reason = resp.text().unwrap_or_default();
        eprintln!("{}", refusal_text(status, &reason));
        return None;
    }
    resp.json().ok()
}

/// What to print when the engine answers a checkpoint with a failure. A 404
/// with no body is the router's own answer for an unknown route, so the engine
/// is older than this CLI. A refused thread carries a JSON body.
fn refusal_text(status: reqwest::StatusCode, body: &str) -> String {
    if status == reqwest::StatusCode::NOT_FOUND && body.trim().is_empty() {
        return format!(
            "The checkpoint was refused ({status}): the running engine has no such route. \
             It is likely older than this CLI. Run the checkpoint again after the next \
             switch to the new version."
        );
    }
    format!("The checkpoint was refused ({status}): {body}")
}

/// The `POST /api/v1/threads/:id/successor` body for an agent's handoff.
pub(crate) fn start_body(
    narrative: String,
    choice: ImplementerChoice,
    at: Option<CheckpointRun>,
) -> serde_json::Value {
    let mut body = serde_json::json!({ "narrative": narrative });
    for (key, value) in at.iter().flat_map(CheckpointRun::start_fields) {
        body[key] = value.into();
    }
    if let Some(agent) = choice.coding_agent {
        body["coding_agent"] = agent.as_wire().into();
    }
    if let Some(model) = choice.model {
        body["coding_agent_model"] = model.into();
    }
    if let Some(effort) = choice.reasoning_effort {
        body["reasoning_effort"] = effort.into();
    }
    body
}

/// What `successor start` prints once the engine recorded the handoff.
pub(crate) fn started_text(body: &serde_json::Value) -> String {
    let runs_on = parse_implementer(&body["implementer"])
        .map(|implementer| format!(" It runs on {}.", implementer.describe()))
        .unwrap_or_default();
    if body["starts"] == "now" {
        format!("The successor session is starting in this thread.{runs_on}")
    } else {
        format!(
            "The successor session starts in this thread once this turn ends.{runs_on} End your \
             turn now: the new session reads your narrative first and carries on from its next \
             step."
        )
    }
}

/// POST the narrative and print what happens next. The engine checks the
/// narrative's sections and the implementer.
pub(crate) fn cmd_start(
    ws: &Workspace,
    narrative_path: &std::path::Path,
    choice: ImplementerChoice,
    at: Option<CheckpointRun>,
) -> Result<(), BoxError> {
    let narrative = std::fs::read_to_string(narrative_path).map_err(|e| {
        format!(
            "Could not read the handoff narrative {}: {e}",
            narrative_path.display()
        )
    })?;
    let thread_id = own_thread_id()?;
    let url = format!("{}/api/v1/threads/{thread_id}/successor", ws.base_url());
    let chosen_backend = choice.coding_agent;
    let resp = http_client()?
        .post(&url)
        .json(&start_body(narrative, choice, at))
        .send()
        .map_err(|e| format!("POST {url} failed: {e}"))?;
    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("POST {url} returned {status} with a non-JSON body: {e}"))?;
    if !status.is_success() {
        let message = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("no reason given");
        // The ladder of the backend the pick named, else this thread's own.
        let ladder = (body["reason"] == crate::intelligence_ladder::OFF_LADDER_REASON)
            .then(|| match chosen_backend {
                Some(agent) => crate::intelligence_ladder::render_for_refusal(
                    &ws.base_url(),
                    None,
                    Some(agent.as_wire()),
                ),
                None => crate::intelligence_ladder::read_own(ws).map(|l| l.render()),
            })
            .flatten()
            .map(|ladder| format!("\n\n{ladder}"))
            .unwrap_or_default();
        return Err(format!("The handoff was refused ({status}): {message}{ladder}").into());
    }
    println!("{}", started_text(&body));
    Ok(())
}

/// A thread's *implementer*, as the engine reports it. An unset model or
/// effort is the backend's own default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Implementer {
    pub(crate) coding_agent: String,
    pub(crate) model: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
}

impl Implementer {
    /// One line naming all three, the way the card text quotes it.
    pub(crate) fn describe(&self) -> String {
        format!(
            "{} with model {} at {} effort",
            self.coding_agent,
            self.model.as_deref().unwrap_or("(the backend's default)"),
            self.reasoning_effort
                .as_deref()
                .unwrap_or("(the backend's default)"),
        )
    }
}

fn parse_implementer(value: &serde_json::Value) -> Option<Implementer> {
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
    Some(Implementer {
        coding_agent: text("coding_agent")?,
        model: text("model"),
        reasoning_effort: text("reasoning_effort"),
    })
}

/// This thread's context at an offered checkpoint, as the engine decides it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ContextReading {
    Read {
        input_tokens: Option<u64>,
        /// The thread may hand off at all. False when the engine did not
        /// say, so a card never offers a handoff it would refuse.
        can_hand_off: bool,
        /// The handoff estimate says a fresh session pays here. False when
        /// the engine could not make a reading.
        pays: bool,
        /// `None` when the engine did not report one.
        implementer: Option<Implementer>,
        /// This thread's intelligence ladder. `None` when it was not read.
        ladder: Option<crate::intelligence_ladder::Ladder>,
    },
    /// The engine could not be asked, or did not answer.
    Unknown,
}

/// Read a checkpoint run's answer for the offered card. The ladder is read
/// separately, so it is `None` here.
pub(crate) fn parse_reading(body: &serde_json::Value) -> ContextReading {
    if body.get("handoff").is_none() {
        return ContextReading::Unknown;
    }
    ContextReading::Read {
        input_tokens: body.get("input_tokens").and_then(|v| v.as_u64()),
        can_hand_off: body.get("can_hand_off").and_then(|v| v.as_bool()) == Some(true),
        pays: body["handoff"].get("pays").and_then(|v| v.as_bool()) == Some(true),
        implementer: body.get("implementer").and_then(parse_implementer),
        ladder: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(name: &str, progress: Option<&str>) -> CheckpointRun {
        CheckpointRun {
            name: name.into(),
            progress: progress.map(str::to_string),
        }
    }

    fn answer(pays: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "input_tokens": 240000, "can_hand_off": true,
            "implementer": {"coding_agent": "claude-code", "model": "claude-opus-5-5", "reasoning_effort": null},
            "handoff": {"name": "plan-approval", "pays": pays},
        })
    }

    /// I1: the offered card reads the engine's pays flag, never the size.
    #[test]
    fn the_offered_reading_is_the_engines_verdict() {
        let ContextReading::Read {
            input_tokens,
            can_hand_off,
            pays,
            implementer: Some(implementer),
            ladder: None,
        } = parse_reading(&answer(false.into()))
        else {
            panic!("no reading");
        };
        assert_eq!(input_tokens, Some(240_000));
        assert!(can_hand_off);
        assert!(!pays, "a large context the estimate rejects does not pay");
        assert_eq!(
            implementer.describe(),
            "claude-code with model claude-opus-5-5 at (the backend's default) effort"
        );
        assert!(matches!(
            parse_reading(&answer(true.into())),
            ContextReading::Read { pays: true, .. }
        ));
        assert!(matches!(
            parse_reading(&answer(serde_json::Value::Null)),
            ContextReading::Read { pays: false, .. }
        ));
        let mut closed = answer(true.into());
        closed["can_hand_off"] = serde_json::Value::Null;
        assert!(matches!(
            parse_reading(&closed),
            ContextReading::Read {
                can_hand_off: false,
                ..
            }
        ));
        assert_eq!(
            parse_reading(&serde_json::json!({"input_tokens": 5})),
            ContextReading::Unknown,
            "a body with no verdict is unknown"
        );
    }

    /// Only what the user picked goes on the wire: an omitted field is the
    /// engine's to carry over.
    #[test]
    fn the_start_body_names_only_the_chosen_fields() {
        assert_eq!(
            start_body("n".into(), ImplementerChoice::default(), None),
            serde_json::json!({"narrative": "n"})
        );
        let chosen = ImplementerChoice {
            coding_agent: Some(crate::CliCodingAgent::Codex),
            model: Some("gpt-5.6-sol".into()),
            reasoning_effort: Some("high".into()),
        };
        assert_eq!(
            start_body("n".into(), chosen, None),
            serde_json::json!({
                "narrative": "n",
                "coding_agent": "codex",
                "coding_agent_model": "gpt-5.6-sol",
                "reasoning_effort": "high",
            })
        );
    }

    #[test]
    fn the_start_body_names_the_checkpoint() {
        assert_eq!(
            start_body(
                "n".into(),
                ImplementerChoice::default(),
                Some(run("phase-commit", Some("2/5")))
            ),
            serde_json::json!({
                "narrative": "n", "checkpoint": "phase-commit", "progress": "2/5",
            })
        );
    }

    /// The agent ends its turn so the session can start; a caller outside a
    /// turn is told it is starting.
    #[test]
    fn the_start_text_says_when_the_session_starts() {
        let implementer = serde_json::json!({"coding_agent": "codex", "model": "gpt-5.6-sol"});
        let after = started_text(
            &serde_json::json!({"starts": "after-this-turn", "implementer": implementer}),
        );
        assert!(after.contains("once this turn ends"), "{after}");
        assert!(after.contains("End your turn now"), "{after}");
        assert!(after.contains("codex with model gpt-5.6-sol"), "{after}");
        let now = started_text(&serde_json::json!({"starts": "now"}));
        assert!(now.contains("is starting"), "{now}");
        assert!(!now.contains("End your turn"), "{now}");
    }

    #[test]
    fn the_checkpoint_body_names_the_run_and_its_kind() {
        let automatic = |default_floor| CheckpointKind::Automatic { default_floor };
        assert_eq!(
            checkpoint_body(&run("root-cause", None), automatic(false)),
            serde_json::json!({"name": "root-cause"})
        );
        assert_eq!(
            checkpoint_body(&run("plan-approval", Some("0/4")), CheckpointKind::Offered),
            serde_json::json!({"name": "plan-approval", "progress": "0/4", "offered": true})
        );
        assert_eq!(
            checkpoint_body(&run("pre-harden", None), automatic(true)),
            serde_json::json!({"name": "pre-harden", "default_floor": true})
        );
    }

    #[test]
    fn a_bodiless_404_names_a_stale_engine_and_a_refused_thread_does_not() {
        use reqwest::StatusCode;
        let unknown_route = refusal_text(StatusCode::NOT_FOUND, "");
        assert!(
            unknown_route.contains("older than this CLI"),
            "{unknown_route}"
        );

        let refused = refusal_text(
            StatusCode::NOT_FOUND,
            r#"{"error":"Thread 1234 does not exist"}"#,
        );
        assert!(refused.contains("Thread 1234 does not exist"), "{refused}");
        assert!(!refused.contains("older than this CLI"), "{refused}");

        let other = refusal_text(StatusCode::FORBIDDEN, "");
        assert!(!other.contains("older than this CLI"), "{other}");
    }

    /// I1: the verdict is read from the engine's fields, never computed.
    #[test]
    fn the_verdict_is_read_never_recomputed() {
        let body = |pays: serde_json::Value| {
            serde_json::json!({"handoff": {
                "name": "pre-harden", "pays": pays,
                "estimate": {"saving_usd": 1.5, "cost_usd": 0.4},
            }})
        };
        assert_eq!(parse_verdict(&body(true.into())), pays());
        assert_eq!(
            parse_verdict(&body(false.into())),
            AutomaticVerdict::DoesNotPay
        );
        assert_eq!(
            parse_verdict(&body(serde_json::Value::Null)),
            AutomaticVerdict::Unknown
        );
        assert_eq!(
            parse_verdict(&serde_json::json!({"input_tokens": 5})),
            AutomaticVerdict::Unknown
        );
    }

    fn checkpoints() -> [CheckpointRun; 2] {
        [run("pre-harden", None), run("phase-commit", Some("2/5"))]
    }

    fn pays() -> AutomaticVerdict {
        AutomaticVerdict::HandOff {
            on: HandoffOn::ThisImplementer {
                saving_usd: Some(1.5),
                cost_usd: Some(0.4),
            },
            risk_rung: None,
        }
    }

    /// An engine answer from a run that declared the default floor. The
    /// rung names are opaque here: the CLI only echoes them.
    fn floor_answer(pays: bool, floor: bool, risk: bool) -> serde_json::Value {
        let mut handoff = serde_json::json!({
            "name": "pre-harden", "pays": pays,
            "estimate": {"saving_usd": 1.5, "cost_usd": 0.4},
        });
        if floor {
            handoff["floor_rung"] = serde_json::json!({"model": "floor-model", "effort": "e2"});
        }
        if risk {
            handoff["risk_rung"] = serde_json::json!({"model": "risk-model", "effort": "e3"});
        }
        serde_json::json!({ "handoff": handoff })
    }

    fn rung(model: &str, effort: &str) -> OfferedIntelligenceRung {
        OfferedIntelligenceRung {
            model: model.into(),
            effort: effort.into(),
        }
    }

    /// I6: the rungs come from the engine's answer, and a floor rung wins
    /// over this thread's implementer whether or not the run pays.
    #[test]
    fn the_rungs_are_read_from_the_engine() {
        for pays in [true, false] {
            assert_eq!(
                parse_verdict(&floor_answer(pays, true, true)),
                AutomaticVerdict::HandOff {
                    on: HandoffOn::FloorRung(rung("floor-model", "e2")),
                    risk_rung: Some(rung("risk-model", "e3")),
                }
            );
        }
        assert_eq!(
            parse_verdict(&floor_answer(true, false, true)),
            AutomaticVerdict::HandOff {
                on: HandoffOn::ThisImplementer {
                    saving_usd: Some(1.5),
                    cost_usd: Some(0.4)
                },
                risk_rung: Some(rung("risk-model", "e3")),
            }
        );
        assert_eq!(
            parse_verdict(&floor_answer(false, false, true)),
            AutomaticVerdict::RiskRungOnly {
                risk_rung: rung("risk-model", "e3"),
            }
        );
        let mut half = floor_answer(false, true, false);
        half["handoff"]["floor_rung"] = serde_json::json!({"model": "floor-model"});
        assert_eq!(
            parse_verdict(&half),
            AutomaticVerdict::DoesNotPay,
            "a rung with no effort is no rung"
        );
    }

    /// I6: a floor rung hands off on the engine's flags, even when the run
    /// does not pay. The risk rung is the named alternative.
    #[test]
    fn a_floor_rung_hands_off_on_the_engines_flags() {
        for checkpoint in checkpoints() {
            let text = automatic_text(
                &parse_verdict(&floor_answer(false, true, true)),
                &checkpoint,
            );
            assert!(text.contains("Hand off now and ask nothing"), "{text}");
            assert!(
                text.contains(&format!(
                    "lucidos successor start --narrative <file> {} --coding-agent-model \
                     floor-model --reasoning-effort e2`",
                    checkpoint.flags()
                )),
                "{text}"
            );
            assert!(
                text.contains("`--coding-agent-model risk-model --reasoning-effort e3` instead"),
                "{text}"
            );
            assert!(text.contains("security work"), "{text}");
            assert!(text.contains("costly one-way door"), "{text}");
            assert!(!text.contains("with no other flags"), "{text}");
        }
    }

    /// A paying run with only a risk rung keeps this thread's implementer,
    /// and names the risk rung's flags as the alternative.
    #[test]
    fn a_paying_handoff_names_the_risk_rung_as_the_alternative() {
        let text = automatic_text(
            &parse_verdict(&floor_answer(true, false, true)),
            &run("pre-harden", None),
        );
        assert!(text.contains("pays here"), "{text}");
        assert!(text.contains("with no other flags"), "{text}");
        assert!(
            text.contains("`--coding-agent-model risk-model --reasoning-effort e3` instead"),
            "{text}"
        );
    }

    /// With only a risk rung, the default is to carry on, and the handoff is
    /// conditional on the two tests. It never asks the owner.
    #[test]
    fn a_risk_rung_alone_carries_on_unless_a_test_passes() {
        let text = automatic_text(
            &parse_verdict(&floor_answer(false, false, true)),
            &run("pre-harden", None),
        );
        assert!(
            text.starts_with("Carry on in this session and ask nothing, unless the change"),
            "{text}"
        );
        assert!(text.contains("security work"), "{text}");
        assert!(
            text.contains("--coding-agent-model risk-model --reasoning-effort e3`"),
            "{text}"
        );
        assert!(!text.contains("ask-owner-approval"), "{text}");
    }

    /// No automatic checkpoint names an implementer flag or a plan card's
    /// option, whatever the verdict.
    #[test]
    fn an_automatic_checkpoint_never_names_an_implementer_or_asks() {
        for checkpoint in checkpoints() {
            for verdict in [
                pays(),
                AutomaticVerdict::DoesNotPay,
                AutomaticVerdict::Unknown,
            ] {
                let text = automatic_text(&verdict, &checkpoint);
                for banned in [
                    "--coding-agent",
                    "--reasoning-effort",
                    "Implement in a",
                    "AskUserQuestion",
                    "ask-owner-approval",
                ] {
                    assert!(!text.contains(banned), "{banned}: {text}");
                }
            }
        }
    }

    /// I7: a reading that does not pay, or could not be made, carries on and
    /// asks nothing.
    #[test]
    fn a_reading_that_does_not_pay_carries_on() {
        for checkpoint in checkpoints() {
            for verdict in [AutomaticVerdict::DoesNotPay, AutomaticVerdict::Unknown] {
                let text = automatic_text(&verdict, &checkpoint);
                assert!(text.starts_with("Carry on in this session and ask nothing"));
                assert!(!text.contains("lucidos successor start"), "{text}");
            }
        }
    }

    /// I7: a paying handoff hands off in place and asks nothing.
    #[test]
    fn a_paying_handoff_hands_off_without_asking() {
        for checkpoint in checkpoints() {
            let text = automatic_text(&pays(), &checkpoint);
            assert!(text.contains("Hand off now and ask nothing"), "{text}");
            assert!(
                text.contains(&format!(
                    "lucidos successor start --narrative <file> {}`, with no other flags",
                    checkpoint.flags()
                )),
                "{text}"
            );
            assert!(text.contains("remaining checkpoint instructions"), "{text}");
            assert!(text.contains("End your turn"), "{text}");
        }
    }
}
