//! Generates `<workspace>/.lucidos/cc-settings.json` — the config CC reads via
//! `--settings` to discover our `PreToolUse` hook on `AskUserQuestion`. The
//! hook invokes `lucidos ask-user-question-hook` (a sibling subcommand on the
//! same `lucidos` CLI binary the engine already prepends to CC's PATH for the
//! MCP permission server — resolved via `LUCIDOS_CLI_BIN` in a packaged build,
//! the exe sibling-walk in dev; see `lucidos_cli::resolve_cli_dir`, and the
//! fail-fast in `ClaudeCodeRuntime::spawn` when the CLI can't be resolved).
//! Keeps the JSON shape literal; no per-spawn interpolation. See
//! `claude_code::permission_mcp_config_json` for the same pattern.
//! It carries hooks and directory grants only, never a model or effort default:
//! see [`build_cc_settings_json`].

use std::path::{Path, PathBuf};

pub(crate) fn cc_settings_path_for_workspace(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".lucidos/cc-settings.json")
}

/// How long a coding agent waits for the user's answer, in seconds: 24 hours.
/// The user may take minutes or hours. Every agent-side cap on that wait is
/// lifted to this one value: CC's PreToolUse hook timeout (default 60 s), its
/// `MCP_TOOL_TIMEOUT` and `MCP_TIMEOUT`, and Codex's `tool_timeout_sec`. A
/// shorter one forces a retry, which surfaces a duplicate card.
pub(crate) const USER_ANSWER_WAIT_SECS: u64 = 86_400;

/// The one working directory a CC session gets beyond its own worktree:
/// [`crate::core::DATA_DIR`], holding the artifacts, knowhow, apps and triggers
/// an agent reaches from a worktree that is their sibling. `.lucidos/` stays
/// out, so a sibling thread's worktree still needs a card.
///
/// It needs granting because CC's path check runs BEFORE allow-rule matching,
/// and never marks an outside-the-working-directories ask rule-overridable. No
/// `cc-allowed-tools` entry can suppress that card.
///
/// `canonicalize` does the three jobs Codex's `sandbox_writable_roots` needs it
/// for. It proves the dir exists. It makes the path ABSOLUTE, which matters
/// because a relative `LUCIDOS_WORKSPACE` is routine and CC resolves a relative
/// entry against the worktree. And it resolves symlinks, as CC's own matching
/// does.
///
/// Resolving means a `data` symlink decides the grant's width, so
/// [`grants_more_than_the_data_tree`] draws the same line for both back ends. A
/// relocated tree stays granted; one widened onto the workspace root is
/// refused. Every refusal grants nothing and logs why, costing the user the
/// cards they had before rather than the spawn.
fn widened_directories(workspace_root: &Path) -> Vec<PathBuf> {
    let data_dir = workspace_root.join(crate::core::DATA_DIR);
    match std::fs::canonicalize(&data_dir).ok().filter(|d| d.is_dir()) {
        Some(dir)
            if crate::runtime::codex::grants_more_than_the_data_tree(&dir, workspace_root) =>
        {
            crate::log!(
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
            crate::log!(
                "[CcSettings] {} is not a reachable directory. Claude Code will keep asking \
                 before it reaches the workspace's data/ tree.",
                data_dir.display()
            );
            Vec::new()
        }
    }
}

/// The OS temp directory, where agents write throwaway files. Distinct from
/// Lucidos *scratch* (`core::TMP_DIR`, under the workspace), which the glossary
/// owns; this one belongs to the machine.
const OS_TMP_DIR: &str = "/tmp";

/// [`OS_TMP_DIR`] as CC should see it: the resolved path, plus the literal
/// beside it when the two differ.
///
/// Why it is granted: an `Edit` outside the session's working directories asks
/// with reason `workingDir`, and CC honours no bare `Edit` allow rule in any
/// mode. So a write there cards every time and nothing can pre-approve it.
///
/// Why BOTH strings: on macOS `/tmp` is a symlink to `/private/tmp`, on Linux
/// it is a real directory. CC resolves paths before comparing them, so either
/// form should match on its own. Emitting both costs one string and removes any
/// dependence on which side it resolves first.
///
/// No width check, unlike [`widened_directories`]. That one guards a path a
/// `data` symlink can aim anywhere. This is a fixed constant naming one
/// directory, so there is nothing for a user to point elsewhere. An
/// unresolvable path grants nothing and logs why, costing the user the cards
/// they had before rather than the spawn.
fn os_tmp_directories() -> Vec<PathBuf> {
    let literal = Path::new(OS_TMP_DIR);
    let Ok(resolved) = std::fs::canonicalize(literal) else {
        crate::log!(
            "[CcSettings] {} is not a reachable directory. Claude Code will keep asking \
             before it writes a throwaway file there.",
            literal.display()
        );
        return Vec::new();
    };
    if resolved == literal {
        vec![resolved]
    } else {
        vec![resolved, literal.to_path_buf()]
    }
}

/// Render the settings file. The `permissions` key is omitted entirely when the
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
                    // CC's NATIVE question tool only. The MCP one it can also
                    // reach (`CC_MCP_ASK_USER_QUESTION_TOOL`) needs no hook: it
                    // calls the same internal endpoint over MCP itself. Both
                    // names are in `runtime::is_user_question_tool`, which is
                    // about what the ENGINE does with the resulting tool_use;
                    // this matcher is about which tool CC has to stop for.
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
                    "matcher": "Agent",
                    "hooks": [{
                        "type": "command",
                        "command": "lucidos cc-agent-guard"
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
    });
    grant_directories(&mut settings, additional_directories);
    settings.to_string()
}

/// Add the `permissions.additionalDirectories` grant, or nothing for an empty
/// slice, so the file never names a directory that is not there. CC names
/// these directories in its system prompt, so every settings file a session
/// or its side-question copy runs under must grant the same set.
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

/// Every directory a CC session is granted beyond its worktree.
fn granted_directories(workspace_root: &Path) -> Vec<PathBuf> {
    let mut granted = widened_directories(workspace_root);
    granted.extend(os_tmp_directories());
    granted
}

/// Write `body` to `path` whole. The temp name is unique, so two writers of
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
    crate::log!("[CcSettings] wrote {}", path.display());
    Ok(())
}

/// The settings a side question's session copy runs under.
pub(crate) fn cc_side_question_settings_path_for_workspace(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".lucidos/cc-side-question-settings.json")
}

/// Render a side question's settings: one PreToolUse hook refusing every tool,
/// plus the session's own directory grants. The hook command single-quotes the
/// refusal for the shell, so the refusal must hold no single quote.
pub(crate) fn build_cc_side_question_settings_json(additional_directories: &[PathBuf]) -> String {
    let refusal = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason":
                crate::engine::agent_session::side_question::SIDE_QUESTION_TOOL_REFUSAL,
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
    settings.to_string()
}

/// Write a side question's settings and return their path.
pub(crate) async fn write_cc_side_question_settings(
    workspace_root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let path = cc_side_question_settings_path_for_workspace(workspace_root);
    let body = build_cc_side_question_settings_json(&granted_directories(workspace_root));
    write_whole(&path, body).await?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A side question must not run a single tool, whatever the model tries,
    /// and its refusal must reach CC as a valid PreToolUse deny.
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

    /// CC names the granted directories in its system prompt, so the copy must
    /// grant exactly what the session does or the prompt cache misses.
    #[test]
    fn side_question_settings_grant_the_sessions_directories() {
        let dirs = [PathBuf::from("/w/data"), PathBuf::from("/tmp")];
        let session: serde_json::Value =
            serde_json::from_str(&build_cc_settings_json(&dirs)).unwrap();
        let copy: serde_json::Value =
            serde_json::from_str(&build_cc_side_question_settings_json(&dirs)).unwrap();
        assert_eq!(session["permissions"], copy["permissions"]);
    }

    /// The engine's settings file outranks the user's own CC settings, so a
    /// default here would silently override the model and effort they chose.
    /// `CcSettingsScope::resolve` also relies on it to skip this file.
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
        // Without the Bash matcher, an in-CC `ps | grep cargo | xargs kill`
        // would once again kill every concurrent CC. Regression test for the
        // 2026-05-10 incident.
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

    /// A background subagent dies when the engine kills Claude Code at idle,
    /// so its report never arrives. Without this matcher a session can end
    /// its turn on one and leave the thread done with no changes.
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
    fn json_registers_plan_gate_hook_on_edit_and_write() {
        // The implementation-plan pre-edit gate is the sole hook on both Edit
        // and Write (the cc-edit-preread Read-before-Edit guard was removed —
        // CC enforces Read-before-Edit natively). The marker must be checked
        // before any edit.
        let json = build_cc_settings_json(&[]);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entries = parsed["hooks"]["PreToolUse"].as_array().expect("array");
        for tool in ["Edit", "Write"] {
            let entry = entries
                .iter()
                .find(|e| e["matcher"] == tool)
                .unwrap_or_else(|| panic!("must register a {tool} matcher"));
            let hooks = entry["hooks"].as_array().expect("hooks array");
            assert!(
                hooks.iter().any(|h| h["command"] == "lucidos cc-plan-gate"),
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

    /// The whole point of the widening. A `cd` into the workspace data dir
    /// raises a card no allow rule can suppress, so the directory is granted as
    /// a working directory instead.
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

    /// `LUCIDOS_WORKSPACE` is used verbatim and is routinely relative: the
    /// Makefile passes `./test-workspace`, and the boot fallback is
    /// `./workspace`. CC resolves a relative entry against the worktree. A
    /// relative grant would open a hole somewhere meaningless, leave the real
    /// `data/` carding, and show nothing to say it had happened.
    ///
    /// A tempdir path is absolute already, so a test built on one cannot fail.
    /// This one enters the tempdir and passes a genuinely relative root.
    #[test]
    fn widened_scope_entry_is_absolute_even_for_a_relative_workspace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("test-workspace");
        std::fs::create_dir_all(workspace.join("data")).expect("data dir");
        // `set_current_dir` is process-global. `cargo test` runs the whole
        // crate's tests as threads in ONE process, so the window is shared with
        // every test in `lucidos-engine`, not just this module. It is the only
        // `set_current_dir` in the crate today. Adding a second, or a test that
        // resolves a relative path, needs a shared lock rather than this note.
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

    /// Neither the workspace root nor `.lucidos/` may be granted. Either one
    /// would let a session walk into a sibling thread's worktree with no card,
    /// which is the scope this change deliberately did not take.
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

    /// Relocating `data/` onto another disk is a supported layout, and Codex
    /// already grants it. Refusing every symlink would card one back end and
    /// not the other for the same workspace.
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

    /// A `data` symlink resolving to the workspace root, or above it, would hand
    /// over `.lucidos/` and every sibling worktree. The shared predicate refuses
    /// it.
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

    /// Pointing `data` INTO the engine's runtime dir reaches a sibling thread's
    /// worktree. It is never an ancestor of the workspace, so the up-only
    /// version of the predicate granted it. The doc comment above promises
    /// `.lucidos/` stays out, and this holds it to that.
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

    /// A settings file naming a directory that is not there could break CC's
    /// startup. No data dir means nothing to reach, so no data entry is
    /// written. Scratch is unconditional and unaffected, which is why this
    /// asserts on the entries rather than on the key's absence.
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

    /// The whole granted set, as CC reads it out of the written file.
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

    /// An `Edit` under the OS temp dir cards with reason `workingDir`, and no
    /// bare `Edit` allow rule can suppress it, so the directory is granted.
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

    /// On macOS `/tmp` is a symlink to `/private/tmp`, so reusing the data
    /// dir's symlink refusal here would grant nothing on every Mac. Every
    /// entry names the same real directory.
    #[test]
    fn os_tmp_entries_all_name_the_same_real_directory() {
        let entries = os_tmp_directories();
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

    /// The literal joins the resolved path only when the two differ. A platform
    /// where `/tmp` is real gets one entry, not a duplicate pair.
    #[test]
    fn os_tmp_emits_the_literal_only_when_it_differs() {
        let entries = os_tmp_directories();
        let real = std::fs::canonicalize(OS_TMP_DIR).expect("OS temp resolves");
        let expected = if real == Path::new(OS_TMP_DIR) { 1 } else { 2 };
        assert_eq!(
            entries.len(),
            expected,
            "one entry when the OS temp path is real, two when it is a symlink",
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
