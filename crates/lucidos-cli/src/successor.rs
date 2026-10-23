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

When: the user says \"handoff this session\", or picks an `Implement in a new thread` option \
on a card. In a Lucidos thread, advice to run `/clear` or to write a handoff file means this.

At a handoff checkpoint, run `lucidos successor offer --at <name>` as your flow writes it, and \
follow what it prints. A repository names its checkpoints in its plan skill, review flow or \
agent instructions; the `coding-agent-flows` system knowhow shows how to wire one. \
`--progress <done>/<total>` says how many units of the flow's work are done, and the engine's \
handoff estimate reads the units left. Without `--ask` the checkpoint is automatic: the \
output says to hand off on this thread's implementer or to carry on. With `--ask` it is \
offered: the output says which question card to put to the user.

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

/// The engine's verdict at an automatic checkpoint, as the CLI reads it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AutomaticVerdict {
    HandOff {
        saving_usd: Option<f64>,
        cost_usd: Option<f64>,
    },
    DoesNotPay,
    /// The engine could not be asked, or could not make a reading.
    Unknown,
}

pub(crate) fn parse_verdict(body: &serde_json::Value) -> AutomaticVerdict {
    let handoff = &body["handoff"];
    match handoff.get("pays").and_then(|v| v.as_bool()) {
        None => AutomaticVerdict::Unknown,
        Some(false) => AutomaticVerdict::DoesNotPay,
        Some(true) => {
            let usd = |key: &str| handoff["estimate"].get(key).and_then(|v| v.as_f64());
            AutomaticVerdict::HandOff {
                saving_usd: usd("saving_usd"),
                cost_usd: usd("cost_usd"),
            }
        }
    }
}

/// What an automatic checkpoint prints. It never names an implementer, and
/// asks nothing.
pub(crate) fn automatic_text(verdict: &AutomaticVerdict, run: &CheckpointRun) -> String {
    const CARRY_ON: &str = "Carry on in this session and ask nothing:";
    let (saving_usd, cost_usd) = match verdict {
        AutomaticVerdict::HandOff {
            saving_usd,
            cost_usd,
        } => (saving_usd, cost_usd),
        AutomaticVerdict::DoesNotPay => {
            return format!(
                "{CARRY_ON} the handoff estimate says a fresh session does not pay here."
            )
        }
        AutomaticVerdict::Unknown => {
            return format!("{CARRY_ON} Lucidos could not estimate a handoff here.")
        }
    };
    let figures = match (saving_usd, cost_usd) {
        (Some(saving), Some(cost)) => {
            format!(" It saves about ${saving:.2} and costs about ${cost:.2}.")
        }
        _ => String::new(),
    };
    format!(
        "The handoff estimate says a fresh session pays here.{figures} Hand off now and ask \
         nothing: the user has no choice to make. {}",
        handoff_steps(run)
    )
}

/// The numbered steps of an automatic handoff, on this thread's implementer.
fn handoff_steps(run: &CheckpointRun) -> String {
    format!(
        "1. Write a handoff narrative to a file outside the repo, with the six sections \
         `lucidos successor --help` names. Its next step is the step of your flow that follows \
         this checkpoint. Quote your flow's remaining checkpoint instructions in it, so the \
         successor session keeps running them. 2. Run `lucidos successor start --narrative \
         <file> {}`, with no other flags, so the successor session runs on this thread's \
         implementer. 3. End your turn: the successor session starts in this thread once it \
         ends.",
        run.flags()
    )
}

/// The body of `POST /api/v1/threads/:id/handoff-checkpoints`.
pub(crate) fn checkpoint_body(run: &CheckpointRun, ask: bool) -> serde_json::Value {
    let mut body = serde_json::json!({ "name": run.name });
    if let Some(progress) = &run.progress {
        body["progress"] = progress.clone().into();
    }
    if ask {
        body["offered"] = true.into();
    }
    body
}

/// Run a checkpoint: record it with the engine and print what to do. An
/// offered run prints the card's shape, an automatic one the verdict.
pub(crate) fn cmd_offer(ws: &Workspace, run: CheckpointRun, ask: bool) {
    let body = post_checkpoint(ws, &run, ask);
    if ask {
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
fn post_checkpoint(ws: &Workspace, run: &CheckpointRun, ask: bool) -> Option<serde_json::Value> {
    let thread_id = own_thread_id().ok()?;
    let url = format!(
        "{}/api/v1/threads/{thread_id}/handoff-checkpoints",
        ws.base_url()
    );
    let resp = http_client()
        .ok()?
        .post(&url)
        .json(&checkpoint_body(run, ask))
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
        assert_eq!(
            checkpoint_body(&run("root-cause", None), false),
            serde_json::json!({"name": "root-cause"})
        );
        assert_eq!(
            checkpoint_body(&run("plan-approval", Some("0/4")), true),
            serde_json::json!({"name": "plan-approval", "progress": "0/4", "offered": true})
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
        assert_eq!(
            parse_verdict(&body(true.into())),
            AutomaticVerdict::HandOff {
                saving_usd: Some(1.5),
                cost_usd: Some(0.4)
            }
        );
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
            saving_usd: Some(1.5),
            cost_usd: Some(0.4),
        }
    }

    /// No automatic checkpoint names an implementer flag, a plan card's
    /// option, or a card of any kind, whatever the verdict.
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
