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
            "PreToolUse": [{
                "matcher": "AskUserQuestion",
                "hooks": [{
                    "type": "command",
                    "command": "lucidos ask-user-question-hook",
                    "timeout": HOOK_TIMEOUT_SECONDS
                }]
            }],
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
