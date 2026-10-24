//! `lucidos frontend-preview`: start, stop, or inspect the supervised Vite dev
//! server that shows a coding-agent worktree's frontend before Apply
//! (`engine::frontend_preview`).
//!
//! This is the half a coding agent can reach. A `vite` the agent starts itself
//! dies the moment its turn ends, because the engine kills the session's whole
//! process group; asking the engine to start one is the entire difference
//! between a preview that survives the message and one that does not.
//!
//! `start` defaults `--thread-id` from `$LUCIDOS_THREAD_ID`, so inside a
//! coding-agent worktree the whole invocation is `lucidos frontend-preview
//! start`, and the printed URL is what goes into the reply.
//!
//! Hand-written rather than generated from the capability parity manifest
//! (ADR 0018): the preview is a development affordance with a supervised
//! process behind it, not a workspace capability, so it has no LLM tool and no
//! SDK facade.

use crate::http::client as http_client;
use crate::workspace::{BoxError, Workspace};

fn endpoint(ws: &Workspace, suffix: &str) -> String {
    format!("{}/api/v1/frontend-preview{}", ws.base_url(), suffix)
}

/// The thread whose worktree to preview: the flag, else the ambient
/// `$LUCIDOS_THREAD_ID` a coding-agent subprocess is spawned with.
pub(crate) fn resolve_thread_id(flag: Option<&str>) -> Result<String, BoxError> {
    if let Some(id) = flag.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(id.to_string());
    }
    match std::env::var("LUCIDOS_THREAD_ID") {
        Ok(id) if !id.trim().is_empty() => Ok(id.trim().to_string()),
        // Named rather than generic: outside a coding-agent subprocess there is
        // no ambient thread, and the caller needs to know the flag is the fix.
        _ => Err("No thread to preview: pass --thread-id <uuid>, or run this inside a coding-agent worktree where $LUCIDOS_THREAD_ID is set.".into()),
    }
}

/// One line describing what the engine reported, for a human or an agent that
/// is about to paste it into a reply. Pure, so the wording is testable.
pub(crate) fn describe(body: &serde_json::Value) -> String {
    if body.get("running").and_then(|v| v.as_bool()) != Some(true) {
        return "No frontend preview is running.".to_string();
    }
    let port = body.get("port").and_then(|v| v.as_u64());
    let thread = body.get("thread_id").and_then(|v| v.as_str());
    let url = body.get("url").and_then(|v| v.as_str());
    let location = match (url, port) {
        (Some(u), _) => u.to_string(),
        (None, Some(p)) => format!("port {p}"),
        (None, None) => "an unreported port".to_string(),
    };
    match thread {
        Some(t) => format!("Frontend preview running for thread {t} at {location}"),
        None => format!("Frontend preview running at {location}"),
    }
}

fn post(ws: &Workspace, suffix: &str, body: serde_json::Value) -> Result<(), BoxError> {
    let url = endpoint(ws, suffix);
    let resp = http_client()?
        .post(&url)
        .json(&body)
        .send()
        .map_err(|e| format!("POST {} failed: {}", url, e))?;
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| format!("POST {} returned {}, body read failed: {}", url, status, e))?;
    if !status.is_success() {
        // The engine's refusals name the worktree or the missing file, so they
        // are surfaced verbatim rather than replaced with a status code.
        return Err(format!("POST {} returned {}: {}", url, status, text).into());
    }
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("POST {} returned non-JSON body: {}", url, e))?;
    println!("{}", describe(&parsed));
    Ok(())
}

pub(crate) fn cmd_start(ws: &Workspace, thread_id: Option<&str>) -> Result<(), BoxError> {
    let thread_id = resolve_thread_id(thread_id)?;
    post(ws, "/start", serde_json::json!({ "thread_id": thread_id }))
}

pub(crate) fn cmd_stop(ws: &Workspace) -> Result<(), BoxError> {
    post(ws, "/stop", serde_json::json!({}))
}

pub(crate) fn cmd_status(ws: &Workspace) -> Result<(), BoxError> {
    let url = endpoint(ws, "");
    let resp = http_client()?
        .get(&url)
        .send()
        .map_err(|e| format!("GET {} failed: {}", url, e))?;
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| format!("GET {} returned {}, body read failed: {}", url, status, e))?;
    if !status.is_success() {
        return Err(format!("GET {} returned {}: {}", url, status, text).into());
    }
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("GET {} returned non-JSON body: {}", url, e))?;
    println!("{}", describe(&parsed));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test, not three, because all three cases mutate the SAME process-wide
    /// env var and Rust runs `#[test]`s in one binary concurrently: split up,
    /// they would race each other's `set_var` / `remove_var`.
    #[test]
    fn the_thread_is_the_flag_then_the_ambient_id_then_an_error_naming_the_flag() {
        std::env::set_var("LUCIDOS_THREAD_ID", "11111111-1111-1111-1111-111111111111");
        // A coding agent previewing a SIBLING thread's worktree must not
        // silently get its own.
        assert_eq!(
            resolve_thread_id(Some("22222222-2222-2222-2222-222222222222")).unwrap(),
            "22222222-2222-2222-2222-222222222222"
        );
        // A blank flag is no flag, not an empty thread id.
        assert_eq!(
            resolve_thread_id(Some("  ")).unwrap(),
            "11111111-1111-1111-1111-111111111111"
        );

        std::env::remove_var("LUCIDOS_THREAD_ID");
        let err = resolve_thread_id(None).unwrap_err().to_string();
        assert!(err.contains("--thread-id"), "unhelpful error: {err}");
    }

    #[test]
    fn a_running_preview_is_described_by_its_url() {
        let body = serde_json::json!({
            "running": true,
            "thread_id": "2951200f-0652-4ee2-baa3-433d608983d8",
            "port": 6173,
            "url": "https://phone.tailnet.ts.net:6173/",
        });
        let line = describe(&body);
        assert!(line.contains("https://phone.tailnet.ts.net:6173/"));
        assert!(line.contains("2951200f-0652-4ee2-baa3-433d608983d8"));
    }

    #[test]
    fn without_a_url_the_port_is_reported_rather_than_a_guessed_host() {
        let body = serde_json::json!({ "running": true, "port": 6173 });
        assert_eq!(describe(&body), "Frontend preview running at port 6173");
    }

    #[test]
    fn a_stopped_preview_says_so_plainly() {
        assert_eq!(
            describe(&serde_json::json!({ "running": false })),
            "No frontend preview is running."
        );
    }
}
