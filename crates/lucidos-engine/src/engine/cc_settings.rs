//! Generates `<workspace>/.lucidos/cc-settings.json` — the config CC reads via
//! `--settings` to discover our `PreToolUse` hook on `AskUserQuestion`. The
//! hook invokes `lucidos ask-user-question-hook` (a sibling subcommand on the
//! same `lucidos` CLI binary the engine already prepends to CC's PATH for the
//! MCP permission server — resolved via `LUCIDOS_CLI_BIN` in a packaged build,
//! the exe sibling-walk in dev; see `lucidos_cli::resolve_cli_dir`, and the
//! fail-fast in `ClaudeCodeRuntime::spawn` when the CLI can't be resolved).
//! Keeps the JSON shape literal; no per-spawn interpolation. See
//! `claude_code::permission_mcp_config_json` for the same pattern.

use std::path::{Path, PathBuf};

pub(crate) fn cc_settings_path_for_workspace(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".lucidos/cc-settings.json")
}

/// CC's default PreToolUse hook timeout is 60 seconds. The hook long-polls
/// the engine for the user's answer; the user may take minutes (or hours).
/// Match the 24-hour `MCP_TOOL_TIMEOUT` ceiling we already use for the MCP
/// permission server. Value is in seconds.
const HOOK_TIMEOUT_SECONDS: u64 = 86_400;

/// Default model for NEW Claude Code sessions, written into the `--settings`
/// file as CC's durable `model` default. Mirrors the chat default
/// (`core::DEFAULT_CHAT_MODEL`), but the two are deliberately independent knobs
/// (CC has its own picker + backend), so this is a distinct constant rather than
/// a reference. CC's `model` setting is the LOWEST-priority model source, so:
///   - a per-thread pick (`--model <value>` on spawn) still overrides it, and
///   - a RESUMED session keeps its own stored model (settings `model` only
///     seeds fresh sessions) — the reason this lives in settings, not an
///     `ANTHROPIC_MODEL` env that would also retarget resumed sessions.
///
/// Vertex id form (`@default`), matching the CC `/model` picker's Opus 5 entry
/// in `runtime/cc_menu_options.json` so it round-trips through
/// `normalize_cc_model_id`.
const CC_DEFAULT_MODEL: &str = "claude-opus-5@default";

pub(crate) fn build_cc_settings_json() -> String {
    serde_json::json!({
        "model": CC_DEFAULT_MODEL,
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
                    "hooks": [{
                        "type": "command",
                        "command": "lucidos cc-plan-gate"
                    }]
                },
                {
                    "matcher": "Write",
                    "hooks": [{
                        "type": "command",
                        "command": "lucidos cc-plan-gate"
                    }]
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
    fn json_sets_default_model_for_new_sessions() {
        // The `model` key makes Opus 5 the durable default for NEW CC sessions.
        // It is CC's lowest-priority model source, so a per-thread `--model`
        // pick still overrides it and a resumed session keeps its own model.
        let json = build_cc_settings_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["model"], "claude-opus-5@default",
            "cc-settings.json must pin the default CC model to Opus 5"
        );
        assert_eq!(parsed["model"], CC_DEFAULT_MODEL);
    }

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
    fn json_registers_plan_gate_hook_on_edit_and_write() {
        // The implementation-plan pre-edit gate is the sole hook on both Edit
        // and Write (the cc-edit-preread Read-before-Edit guard was removed —
        // CC enforces Read-before-Edit natively). The marker must be checked
        // before any edit.
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
            // The removed preread guard must not reappear.
            assert!(
                !hooks
                    .iter()
                    .any(|h| h["command"] == "lucidos cc-edit-preread"),
                "{tool} matcher must NOT carry the removed cc-edit-preread hook",
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
