//! Generates `<workspace>/.lucidos/cc-settings.json` — the config CC reads via
//! `--settings` to discover our `PreToolUse` hook on `AskUserQuestion`. The
//! hook invokes `lucidos ask-user-question-hook` (a sibling subcommand on the
//! same `lucidos-cli` binary the engine already prepends to CC's PATH for
//! the MCP permission server). Keeps the JSON shape literal; no per-spawn
//! interpolation. See `claude_code::permission_mcp_config_json` for the same
//! pattern.

use std::path::{Path, PathBuf};

pub(crate) fn cc_settings_path_for_workspace(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".lucidos/cc-settings.json")
}

/// CC's default PreToolUse hook timeout is 60 seconds. The hook long-polls
/// the engine for the user's answer; the user may take minutes (or hours).
/// Match the 24-hour `MCP_TOOL_TIMEOUT` ceiling we already use for the MCP
/// permission server. Value is in seconds.
const HOOK_TIMEOUT_SECONDS: u64 = 86_400;

pub(crate) fn build_cc_settings_json() -> String {
    serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "AskUserQuestion",
                    "hooks": [{
                        "type": "command",
                        "command": "lucidos ask-user-question-hook",
                        "timeout": HOOK_TIMEOUT_SECONDS
                    }]
                },
                {
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "lucidos cc-bash-guard"
                    }]
                },
                {
                    "matcher": "Read",
                    "hooks": [{
                        "type": "command",
                        "command": "lucidos cc-read-coerce"
                    }]
                },
                {
                    "matcher": "Edit",
                    "hooks": [
                        {
                            "type": "command",
                            "command": "lucidos cc-edit-preread"
                        },
                        {
                            "type": "command",
                            "command": "lucidos cc-plan-gate"
                        }
                    ]
                },
                {
                    "matcher": "Write",
                    "hooks": [
                        {
                            "type": "command",
                            "command": "lucidos cc-edit-preread"
                        },
                        {
                            "type": "command",
                            "command": "lucidos cc-plan-gate"
                        }
                    ]
                }
            ],
            "Stop": [{
                "hooks": [{
                    "type": "command",
                    "command": "lucidos cc-stop-reminder"
                }]
            }]
        }
    })
    .to_string()
}

pub(crate) async fn write_cc_settings(
    path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let body = build_cc_settings_json();
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, body).await?;
    tokio::fs::rename(&tmp, path).await?;
    crate::log!("[CcSettings] wrote {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_registers_pretooluse_hook_for_askuserquestion() {
        let json = build_cc_settings_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entries = &parsed["hooks"]["PreToolUse"];
        assert!(entries.is_array(), "PreToolUse must be an array");
        assert_eq!(entries[0]["matcher"], "AskUserQuestion");
        assert_eq!(entries[0]["hooks"][0]["type"], "command");
        assert_eq!(
            entries[0]["hooks"][0]["command"],
            "lucidos ask-user-question-hook"
        );
        assert_eq!(
            entries[0]["hooks"][0]["timeout"],
            serde_json::json!(HOOK_TIMEOUT_SECONDS),
            "must override CC's 60s default so long-running user thinking doesn't kill the hook"
        );
    }

    #[test]
    fn json_registers_pretooluse_hook_for_bash_guard() {
        // Without the Bash matcher, an in-CC `ps | grep cargo | xargs kill`
        // would once again kill every concurrent CC. Regression test for the
        // 2026-05-10 incident.
        let json = build_cc_settings_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entries = parsed["hooks"]["PreToolUse"].as_array().expect("array");
        let bash_entry = entries
            .iter()
            .find(|e| e["matcher"] == "Bash")
            .expect("must register a Bash matcher");
        assert_eq!(bash_entry["hooks"][0]["type"], "command");
        assert_eq!(
            bash_entry["hooks"][0]["command"], "lucidos cc-bash-guard",
            "must invoke the cc-bash-guard subcommand the engine ships",
        );
        assert!(
            bash_entry["hooks"][0]["timeout"].is_null(),
            "guard is fast — should not need an explicit timeout override",
        );
    }

    #[test]
    fn json_registers_pretooluse_hook_for_read_coerce() {
        // The model occasionally sends `"offset": "16384"` (string) instead
        // of a number; CC's input validator then fails the call. The
        // `cc-read-coerce` hook absorbs that via the `updatedInput` mechanism
        // before validation runs. Without the matcher wired here, the
        // workaround never fires.
        let json = build_cc_settings_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entries = parsed["hooks"]["PreToolUse"].as_array().expect("array");
        let read_entry = entries
            .iter()
            .find(|e| e["matcher"] == "Read")
            .expect("must register a Read matcher");
        assert_eq!(read_entry["hooks"][0]["type"], "command");
        assert_eq!(
            read_entry["hooks"][0]["command"], "lucidos cc-read-coerce",
            "must invoke the cc-read-coerce subcommand the engine ships",
        );
    }

    #[test]
    fn json_registers_pretooluse_hook_for_write_preread() {
        // Write also requires a prior Read for existing files; without
        // the matcher those calls bypass the preread hook entirely.
        let json = build_cc_settings_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entries = parsed["hooks"]["PreToolUse"].as_array().expect("array");
        let write_entry = entries
            .iter()
            .find(|e| e["matcher"] == "Write")
            .expect("must register a Write matcher");
        assert_eq!(write_entry["hooks"][0]["type"], "command");
        assert_eq!(
            write_entry["hooks"][0]["command"], "lucidos cc-edit-preread",
            "Write matcher must reuse the cc-edit-preread command — the hook \
             reads tool_name from the payload to switch behaviour"
        );
    }

    #[test]
    fn json_registers_pretooluse_hook_for_edit_preread() {
        // The "File has not been read yet" failure mode (14 occurrences in
        // 24h post the doc-only nudge) is a workflow loop, not a doc gap —
        // the agent jumps to Edit after sub-task switches and CC's internal
        // validator rejects. The `cc-edit-preread` hook turns the rejection
        // into an explicit "Read first, then retry Edit" deny so the model
        // is forced into the right loop.
        let json = build_cc_settings_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entries = parsed["hooks"]["PreToolUse"].as_array().expect("array");
        let edit_entry = entries
            .iter()
            .find(|e| e["matcher"] == "Edit")
            .expect("must register an Edit matcher");
        assert_eq!(edit_entry["hooks"][0]["type"], "command");
        assert_eq!(
            edit_entry["hooks"][0]["command"], "lucidos cc-edit-preread",
            "must invoke the cc-edit-preread subcommand the engine ships",
        );
    }

    #[test]
    fn json_registers_plan_gate_hook_on_edit_and_write() {
        // The implementation-plan pre-edit gate runs alongside cc-edit-preread
        // on both Edit and Write. It must be the SECOND hook (index 1) so the
        // existing preread assertions on index 0 stay valid; both run for the
        // same tool, composing the two preconditions (read-first AND planned).
        let json = build_cc_settings_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entries = parsed["hooks"]["PreToolUse"].as_array().expect("array");
        for tool in ["Edit", "Write"] {
            let entry = entries
                .iter()
                .find(|e| e["matcher"] == tool)
                .unwrap_or_else(|| panic!("must register a {tool} matcher"));
            let hooks = entry["hooks"].as_array().expect("hooks array");
            assert!(
                hooks
                    .iter()
                    .any(|h| h["command"] == "lucidos cc-plan-gate"),
                "{tool} matcher must include the cc-plan-gate hook so the \
                 implementation-plan marker is enforced before edits",
            );
            // Preread must still be present — the two gates compose.
            assert!(
                hooks
                    .iter()
                    .any(|h| h["command"] == "lucidos cc-edit-preread"),
                "{tool} matcher must keep the cc-edit-preread hook",
            );
        }
    }

    #[test]
    fn json_registers_stop_hook_for_harden_reminder() {
        let json = build_cc_settings_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entries = &parsed["hooks"]["Stop"];
        assert!(
            entries.is_array(),
            "Stop must be an array — needed so CC checks for harden state when it tries to idle"
        );
        let hook = &entries[0]["hooks"][0];
        assert_eq!(hook["type"], "command");
        assert_eq!(
            hook["command"], "lucidos cc-stop-reminder",
            "must invoke the reminder subcommand the engine ships",
        );
    }

    #[tokio::test]
    async fn write_creates_file_atomically() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".lucidos/cc-settings.json");
        write_cc_settings(&path).await.expect("write");
        assert!(path.exists(), "file must exist after write");
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(
            contents.contains("ask-user-question-hook"),
            "contents must reference the subcommand"
        );
    }

    #[test]
    fn path_helper_targets_dot_lucidos_dir() {
        let workspace = Path::new("/tmp/ws");
        assert_eq!(
            cc_settings_path_for_workspace(workspace),
            PathBuf::from("/tmp/ws/.lucidos/cc-settings.json")
        );
    }
}
