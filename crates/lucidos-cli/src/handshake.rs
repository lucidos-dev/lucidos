//! `lucidos handshake`, the approval side of ADR 0144.
//!
//! An auth handshake script runs only when the engine recorded who wrote it.
//! The Lucidos Agent's file tools record as they write, so this command is for
//! the other case: a script edited in the Files panel, in an editor, or landed
//! by a plugin.
//!
//! Deliberately CLI-only. The approve route refuses a browser-shaped caller, so
//! there is no in-product button and there cannot be one: an app UI shares the
//! shell's origin and would be able to press it.

use crate::http::{client, send_expect_json};
use crate::workspace::{BoxError, Workspace};

/// `lucidos handshake list`, one line per script `apis.json` names.
pub(crate) fn cmd_list(ws: &Workspace) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/handshake-scripts", ws.base_url());
    let parsed = send_expect_json("GET", &url, client()?.get(&url))?;
    let scripts = parsed["scripts"].as_array().cloned().unwrap_or_default();
    if scripts.is_empty() {
        println!("No handshake scripts are configured in data/config/apis.json.");
        return Ok(());
    }
    for script in scripts {
        let path = script["path"].as_str().unwrap_or("?");
        let state = match (
            script["exists"].as_bool().unwrap_or(false),
            script["approved"].as_bool().unwrap_or(false),
        ) {
            (false, _) => "missing",
            (true, true) => "approved",
            (true, false) => "NOT APPROVED",
        };
        println!("{:<12} {}", state, path);
    }
    Ok(())
}

/// `lucidos handshake approve <path>`, recording the file as it stands now.
pub(crate) fn cmd_approve(ws: &Workspace, path: &str) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/handshake-scripts/approve", ws.base_url());
    let req = client()?
        .post(&url)
        .json(&serde_json::json!({ "path": path }));
    let parsed = send_expect_json("POST", &url, req)?;
    let approved = parsed["path"].as_str().unwrap_or(path);
    match parsed["changed"].as_bool().unwrap_or(true) {
        true => println!("Approved {}", approved),
        false => println!("{} was already approved, unchanged", approved),
    }
    Ok(())
}
