//! `lucidos successor`: run a *handoff checkpoint*, and continue this thread
//! in a *successor thread*, which carries on with a fresh context and this
//! thread's branch.

use crate::http::client as http_client;
use crate::workspace::{BoxError, Workspace};

/// `lucidos successor --help`: the whole procedure, so the engine prompt can
/// stay one line.
pub(crate) const HELP: &str = "\
Continue this thread in a successor thread, which carries on with a fresh context.

When: the user says \"handoff this session\", or picks an `Implement in a new thread` option on \
a card. In a Lucidos thread, advice to run `/clear` or to write a handoff file means this.

At a handoff checkpoint, run `lucidos successor offer --at <name>` as your flow writes it, and \
follow what it prints. A repository names its checkpoints in its plan skill, review flow or \
agent instructions; the `coding-agent-flows` system knowhow shows how to wire one. \
`--progress <done>/<total>` says how many units of the flow's work are done, and the engine's \
handoff estimate reads the units left. Without `--ask` the checkpoint is automatic: the \
output says to hand off on this thread's implementer or to carry on. It asks the owner only \
when a handoff pays and needs their approval. With `--ask` it is offered: the output says which question card to put to the user.

How:
1. Commit your work. Lucidos refuses a worktree with uncommitted changes.
2. Write a handoff narrative to a file outside the repo, for example under $TMPDIR. It needs \
one markdown heading for each of: Outcomes, Decisions, Gotchas, State, Next step, Files. \
Write it for a reader with none of this thread's context. Cover what was learned, what was \
settled and why, what bit you, and where things stand. Then name the first step and the files \
that matter.
3. Run `lucidos successor spawn --narrative <file>`, put the link it prints in your final \
message, and end your turn. At an automatic checkpoint, add the `--at` and `--progress` flags \
`offer` printed. Add `--build-chain` when your flow says so.

The successor runs on this thread's implementer: its backend, model and reasoning effort. \
`--coding-agent`, `--coding-agent-model` and `--reasoning-effort` name another, as on \
`spawn-thread`. A model carries over only on the same backend. Naming this thread's model \
without a `[1m]` suffix keeps its exact id, `[1m]` included, so the successor reads the same \
prompt cache. An effort carries over only \
on the same backend while the successor's model accepts it. Lucidos refuses a model or effort the backend does not \
offer, and lists the ones it does. A model or effort you name must be a rung on the \
intelligence ladder (`lucidos intelligence-ladder`). Carrying this thread's own over is never \
refused.

The successor's branch starts at this thread's last commit, so its change carries all of this \
thread's work, and this thread proposes nothing. It sits in this thread's place: top-level if \
this thread is, otherwise a child of the same parent, which then waits on it. A top-level \
successor needs no approval. A child one, from a turn the owner did not open, needs \
spawn-successor on the parent. A thread that inherited the consent that started it needs none \
at an automatic checkpoint whose estimate pays, named with `--at`. This thread archives itself once the successor starts and \
nothing blocks it.

`--build-chain` on a top-level thread starts a build chain instead: the builder is a child of \
this thread, which stays open. A chain member hands off under the same planner with no \
approval when the estimate for its `--at` checkpoint pays.";

/// The calling thread, which is the only thread a handoff can start from.
fn own_thread_id() -> Result<String, BoxError> {
    std::env::var("LUCIDOS_THREAD_ID").map_err(|_| {
        "LUCIDOS_THREAD_ID is not set: run this inside the coding-agent thread that continues"
            .into()
    })
}

/// The *implementer* a handoff names for its successor. An unset field
/// carries over from this thread, as the engine decides.
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
    /// The flags that name this run again on `successor spawn`.
    fn flags(&self) -> String {
        match &self.progress {
            Some(progress) => format!("--at {} --progress {progress}", self.name),
            None => format!("--at {}", self.name),
        }
    }

    /// The `checkpoint` and `progress` fields of the spawn body.
    fn spawn_fields(&self) -> Vec<(&'static str, String)> {
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
    /// The handoff pays, and this caller needs no approval card for it.
    HandOff {
        saving_usd: Option<f64>,
        cost_usd: Option<f64>,
    },
    DoesNotPay,
    /// It pays, but the spawn needs an owner approval card for
    /// `spawn-successor` on `parent`.
    NeedsCard {
        parent: String,
    },
    /// The engine could not be asked, or could not make a reading.
    Unknown,
}

pub(crate) fn parse_verdict(body: &serde_json::Value) -> AutomaticVerdict {
    let handoff = &body["handoff"];
    let without_card = handoff.get("without_card").and_then(|v| v.as_bool()) == Some(true);
    match handoff.get("pays").and_then(|v| v.as_bool()) {
        None => AutomaticVerdict::Unknown,
        Some(false) => AutomaticVerdict::DoesNotPay,
        Some(true) if without_card => {
            let usd = |key: &str| handoff["estimate"].get(key).and_then(|v| v.as_f64());
            AutomaticVerdict::HandOff {
                saving_usd: usd("saving_usd"),
                cost_usd: usd("cost_usd"),
            }
        }
        // A reading that needs a card but names no target is unknown.
        Some(true) => handoff
            .get("approval_target")
            .and_then(|v| v.as_str())
            .map_or(AutomaticVerdict::Unknown, |parent| {
                AutomaticVerdict::NeedsCard {
                    parent: parent.to_string(),
                }
            }),
    }
}

/// What an automatic checkpoint prints. It never names an implementer, and
/// only a handoff that needs the owner's approval asks anything.
pub(crate) fn automatic_text(verdict: &AutomaticVerdict, run: &CheckpointRun) -> String {
    const CARRY_ON: &str = "Carry on in this thread and ask nothing:";
    let (saving_usd, cost_usd) = match verdict {
        AutomaticVerdict::HandOff {
            saving_usd,
            cost_usd,
        } => (saving_usd, cost_usd),
        AutomaticVerdict::DoesNotPay => {
            return format!(
                "{CARRY_ON} the handoff estimate says a fresh thread does not pay here."
            )
        }
        AutomaticVerdict::NeedsCard { parent } => return ask_owner_text(parent, run),
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
        "The handoff estimate says a fresh thread pays here.{figures} Hand off now and ask \
         nothing: the user has no choice to make. {}",
        handoff_steps(run)
    )
}

/// A paying handoff that needs the owner's approval: ask once on an owner
/// approval card, then hand off or carry on as they answer.
fn ask_owner_text(parent: &str, run: &CheckpointRun) -> String {
    format!(
        "The handoff estimate says a fresh thread pays here, but starting it needs the \
         owner's approval. Ask once: 1. Run `lucidos ask-owner-approval spawn-successor \
         --thread {parent} --reason \"<why a fresh thread pays here>\"`. 2. Ask your question \
         tool with the id it prints as the only question. If the owner answers Allow once, \
         hand off: {} If they answer Don't allow, carry on in this thread and ask nothing \
         more.",
        handoff_steps(run)
    )
}

/// The numbered steps of an automatic handoff, on this thread's implementer.
fn handoff_steps(run: &CheckpointRun) -> String {
    format!(
        "1. Commit your work. 2. Write a handoff narrative to a file outside the repo, with the \
         six sections `lucidos successor --help` names. Its next step is the step of your flow \
         that follows this checkpoint. Quote your flow's remaining checkpoint instructions in \
         it, so the successor keeps running them. 3. Run `lucidos successor spawn --narrative \
         <file> {}`, with no other flags, so the successor runs on this thread's implementer. \
         4. Put the link it prints in your final message and end your turn.",
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

/// How a handoff places its successor and which checkpoint it names.
#[derive(Debug, Default)]
pub(crate) struct SpawnPlacement {
    /// Start a build chain where the engine can, as a flow's pick says.
    pub(crate) build_chain: bool,
    pub(crate) at: Option<CheckpointRun>,
}

/// The `POST /api/v1/threads/:id/successor` body for an agent's handoff.
pub(crate) fn spawn_body(
    narrative: String,
    choice: ImplementerChoice,
    placement: SpawnPlacement,
) -> serde_json::Value {
    let mut body = serde_json::json!({ "narrative": narrative });
    if placement.build_chain {
        body["build_chain"] = true.into();
    }
    for (key, value) in placement.at.iter().flat_map(CheckpointRun::spawn_fields) {
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

/// POST the narrative and print the successor's link, as `spawn-thread`
/// prints one. The engine checks the narrative's sections, the implementer,
/// the placement and the owner's authority, and refuses uncommitted work.
pub(crate) fn cmd_spawn(
    ws: &Workspace,
    narrative_path: &std::path::Path,
    choice: ImplementerChoice,
    placement: SpawnPlacement,
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
        .json(&spawn_body(narrative, choice, placement))
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
    let link = body
        .get("link")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("POST {url} answered without a link: {body}"))?;
    println!("{link}");
    if let Some(implementer) = parse_implementer(&body["implementer"]) {
        println!("It runs on {}.", implementer.describe());
    }
    if body.get("build_chain").and_then(|v| v.as_bool()) == Some(true) {
        println!(
            "The builder carries on from here, as a child of this thread in a build chain. Put this link in your final message and end your turn. This thread stays open and proposes nothing."
        );
    } else {
        println!(
            "The successor carries on from here. Put this link in your final message and end your turn. Lucidos archives this thread once the successor starts and this turn settles."
        );
    }
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
        /// The thread may start a successor at all. False when the engine
        /// did not say, so a card never offers a handoff it would refuse.
        can_hand_off: bool,
        /// The handoff estimate says a fresh thread pays here. False when
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
            "handoff": {"name": "plan-approval", "pays": pays, "without_card": true},
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
    fn the_spawn_body_names_only_the_chosen_fields() {
        assert_eq!(
            spawn_body(
                "n".into(),
                ImplementerChoice::default(),
                SpawnPlacement::default()
            ),
            serde_json::json!({"narrative": "n"})
        );
        let chosen = ImplementerChoice {
            coding_agent: Some(crate::CliCodingAgent::Codex),
            model: Some("gpt-5.6-sol".into()),
            reasoning_effort: Some("high".into()),
        };
        assert_eq!(
            spawn_body("n".into(), chosen, SpawnPlacement::default()),
            serde_json::json!({
                "narrative": "n",
                "coding_agent": "codex",
                "coding_agent_model": "gpt-5.6-sol",
                "reasoning_effort": "high",
            })
        );
    }

    #[test]
    fn the_spawn_body_names_the_placement_and_the_checkpoint() {
        let placement = SpawnPlacement {
            build_chain: true,
            at: Some(run("phase-commit", Some("2/5"))),
        };
        assert_eq!(
            spawn_body("n".into(), ImplementerChoice::default(), placement),
            serde_json::json!({
                "narrative": "n", "build_chain": true,
                "checkpoint": "phase-commit", "progress": "2/5",
            })
        );
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

    /// I1: the verdict is read from the engine's fields, never computed,
    /// and every reading but a paying one without a card carries on.
    #[test]
    fn the_verdict_is_read_never_recomputed() {
        let body = |pays: serde_json::Value, without_card: bool| {
            let mut body = serde_json::json!({"handoff": {
                "name": "pre-harden", "pays": pays, "without_card": without_card,
                "estimate": {"saving_usd": 1.5, "cost_usd": 0.4},
            }});
            if !without_card {
                body["handoff"]["approval_target"] = "parent-id".into();
            }
            body
        };
        assert_eq!(
            parse_verdict(&body(true.into(), true)),
            AutomaticVerdict::HandOff {
                saving_usd: Some(1.5),
                cost_usd: Some(0.4)
            }
        );
        assert_eq!(
            parse_verdict(&body(true.into(), false)),
            AutomaticVerdict::NeedsCard {
                parent: "parent-id".into()
            }
        );
        assert_eq!(
            parse_verdict(&serde_json::json!({"handoff": {"pays": true, "without_card": false}})),
            AutomaticVerdict::Unknown,
            "a card with no target to name is unknown, never a handoff"
        );
        assert_eq!(
            parse_verdict(&body(false.into(), true)),
            AutomaticVerdict::DoesNotPay
        );
        assert_eq!(
            parse_verdict(&body(serde_json::Value::Null, true)),
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

    fn needs_card() -> AutomaticVerdict {
        AutomaticVerdict::NeedsCard {
            parent: "parent-id".into(),
        }
    }

    /// No automatic checkpoint names an implementer flag or a plan card's
    /// option, whatever the verdict.
    #[test]
    fn an_automatic_checkpoint_never_names_an_implementer() {
        for checkpoint in checkpoints() {
            for verdict in [
                pays(),
                needs_card(),
                AutomaticVerdict::DoesNotPay,
                AutomaticVerdict::Unknown,
            ] {
                let text = automatic_text(&verdict, &checkpoint);
                for banned in [
                    "--coding-agent",
                    "--reasoning-effort",
                    "Implement in a new thread",
                    "AskUserQuestion",
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
                assert!(text.starts_with("Carry on in this thread and ask nothing"));
                assert!(!text.contains("lucidos successor spawn"), "{text}");
                assert!(!text.contains("ask-owner-approval"), "{text}");
            }
        }
    }

    /// I7: a paying handoff with consent hands off and asks nothing.
    #[test]
    fn a_paying_handoff_with_consent_hands_off_without_asking() {
        for checkpoint in checkpoints() {
            let text = automatic_text(&pays(), &checkpoint);
            assert!(text.contains("Hand off now and ask nothing"), "{text}");
            assert!(text.contains(&checkpoint.flags()), "{text}");
            assert!(text.contains("remaining checkpoint instructions"), "{text}");
            assert!(!text.contains("ask-owner-approval"), "{text}");
        }
        let handoff = automatic_text(&pays(), &run("phase-commit", Some("2/5")));
        assert!(handoff.contains("--at phase-commit --progress 2/5"));
    }

    /// I7: only a paying handoff without consent asks, on an owner approval
    /// card for `spawn-successor` on the parent, then hands off or carries on.
    #[test]
    fn a_paying_handoff_without_consent_asks_the_owner() {
        for checkpoint in checkpoints() {
            let text = automatic_text(&needs_card(), &checkpoint);
            assert!(
                text.contains(
                    "lucidos ask-owner-approval spawn-successor --thread parent-id --reason"
                ),
                "{text}"
            );
            assert!(text.contains("Allow once"), "{text}");
            assert!(text.contains("Don't allow, carry on"), "{text}");
            assert!(
                text.contains(&format!(
                    "lucidos successor spawn --narrative <file> {}`, with no other flags",
                    checkpoint.flags()
                )),
                "{text}"
            );
        }
    }
}
