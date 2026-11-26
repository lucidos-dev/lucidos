use serde_json::{json, Value};

use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

pub(crate) struct AwaitEventArgs<'a> {
    pub on: &'a [String],
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
