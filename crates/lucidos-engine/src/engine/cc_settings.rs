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
pub(crate) fn build_cc_settings_json(additional_directories: &[PathBuf]) -> String {
    let mut settings = serde_json::json!({
        "model": CC_DEFAULT_MODEL,
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
    });
    if !additional_directories.is_empty() {
        settings["permissions"] = serde_json::json!({
            "additionalDirectories": additional_directories
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        });
    }
    settings.to_string()
}

pub(crate) async fn write_cc_settings(
    workspace_root: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = cc_settings_path_for_workspace(workspace_root);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut granted = widened_directories(workspace_root);
    granted.extend(os_tmp_directories());
    let body = build_cc_settings_json(&granted);
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, body).await?;
    tokio::fs::rename(&tmp, &path).await?;
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
        let json = build_cc_settings_json(&[]);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["model"], "claude-opus-5@default",
            "cc-settings.json must pin the default CC model to Opus 5"
        );
        assert_eq!(parsed["model"], CC_DEFAULT_MODEL);
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
            serde_json::json!(HOOK_TIMEOUT_SECONDS),
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
    fn json_registers_stop_hook_for_harden_reminder() {
        let json = build_cc_settings_json(&[]);
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
