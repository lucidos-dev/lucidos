use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

/// List the merged user + system knowhow catalog as JSON (the
/// `GET /api/v1/knowhow` payload verbatim: `{ knowhow: [{ id, name,
/// description }] }`). Read `.knowhow[].id` to find the id to pass to
/// `cmd_read`.
pub(crate) fn cmd_list(ws: &Workspace) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/knowhow", ws.base_url());
    send_and_print("GET", &url, http_client()?.get(&url))
}

/// Read one knowhow doc's full content by id (the `GET /api/v1/knowhow/read`
/// payload verbatim — a `[KNOW-HOW: …]` / `[SYSTEM-KNOWHOW: …]` block).
/// Exit non-zero with the engine's not-found sentinel on a miss.
pub(crate) fn cmd_read(ws: &Workspace, id: &str) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/knowhow/read", ws.base_url());
    send_and_print("GET", &url, http_client()?.get(&url).query(&[("id", id)]))
}
