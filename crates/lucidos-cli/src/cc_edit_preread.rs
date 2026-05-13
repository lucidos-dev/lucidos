//! PreToolUse hook for Claude Code's `Edit` tool — turns CC's internal
//! `<tool_use_error>File has not been read yet</tool_use_error>` rejection
//! into an explicit `permissionDecision: "deny"` with a clear "Read first,
//! then retry Edit" reason.
//!
//! Background: workspace-learning observed 14 occurrences in 24h *after*
//! the doc-only nudge in `CLAUDE.md` (commit `ac2d062dd`) — the agent
//! still skips Read after sub-task switches or long context windows. This
//! hook escalates from documentation to a structural enforcement: the
//! engine knows the thread's tool history (events table), so the hook
//! consults the engine and refuses Edit when no prior Read or Write
//! recorded the same `file_path` in the same thread.
//!
//! Why deny rather than self-satisfy:
//!   * Re-implementing Edit semantics (find/replace, replace_all, exact
//!     uniqueness, encoding, atomic write) would silently diverge from
//!     CC's own Edit over time.
//!   * The engine cannot mutate CC's in-process "has been read" tracking,
//!     so an "auto-Read" would be invisible to CC's own check that fires
//!     next.
//!   * A clear deny is the only delivery channel we control; the model
//!     follows a deny reason from a hook far more reliably than it
//!     follows CLAUDE.md (the report-measured failure mode of the
//!     doc-only fix).
//!
//! Wired into `<workspace>/.lucidos/cc-settings.json` via the engine's
//! `cc_settings.rs`. Fails OPEN on parse / I/O / engine-unreachable errors
//! so a hook bug can't brick every Edit call.

use serde::Deserialize;
use std::io::Read;

use crate::http::client;
use crate::workspace::{resolve_from_env, BoxError, Workspace};

#[derive(Debug, Deserialize)]
struct HookPayload {
    tool_input: ToolInput,
}

#[derive(Debug, Deserialize)]
struct ToolInput {
    #[serde(default)]
    file_path: String,
}

#[derive(Debug, Deserialize)]
struct PrereadResponse {
    has_recent_read: bool,
}

/// Build the deny JSON CC expects on a PreToolUse hook's stdout when the
/// hook wants to block the tool with a model-visible reason. The reason
/// names the missing precondition explicitly and tells the model exactly
/// what to do next — measured-better than CC's bare validation error.
pub(crate) fn build_deny_json(file_path: &str) -> String {
    let reason = format!(
        "Edit blocked: `{path}` has not been Read in this thread. CC's Edit tool requires a prior Read of the same absolute file_path. \
         Call Read with `{{\"file_path\": \"{path}\"}}` first, then retry your Edit.",
        path = file_path,
    );
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

/// Returns `Ok(true)` to allow the Edit silently (engine confirmed a prior
/// Read/Write). Returns `Ok(false)` to deny. `Err` is fail-open territory:
/// the caller emits a warning and allows.
fn check_engine(workspace: &Workspace, thread_id: &str, file_path: &str) -> Result<bool, BoxError> {
    let url = format!(
        "{}/api/internal/cc-edit-preread",
        workspace.base_url(),
    );
    let resp = client()?
        .get(&url)
        .query(&[("thread_id", thread_id), ("file_path", file_path)])
        .send()
        .map_err(|e| format!("preread engine call failed: {}", e))?
        .error_for_status()
        .map_err(|e| format!("preread engine HTTP status: {}", e))?
        .json::<PrereadResponse>()
        .map_err(|e| format!("preread engine response parse: {}", e))?;
    Ok(resp.has_recent_read)
}

pub(crate) fn run() -> Result<(), BoxError> {
    let mut buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
        eprintln!("cc-edit-preread: stdin read failed, allowing: {}", e);
        return Ok(());
    }
    let payload: HookPayload = match serde_json::from_str(&buf) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cc-edit-preread: payload parse failed, allowing: {}", e);
            return Ok(());
        }
    };
    if payload.tool_input.file_path.is_empty() {
        // No file_path means CC will reject Edit anyway with a different
        // validation error — there's nothing useful for us to add.
        return Ok(());
    }

    let thread_id = match std::env::var("LUCIDOS_THREAD_ID") {
        Ok(s) if !s.is_empty() => s,
        _ => {
            eprintln!(
                "cc-edit-preread: LUCIDOS_THREAD_ID env var missing, allowing (read-only fallback)"
            );
            return Ok(());
        }
    };

    let ws = match resolve_from_env() {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!(
                "cc-edit-preread: workspace resolve failed, allowing: {}",
                e
            );
            return Ok(());
        }
    };

    match check_engine(&ws, &thread_id, &payload.tool_input.file_path) {
        Ok(true) => {
            // Allow silently — no JSON output means CC proceeds with the
            // original input under whatever permission mode applies.
            Ok(())
        }
        Ok(false) => {
            println!("{}", build_deny_json(&payload.tool_input.file_path));
            Ok(())
        }
        Err(e) => {
            // Engine unreachable / DB error / parse failure — fail OPEN so
            // a transient engine hiccup doesn't block every Edit. CC's own
            // internal check is still authoritative if our hook can't run.
            eprintln!("cc-edit-preread: engine check failed, allowing: {}", e);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn deny_json_uses_documented_envelope() {
        let out = build_deny_json("/tmp/x.rs");
        let parsed: Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "deny");
        let reason = parsed["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .expect("reason must be a string");
        assert!(
            reason.contains("/tmp/x.rs"),
            "reason must name the file the model tried to Edit so the next Read targets the same path: {reason}"
        );
    }

    #[test]
    fn deny_reason_instructs_read_then_retry() {
        // The reason text is the only thing the model sees — it MUST
        // describe the recovery path, not just the failure. Anything
        // else recreates the doc-nudge problem (the model sees an
        // error, doesn't know what to do, retries Edit verbatim).
        let out = build_deny_json("/x");
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let reason = parsed["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap();
        let lower = reason.to_lowercase();
        assert!(
            lower.contains("read"),
            "reason must mention Read so the model knows the recovery tool: {reason}"
        );
        assert!(
            lower.contains("retry") || lower.contains("then"),
            "reason must describe ordering (Read first, then Edit): {reason}"
        );
    }

    #[test]
    fn parse_hook_payload_extracts_file_path() {
        let raw = r#"{
            "tool_input": {
                "file_path": "/abs/path/to/file.rs",
                "old_string": "foo",
                "new_string": "bar"
            }
        }"#;
        let payload: HookPayload = serde_json::from_str(raw).expect("valid payload");
        assert_eq!(payload.tool_input.file_path, "/abs/path/to/file.rs");
    }

    #[test]
    fn parse_hook_payload_tolerates_missing_file_path() {
        // Defensive: CC's Edit always sends file_path, but a defaulted
        // empty string keeps the hook fail-open if a future schema change
        // moves the field.
        let raw = r#"{ "tool_input": { "old_string": "foo", "new_string": "bar" } }"#;
        let payload: HookPayload = serde_json::from_str(raw).expect("valid payload");
        assert_eq!(payload.tool_input.file_path, "");
    }
}
