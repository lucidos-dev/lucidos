//! `lucidos model-prices`: the *price table* as the engine serves it, the
//! shipped cards merged with this workspace's override (ADR 0525). The shipped
//! cards live inside the engine binary, so a script reads prices here.

use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

/// Print the merged price table: the `GET /api/v1/model-prices` body verbatim.
pub(crate) fn cmd(ws: &Workspace) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/model-prices", ws.base_url());
    send_and_print("GET", &url, http_client()?.get(&url))
}
