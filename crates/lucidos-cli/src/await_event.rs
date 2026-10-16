//! `lucidos await-event`: subscribe this coding-agent thread to an event.
//!
//! The coding-agent counterpart of the chat agent's `await_event` LLM tool, and
//! deliberately the same registration underneath (`POST
//! /api/v1/threads/<id>/event-waits`), so the caps, the subscribability gate
//! and the refusal wording cannot drift between the two agents.
//!
//! It returns immediately. Nothing blocks, nothing is polled here: the engine
//! re-opens the thread with a follow-up message when the subscription matches,
//! or tells it the deadline passed. Finishing the session is the correct thing to
//! do right after calling this.
//!
//! `$LUCIDOS_THREAD_ID` names the thread, the same way `spawn-thread` reads it.

use serde_json::{json, Value};

use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

pub(crate) struct AwaitEventArgs<'a> {
    /// Event names to watch for. Any match re-opens the thread (OR).
    pub on: &'a [String],
    /// Optional payload filter, applied to EVERY name in `on`. The tool form
    /// takes a condition per entry; a CLI caller almost always watches one
    /// event, so one flag covers the real case without an argument shape that
    /// pairs two lists positionally.
    pub condition: Option<&'a str>,
    pub timeout_secs: i64,
    pub reason: &'a str,
}

pub(crate) fn cmd_await_event(ws: &Workspace, args: AwaitEventArgs<'_>) -> Result<(), BoxError> {
    let thread_id = std::env::var("LUCIDOS_THREAD_ID").map_err(|_| {
        "LUCIDOS_THREAD_ID is not set, so there is no thread to subscribe. This \
         subcommand only works from inside a Lucidos coding-agent session."
    })?;

    let condition: Option<Value> = match args.condition {
        Some(raw) => {
            let parsed: Value = serde_json::from_str(raw)
                .map_err(|e| format!("Invalid --condition JSON: {}", e))?;
            if !parsed.is_object() {
                return Err("--condition must be a JSON object of field filters".into());
            }
            Some(parsed)
        }
        None => None,
    };

    let on: Vec<Value> = args
        .on
        .iter()
        .map(|event_type| match &condition {
            Some(c) => json!({ "event_type": event_type, "condition": c }),
            None => json!({ "event_type": event_type }),
        })
        .collect();

    let url = format!("{}/api/v1/threads/{}/event-waits", ws.base_url(), thread_id);
    let body = json!({
        "on": on,
        "timeout_secs": args.timeout_secs,
        "reason": args.reason,
    });
    send_and_print("POST", &url, http_client()?.post(&url).json(&body))
}
