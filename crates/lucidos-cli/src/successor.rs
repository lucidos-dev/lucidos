//! `lucidos successor`: continue this thread in a *successor thread*, which
//! carries on with a fresh context and this thread's branch. Also how
//! `lucidos planned mark` learns this thread's context size.

use crate::http::client as http_client;
use crate::workspace::{BoxError, Workspace};

/// `lucidos successor --help`: the whole procedure, so the engine prompt can
/// stay one line.
pub(crate) const HELP: &str = "\
Continue this thread in a successor thread, which carries on with a fresh context.

When: the user says \"handoff this session\", or picks \"Implement in a new thread\" for a \
plan. In a Lucidos thread, advice to run `/clear` or to \
write a handoff file means this.

How:
1. Commit your work. Lucidos refuses a worktree with uncommitted changes.
2. Write a handoff narrative to a file outside the repo, for example under $TMPDIR. It needs \
one markdown heading for each of: Outcomes, Decisions, Gotchas, State, Next step, Files. \
Write it for a reader with none of this thread's context. Cover what was learned, what was \
settled and why, what bit you, and where things stand. Then name the first step and the files \
that matter.
3. Run `lucidos successor spawn --narrative <file>`, put the link it prints in your final \
message, and end your turn.

The successor runs on this thread's implementer: its backend, model and reasoning effort. \
`--coding-agent`, `--coding-agent-model` and `--reasoning-effort` name another, as on \
`spawn-thread`. A model carries over only on the same backend. An effort carries over only \
on the same backend while the successor's model accepts it. Lucidos refuses a model or effort the backend does not \
offer, and lists the ones it does.

The successor's branch starts at this thread's last commit, so its change carries all of this \
thread's work, and this thread proposes nothing. It sits in this thread's place: top-level if \
this thread is, otherwise a child of the same parent, which then waits on it. A top-level \
successor needs no approval. A child one, from a turn the owner did not open, needs \
spawn-successor on the parent. This thread archives itself once the successor starts and \
nothing blocks it.";

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

/// The `POST /api/v1/threads/:id/successor` body for an agent's handoff.
pub(crate) fn spawn_body(narrative: String, choice: ImplementerChoice) -> serde_json::Value {
    let mut body = serde_json::json!({ "narrative": narrative });
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
) -> Result<(), BoxError> {
    let narrative = std::fs::read_to_string(narrative_path).map_err(|e| {
        format!(
            "Could not read the handoff narrative {}: {e}",
            narrative_path.display()
        )
    })?;
    let thread_id = own_thread_id()?;
    let url = format!("{}/api/v1/threads/{thread_id}/successor", ws.base_url());
    let resp = http_client()?
        .post(&url)
        .json(&spawn_body(narrative, choice))
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
        return Err(format!("The handoff was refused ({status}): {message}").into());
    }
    let link = body
        .get("link")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("POST {url} answered without a link: {body}"))?;
    println!("{link}");
    if let Some(implementer) = parse_implementer(&body["implementer"]) {
        println!("It runs on {}.", implementer.describe());
    }
    println!(
        "The successor carries on from here. Put this link in your final message and end your turn. Lucidos archives this thread once the successor starts and this turn settles."
    );
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

/// This thread's context, as the engine decides it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContextReading {
    Read {
        input_tokens: Option<u64>,
        offer: bool,
        /// `None` when the engine did not report one.
        implementer: Option<Implementer>,
    },
    /// The engine could not be asked, or did not answer.
    Unknown,
}

/// Read `GET /api/v1/threads/:id/context` for the calling thread. Never fails:
/// an unreachable engine reads as unknown, never as small.
pub(crate) fn read_own_context(ws: &Workspace) -> ContextReading {
    let Ok(thread_id) = own_thread_id() else {
        return ContextReading::Unknown;
    };
    let url = format!("{}/api/v1/threads/{thread_id}/context", ws.base_url());
    let Ok(client) = http_client() else {
        return ContextReading::Unknown;
    };
    let Ok(resp) = client.get(&url).send() else {
        return ContextReading::Unknown;
    };
    if !resp.status().is_success() {
        return ContextReading::Unknown;
    }
    match resp.json::<serde_json::Value>() {
        Ok(body) => parse_context(&body),
        Err(_) => ContextReading::Unknown,
    }
}

pub(crate) fn parse_context(body: &serde_json::Value) -> ContextReading {
    match body.get("offer").and_then(|v| v.as_bool()) {
        Some(offer) => ContextReading::Read {
            input_tokens: body.get("input_tokens").and_then(|v| v.as_u64()),
            offer,
            implementer: body.get("implementer").and_then(parse_implementer),
        },
        None => ContextReading::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_decision_is_read_never_recomputed() {
        assert_eq!(
            parse_context(&serde_json::json!({"input_tokens": 5, "offer": true, "nudge": false})),
            ContextReading::Read {
                input_tokens: Some(5),
                offer: true,
                implementer: None,
            }
        );
        assert_eq!(
            parse_context(&serde_json::json!({"input_tokens": 900000, "offer": false})),
            ContextReading::Read {
                input_tokens: Some(900_000),
                offer: false,
                implementer: None,
            }
        );
    }

    #[test]
    fn the_reading_carries_the_threads_implementer() {
        let reading = parse_context(&serde_json::json!({
            "input_tokens": 5,
            "offer": true,
            "implementer": {"coding_agent": "claude-code", "model": "claude-opus-5-5", "reasoning_effort": null},
        }));
        let ContextReading::Read {
            implementer: Some(implementer),
            ..
        } = reading
        else {
            panic!("no implementer: {reading:?}");
        };
        assert_eq!(
            implementer.describe(),
            "claude-code with model claude-opus-5-5 at (the backend's default) effort"
        );
    }

    /// Only what the user picked goes on the wire: an omitted field is the
    /// engine's to carry over.
    #[test]
    fn the_spawn_body_names_only_the_chosen_fields() {
        assert_eq!(
            spawn_body("n".into(), ImplementerChoice::default()),
            serde_json::json!({"narrative": "n"})
        );
        let chosen = ImplementerChoice {
            coding_agent: Some(crate::CliCodingAgent::Codex),
            model: Some("gpt-5.6-sol".into()),
            reasoning_effort: Some("high".into()),
        };
        assert_eq!(
            spawn_body("n".into(), chosen),
            serde_json::json!({
                "narrative": "n",
                "coding_agent": "codex",
                "coding_agent_model": "gpt-5.6-sol",
                "reasoning_effort": "high",
            })
        );
    }

    #[test]
    fn a_body_without_the_decision_is_unknown() {
        assert_eq!(
            parse_context(&serde_json::json!({"input_tokens": 5})),
            ContextReading::Unknown
        );
        assert_eq!(
            parse_context(&serde_json::json!({})),
            ContextReading::Unknown
        );
    }
}
