//! `lucidos event-waits list` / `lucidos event-waits cancel`: read and stand
//! down the calling thread's own event subscriptions.
//!
//! The coding-agent counterparts of the chat agent's `list_event_waits` and
//! `cancel_event_wait` LLM tools, over the same routes and the same wording
//! underneath (`GET /api/v1/threads/<id>/event-waits`,
//! `POST /api/v1/threads/<id>/event-waits/cancel`), so neither the refusals nor
//! the report can drift between the two agents.
//!
//! `lucidos await-event` is the third verb of the same surface, kept as its own
//! top-level subcommand because it is what shipped.
//!
//! `$LUCIDOS_THREAD_ID` names the thread, the same way `await-event` does, and
//! it is the ONLY way either verb picks one: there is no `--thread` flag, so a
//! session cannot read or stop another thread's subscriptions.

use serde_json::json;

use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

/// The thread these verbs act on. Shared so both fail the same way outside a
/// session, rather than one 404ing on an empty path segment.
fn calling_thread() -> Result<String, BoxError> {
    std::env::var("LUCIDOS_THREAD_ID").map_err(|_| {
        "LUCIDOS_THREAD_ID is not set, so there is no thread whose subscriptions these \
         act on. This subcommand only works from inside a Lucidos coding-agent session."
            .into()
    })
}

pub(crate) fn cmd_event_waits_list(ws: &Workspace) -> Result<(), BoxError> {
    let thread_id = calling_thread()?;
    let url = format!("{}/api/v1/threads/{}/event-waits", ws.base_url(), thread_id);
    send_and_print("GET", &url, http_client()?.get(&url))
}

pub(crate) fn cmd_event_waits_cancel(
    ws: &Workspace,
    wait_id: Option<&str>,
    all: bool,
) -> Result<(), BoxError> {
    let thread_id = calling_thread()?;
    // Both / neither is decided by the engine, so the two agents read the same
    // refusal. Only the shape of the body is built here.
    let body = match (wait_id, all) {
        (Some(id), false) => json!({ "wait_id": id }),
        (None, true) => json!({ "all": true }),
        (Some(id), true) => json!({ "wait_id": id, "all": true }),
        (None, false) => json!({}),
    };
    let url = format!(
        "{}/api/v1/threads/{}/event-waits/cancel",
        ws.base_url(),
        thread_id
    );
    send_and_print("POST", &url, http_client()?.post(&url).json(&body))
}
