//! `lucidos ask-owner-approval <verb> [--thread <id>] --reason "<why>"`: ask
//! the workspace owner to let this thread press one clause-4 act once
//! (ADR 0387).
//!
//! The engine records the request on the thread the origin token names, and
//! words the card itself. A question card lives inside the turn that asks it.
//! So the agent then asks `AskUserQuestion` with the printed id as its only
//! question, and the engine shows its own card in that one's place. The verb
//! list lives in the engine, which refuses an unknown or unaskable verb.

use serde_json::json;

use crate::http::{client as http_client, send_expect_json};
use crate::workspace::{BoxError, Workspace};

pub(crate) fn cmd_ask(
    ws: &Workspace,
    verb: &str,
    thread: Option<&str>,
    reason: &str,
) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/owner-approvals", ws.base_url());
    let body = json!({
        "verb": verb,
        "target_thread_id": thread,
        "reason": reason,
    });
    let resp = send_expect_json("POST", &url, http_client()?.post(&url).json(&body))?;
    println!("{}", outcome_text(&resp));
    Ok(())
}

/// What the agent reads back: the request id, and the one way to ask with it.
fn outcome_text(resp: &serde_json::Value) -> String {
    let id = resp["request_id"].as_str().unwrap_or("?");
    format!(
        "Requested {id}.\n\
         Now ask the owner: call AskUserQuestion (Codex: ask_user_question) with exactly \
         one question whose text is `{id}`, and any two options. Lucidos shows its own \
         card, naming the act, in that question's place. If the owner answers Allow once, \
         do the act next, in this turn. Any other answer allows nothing."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_is_told_to_ask_with_the_request_id() {
        let text = outcome_text(&json!({ "request_id": "owner-approval:abc" }));
        assert!(text.contains("`owner-approval:abc`"), "{text}");
        assert!(text.contains("AskUserQuestion"), "{text}");
    }
}
