//! INVARIANT: Generates `<workspace>/.lucidos/cc-settings.json` — the config CC reads via
//! `--settings` to discover our `PreToolUse` hook on `AskUserQuestion`. The
//! hook invokes `lucidos ask-user-question-hook` (a sibling subcommand on the
//! same `lucidos` CLI binary the engine already prepends to CC's PATH for the
//! MCP permission server — resolved via `LUCIDOS_CLI_BIN` in a packaged build,
//! the engine's pinned copy in dev; see `lucidos_cli::lucidos_cli_dir`, and the
//! fail-fast in `ClaudeCodeRuntime::spawn` when the CLI can't be resolved).
//! Keeps the JSON shape literal; no per-spawn interpolation. See
//! `claude_code::permission_mcp_config_json` for the same pattern.
//! It carries hooks, directory grants and hidden skills, never a model or
//! effort default: see [`build_cc_settings_json`].

use std::path::{Path, PathBuf};

pub(crate) fn cc_settings_path_for_workspace(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".lucidos/cc-settings.json")
}

/// INVARIANT: How long a coding agent waits for the user's answer, in seconds: 24 hours.
/// The user may take minutes or hours. Every agent-side cap on that wait is
/// lifted to this one value: CC's PreToolUse hook timeout (default 60 s), its
/// `MCP_TOOL_TIMEOUT` and `MCP_TIMEOUT`, Codex's `tool_timeout_sec`, and the
/// `yield_time_ms` its prompt tells a question's `exec` cell to use. A
/// shorter one forces a retry, which surfaces a duplicate card.
pub const USER_ANSWER_WAIT_SECS: u64 = 86_400;

fn widened_directories(workspace_root: &Path) -> Vec<PathBuf> {
    let data_dir = workspace_root.join(crate::core::DATA_DIR);
    match std::fs::canonicalize(&data_dir).ok().filter(|d| d.is_dir()) {
        Some(dir)
            if crate::runtime::codex::grants_more_than_the_data_tree(&dir, workspace_root) =>
        {
            crate::log_legacy!(
                "[CcSettings] {} resolves to {}, which contains the workspace. Refusing to \
                 grant it: Claude Code will keep asking before it reaches the workspace's \
                 data/ tree.",
                data_dir.display(),
                dir.display()
            );
            Vec::new()
        }
        Some(dir) => vec![dir],
        None => {
            crate::log_legacy!(
                "[CcSettings] {} is not a reachable directory. Claude Code will keep asking \
                 before it reaches the workspace's data/ tree.",
                data_dir.display()
            );
            Vec::new()
        }
    }
}

const OS_TMP_DIR: &str = "/tmp";

fn os_tmp_directories() -> Vec<PathBuf> {
    os_tmp_directories_with(std::env::var_os("TMPDIR"))
}

fn os_tmp_directories_with(tmpdir: Option<std::ffi::OsString>) -> Vec<PathBuf> {
    let tmpdir = tmpdir.filter(|dir| !dir.is_empty()).map(PathBuf::from);
    let mut granted: Vec<PathBuf> = Vec::new();
    for literal in std::iter::once(PathBuf::from(OS_TMP_DIR)).chain(tmpdir) {
        let Ok(resolved) = std::fs::canonicalize(&literal) else {
            crate::log_legacy!(
                "[CcSettings] {} is not a reachable directory. Claude Code will keep asking \
                 before it writes a throwaway file there.",
                literal.display()
            );
            continue;
        };
        for entry in [resolved, literal] {
            if !granted.contains(&entry) {
                granted.push(entry);
            }
        }
    }
    granted
}

/// INVARIANT: Render the settings file. The `permissions` key is omitted entirely when the
/// slice is empty, so the file never names a directory that is not there.
///
/// No `model` or `effortLevel` key, ever. CC ranks this file (its
/// `flagSettings` source) above the user's own settings, so either key would
/// override the user's CC config for every unpinned session. Without them CC
/// resolves as it does outside Lucidos: `--model`, then `ANTHROPIC_MODEL`, then
/// the settings files, then its built-in default.
pub(crate) fn build_cc_settings_json(additional_directories: &[PathBuf]) -> String {
    let mut settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": crate::runtime::CC_NATIVE_ASK_USER_QUESTION_TOOL,
                    "hooks": [{
                        "type": "command",
                        "command": "lucidos ask-user-question-hook",
                        "timeout": USER_ANSWER_WAIT_SECS
                    }]
                },
                {
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": "lucidos cc-bash-guard",
                        "onFailure": "block",
                        "timeout": 10
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
                    "matcher": "Agent",
                    "hooks": [{
                        "type": "command",
                        "command": "lucidos cc-agent-guard"
                    }]
                },
                {
                    "matcher": "*",
                    "hooks": [{
                        "type": "command",
                        "command": question_guard_command()
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
    });
    grant_directories(&mut settings, additional_directories);
    hide_unused_skills(&mut settings);
    turn_off_own_sandbox(&mut settings);
    settings.to_string()
}

fn question_guard_command() -> String {
    use crate::engine::question_park_sql::OPEN_QUESTION_TOOL_REFUSAL;
    format!(
        "lucidos cc-question-guard --refusal '{OPEN_QUESTION_TOOL_REFUSAL}' \
         --question-tool {} --question-tool {}",
        crate::runtime::CC_NATIVE_ASK_USER_QUESTION_TOOL,
        crate::runtime::CC_MCP_ASK_USER_QUESTION_TOOL,
    )
}

const HIDDEN_SKILLS: [&str; 4] = [
    "loop",
    "workflow-authoring",
    "fewer-permission-prompts",
    "keybindings-help",
];

/// INVARIANT: Add `skillOverrides` for [`HIDDEN_SKILLS`]. CC lists skills in the prompt
/// prefix, so a side question's copy must hide the same set as its session or
/// the prompt cache misses.
fn hide_unused_skills(settings: &mut serde_json::Value) {
    settings["skillOverrides"] = HIDDEN_SKILLS
        .iter()
        .map(|skill| (skill.to_string(), serde_json::json!("off")))
        .collect::<serde_json::Map<_, _>>()
        .into();
}

fn turn_off_own_sandbox(settings: &mut serde_json::Value) {
    settings["sandbox"] = serde_json::json!({ "enabled": false });
}

fn grant_directories(settings: &mut serde_json::Value, additional_directories: &[PathBuf]) {
    if !additional_directories.is_empty() {
        settings["permissions"] = serde_json::json!({
            "additionalDirectories": additional_directories
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        });
    }
}

fn granted_directories(workspace_root: &Path) -> Vec<PathBuf> {
    let mut granted = widened_directories(workspace_root);
    granted.extend(os_tmp_directories());
    granted
}

/// INVARIANT: Write `body` to `path` whole. The temp name is unique, so two writers of
/// the same file never rename each other's half-written copy into place.
async fn write_whole(
    path: &Path,
    body: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4().simple()));
    tokio::fs::write(&tmp, body).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

pub(crate) async fn write_cc_settings(
    workspace_root: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = cc_settings_path_for_workspace(workspace_root);
    write_whole(
        &path,
        build_cc_settings_json(&granted_directories(workspace_root)),
    )
    .await?;
    crate::log_legacy!("[CcSettings] wrote {}", path.display());
    Ok(())
}

pub(crate) fn cc_side_question_settings_path_for_workspace(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".lucidos/cc-side-question-settings.json")
}

/// INVARIANT: Render a side question's settings: one PreToolUse hook refusing every tool,
/// plus the session's own directory grants. The hook command single-quotes the
/// refusal for the shell, so the refusal must hold no single quote.
pub(crate) fn build_cc_side_question_settings_json(additional_directories: &[PathBuf]) -> String {
    let refusal = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason":
                crate::engine::side_question_policy::SIDE_QUESTION_TOOL_REFUSAL,
        }
    });
    let mut settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": format!("printf '%s' '{refusal}'"),
                }]
            }]
        }
    });
    grant_directories(&mut settings, additional_directories);
    hide_unused_skills(&mut settings);
    turn_off_own_sandbox(&mut settings);
    settings.to_string()
}

pub(crate) async fn write_cc_side_question_settings(
    workspace_root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let path = cc_side_question_settings_path_for_workspace(workspace_root);
    let body = build_cc_side_question_settings_json(&granted_directories(workspace_root));
    write_whole(&path, body).await?;
    Ok(path)
}

pub(crate) fn has_plan_gate(settings: &serde_json::Value, tool: &str) -> bool {
    settings["hooks"]["PreToolUse"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["matcher"] == tool)
        .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
        .any(|hook| hook["type"] == "command" && hook["command"] == "lucidos cc-plan-gate")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_question_settings_refuse_every_tool() {
        let json = build_cc_side_question_settings_json(&[PathBuf::from("/w/data")]);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entries = parsed["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 1, "one hook, and it covers everything");
        assert_eq!(entries[0]["matcher"], "*");
        let command = entries[0]["hooks"][0]["command"].as_str().unwrap();
        let printed = command
            .strip_prefix("printf '%s' '")
            .and_then(|rest| rest.strip_suffix('\''))
            .expect("a single-quoted printf");
        let output: serde_json::Value = serde_json::from_str(printed).unwrap();
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(output["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert!(parsed.get("hooks").unwrap().get("Stop").is_none());
    }

    #[test]
    fn side_question_settings_grant_the_sessions_directories() {
        let dirs = [PathBuf::from("/w/data"), PathBuf::from("/tmp")];
        let session: serde_json::Value =
            serde_json::from_str(&build_cc_settings_json(&dirs)).unwrap();
        let copy: serde_json::Value =
            serde_json::from_str(&build_cc_side_question_settings_json(&dirs)).unwrap();
        assert_eq!(session["permissions"], copy["permissions"]);
    }

    #[test]
    fn both_files_turn_off_claude_codes_own_sandbox() {
        for body in [
            build_cc_settings_json(&[]),
            build_cc_side_question_settings_json(&[]),
        ] {
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(parsed["sandbox"]["enabled"], false, "{body}");
        }
    }

    #[test]
    fn skill_overrides_hide_only_the_unused_skills_in_both_files() {
        for json in [
            build_cc_settings_json(&[]),
            build_cc_side_question_settings_json(&[]),
        ] {
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
            let overrides = parsed["skillOverrides"].as_object().unwrap();
            let mut hidden: Vec<&str> = overrides.keys().map(String::as_str).collect();
            hidden.sort_unstable();
            assert_eq!(
                hidden,
                [
                    "fewer-permission-prompts",
                    "keybindings-help",
                    "loop",
                    "workflow-authoring"
                ]
            );
            assert!(overrides.values().all(|v| v == "off"));
        }
    }

    #[test]
    fn json_carries_no_model_or_effort_default() {
        let json = build_cc_settings_json(&[]);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        for key in ["model", "effortLevel"] {
            assert!(
                parsed.get(key).is_none(),
                "cc-settings.json must not set `{key}`: it would override the user's CC config"
            );
        }
    }

    #[test]
    fn json_registers_pretooluse_hook_for_askuserquestion() {
        let json = build_cc_settings_json(&[]);
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
            serde_json::json!(USER_ANSWER_WAIT_SECS),
            "must override CC's 60s default so long-running user thinking doesn't kill the hook"
        );
    }

    #[test]
    fn json_registers_pretooluse_hook_for_bash_guard() {
        let json = build_cc_settings_json(&[]);
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
        assert_eq!(bash_entry["hooks"][0]["onFailure"], "block");
        assert_eq!(bash_entry["hooks"][0]["timeout"], 10);
    }

    #[test]
    fn json_registers_pretooluse_hook_for_read_coerce() {
        let json = build_cc_settings_json(&[]);
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
    fn json_registers_pretooluse_hook_for_agent_guard() {
        let json = build_cc_settings_json(&[]);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entries = parsed["hooks"]["PreToolUse"].as_array().expect("array");
        let agent_entry = entries
            .iter()
            .find(|e| e["matcher"] == "Agent")
            .expect("must register an Agent matcher");
        assert_eq!(agent_entry["hooks"][0]["type"], "command");
        assert_eq!(
            agent_entry["hooks"][0]["command"], "lucidos cc-agent-guard",
            "must invoke the cc-agent-guard subcommand the engine ships",
        );
    }

    #[test]
    fn repository_settings_register_the_plan_gate_for_file_edits() {
        let repository: serde_json::Value =
            serde_json::from_str(include_str!("../../../../.claude/settings.json")).unwrap();
        let managed: serde_json::Value =
            serde_json::from_str(&build_cc_settings_json(&[])).unwrap();
        for tool in ["Edit", "Write"] {
            assert!(
                has_plan_gate(&repository, tool),
                "{tool} must check the plan marker"
            );
            assert!(
                !has_plan_gate(&managed, tool),
                "{tool} must check the marker once"
            );
        }
    }

    #[test]
    fn json_registers_the_question_guard_on_every_tool() {
        use crate::engine::question_park_sql::OPEN_QUESTION_TOOL_REFUSAL;
        let parsed: serde_json::Value = serde_json::from_str(&build_cc_settings_json(&[])).unwrap();
        let entries = parsed["hooks"]["PreToolUse"].as_array().expect("array");
        let guard = entries
            .iter()
            .find(|e| e["matcher"] == "*")
            .expect("must register a * matcher");
        let command = guard["hooks"][0]["command"].as_str().unwrap();
        assert!(
            command.starts_with("lucidos cc-question-guard "),
            "{command}"
        );
        assert!(command.contains(&format!("'{OPEN_QUESTION_TOOL_REFUSAL}'")));
        assert!(!OPEN_QUESTION_TOOL_REFUSAL.contains('\''));
        for tool in [
            crate::runtime::CC_NATIVE_ASK_USER_QUESTION_TOOL,
            crate::runtime::CC_MCP_ASK_USER_QUESTION_TOOL,
        ] {
            assert!(
                command.contains(&format!("--question-tool {tool}")),
                "{command}"
            );
        }
    }

    #[test]
    fn json_registers_stop_hook_for_the_question_redirect() {
        let json = build_cc_settings_json(&[]);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entries = &parsed["hooks"]["Stop"];
        assert!(
            entries.is_array(),
            "Stop must be an array, so CC asks the hook about a plaintext question at idle"
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
        write_cc_settings(dir.path()).await.expect("write");
        let path = cc_settings_path_for_workspace(dir.path());
        assert!(path.exists(), "file must exist after write");
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(
            contents.contains("ask-user-question-hook"),
            "contents must reference the subcommand"
        );
    }

    #[test]
    fn widened_scope_is_the_workspace_data_dir_and_nothing_else() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("data")).expect("data dir");
        let real = std::fs::canonicalize(dir.path().join("data")).expect("data resolves");
        assert_eq!(
            widened_directories(dir.path()),
            vec![real],
            "exactly one entry, the workspace data dir",
        );
    }

    #[test]
    fn widened_scope_entry_is_absolute_even_for_a_relative_workspace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("test-workspace");
        std::fs::create_dir_all(workspace.join("data")).expect("data dir");
        let restore = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(dir.path()).expect("enter tempdir");
        let granted = widened_directories(Path::new("./test-workspace"));
        std::env::set_current_dir(&restore).expect("restore cwd");

        let entry = granted.first().expect("a relative root must still grant");
        assert!(
            entry.is_absolute(),
            "a relative workspace root must still yield an absolute grant, got {entry:?}",
        );
        assert_eq!(
            std::fs::canonicalize(entry).expect("entry resolves"),
            std::fs::canonicalize(workspace.join("data")).expect("data resolves"),
            "the grant must name the real data dir",
        );
    }

    #[test]
    fn widened_scope_covers_neither_the_root_nor_dot_lucidos() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("data")).expect("data dir");
        for granted in widened_directories(dir.path()) {
            assert_ne!(granted, dir.path(), "must not grant the workspace root");
            assert!(
                !granted.starts_with(dir.path().join(".lucidos")),
                "must not grant anything under .lucidos: {}",
                granted.display(),
            );
        }
    }

    #[test]
    fn widened_scope_grants_a_relocated_data_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("ws");
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir(&elsewhere).expect("target dir");
        std::os::unix::fs::symlink(&elsewhere, workspace.join("data")).expect("symlink");
        assert_eq!(
            widened_directories(&workspace),
            vec![std::fs::canonicalize(&elsewhere).expect("target resolves")],
            "a relocated data dir must be granted at its resolved path",
        );
    }

    #[test]
    fn widened_scope_refuses_a_data_symlink_onto_the_workspace_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(workspace.join(".lucidos/worktrees")).expect("workspace");
        std::os::unix::fs::symlink(&workspace, workspace.join("data")).expect("symlink");
        assert!(
            widened_directories(&workspace).is_empty(),
            "a data symlink onto the workspace root must grant nothing",
        );
    }

    #[test]
    fn widened_scope_refuses_a_data_symlink_into_a_sibling_worktree() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("ws");
        let sibling = workspace.join(".lucidos/worktrees/thread-other");
        std::fs::create_dir_all(&sibling).expect("sibling worktree");
        std::os::unix::fs::symlink(&sibling, workspace.join("data")).expect("symlink");
        assert!(
            widened_directories(&workspace).is_empty(),
            "a data symlink into .lucidos must grant nothing",
        );
    }

    #[tokio::test]
    async fn a_missing_data_dir_grants_no_data_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(widened_directories(dir.path()).is_empty());
        write_cc_settings(dir.path()).await.expect("write");
        let contents = tokio::fs::read_to_string(cc_settings_path_for_workspace(dir.path()))
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        for entry in granted_entries(&parsed) {
            assert!(
                !Path::new(&entry).starts_with(dir.path()),
                "no data dir must grant nothing under the workspace: {entry}",
            );
        }
    }

    fn granted_entries(parsed: &serde_json::Value) -> Vec<String> {
        parsed["permissions"]["additionalDirectories"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn the_os_tmp_dir_is_granted_unconditionally() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_cc_settings(dir.path()).await.expect("write");
        let contents = tokio::fs::read_to_string(cc_settings_path_for_workspace(dir.path()))
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let os_tmp = std::fs::canonicalize(OS_TMP_DIR).expect("OS temp resolves");
        assert!(
            granted_entries(&parsed).contains(&os_tmp.to_string_lossy().into_owned()),
            "the resolved OS temp dir must be granted even with no data dir",
        );
    }

    #[test]
    fn os_tmp_entries_all_name_the_same_real_directory() {
        let entries = os_tmp_directories_with(None);
        assert!(
            !entries.is_empty(),
            "the OS temp dir must resolve on this platform"
        );
        let real = std::fs::canonicalize(OS_TMP_DIR).expect("OS temp resolves");
        for entry in &entries {
            assert!(entry.is_absolute(), "entry must be absolute: {entry:?}");
            assert_eq!(
                std::fs::canonicalize(entry).expect("entry resolves"),
                real,
                "every OS temp entry must name the same real directory",
            );
        }
    }

    #[test]
    fn os_tmp_emits_the_literal_only_when_it_differs() {
        let entries = os_tmp_directories_with(None);
        let real = std::fs::canonicalize(OS_TMP_DIR).expect("OS temp resolves");
        let expected = if real == Path::new(OS_TMP_DIR) { 1 } else { 2 };
        assert_eq!(
            entries.len(),
            expected,
            "one entry when the OS temp path is real, two when it is a symlink",
        );
    }

    #[test]
    fn the_per_user_temp_dir_is_granted_beside_os_tmp() {
        let tmpdir = tempfile::tempdir().unwrap();
        let entries = os_tmp_directories_with(Some(tmpdir.path().into()));
        let real = std::fs::canonicalize(tmpdir.path()).unwrap();
        assert!(entries.contains(&real), "{entries:?}");
        assert!(entries.len() > os_tmp_directories_with(None).len());
        assert_eq!(
            os_tmp_directories_with(Some(std::ffi::OsString::new())),
            os_tmp_directories_with(None)
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
