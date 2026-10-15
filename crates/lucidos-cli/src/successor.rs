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

/// POST the narrative and print the successor's link, as `spawn-thread`
/// prints one. The engine checks the narrative's sections, the placement and
/// the owner's authority, and refuses uncommitted work.
pub(crate) fn cmd_spawn(ws: &Workspace, narrative_path: &std::path::Path) -> Result<(), BoxError> {
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
        .json(&serde_json::json!({ "narrative": narrative }))
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
    println!(
        "The successor carries on from here. Put this link in your final message and end your turn. Lucidos archives this thread once the successor starts and this turn settles."
    );
    Ok(())
}

/// This thread's context, as the engine decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextReading {
    Read {
        input_tokens: Option<u64>,
        offer: bool,
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
                offer: true
            }
        );
        assert_eq!(
            parse_context(&serde_json::json!({"input_tokens": 900000, "offer": false})),
            ContextReading::Read {
                input_tokens: Some(900_000),
                offer: false
            }
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
