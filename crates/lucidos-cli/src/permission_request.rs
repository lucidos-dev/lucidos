//! `lucidos ask-permission <action> --thread <id> --reason "<why>"`: ask the
//! workspace owner for a *permission* to do one act outside this thread's own
//! authority (ADR 0664).
//!
//! The engine records the request on the thread the origin token names, and
//! words the card itself. A question card lives inside the turn that asks it.
//! So the agent then asks its question tool with the printed id alone. The
//! engine shows its own card in that question's place. The action list lives
//! in the engine, which refuses an unknown or unaskable action.

use std::time::Instant;

use serde_json::json;

use crate::http::{
    child_reports_note, client as http_client, format_request_error, send_expect_json, ConnectRetry,
};
use crate::workspace::{BoxError, Workspace};

/// The exit code for a call that waits on the owner's permission.
pub(crate) const EXIT_NEEDS_PERMISSION: u8 = 2;

/// What a call that may need the owner's permission came to.
#[derive(Debug, PartialEq)]
pub(crate) enum Reply {
    Done(String),
    NeedsPermission(String),
    /// The engine parked the act on a card in Lucidos. The owner's Allow once
    /// runs it there, so the caller does not run it again.
    WaitingForOwner,
}

/// Read the answer to such a call. A refusal shows only the engine's `error`
/// text, never the rest of the body.
pub(crate) fn reply(method: &str, url: &str, status: u16, text: &str) -> Result<Reply, BoxError> {
    let body: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    if status == 202 && body["reason"] == "waiting_for_owner" {
        return Ok(Reply::WaitingForOwner);
    }
    if (200..300).contains(&status) {
        // Some routes answer 200 with `{success: false, error}` for a refusal.
        if body["success"] == serde_json::Value::Bool(false) {
            // Some of them carry the reason in `message`, and some in neither.
            let reason = body["error"]
                .as_str()
                .or(body["message"].as_str())
                .unwrap_or(text.trim());
            return Err(format!("{method} {url} did not succeed: {reason}").into());
        }
        return Ok(Reply::Done(text.trim().to_owned()));
    }
    if status == 403 && body["reason"] == "permission_required" {
        if let Some(id) = body["request_id"].as_str() {
            return Ok(Reply::NeedsPermission(id.to_owned()));
        }
    }
    let error = body["error"].as_str().unwrap_or("no error text");
    Err(format!("{method} {url} returned {status}: {error}").into())
}

/// What the caller reads when the act waits on the owner's card.
pub(crate) const WAITING_FOR_OWNER_TEXT: &str =
    "Nothing happened yet. This request does not prove it came from the workspace owner, \
     so Lucidos asked them on a card in Lucidos. Their Allow once runs it there; do not \
     run this command again.";

pub(crate) fn permission_text(request_id: &str) -> String {
    format!(
        "This needs the owner's permission. {}\n\
         The act is this same command: run it again once the owner allows it.",
        outcome_text(request_id)
    )
}

/// Send one act with a JSON body. See [`send_and_report`].
pub(crate) fn send_act(
    client: reqwest::blocking::Client,
    method: reqwest::Method,
    url: &str,
    body: &serde_json::Value,
) -> Result<u8, BoxError> {
    let request = client.request(method.clone(), url).json(body);
    send_and_report(method.as_str(), url, request)
}

/// Send one request that may need the owner's permission. Prints the
/// engine's answer, or how to ask the owner, and returns the exit code.
/// Every generated command sends through here.
pub(crate) fn send_and_report(
    method: &str,
    url: &str,
    request: reqwest::blocking::RequestBuilder,
) -> Result<u8, BoxError> {
    let start = Instant::now();
    let resp = request
        .send_with_connect_retry()
        .map_err(|e| format_request_error(method, url, &e, start.elapsed()))?;
    let status = resp.status().as_u16();
    if let Some(note) = child_reports_note(resp.headers()) {
        // A side channel for the agent reading this, beside the body it parses.
        eprintln!("{note}");
    }
    let text = resp
        .text()
        .map_err(|e| format!("Failed to read response body: {e}"))?;
    match reply(method, url, status, &text)? {
        Reply::Done(body) => {
            println!("{}", if body.is_empty() { "Done." } else { &body });
            Ok(0)
        }
        Reply::NeedsPermission(id) => {
            println!("{}", permission_text(&id));
            Ok(EXIT_NEEDS_PERMISSION)
        }
        Reply::WaitingForOwner => {
            println!("{WAITING_FOR_OWNER_TEXT}");
            Ok(EXIT_NEEDS_PERMISSION)
        }
    }
}

pub(crate) fn cmd_ask(
    ws: &Workspace,
    action: &str,
    thread: Option<&str>,
    target_workspace: Option<&str>,
    snapshot: Option<&str>,
    trigger: Option<(&str, Vec<String>, Vec<String>)>,
    reason: &str,
) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/permission-requests", ws.base_url());
    let mut body = json!({
        "action": action,
        "target_thread_id": thread,
        "target_workspace_id": target_workspace,
        "snapshot_handle": snapshot,
        "reason": reason,
    });
    let is_trigger_grant = trigger.is_some();
    if let Some((trigger, folders, programs)) = trigger {
        body["target_trigger_id"] = json!(trigger);
        if !folders.is_empty() {
            body["folders"] = json!(folders);
        }
        if !programs.is_empty() {
            body["programs"] = json!(programs);
        }
    }
    let resp = send_expect_json("POST", &url, http_client()?.post(&url).json(&body))?;
    let id = resp["request_id"]
        .as_str()
        .ok_or("The permission response has no request_id")?;
    let text = if is_trigger_grant {
        format!(
            "{} If the owner allows it, Lucidos saves the grants shown on the card.",
            ask_instruction(id)
        )
    } else {
        outcome_text(id)
    };
    println!("{text}");
    Ok(())
}

/// Prepare a batch and print the instruction for asking with it.
pub(crate) fn cmd_batch(ws: &Workspace, items: serde_json::Value) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/permissions/batches", ws.base_url());
    let resp = send_expect_json(
        "POST",
        &url,
        http_client()?.post(&url).json(&json!({ "items": items })),
    )?;
    let id = resp["request_id"]
        .as_str()
        .ok_or("The batch response has no request_id")?;
    println!("{}", batch_outcome_text(id));
    Ok(())
}

fn ask_instruction(id: &str) -> String {
    format!(
        "Requested {id}.\n\
         Now ask the owner: call AskUserQuestion (Codex: ask_user_question) with exactly \
         one question whose text is `{id}`, and any two options. Lucidos shows its own \
         card, naming the act, in that question's place."
    )
}

/// What the agent reads back: the request id, and the one way to ask with it.
pub(crate) fn outcome_text(id: &str) -> String {
    format!(
        "{} If the owner allows it, do the act next, in this turn. \
         Don't allow, or any other answer, allows nothing.",
        ask_instruction(id)
    )
}

pub(crate) fn batch_outcome_text(id: &str) -> String {
    format!(
        "{} On confirmation, Lucidos runs the batch once and records each item's result. \
         Don't run the batch again. Choosing Don't run executes nothing.",
        ask_instruction(id)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_asks_with_its_id_and_never_tells_the_agent_to_run_it_again() {
        let text = batch_outcome_text("permission-request:batch");
        assert!(text.contains("AskUserQuestion"), "{text}");
        assert!(text.contains("`permission-request:batch`"), "{text}");
        assert!(text.contains("Lucidos runs the batch"), "{text}");
        assert!(!text.contains("do the act"), "{text}");
    }

    #[test]
    fn the_agent_is_told_to_ask_with_the_request_id() {
        let text = outcome_text("permission-request:abc");
        assert!(text.contains("`permission-request:abc`"), "{text}");
        assert!(text.contains("AskUserQuestion"), "{text}");
    }

    /// A script reads the exit code, so a refusal in a 200 body must fail it.
    #[test]
    fn a_success_false_body_fails_the_command() {
        let body = r#"{"success":false,"error":"Preference 'x' not found"}"#;
        let err = reply("DELETE", "u", 200, body).unwrap_err().to_string();
        assert!(err.contains("Preference 'x' not found"), "{err}");
        let paused = r#"{"success":false,"message":"resume the trigger first"}"#;
        let err = reply("POST", "u", 200, paused).unwrap_err().to_string();
        assert!(err.contains("resume the trigger first"), "{err}");
        let bare = r#"{"success":false,"held":3}"#;
        let err = reply("POST", "u", 200, bare).unwrap_err().to_string();
        assert!(err.contains(r#""held":3"#), "{err}");
        assert_eq!(
            reply("POST", "u", 200, r#"{"success":true}"#).unwrap(),
            Reply::Done(r#"{"success":true}"#.into())
        );
    }

    #[test]
    fn a_permission_refusal_yields_its_request_id() {
        let body = r#"{"error": "Ask the owner.", "reason": "permission_required",
                       "request_id": "permission-request:abc"}"#;
        assert_eq!(
            reply("POST", "u", 403, body).unwrap(),
            Reply::NeedsPermission("permission-request:abc".into())
        );
        let text = permission_text("permission-request:abc");
        assert!(text.contains("`permission-request:abc`"), "{text}");
        assert!(text.contains("run it again"), "{text}");
    }

    /// A refusal prints the engine's `error` text and nothing else from the
    /// body, so no other field can reach the agent's output.
    #[test]
    fn a_refusal_shows_only_the_error_text() {
        let body = r#"{"error": "That workspace is the user's.", "detail": "x-secret-value"}"#;
        let err = reply("DELETE", "http://h/api/v1/hooks/gate", 403, body)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(
                "DELETE http://h/api/v1/hooks/gate returned 403: That workspace is the user's."
            ),
            "{err}"
        );
        assert!(!err.contains("x-secret-value"), "{err}");
        let err = reply("POST", "u", 405, "not json").unwrap_err().to_string();
        assert!(!err.contains("not json"), "{err}");
    }

    /// A parked act is not done: the owner's card runs it later, or never.
    #[test]
    fn an_act_parked_on_the_owners_card_waits_and_is_not_done() {
        let body = r#"{"error": "Only the workspace owner may do this.",
                       "reason": "waiting_for_owner", "thread_id": "t"}"#;
        assert_eq!(
            reply("POST", "u", 202, body).unwrap(),
            Reply::WaitingForOwner
        );
        assert!(WAITING_FOR_OWNER_TEXT.contains("Nothing happened yet"));
    }

    #[test]
    fn a_success_with_no_body_is_done() {
        assert_eq!(
            reply("POST", "u", 204, "").unwrap(),
            Reply::Done(String::new())
        );
    }
}
