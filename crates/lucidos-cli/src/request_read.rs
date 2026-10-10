//! `lucidos request-read`: ask the user to read this thread's latest reply.
//!
//! The coding-agent counterpart of the chat agent's `request_read` LLM tool,
//! over `POST /api/v1/threads/<id>/read-request`, which records the same event
//! (ADR 0409). `$LUCIDOS_THREAD_ID` names the thread and there is no
//! `--thread` flag, so a session can only ask for its own reply.

use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

pub(crate) fn cmd_request_read(ws: &Workspace) -> Result<(), BoxError> {
    let thread_id = std::env::var("LUCIDOS_THREAD_ID").map_err(|_| -> BoxError {
        "LUCIDOS_THREAD_ID is not set, so there is no thread whose reply to read. \
         This subcommand only works from inside a Lucidos coding-agent session."
            .into()
    })?;
    let url = format!(
        "{}/api/v1/threads/{}/read-request",
        ws.base_url(),
        thread_id
    );
    send_and_print("POST", &url, http_client()?.post(&url))
}
