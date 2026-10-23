use super::*;
use crate::engine::agent_session::side_question::SIDE_QUESTION_MAX_TURNS;

fn collect_envs(
    cmd: &tokio::process::Command,
) -> std::collections::HashMap<std::ffi::OsString, std::ffi::OsString> {
    cmd.as_std()
        .get_envs()
        .filter_map(|(k, v)| v.map(|v| (k.to_owned(), v.to_owned())))
        .collect()
}

/// The command alone, for tests that only read its flags and env.
fn session_command(args: &SpawnArgs<'_>, cli_dir: Option<&Path>) -> tokio::process::Command {
    build_command(args, cli_dir)
        .expect("the command builds")
        .cmd
}

fn test_spawn_args<'a>(
    worktree: &'a Path,
    workspace: &'a Path,
    thread_id: uuid::Uuid,
) -> SpawnArgs<'a> {
    SpawnArgs {
        worktree_path: worktree,
        coding_agent_kind: Default::default(),
        workspace_path: workspace,
        allowed_tools: None,
        system_prompt: None,
        session_context: None,
        resume_session_id: None,
        model: None,
        reasoning_effort: None,
        thread_id,
        spawning_event_id: None,
        repo_name: None,
        interactive: false,
        user_env_vars: &[],
        account_pin: None,
        binary_override: None,
        permission_mode: None,
        additional_directories: &[],
        auto_compact: None,
        notes: crate::runtime::claude_code_provider::NotesVisibility::Hidden,
        agent_run: crate::runtime::agent_run_marker::AgentRun::unrecorded(),
    }
}

fn test_spawn_args_with_event<'a>(
    worktree: &'a Path,
    workspace: &'a Path,
    thread_id: uuid::Uuid,
    spawning_event_id: Option<uuid::Uuid>,
) -> SpawnArgs<'a> {
    SpawnArgs {
        worktree_path: worktree,
        coding_agent_kind: Default::default(),
        workspace_path: workspace,
        allowed_tools: None,
        system_prompt: None,
        session_context: None,
        resume_session_id: None,
        model: None,
        reasoning_effort: None,
        thread_id,
        spawning_event_id,
        repo_name: None,
        interactive: false,
        user_env_vars: &[],
        account_pin: None,
        binary_override: None,
        permission_mode: None,
        additional_directories: &[],
        auto_compact: None,
        notes: crate::runtime::claude_code_provider::NotesVisibility::Hidden,
        agent_run: crate::runtime::agent_run_marker::AgentRun::unrecorded(),
    }
}

fn test_spawn_args_with_repo<'a>(
    worktree: &'a Path,
    workspace: &'a Path,
    thread_id: uuid::Uuid,
    repo_name: Option<&'a str>,
) -> SpawnArgs<'a> {
    SpawnArgs {
        worktree_path: worktree,
        coding_agent_kind: Default::default(),
        workspace_path: workspace,
        allowed_tools: None,
        system_prompt: None,
        session_context: None,
        resume_session_id: None,
        model: None,
        reasoning_effort: None,
        thread_id,
        spawning_event_id: None,
        repo_name,
        interactive: false,
        user_env_vars: &[],
        account_pin: None,
        binary_override: None,
        permission_mode: None,
        additional_directories: &[],
        auto_compact: None,
        notes: crate::runtime::claude_code_provider::NotesVisibility::Hidden,
        agent_run: crate::runtime::agent_run_marker::AgentRun::unrecorded(),
    }
}

#[test]
fn build_command_injects_user_env_vars_and_engine_wins() {
    // A user-defined env var lands in the coding-agent subprocess env, and an
    // engine-owned var (LUCIDOS_REPO, set from repo_name) wins over a user var
    // of the same name because user vars are applied FIRST in apply_lucidos_env.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let user_env = vec![
        ("MY_FLAG".to_string(), "1".to_string()),
        ("LUCIDOS_REPO".to_string(), "user-supplied".to_string()),
    ];
    let args = SpawnArgs {
        worktree_path: p,
        coding_agent_kind: Default::default(),
        workspace_path: p,
        allowed_tools: None,
        system_prompt: None,
        session_context: None,
        resume_session_id: None,
        model: None,
        reasoning_effort: None,
        thread_id,
        spawning_event_id: None,
        repo_name: Some("engine-repo"),
        interactive: false,
        user_env_vars: &user_env,
        account_pin: None,
        binary_override: None,
        permission_mode: None,
        additional_directories: &[],
        auto_compact: None,
        notes: crate::runtime::claude_code_provider::NotesVisibility::Hidden,
        agent_run: crate::runtime::agent_run_marker::AgentRun::unrecorded(),
    };
    let cmd = session_command(&args, None);
    let env = collect_envs(&cmd);
    assert_eq!(
        env.get(std::ffi::OsStr::new("MY_FLAG"))
            .map(|v| v.as_os_str()),
        Some(std::ffi::OsStr::new("1")),
        "user env var must be injected into the spawned subprocess"
    );
    assert_eq!(
        env.get(std::ffi::OsStr::new("LUCIDOS_REPO"))
            .map(|v| v.as_os_str()),
        Some(std::ffi::OsStr::new("engine-repo")),
        "engine-owned LUCIDOS_REPO must override a user var of the same name"
    );
}

#[test]
fn build_command_pins_claude_config_dir_over_user_env() {
    // A RESUME must run under the config dir the session was created in, even
    // when the user has since toggled CLAUDE_CONFIG_DIR to something else. The
    // pinned value (SpawnArgs.account_pin) is set AFTER apply_lucidos_env's
    // user-env loop, so it wins the collision — the fix for dev/bf997e21.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let user_env = vec![(
        "CLAUDE_CONFIG_DIR".to_string(),
        "/home/u/.claude-personal".to_string(),
    )];
    let mut args = test_spawn_args(p, p, thread_id);
    args.user_env_vars = &user_env;
    let pin = AccountPin::ExplicitConfigDir {
        dir: "/home/u/.claude".to_string(),
    };
    args.account_pin = Some(&pin);
    let cmd = session_command(&args, None);
    let env = collect_envs(&cmd);
    assert_eq!(
        env.get(std::ffi::OsStr::new("CLAUDE_CONFIG_DIR"))
            .map(|v| v.as_os_str()),
        Some(std::ffi::OsStr::new("/home/u/.claude")),
        "pinned config dir must override the user's live CLAUDE_CONFIG_DIR on resume"
    );
}

/// The "Please run /login" mid-thread bug. Turn 1 ran with `CLAUDE_CONFIG_DIR`
/// unset, so it read the default keychain entry the user logged into. A respawn
/// that sets the variable, even to `$HOME/.claude`, reads another entry and
/// another `.claude.json`, and finds no login.
#[test]
fn build_command_keeps_a_default_profile_pin_unset() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let user_env = vec![(
        "CLAUDE_CONFIG_DIR".to_string(),
        "/home/u/.claude-personal".to_string(),
    )];
    let mut args = test_spawn_args(p, p, thread_id);
    args.user_env_vars = &user_env;
    let pin = AccountPin::DefaultConfigDir {
        dir: "/home/u/.claude".to_string(),
    };
    args.account_pin = Some(&pin);
    let cmd = session_command(&args, None);
    let removed = cmd
        .as_std()
        .get_envs()
        .find(|(k, _)| *k == std::ffi::OsStr::new("CLAUDE_CONFIG_DIR"))
        .map(|(_, v)| v);
    assert_eq!(
        removed,
        Some(None),
        "a default-profile pin must remove CLAUDE_CONFIG_DIR, both the user's live \
         value and anything the engine inherited, and never set it to the default path"
    );
}

#[test]
fn build_command_leaves_user_claude_config_dir_when_not_pinned() {
    // A FRESH session (no pin) must leave the user's CLAUDE_CONFIG_DIR untouched.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let user_env = vec![(
        "CLAUDE_CONFIG_DIR".to_string(),
        "/home/u/.claude-personal".to_string(),
    )];
    let mut args = test_spawn_args(p, p, thread_id);
    args.user_env_vars = &user_env;
    // account_pin left None (fresh session)
    let cmd = session_command(&args, None);
    let env = collect_envs(&cmd);
    assert_eq!(
        env.get(std::ffi::OsStr::new("CLAUDE_CONFIG_DIR"))
            .map(|v| v.as_os_str()),
        Some(std::ffi::OsStr::new("/home/u/.claude-personal")),
        "a fresh session must keep the user's CLAUDE_CONFIG_DIR (no engine override)"
    );
}

#[test]
fn build_command_sets_lucidos_thread_id_env() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    let value = env
        .get(std::ffi::OsStr::new("LUCIDOS_THREAD_ID"))
        .expect("LUCIDOS_THREAD_ID env var must be set on the spawned subprocess");
    assert_eq!(value, std::ffi::OsStr::new(&thread_id.to_string()));
}

#[test]
fn build_command_sets_lucidos_workspace_env() {
    let thread_id = uuid::Uuid::new_v4();
    let workspace = std::path::Path::new("/some/workspace");
    let worktree = std::path::Path::new("/some/workspace/.lucidos/worktrees/abc");
    let cmd = session_command(&test_spawn_args(worktree, workspace, thread_id), None);
    let env = collect_envs(&cmd);
    assert_eq!(
        env.get(std::ffi::OsStr::new("LUCIDOS_WORKSPACE"))
            .map(|v| v.as_os_str()),
        Some(workspace.as_os_str())
    );
}

/// The last hop of the effort chain, and the only one that leaves the engine.
///
/// A spawn request's `reasoning_effort` is resolved in `run_direct_agent` and
/// arrives here as `SpawnArgs::reasoning_effort`. `CLAUDE_CODE_EFFORT_LEVEL` is
/// how Claude Code learns it, so a caller asking for `max` is only honoured if
/// this env var says so. `lucidos spawn-thread --reasoning-effort max` is one
/// such caller.
#[test]
fn build_command_sets_cc_effort_level_env_from_reasoning_effort() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let mut args = test_spawn_args(p, p, thread_id);
    args.reasoning_effort = Some("max");
    let cmd = session_command(&args, None);
    let env = collect_envs(&cmd);
    let value = env
        .get(std::ffi::OsStr::new("CLAUDE_CODE_EFFORT_LEVEL"))
        .expect("a requested effort must reach the subprocess");
    assert_eq!(value, std::ffi::OsStr::new("max"));
}

/// With nothing resolved, the var stays unset. Claude Code then reads its own
/// settings files, which `CcSettingsScope` already found empty.
#[test]
fn build_command_omits_cc_effort_level_env_when_no_effort_resolved() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args(p, p, thread_id), None);
    assert!(
        !cmd.as_std()
            .get_envs()
            .any(|(k, _)| k == "CLAUDE_CODE_EFFORT_LEVEL"),
        "an unset effort must not touch the level on the subprocess"
    );
}

fn spawned_effort(cmd: &tokio::process::Command) -> Option<std::ffi::OsString> {
    collect_envs(cmd).remove(std::ffi::OsStr::new("CLAUDE_CODE_EFFORT_LEVEL"))
}

/// The label records `SpawnArgs::reasoning_effort`, and CC runs at the env var.
/// They must agree for a default read from the workspace env. They must also
/// agree for a pin, which a workspace var of the same name used to overwrite.
#[test]
fn build_command_runs_at_the_effort_the_thread_records() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let project = tempfile::TempDir::new().unwrap();
    let user_env = vec![("CLAUDE_CODE_EFFORT_LEVEL".to_string(), "high".to_string())];
    let recorded = CcSettingsScope {
        env: &user_env,
        inherited: |_| None,
        config_dir: None,
        project_dir: project.path(),
    }
    .default_effort();
    assert_eq!(recorded.as_deref(), Some("high"));

    let mut args = test_spawn_args(p, p, thread_id);
    args.user_env_vars = &user_env;
    args.reasoning_effort = recorded.as_deref();
    assert_eq!(
        spawned_effort(&session_command(&args, None)),
        recorded.as_deref().map(Into::into)
    );

    args.reasoning_effort = Some("max");
    assert_eq!(
        spawned_effort(&session_command(&args, None)).as_deref(),
        Some(std::ffi::OsStr::new("max")),
        "a pinned effort must beat the workspace env var"
    );
}

#[test]
fn build_command_sets_lucidos_event_id_when_spawning_event_id_set() {
    // The Claude Code subprocess needs `LUCIDOS_EVENT_ID` so the `lucidos spawn-thread`
    // CLI can default `--caller-event-id` for cross-workspace POSTs without
    // the user having to thread the value through every invocation.
    let thread_id = uuid::Uuid::new_v4();
    let event_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(
        &test_spawn_args_with_event(p, p, thread_id, Some(event_id)),
        None,
    );
    let env = collect_envs(&cmd);
    let value = env
        .get(std::ffi::OsStr::new("LUCIDOS_EVENT_ID"))
        .expect("LUCIDOS_EVENT_ID must be set when spawning_event_id is provided");
    assert_eq!(value, std::ffi::OsStr::new(&event_id.to_string()));
}

#[test]
fn build_command_omits_lucidos_event_id_when_spawning_event_id_none() {
    // Recovery, hardening, and other engine-internal spawns have no parent
    // event — the env var must be unset so the CLI falls back to omitting
    // `caller_event_id` rather than stamping a stale or fabricated id.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args_with_event(p, p, thread_id, None), None);
    let env = collect_envs(&cmd);
    assert!(
        !env.contains_key(std::ffi::OsStr::new("LUCIDOS_EVENT_ID")),
        "LUCIDOS_EVENT_ID must be unset when no spawning_event_id"
    );
}

#[test]
fn build_command_sets_lucidos_session_kind_when_interactive() {
    // Interactive sessions (chat / recovery / external-repo) must set
    // LUCIDOS_SESSION_KIND=interactive so the cc-stop-reminder hook knows
    // it can safely block CC with an AskUserQuestion redirect when CC ends
    // a turn with a plaintext question.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let mut args = test_spawn_args(p, p, thread_id);
    args.interactive = true;
    let cmd = session_command(&args, None);
    let env = collect_envs(&cmd);
    assert_eq!(
        env.get(std::ffi::OsStr::new("LUCIDOS_SESSION_KIND"))
            .map(|v| v.as_os_str()),
        Some(std::ffi::OsStr::new("interactive")),
    );
}

#[test]
fn build_command_omits_lucidos_session_kind_when_not_interactive() {
    // Conflict-resolution sessions are unattended — they would hang on a
    // question redirect waiting for an answer that's not coming. The
    // cc-stop-reminder hook treats absence of LUCIDOS_SESSION_KIND as
    // "unattended, skip the question redirect".
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let args = test_spawn_args(p, p, thread_id); // interactive=false by default
    let cmd = session_command(&args, None);
    let env = collect_envs(&cmd);
    assert!(!env.contains_key(std::ffi::OsStr::new("LUCIDOS_SESSION_KIND")),);
}

#[test]
fn build_command_sets_lucidos_repo_when_repo_name_set() {
    // The Claude Code subprocess needs `LUCIDOS_REPO` so the `lucidos spawn-thread`
    // CLI defaults `--repo` to the calling thread's repo, keeping CC
    // sidequests in the same repo as their caller in workspaces hosting
    // worktrees from multiple repos.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(
        &test_spawn_args_with_repo(p, p, thread_id, Some("example-repo")),
        None,
    );
    let env = collect_envs(&cmd);
    let value = env
        .get(std::ffi::OsStr::new("LUCIDOS_REPO"))
        .expect("LUCIDOS_REPO must be set when repo_name is provided");
    assert_eq!(value, std::ffi::OsStr::new("example-repo"));
}

#[test]
fn build_command_omits_lucidos_repo_when_repo_name_none() {
    // Engine-internal spawns that don't know their repo (very early startup)
    // must leave the env var unset so the CLI falls back to the workspace
    // default repo rather than stamping a stale or fabricated name.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args_with_repo(p, p, thread_id, None), None);
    let env = collect_envs(&cmd);
    assert!(
        !env.contains_key(std::ffi::OsStr::new("LUCIDOS_REPO")),
        "LUCIDOS_REPO must be unset when no repo_name"
    );
}

#[test]
fn build_command_prepends_cli_dir_to_path() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cli_dir = std::path::Path::new("/opt/lucidos/bin");
    let cmd = session_command(&test_spawn_args(p, p, thread_id), Some(cli_dir));
    let env = collect_envs(&cmd);
    let path = env
        .get(std::ffi::OsStr::new("PATH"))
        .expect("PATH should be set");
    let path_str = path.to_string_lossy();
    assert!(
        path_str.starts_with(cli_dir.to_string_lossy().as_ref()),
        "PATH {:?} should start with lucidos cli dir {:?}",
        path_str,
        cli_dir
    );
}

fn collect_args(cmd: &tokio::process::Command) -> Vec<String> {
    cmd.as_std()
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn build_command_uses_permission_prompt_tool_not_skip_permissions() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    // The permission MCP server is launched via the ABSOLUTE path to the bundled
    // `lucidos` binary next to the engine — not the bare name — so it doesn't
    // depend on PATH propagating through the engine → claude(Node) → MCP-server
    // spawn chain (the bug that broke Claude Code in the packaged .app). Stage a
    // fake bundled binary and assert the config points at its absolute path.
    let cli = tempfile::TempDir::new().expect("tempdir");
    let lucidos_path = cli.path().join(LUCIDOS_BIN_NAME);
    std::fs::write(&lucidos_path, b"#!/bin/sh\n").expect("write fake lucidos");
    let cmd = session_command(&test_spawn_args(p, p, thread_id), Some(cli.path()));
    let args = collect_args(&cmd);

    assert!(
        !args.iter().any(|a| a == "--dangerously-skip-permissions"),
        "must not pass --dangerously-skip-permissions (would short-circuit the prompt tool)"
    );
    assert!(
        args.iter().any(|a| a == "--permission-prompt-tool"),
        "--permission-prompt-tool must be set"
    );
    let prompt_tool_idx = args
        .iter()
        .position(|a| a == "--permission-prompt-tool")
        .unwrap();
    assert_eq!(args[prompt_tool_idx + 1], "mcp__lucidos_perm__approve");

    let mcp_config_idx = args
        .iter()
        .position(|a| a == "--mcp-config")
        .expect("--mcp-config must be present");
    let cfg: serde_json::Value = serde_json::from_str(&args[mcp_config_idx + 1])
        .expect("--mcp-config value must be valid JSON");
    assert_eq!(
        cfg["mcpServers"]["lucidos_perm"]["command"],
        lucidos_path.to_string_lossy().as_ref(),
        "permission server must be launched via the absolute bundled `lucidos` path"
    );
    assert_eq!(
        cfg["mcpServers"]["lucidos_perm"]["args"],
        serde_json::json!(["mcp-permission-server", "--permission-only"]),
        "CC must spawn the server narrowed to `approve`: the shared binary also \
         serves Codex's `ask_user_question`, which CC would otherwise see as a \
         duplicate of its native AskUserQuestion and have to ask permission for"
    );
}

/// The strict flag would hide every plugin MCP server from the thread. The
/// permission gate never needed it: each MCP tool call still meets
/// `--allowedTools`, then `lucidos_perm` (ADR 0398).
#[test]
fn build_command_lets_claude_code_load_its_own_mcp_sources() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let args = collect_args(&session_command(&test_spawn_args(p, p, thread_id), None));

    assert!(
        !args.iter().any(|a| a == "--strict-mcp-config"),
        "--strict-mcp-config stops plugin MCP servers from starting (ADR 0398)"
    );
    assert!(
        args.iter().any(|a| a == "--mcp-config"),
        "the permission server still rides on --mcp-config"
    );
}

/// Both wire names must stay derived from the mount name, because the second
/// one is not spelled anywhere CC can correct us: `CC_MCP_ASK_USER_QUESTION_TOOL`
/// is how a tool CC advertises ARRIVES BACK in `CodingAgentToolCalled`, and the
/// gates in `runtime::is_user_question_tool` match it by string. Rename the
/// server without moving it and the question tool silently stops being
/// recognized, which is the exact failure this constant was added for: the user
/// gets a permission card in front of the question and a pending step above it.
///
/// The mount name itself is pinned against the spawned `--mcp-config` by the
/// test above, so the chain runs from the real command line to both constants.
#[test]
fn the_mcp_tool_names_carry_the_mount_name_cc_spawns() {
    for tool in [CC_PERMISSION_PROMPT_TOOL, CC_MCP_ASK_USER_QUESTION_TOOL] {
        assert!(
            tool.starts_with(&format!("mcp__{CC_PERMISSION_MCP_SERVER}__")),
            "{tool} must carry the `mcp__{CC_PERMISSION_MCP_SERVER}__` prefix CC prepends"
        );
    }
    // The native tool is CC's own, intercepted by the PreToolUse hook rather
    // than routed over MCP, so it must NOT wear a prefix.
    assert!(!CC_NATIVE_ASK_USER_QUESTION_TOOL.contains("mcp__"));
}

#[test]
fn resolve_lucidos_binary_prefers_bundled_cli_dir() {
    // The bundled `lucidos` next to the engine wins, resolved to its absolute
    // path so the permission MCP server never depends on PATH.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let bin = dir.path().join(LUCIDOS_BIN_NAME);
    std::fs::write(&bin, b"#!/bin/sh\n").expect("write fake lucidos");
    let resolved =
        resolve_lucidos_binary_in(Some(dir.path()), None).expect("must resolve the bundled binary");
    assert_eq!(resolved, bin);
}

#[test]
fn resolve_lucidos_binary_falls_back_to_path() {
    // No bundled binary next to the engine, but `lucidos` is on PATH (Homebrew,
    // npm, a dev's target dir) → resolve via PATH rather than failing.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let bin = dir.path().join(LUCIDOS_BIN_NAME);
    std::fs::write(&bin, b"#!/bin/sh\n").expect("write fake lucidos");
    let path_env = dir.path().as_os_str().to_owned();
    let resolved =
        resolve_lucidos_binary_in(None, Some(path_env.as_os_str())).expect("must resolve via PATH");
    assert_eq!(resolved, bin);
}

#[test]
fn resolve_lucidos_binary_errors_when_missing_everywhere() {
    // Reachable from neither cli_dir nor PATH → fail fast with a descriptive,
    // packaging-aware error instead of letting CC start a doomed session whose
    // first tool call dies with "Available MCP tools: none".
    let empty = tempfile::TempDir::new().expect("tempdir"); // exists, no `lucidos` inside
    let path_env = empty.path().as_os_str().to_owned();
    let err = resolve_lucidos_binary_in(Some(empty.path()), Some(path_env.as_os_str()))
        .expect_err("must error when lucidos is unreachable");
    let msg = err.to_string();
    assert!(
        msg.contains("lucidos") && msg.contains("PATH"),
        "error must name the missing binary and explain where it looked: {msg}"
    );
}

#[test]
fn build_command_passes_settings_flag_with_workspace_path() {
    let thread_id = uuid::Uuid::new_v4();
    let workspace = std::path::Path::new("/some/workspace");
    let worktree = std::path::Path::new("/some/workspace/.lucidos/worktrees/abc");
    let cmd = session_command(&test_spawn_args(worktree, workspace, thread_id), None);
    let args = collect_args(&cmd);

    let settings_idx = args
        .iter()
        .position(|a| a == "--settings")
        .expect("--settings must be present");
    let settings_path = args[settings_idx + 1].as_str();
    assert_eq!(
        settings_path,
        std::path::Path::new("/some/workspace/.lucidos/cc-settings.json")
            .to_string_lossy()
            .as_ref(),
        "--settings path must point at workspace .lucidos/cc-settings.json"
    );
}

/// Every mode the catalog lets a user store has a Claude Code mode, so the
/// mapping's `unreachable!` stays unreachable.
#[test]
fn every_catalogued_permission_mode_maps() {
    let crate::core::preference_catalog::PrefValue::Enum(modes) =
        crate::core::prefs::CODING_AGENT_CLAUDE_PERMISSION_MODE
            .spec
            .value
    else {
        panic!("the permission mode is an enum");
    };
    for mode in modes {
        resolve_permission_mode(Some(mode));
    }
    resolve_permission_mode(None);
}

#[test]
fn build_command_sets_permission_mode_accept_edits() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args(p, p, thread_id), None);
    let args = collect_args(&cmd);

    let mode_idx = args
        .iter()
        .position(|a| a == "--permission-mode")
        .expect("--permission-mode must be set");
    assert_eq!(
        args[mode_idx + 1],
        "acceptEdits",
        "acceptEdits auto-approves in-cwd writes; only out-of-cwd / Bash routes through the prompt tool"
    );
}

#[test]
fn build_command_includes_partial_messages_on_fresh_and_resumed_sessions() {
    // `--include-partial-messages` makes CC stream `stream_event` deltas, which
    // the engine turns into `AgentEvent::StreamActivity` liveness pings that keep
    // the watchdog's inactivity clock fresh through a long single step (extended
    // thinking on a hard problem). It MUST be set on RESUMED sessions too —
    // follow-ups, engine-restart recovery, merge-conflict resolution, hardening —
    // because those unattended long steps are exactly what the watchdog used to
    // kill mid-work: a `--resume` spawn emitted no deltas at all, so the clock
    // only ticked at step boundaries. The flag is a streaming-output option,
    // orthogonal to `--resume`; the old fresh-only gate was a fossil from when
    // resume lived in a separate spawn path.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");

    let fresh = session_command(&test_spawn_args(p, p, thread_id), None);
    assert!(
        collect_args(&fresh)
            .iter()
            .any(|a| a == "--include-partial-messages"),
        "fresh session must request partial messages"
    );

    let mut resumed_args = test_spawn_args(p, p, thread_id);
    resumed_args.resume_session_id = Some("sess-1");
    let resumed = session_command(&resumed_args, None);
    assert!(
        collect_args(&resumed)
            .iter()
            .any(|a| a == "--include-partial-messages"),
        "resumed session must ALSO request partial messages — else the watchdog is blind to its streaming and kills long active steps"
    );
}

#[test]
fn build_command_sets_mcp_tool_timeout_to_effective_infinity() {
    // The engine permission handler waits indefinitely (matching
    // AskUserQuestion). CC's MCP client must not time out either — a deny on
    // timeout would push the model into a retry that surfaces another card.
    // Verify the env var is set to "effectively never" (≥ 1 hour) so any
    // realistic user delay is covered.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    let raw = env
        .get(std::ffi::OsStr::new("MCP_TOOL_TIMEOUT"))
        .expect("MCP_TOOL_TIMEOUT must be set so CC's MCP client doesn't retry the prompt");
    let ms: u64 = raw
        .to_string_lossy()
        .parse()
        .expect("MCP_TOOL_TIMEOUT must be a number of milliseconds");
    let one_hour_ms: u64 = 3_600 * 1_000;
    assert!(
        ms >= one_hour_ms,
        "MCP_TOOL_TIMEOUT must be ≥ 1 hour for indefinite-wait behavior; got {ms}ms"
    );
}

#[test]
fn build_command_sets_mcp_timeout_to_effective_infinity() {
    // CC's MCP client also has a per-request timeout (`MCP_TIMEOUT`, default
    // 30s) that fires *before* MCP_TOOL_TIMEOUT for the permission_prompt
    // RPC. Without overriding it, every permission request is canceled after
    // 30s, the engine sees the receiver dropped (gc'd), CC's model retries
    // the original tool — and the user sees an apparent loop of identical
    // permission cards every ~30s. Set it to "effectively never" so the only
    // bound is the user's patience.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    let raw = env.get(std::ffi::OsStr::new("MCP_TIMEOUT")).expect(
        "MCP_TIMEOUT must be set so CC's MCP client doesn't cancel the permission RPC at 30s",
    );
    let ms: u64 = raw
        .to_string_lossy()
        .parse()
        .expect("MCP_TIMEOUT must be a number of milliseconds");
    let one_hour_ms: u64 = 3_600 * 1_000;
    assert!(
        ms >= one_hour_ms,
        "MCP_TIMEOUT must be ≥ 1 hour for indefinite-wait behavior; got {ms}ms"
    );
}

#[test]
fn resolve_claude_binary_prefers_native_installer_path() {
    // CC native installer canonical layout: $HOME/.local/bin/claude →
    // versioned binary. Engine must resolve to the absolute path so a short
    // PATH (launchd, IDE, non-interactive shell — anything that skips
    // .zshrc) doesn't break the spawn.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let local_bin = tmp.path().join(".local").join("bin");
    std::fs::create_dir_all(&local_bin).expect("create .local/bin");
    let claude_path = local_bin.join("claude");
    std::fs::write(&claude_path, b"#!/bin/sh\necho fake\n").expect("write fake claude");

    let resolved = resolve_claude_binary(Some(tmp.path()), None);
    assert_eq!(
        resolved,
        claude_path.as_os_str(),
        "must resolve to the absolute native-installer path when present"
    );
}

#[test]
fn resolve_claude_binary_override_wins_over_probes() {
    // A user-configured path (coding_agent_claude_path) beats every probe —
    // even when the native-installer path exists.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let local_bin = tmp.path().join(".local").join("bin");
    std::fs::create_dir_all(&local_bin).expect("create .local/bin");
    std::fs::write(local_bin.join("claude"), b"#!/bin/sh\n").expect("write fake claude");
    let override_path = tmp.path().join("custom-claude");

    let resolved = resolve_claude_binary(Some(tmp.path()), Some(&override_path));
    assert_eq!(
        resolved,
        override_path.as_os_str(),
        "a configured binary path must win over the probe list"
    );
}

#[test]
fn resolve_claude_binary_probes_claude_local_install() {
    // The older `claude migrate-installer` layout: $HOME/.claude/local/claude.
    // Probed after ~/.local/bin so the canonical native install still wins.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let local_install = tmp.path().join(".claude").join("local");
    std::fs::create_dir_all(&local_install).expect("create .claude/local");
    let claude_path = local_install.join("claude");
    std::fs::write(&claude_path, b"#!/bin/sh\n").expect("write fake claude");

    let resolved = resolve_claude_binary(Some(tmp.path()), None);
    assert_eq!(
        resolved,
        claude_path.as_os_str(),
        "must probe the ~/.claude/local install location"
    );
}

#[test]
fn resolve_binary_override_rejects_missing_path() {
    // A typo'd override must FAIL naming the setting — never silently fall
    // back to probing (that would spawn a different binary than configured).
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let missing = tmp.path().join("nope/claude");
    let err = crate::runtime::spawn_env::resolve_binary_override(
        missing.to_str().unwrap(),
        "Claude Code (`claude`)",
        crate::core::prefs::CODING_AGENT_CLAUDE_PATH.key(),
    )
    .expect_err("nonexistent override must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains(crate::core::prefs::CODING_AGENT_CLAUDE_PATH.key()),
        "error must name the preference so the user can fix it: {msg}"
    );
}

#[cfg(unix)]
#[test]
fn resolve_binary_override_rejects_non_executable_file() {
    // Present-but-not-executable fails with the exec-bit message instead of a
    // later cryptic spawn EACCES that doesn't name the setting.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let file = tmp.path().join("claude");
    std::fs::write(&file, b"not a binary").expect("write file");
    let err = crate::runtime::spawn_env::resolve_binary_override(
        file.to_str().unwrap(),
        "Claude Code (`claude`)",
        crate::core::prefs::CODING_AGENT_CLAUDE_PATH.key(),
    )
    .expect_err("non-executable override must be rejected");
    assert!(err.to_string().contains("not executable"), "{err}");
}

#[test]
fn resolve_claude_binary_falls_back_to_bare_name_when_native_missing() {
    // No native-installer binary → fall back to bare "claude" so
    // Command::spawn does its own PATH lookup. Preserves working installs
    // for users who installed via Homebrew, npm, or a custom symlink.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    // .local/bin intentionally not created.

    let resolved = resolve_claude_binary(Some(tmp.path()), None);
    // May resolve to a system install (Homebrew) when present on the test
    // host; otherwise the bare name. Either way it must not point inside the
    // empty temp home.
    assert!(!resolved.is_empty());
    assert!(
        !resolved
            .to_string_lossy()
            .starts_with(&*tmp.path().to_string_lossy()),
        "an empty HOME must not resolve to a HOME-relative path: {resolved:?}"
    );
}

#[test]
fn resolve_claude_binary_falls_back_when_no_home() {
    // No HOME → no HOME-derived probe candidates. The Homebrew prefixes may
    // still resolve on the test host; assert only that the result is usable
    // and not empty (the bare "claude" fallback otherwise).
    let resolved = resolve_claude_binary(None, None);
    assert!(!resolved.is_empty());
}

#[test]
fn build_command_skips_path_injection_when_no_cli_dir() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    // A host with `LUCIDOS_PG_BIN_DIR` set still prepends that dir, so PATH
    // may be set. It must never drop the inherited PATH.
    let Some(path) = env.get(std::ffi::OsStr::new("PATH")) else {
        return;
    };
    let path = std::env::split_paths(path).collect::<Vec<_>>();
    let pg_bin_dir = std::env::var_os("LUCIDOS_PG_BIN_DIR")
        .map(std::path::PathBuf::from)
        .filter(|dir| dir.is_dir());
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let expected = pg_bin_dir
        .into_iter()
        .chain(std::env::split_paths(&inherited))
        .collect::<Vec<_>>();
    assert_eq!(
        path, expected,
        "with no lucidos CLI dir, PATH is at most the Postgres dir before the inherited PATH"
    );
}

#[test]
fn build_command_forwards_host_protection_env_vars() {
    // build_command must route the kill-guard env block (LUCIDOS_HOST_PID,
    // LUCIDOS_FRONTEND_PID, LUCIDOS_API_PORT) through the shared helper in
    // api::actor — without it, a Claude Code subprocess could take down its own host
    // via a script that frees up "stale" ports. The helper's branch-by-branch
    // behavior is tested in api::actor::tests; here we just verify the
    // integration: at minimum, LUCIDOS_HOST_PID is set to the engine's pid.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    assert_eq!(
        env.get(std::ffi::OsStr::new("LUCIDOS_HOST_PID"))
            .map(|v| v.as_os_str()),
        Some(std::ffi::OsStr::new(&std::process::id().to_string())),
    );
}

#[test]
fn build_command_carries_the_agent_compile_env() {
    // RUSTC_WRAPPER must ALWAYS be set — to "sccache" when it's on PATH, else to
    // "" (empty). The empty value is load-bearing: the Lucidos repo's tracked
    // .cargo/config.toml sets `build.rustc-wrapper = "sccache"`, and cargo falls
    // back to that config when RUSTC_WRAPPER is merely unset. The SCCACHE_*
    // values route agent compiles to the agents' own daemon (ADR 0343).
    // Other tests move HOME and XDG_CACHE_HOME, so this reads the built env
    // once and checks its shape. The helper's own tests pin the exact values.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    let var = |key: &str| {
        env.get(std::ffi::OsStr::new(key))
            .map(|v| v.to_string_lossy().into_owned())
    };
    let sccache = crate::runtime::spawn_env::sccache_on_path(std::env::var_os("PATH").as_deref());
    assert_eq!(
        var("RUSTC_WRAPPER").as_deref(),
        Some(if sccache { "sccache" } else { "" }),
        "RUSTC_WRAPPER must be \"sccache\" when on PATH, else \"\" to disable the .cargo/config.toml fallback"
    );
    match (var("SCCACHE_SERVER_PORT"), var("SCCACHE_DIR")) {
        (None, None) => {}
        (Some(port), Some(dir)) => {
            assert!(sccache, "an agents' daemon is named only with sccache");
            assert_eq!(port, "4227");
            assert!(dir.ends_with("sccache-agents"), "{dir}");
        }
        other => panic!("the port and the cache dir come as a pair: {other:?}"),
    }
}

#[test]
fn format_user_input_text_only() {
    let uuid = uuid::Uuid::new_v4();
    let input = AgentInput {
        text: "hello".into(),
        images: vec![],
        uuid,
    };
    let line = format_user_input(&input, Some("sess-1"));
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(parsed["type"], "user");
    assert_eq!(parsed["message"]["role"], "user");
    assert_eq!(parsed["message"]["content"], "hello");
    assert_eq!(parsed["session_id"], "sess-1");
    // The name a withdraw uses to take the input back.
    assert_eq!(parsed["uuid"], uuid.to_string());
}

#[test]
fn format_user_input_with_images_uses_blocks() {
    let input = AgentInput {
        text: "describe".into(),
        images: vec![crate::api::ChatImage {
            base64: "deadbeef".into(),
            mime_type: "image/png".into(),
        }],
        uuid: uuid::Uuid::new_v4(),
    };
    let line = format_user_input(&input, None);
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    let content = parsed["message"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "describe");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["source"]["data"], "deadbeef");
    assert_eq!(parsed["session_id"], "default");
}

// ── CC byte-idle streaming deadline (CC_BYTE_STREAM_IDLE_TIMEOUT_MS) ────────
// CC's own SSE watchdog aborts a turn after 300s of zero bytes and reports
// `API Error: Stream idle timeout - no chunks received`, which is TERMINAL:
// no non-streaming fallback, at most one retry and only while nothing but
// thinking has been produced. A 200k+ token cache-cold prompt is silent on the
// wire for longer than that (measured 303s twice on 2026-08-02). The engine's
// own inactivity watchdog covers the same silence with a RECOVERABLE outcome,
// so CC's deadline has to be the outer one.
// See docs/investigations/2026-08-02-cc-stream-idle-timeout.md.

#[test]
fn build_command_raises_cc_byte_idle_deadline_past_the_engine_watchdog() {
    // The ordering IS the fix: whichever deadline is shorter decides what a
    // provider stall costs. Engine watchdog first => kill plus auto-resume via
    // ContinuationRequested. CC watchdog first => dead thread. Asserted against
    // the real constant so the two cannot drift into the wrong order.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    let raw = env
        .get(std::ffi::OsStr::new("CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS"))
        .expect(
            "CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS must be set, or CC kills a stalled turn at 300s",
        );
    let ms: i64 = raw
        .to_string_lossy()
        .parse()
        .expect("CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS must be a number of milliseconds");
    // BOTH engine silence detectors, not just the in-loop one: the external
    // watchdog is the out-of-loop backstop for a session the run loop lost
    // track of, and its decision is `Resume` / `ResumeIfRunning`, equally
    // non-destructive. CC's deadline has to be outside whichever of the two
    // fires last, or the destructive handler wins again for that case.
    for (name, engine_limit) in [
        (
            "in-loop inactivity watchdog",
            crate::engine::agent_session::lifecycle::WATCHDOG_INACTIVITY_LIMIT_MS,
        ),
        (
            "external watchdog",
            crate::engine::agent_session::external_watchdog::EXTERNAL_WATCHDOG_LIMIT_MS,
        ),
    ] {
        assert!(
            ms > engine_limit,
            "CC's byte-idle deadline ({ms}ms) must exceed the engine's {name} \
             ({engine_limit}ms) so a provider stall auto-resumes instead of failing the thread"
        );
    }
    // CC clamps the value to [10_000, 1_800_000]; anything above the ceiling is
    // silently reduced, so sending more would misrepresent the effective deadline.
    assert!(
        (10_000..=1_800_000).contains(&ms),
        "CC clamps this env var to [10000, 1800000]ms; {ms}ms would not survive the clamp"
    );
}

#[test]
fn build_command_lets_a_workspace_env_var_override_the_byte_idle_deadline() {
    // The deadline is a tunable default, not an engine contract, so it is set
    // BEFORE apply_lucidos_env (which applies user vars first and lets anything
    // written after it win). This test pins that placement: writing the env
    // after apply_lucidos_env would make the value unoverridable without a
    // rebuild, which is exactly what we don't want for a timeout knob.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let user_env = vec![(
        "CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS".to_string(),
        "900000".to_string(),
    )];
    let mut args = test_spawn_args(p, p, thread_id);
    args.user_env_vars = &user_env;
    let cmd = session_command(&args, None);
    let env = collect_envs(&cmd);
    assert_eq!(
        env.get(std::ffi::OsStr::new("CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS"))
            .map(|v| v.as_os_str()),
        Some(std::ffi::OsStr::new("900000")),
        "a workspace env var must override the engine's default byte-idle deadline"
    );
}

fn auto_compact_env(args: &SpawnArgs<'_>) -> Option<String> {
    collect_envs(&session_command(args, None))
        .get(std::ffi::OsStr::new(
            crate::engine::auto_compact_trial::AUTO_COMPACT_WINDOW_ENV,
        ))
        .map(|v| v.to_string_lossy().into_owned())
}

#[test]
fn build_command_is_unchanged_while_the_auto_compact_trial_is_off() {
    use crate::engine::auto_compact_trial::{decide, TrialMode};
    let p = std::path::Path::new("/tmp");
    let thread_id = uuid::Uuid::new_v4();
    let unset = test_spawn_args(p, p, thread_id);
    let mut off = test_spawn_args(p, p, thread_id);
    off.auto_compact = decide(TrialMode::Off, None, thread_id);
    let without_marker = |args: &SpawnArgs<'_>| {
        let mut env = collect_envs(&session_command(args, None));
        env.remove(std::ffi::OsStr::new(
            crate::runtime::agent_run_marker::AGENT_RUN_ENV,
        ));
        env
    };
    assert_eq!(without_marker(&unset), without_marker(&off));
    assert_eq!(auto_compact_env(&off), None);
}

#[test]
fn build_command_sets_the_window_the_setting_records() {
    use crate::engine::auto_compact_trial::{decide, AutoCompactArm, TrialMode};
    let p = std::path::Path::new("/tmp");
    let mut args = test_spawn_args(p, p, uuid::Uuid::new_v4());
    let setting = decide(TrialMode::On, None, uuid::Uuid::new_v4()).unwrap();
    assert_eq!(setting.arm, AutoCompactArm::Window);
    args.auto_compact = Some(setting);
    assert_eq!(
        auto_compact_env(&args),
        setting.window.map(|w| w.to_string()),
        "the spawned window is the recorded one"
    );
}

#[test]
fn build_command_leaves_a_workspace_auto_compact_window_alone() {
    use crate::engine::auto_compact_trial::{decide, TrialMode, AUTO_COMPACT_WINDOW_ENV};
    let p = std::path::Path::new("/tmp");
    let user_env = vec![(AUTO_COMPACT_WINDOW_ENV.to_string(), "250000".to_string())];
    let mut args = test_spawn_args(p, p, uuid::Uuid::new_v4());
    args.user_env_vars = &user_env;
    args.auto_compact = decide(TrialMode::Ab, Some("250000"), uuid::Uuid::new_v4());
    assert_eq!(auto_compact_env(&args).as_deref(), Some("250000"));
}

/// The argv `--permission-mode` value, for the mode assertions below.
fn permission_mode_arg(cmd: &tokio::process::Command) -> Option<String> {
    let args: Vec<String> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let at = args.iter().position(|a| a == "--permission-mode")?;
    args.get(at + 1).cloned()
}

fn auto_opt_in(cmd: &tokio::process::Command) -> Option<std::ffi::OsString> {
    collect_envs(cmd)
        .get(std::ffi::OsStr::new("CLAUDE_CODE_ENABLE_AUTO_MODE"))
        .cloned()
}

#[test]
fn an_unset_preference_keeps_the_pre_existing_accept_edits_mode() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args(p, p, thread_id), None);
    assert_eq!(
        permission_mode_arg(&cmd).as_deref(),
        Some("acceptEdits"),
        "no preference must spawn the mode every session ran before it existed",
    );
    assert!(
        auto_opt_in(&cmd).is_none(),
        "the opt-in var must stay absent unless auto was asked for",
    );
}

#[test]
fn an_unrecognised_preference_falls_back_rather_than_reaching_cc() {
    // Passing a junk value straight through would let CC reject the flag and
    // fail the spawn. Anything unrecognised is the default instead.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    for stored in ["", "  ", "garbage", "default", "bypassPermissions", "plan"] {
        let mut args = test_spawn_args(p, p, thread_id);
        args.permission_mode = Some(stored);
        let cmd = session_command(&args, None);
        assert_eq!(
            permission_mode_arg(&cmd).as_deref(),
            Some("acceptEdits"),
            "{stored:?} must resolve to acceptEdits",
        );
        assert!(auto_opt_in(&cmd).is_none(), "{stored:?} must not opt in");
    }
}

#[test]
fn the_stored_default_value_resolves_to_accept_edits() {
    // The kebab-case value the UI writes and CC's own spelling live in
    // different crates. Nothing else ties them, and the catch-all arm that
    // maps this one also swallows garbage, so a mistyped rename stays green.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let mut args = test_spawn_args(p, p, thread_id);
    args.permission_mode = Some("accept-edits");
    let cmd = session_command(&args, None);
    assert_eq!(
        permission_mode_arg(&cmd).as_deref(),
        Some("acceptEdits"),
        "the catalog's default value must reach CC as its own spelling",
    );
    assert!(auto_opt_in(&cmd).is_none());
}

#[test]
fn auto_ships_the_flag_and_its_opt_in_together() {
    // CC's gate downgrades auto to `default` on a non-first-party provider
    // without the opt-in var, and `default` cards MORE than acceptEdits. So the
    // two must never be separable.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let mut args = test_spawn_args(p, p, thread_id);
    args.permission_mode = Some("auto");
    let cmd = session_command(&args, None);
    assert_eq!(permission_mode_arg(&cmd).as_deref(), Some("auto"));
    assert_eq!(
        auto_opt_in(&cmd).as_deref(),
        Some(std::ffi::OsStr::new("1")),
        "auto must carry its opt-in var",
    );
}

#[test]
fn a_user_env_var_cannot_strand_a_session_that_asked_for_auto() {
    // Engine-owned, so it is written AFTER apply_lucidos_env. A stale
    // workspace env var set to zero would otherwise silently downgrade the
    // mode the user just chose.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let user_env = vec![("CLAUDE_CODE_ENABLE_AUTO_MODE".to_string(), "0".to_string())];
    let mut args = test_spawn_args(p, p, thread_id);
    args.permission_mode = Some("auto");
    args.user_env_vars = &user_env;
    let cmd = session_command(&args, None);
    assert_eq!(
        auto_opt_in(&cmd).as_deref(),
        Some(std::ffi::OsStr::new("1")),
        "the engine-owned opt-in must win over a user env var",
    );
}

fn background_tasks_switch(cmd: &tokio::process::Command) -> Option<std::ffi::OsString> {
    collect_envs(cmd)
        .get(std::ffi::OsStr::new("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"))
        .cloned()
}

#[test]
fn every_session_runs_with_claude_codes_background_tasks_off() {
    // A background job dies with the turn, yet Claude Code tells the model it
    // will be notified. The switch removes `run_in_background` and stops a
    // timed-out command from moving to the background (ADR 0358).
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args(p, p, uuid::Uuid::new_v4()), None);
    assert_eq!(
        background_tasks_switch(&cmd).as_deref(),
        Some(std::ffi::OsStr::new("1")),
    );
}

#[test]
fn a_user_env_var_cannot_turn_claude_codes_background_tasks_back_on() {
    let p = std::path::Path::new("/tmp");
    let user_env = vec![(
        "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS".to_string(),
        "0".to_string(),
    )];
    let mut args = test_spawn_args(p, p, uuid::Uuid::new_v4());
    args.user_env_vars = &user_env;
    let cmd = session_command(&args, None);
    assert_eq!(
        background_tasks_switch(&cmd).as_deref(),
        Some(std::ffi::OsStr::new("1")),
        "the engine-owned switch must win over a user env var",
    );
}

fn disallowed_tools(argv: &[String]) -> Option<String> {
    flag_value(argv, "--disallowedTools")
}

/// A resume and a side question share the session's tool list, so all three
/// shapes must carry the one list (ADR 0535).
#[test]
fn every_spawn_shape_disallows_the_tools_that_promise_later_work() {
    let workspace = tempfile::TempDir::new().unwrap();
    let worktree = PathBuf::from("/tmp/wt");
    let mut args = test_spawn_args(&worktree, workspace.path(), uuid::Uuid::new_v4());
    let fresh = collect_args(&session_command(&args, None));
    args.resume_session_id = Some("sess-1");
    let resumed = collect_args(&session_command(&args, None));
    let settings = PathBuf::from("/tmp/ws/.lucidos/cc-side-question-settings.json");
    let side_question = collect_args(
        &build_session_copy_command(&args, None, &settings, SIDE_QUESTION_MAX_TURNS)
            .unwrap()
            .cmd,
    );

    let expected = DISALLOWED_TOOLS.join(",");
    for (shape, argv) in [
        ("fresh", &fresh),
        ("resume", &resumed),
        ("side question", &side_question),
    ] {
        assert_eq!(
            disallowed_tools(argv).as_deref(),
            Some(expected.as_str()),
            "{shape}"
        );
        assert_eq!(
            argv.iter().filter(|a| *a == "--disallowedTools").count(),
            1,
            "{shape} carries one list",
        );
    }
}

#[test]
fn the_disallowed_tools_never_name_a_tool_a_session_needs() {
    for needed in [
        "Bash",
        "Read",
        "Edit",
        "Write",
        "Glob",
        "Grep",
        "Agent",
        "AskUserQuestion",
        "Skill",
        "ToolSearch",
        "ReportFindings",
        "SendMessage",
        CC_PERMISSION_PROMPT_TOOL,
        CC_MCP_ASK_USER_QUESTION_TOOL,
    ] {
        assert!(!DISALLOWED_TOOLS.contains(&needed), "{needed}");
    }
}

// ── The Vertex relay stamp (plan invariants 3 and 9) ───────────────────────

fn vertex_base_url(cmd: &tokio::process::Command) -> Option<String> {
    collect_envs(cmd)
        .get(std::ffi::OsStr::new("ANTHROPIC_VERTEX_BASE_URL"))
        .map(|v| v.to_string_lossy().into_owned())
}

#[test]
fn a_session_with_a_relay_sends_its_vertex_calls_through_it() {
    crate::api::actor::init_agent_origin_secret("build-command-relay-secret".to_string());
    let p = std::path::Path::new("/tmp");
    let args = test_spawn_args(p, p, uuid::Uuid::new_v4());
    let mut cmd = tokio::process::Command::new("true");
    stamp_vertex_relay(&mut cmd, &args, Some(4321));
    let url = vertex_base_url(&cmd).expect("the relay URL is stamped");
    assert!(
        url.starts_with("http://127.0.0.1:4321/api/v1/vertex-relay/"),
        "{url}"
    );
}

/// A workspace's own Vertex base URL must not bypass the relay. It rides the
/// token as the upstream instead.
#[test]
fn a_workspace_vertex_url_becomes_the_relays_upstream() {
    crate::api::actor::init_agent_origin_secret("build-command-relay-secret".to_string());
    let p = std::path::Path::new("/tmp");
    let user_env = vec![(
        "ANTHROPIC_VERTEX_BASE_URL".to_string(),
        "https://proxy.example/v1".to_string(),
    )];
    let mut args = test_spawn_args(p, p, uuid::Uuid::new_v4());
    args.user_env_vars = &user_env;
    let mut with_relay = tokio::process::Command::new("true");
    with_relay.env("ANTHROPIC_VERTEX_BASE_URL", "https://proxy.example/v1");
    stamp_vertex_relay(&mut with_relay, &args, Some(4321));
    let url = vertex_base_url(&with_relay).unwrap();
    let override_hex = crate::api::hex::hex_lower(b"https://proxy.example/v1");
    assert!(url.contains(&override_hex), "{url}");
}

/// No relay running leaves the session exactly as it was.
#[test]
fn a_session_without_a_relay_keeps_its_own_vertex_url() {
    let p = std::path::Path::new("/tmp");
    let user_env = vec![(
        "ANTHROPIC_VERTEX_BASE_URL".to_string(),
        "https://proxy.example/v1".to_string(),
    )];
    let mut args = test_spawn_args(p, p, uuid::Uuid::new_v4());
    args.user_env_vars = &user_env;
    let mut cmd = tokio::process::Command::new("true");
    cmd.env("ANTHROPIC_VERTEX_BASE_URL", "https://proxy.example/v1");
    stamp_vertex_relay(&mut cmd, &args, None);
    assert_eq!(
        vertex_base_url(&cmd).as_deref(),
        Some("https://proxy.example/v1")
    );
}

/// The replay is how the engine learns an input was read (ADR 0268). A spawn
/// without it would owe every forwarded input forever, so no idle could exit.
#[test]
fn build_command_requests_input_replays_on_fresh_and_resumed_sessions() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let replays = |cmd: &tokio::process::Command| {
        collect_args(cmd)
            .iter()
            .any(|a| a == "--replay-user-messages")
    };

    assert!(replays(&session_command(
        &test_spawn_args(p, p, thread_id),
        None
    )));
    let mut resumed_args = test_spawn_args(p, p, thread_id);
    resumed_args.resume_session_id = Some("sess-1");
    assert!(replays(&session_command(&resumed_args, None)));
}

/// A side question's copy resumes the thread's session with the session's own
/// flags, so the prompt prefix matches and the transcript reads from cache. It
/// writes nothing to the transcript, takes few turns, and runs under the
/// settings that refuse every tool.
#[test]
fn side_question_command_copies_the_session_without_persisting() {
    let worktree = PathBuf::from("/tmp/wt");
    let workspace = tempfile::TempDir::new().unwrap();
    let mut args = test_spawn_args(&worktree, workspace.path(), uuid::Uuid::new_v4());
    args.resume_session_id = Some("sess-9");
    args.model = Some("opus");
    args.system_prompt = Some("the session's prompt");
    args.allowed_tools = Some("Read,Bash");
    let grants = [PathBuf::from("/p/sibling"), PathBuf::from("/p/other")];
    args.additional_directories = &grants;
    let settings = PathBuf::from("/tmp/ws/.lucidos/cc-side-question-settings.json");
    let argv = |cmd: &tokio::process::Command| -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    };
    let copy_command =
        build_session_copy_command(&args, None, &settings, SIDE_QUESTION_MAX_TURNS).unwrap();
    let session_command = build_command(&args, None).unwrap();
    let copy = argv(&copy_command.cmd);
    let session = argv(&session_command.cmd);
    let value_of = |argv: &[String], flag: &str| {
        argv.iter()
            .position(|a| a == flag)
            .map(|at| argv[at + 1].clone())
    };
    assert_eq!(value_of(&copy, "--resume").as_deref(), Some("sess-9"));
    assert!(copy.iter().any(|a| a == "--no-session-persistence"));
    assert_eq!(
        value_of(&copy, "--max-turns"),
        Some(SIDE_QUESTION_MAX_TURNS.to_string())
    );
    assert!(
        !copy.iter().any(|a| a == "--tools"),
        "the tool list must match"
    );
    assert_eq!(
        value_of(&copy, "--settings").as_deref(),
        Some(settings.to_str().unwrap())
    );
    for flag in ["--model", "--allowedTools", "--permission-mode"] {
        assert_eq!(value_of(&copy, flag), value_of(&session, flag), "{flag}");
    }
    let prompt_of = |argv: &[String]| {
        std::fs::read_to_string(value_of(argv, "--append-system-prompt-file").unwrap()).unwrap()
    };
    assert_eq!(prompt_of(&copy), "the session's prompt");
    assert_eq!(prompt_of(&session), "the session's prompt");
    // Claude Code names the granted directories in its system prompt.
    assert_eq!(add_dirs(&copy), add_dirs(&session));
    assert_eq!(add_dirs(&copy), ["/p/sibling", "/p/other"]);
}

/// The values of every `--add-dir`, in order.
fn add_dirs(argv: &[String]) -> Vec<&str> {
    argv.iter()
        .enumerate()
        .filter(|(_, a)| *a == "--add-dir")
        .map(|(at, _)| argv[at + 1].as_str())
        .collect()
}

/// A repo's grants reach Claude Code as one `--add-dir` each, in order.
#[test]
fn build_command_passes_each_repo_grant_as_add_dir() {
    let p = Path::new("/tmp");
    let grants = [PathBuf::from("/p/sibling"), PathBuf::from("/p/other")];
    let mut args = test_spawn_args(p, p, uuid::Uuid::new_v4());
    args.additional_directories = &grants;
    let argv = collect_args(&session_command(&args, None));
    assert_eq!(add_dirs(&argv), ["/p/sibling", "/p/other"]);
}

/// A repo without the setting spawns exactly as before.
#[test]
fn build_command_has_no_add_dir_without_repo_grants() {
    let p = Path::new("/tmp");
    let argv = collect_args(&session_command(
        &test_spawn_args(p, p, uuid::Uuid::new_v4()),
        None,
    ));
    assert!(add_dirs(&argv).is_empty());
}

/// `lucidos cc-agent-guard` reads agent definitions from the `--add-dir`
/// directories, so both commands export exactly that list, in order.
#[test]
fn build_command_exports_the_add_dir_list_for_the_agent_guard() {
    let p = Path::new("/tmp");
    let grants = [
        PathBuf::from("/p/with space"),
        PathBuf::from("/p/with:colon"),
    ];
    let mut args = test_spawn_args(p, p, uuid::Uuid::new_v4());
    args.additional_directories = &grants;
    let settings = PathBuf::from("/tmp/ws/.lucidos/cc-side-question-settings.json");
    for cmd in [
        session_command(&args, None),
        build_session_copy_command(&args, None, &settings, SIDE_QUESTION_MAX_TURNS)
            .expect("the command builds")
            .cmd,
    ] {
        let exported = collect_envs(&cmd)
            .remove(std::ffi::OsStr::new(CC_ADDITIONAL_DIRECTORIES_ENV))
            .expect("the list is exported");
        let exported: Vec<String> =
            serde_json::from_str(exported.to_str().unwrap()).expect("a JSON array");
        assert_eq!(exported, add_dirs(&collect_args(&cmd)));
        assert_eq!(exported, ["/p/with space", "/p/with:colon"]);
    }
}

/// With no grants the variable is removed, so an inherited one cannot leak in.
#[test]
fn build_command_removes_the_add_dir_list_without_grants() {
    let p = Path::new("/tmp");
    let cmd = session_command(&test_spawn_args(p, p, uuid::Uuid::new_v4()), None);
    let removed = cmd
        .as_std()
        .get_envs()
        .any(|(k, v)| k == std::ffi::OsStr::new(CC_ADDITIONAL_DIRECTORIES_ENV) && v.is_none());
    assert!(removed);
}

/// Linux's `MAX_ARG_STRLEN`: the most bytes one argv string may hold.
const LINUX_MAX_ARG_STRLEN: usize = 131_072;

/// A long thread's system prompt never rides on argv. On argv it would fail
/// every spawn with `E2BIG` once it passed the Linux cap, and anyone on the
/// host could read it in `/proc/<pid>/cmdline`. It goes in a file only the
/// owner can read, and the file goes when the command does.
#[test]
fn a_huge_system_prompt_travels_in_a_private_file_not_argv() {
    let workspace = tempfile::TempDir::new().unwrap();
    let p = workspace.path();
    let marker = "THREAD-HISTORY-LINE ";
    let prompt = marker.repeat(1024 * 1024 / marker.len());
    let mut args = test_spawn_args(p, p, uuid::Uuid::new_v4());
    args.system_prompt = Some(&prompt);
    let command = build_command(&args, None).expect("the command builds");
    let argv = collect_args(&command.cmd);

    for arg in &argv {
        assert!(
            arg.len() <= LINUX_MAX_ARG_STRLEN,
            "argv string of {} bytes",
            arg.len()
        );
        assert!(!arg.contains(marker), "the prompt leaked into argv");
    }
    let at = argv
        .iter()
        .position(|a| a == "--append-system-prompt-file")
        .expect("the prompt file is named");
    let file = PathBuf::from(&argv[at + 1]);
    assert!(
        file.starts_with(session_files_dir(p)),
        "the workspace owns the file"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), prompt);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "only the owner may read the prompt");
    }
    drop(command);
    assert!(!file.exists(), "the prompt file outlived its command");
}

/// No system prompt means no prompt file and no flag naming one.
#[test]
fn no_system_prompt_names_no_prompt_file() {
    let p = Path::new("/tmp");
    let command = build_command(&test_spawn_args(p, p, uuid::Uuid::new_v4()), None).unwrap();
    assert!(command.system_prompt_file.is_none());
    assert!(!collect_args(&command.cmd)
        .iter()
        .any(|a| a.starts_with("--append-system-prompt")));
}

/// Boot deletes the prompt files an exited engine left behind, and keeps one
/// a side question may have written moments ago.
#[test]
fn the_boot_sweep_deletes_only_stale_system_prompt_files() {
    let workspace = tempfile::TempDir::new().unwrap();
    let dir = session_files_dir(workspace.path());
    std::fs::create_dir_all(&dir).unwrap();
    let stale = dir.join("system-prompt-old.md");
    let fresh = dir.join("system-prompt-new.md");
    std::fs::write(&fresh, "new").unwrap();
    std::fs::File::create(&stale)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
        .unwrap();

    sweep_stale_session_files(workspace.path());

    assert!(!stale.exists(), "a leftover file must go");
    assert!(fresh.exists(), "a file this engine may own must stay");
}

/// A workspace that never ran a session has no prompt dir, and that is fine.
#[test]
fn the_boot_sweep_tolerates_a_missing_prompt_dir() {
    let workspace = tempfile::TempDir::new().unwrap();
    sweep_stale_session_files(workspace.path());
    assert!(!session_files_dir(workspace.path()).exists());
}

// ---- startup instructions ride the shared system prompt ----

/// A worktree with a CLAUDE.md, a config directory, a `claude` that prints a
/// fixed version, and a workspace holding the base settings.
#[cfg(unix)]
struct StartupWorkspace {
    workspace: tempfile::TempDir,
    worktree: tempfile::TempDir,
    config: tempfile::TempDir,
    claude: tempfile::TempDir,
    env: Vec<(String, String)>,
}

#[cfg(unix)]
const LUCIDOS_PROMPT: &str = "THE LUCIDOS PROMPT";

#[cfg(unix)]
impl StartupWorkspace {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let config = tempfile::tempdir().unwrap();
        let env = vec![(
            "CLAUDE_CONFIG_DIR".to_string(),
            config.path().to_string_lossy().into_owned(),
        )];
        let this = Self {
            workspace: tempfile::tempdir().unwrap(),
            worktree: tempfile::tempdir().unwrap(),
            config,
            claude: tempfile::tempdir().unwrap(),
            env,
        };
        std::fs::write(this.worktree.path().join("CLAUDE.md"), "# Project rules\n").unwrap();
        let bin = this.claude.path().join("claude");
        std::fs::write(&bin, "#!/bin/sh\necho '7.7.7 (Claude Code)'\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let base =
            crate::engine::cc_settings::cc_settings_path_for_workspace(this.workspace.path());
        std::fs::create_dir_all(base.parent().unwrap()).unwrap();
        std::fs::write(
            &base,
            crate::engine::cc_settings::build_cc_settings_json(&[]),
        )
        .unwrap();
        this
    }

    fn bin(&self) -> String {
        self.claude
            .path()
            .join("claude")
            .to_string_lossy()
            .into_owned()
    }

    /// The args of a session with the Lucidos prompt. The `claude` path is
    /// borrowed from `bin`, which the caller keeps alive.
    fn args<'a>(&'a self, bin: &'a str) -> SpawnArgs<'a> {
        let mut args = test_spawn_args(
            self.worktree.path(),
            self.workspace.path(),
            uuid::Uuid::new_v4(),
        );
        args.system_prompt = Some(LUCIDOS_PROMPT);
        args.binary_override = Some(bin);
        args.user_env_vars = &self.env;
        args
    }

    fn mirror(&self) -> super::startup_instructions::StartupInstructions {
        super::startup_instructions::StartupInstructions::mirror(
            self.worktree.path(),
            self.config.path(),
            None,
        )
    }

    fn mark(&self, verdict: super::startup_instructions::Verdict) {
        super::startup_instructions::StartupLedger::record(
            self.workspace.path(),
            &self.mirror().key("7.7.7 (Claude Code)"),
            verdict,
        );
    }
}

#[cfg(unix)]
fn flag_value(argv: &[String], flag: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == flag)
        .map(|at| argv[at + 1].clone())
}

/// An unverified key runs as today: Claude Code's own settings file, the
/// Lucidos prompt alone, and a check owed after the first turn.
#[cfg(unix)]
#[test]
fn an_unverified_key_runs_exactly_as_today() {
    let ws = StartupWorkspace::new();
    let bin = ws.bin();
    let command = build_command(&ws.args(&bin), None).unwrap();
    let argv = collect_args(&command.cmd);
    let base = crate::engine::cc_settings::cc_settings_path_for_workspace(ws.workspace.path());
    assert_eq!(
        flag_value(&argv, "--settings").unwrap(),
        base.to_str().unwrap()
    );
    let prompt = flag_value(&argv, "--append-system-prompt-file").unwrap();
    assert_eq!(std::fs::read_to_string(prompt).unwrap(), LUCIDOS_PROMPT);
    assert!(command.session_settings_file.is_none());
    assert!(matches!(
        command.startup_follow_up,
        Some(StartupFollowUp::Check(_))
    ));
}

#[cfg(unix)]
#[test]
fn a_failed_key_runs_exactly_as_today_and_asks_for_no_check() {
    let ws = StartupWorkspace::new();
    ws.mark(super::startup_instructions::Verdict::Failed);
    let bin = ws.bin();
    let command = build_command(&ws.args(&bin), None).unwrap();
    let argv = collect_args(&command.cmd);
    let prompt = flag_value(&argv, "--append-system-prompt-file").unwrap();
    assert_eq!(std::fs::read_to_string(prompt).unwrap(), LUCIDOS_PROMPT);
    assert!(command.session_settings_file.is_none());
    assert!(command.startup_follow_up.is_none());
}

/// A verified key moves the files: the block follows the Lucidos prompt, and
/// the settings exclude exactly the files the block holds.
#[cfg(unix)]
#[test]
fn a_verified_key_appends_the_block_after_the_prompt_and_excludes_the_files() {
    let ws = StartupWorkspace::new();
    ws.mark(super::startup_instructions::Verdict::Verified);
    let bin = ws.bin();
    let command = build_command(&ws.args(&bin), None).unwrap();
    let argv = collect_args(&command.cmd);

    let prompt =
        std::fs::read_to_string(flag_value(&argv, "--append-system-prompt-file").unwrap()).unwrap();
    assert_eq!(
        prompt,
        format!("{LUCIDOS_PROMPT}\n\n{}", ws.mirror().render()),
        "the Lucidos prompt stays first and unchanged"
    );
    assert!(prompt.contains("Contents of CLAUDE.md (project instructions"));

    let settings_path = flag_value(&argv, "--settings").unwrap();
    let base = crate::engine::cc_settings::cc_settings_path_for_workspace(ws.workspace.path());
    assert_ne!(settings_path, base.to_str().unwrap());
    let mut settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    let excluded = settings
        .as_object_mut()
        .unwrap()
        .remove("claudeMdExcludes")
        .unwrap();
    assert_eq!(excluded, serde_json::json!(ws.mirror().excluded_paths()));
    assert_eq!(
        settings,
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&base).unwrap())
            .unwrap(),
        "everything else in the workspace settings carries over"
    );
    assert!(
        matches!(
            command.startup_follow_up,
            Some(StartupFollowUp::Moved { .. })
        ),
        "a moved session owes the ledger a record of the move"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&settings_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    drop(command);
    assert!(
        !Path::new(&settings_path).exists(),
        "the settings file outlived its command"
    );
}

/// Two worktrees at different paths with the same files append the same bytes,
/// which is what lets the cache serve both.
#[cfg(unix)]
#[test]
fn two_worktrees_with_the_same_files_append_the_same_bytes() {
    let (a, b) = (StartupWorkspace::new(), StartupWorkspace::new());
    for ws in [&a, &b] {
        ws.mark(super::startup_instructions::Verdict::Verified);
    }
    let appended = |ws: &StartupWorkspace| {
        let bin = ws.bin();
        let command = build_command(&ws.args(&bin), None).unwrap();
        let argv = collect_args(&command.cmd);
        std::fs::read_to_string(flag_value(&argv, "--append-system-prompt-file").unwrap()).unwrap()
    };
    assert_ne!(a.worktree.path(), b.worktree.path());
    assert_eq!(appended(&a), appended(&b));
}

/// A move that cannot write its settings leaves the files to Claude Code, so
/// no session runs with neither the files nor the block.
#[cfg(unix)]
#[test]
fn a_move_that_cannot_set_up_its_settings_keeps_claude_codes_loading() {
    let ws = StartupWorkspace::new();
    ws.mark(super::startup_instructions::Verdict::Verified);
    let base = crate::engine::cc_settings::cc_settings_path_for_workspace(ws.workspace.path());
    std::fs::write(&base, "not json").unwrap();
    let bin = ws.bin();
    let command = build_command(&ws.args(&bin), None).unwrap();
    let argv = collect_args(&command.cmd);
    assert_eq!(
        flag_value(&argv, "--settings").unwrap(),
        base.to_str().unwrap()
    );
    let prompt = flag_value(&argv, "--append-system-prompt-file").unwrap();
    assert_eq!(std::fs::read_to_string(prompt).unwrap(), LUCIDOS_PROMPT);
    assert!(command.session_settings_file.is_none());
}

/// A spawn with no Lucidos prompt, such as a model probe, has nothing for the
/// block to follow.
#[cfg(unix)]
#[test]
fn a_spawn_with_no_lucidos_prompt_never_moves() {
    let ws = StartupWorkspace::new();
    ws.mark(super::startup_instructions::Verdict::Verified);
    let bin = ws.bin();
    let mut args = ws.args(&bin);
    args.system_prompt = None;
    let command = build_command(&args, None).unwrap();
    assert!(command.system_prompt_file.is_none());
    assert!(command.session_settings_file.is_none());
    assert!(command.startup_follow_up.is_none());
}

/// The side question's copy resumes the session. It moves the files exactly
/// when the session started moved, and keeps its own settings for the rest.
#[cfg(unix)]
#[test]
fn a_side_question_copy_moves_the_files_over_its_own_settings() {
    let ws = StartupWorkspace::new();
    super::startup_instructions::StartupLedger::record_moved_session(ws.workspace.path(), "sess-1");
    let bin = ws.bin();
    let mut args = ws.args(&bin);
    args.resume_session_id = Some("sess-1");
    let own = ws
        .workspace
        .path()
        .join(".lucidos/cc-side-question-settings.json");
    let own_json = crate::engine::cc_settings::build_cc_side_question_settings_json(&[]);
    std::fs::write(&own, &own_json).unwrap();
    let copy = build_session_copy_command(&args, None, &own, SIDE_QUESTION_MAX_TURNS).unwrap();
    let argv = collect_args(&copy.cmd);
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(flag_value(&argv, "--settings").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        settings["claudeMdExcludes"],
        serde_json::json!(ws.mirror().excluded_paths())
    );
    assert_eq!(
        settings["hooks"],
        serde_json::from_str::<serde_json::Value>(&own_json).unwrap()["hooks"]
    );
    let prompt =
        std::fs::read_to_string(flag_value(&argv, "--append-system-prompt-file").unwrap()).unwrap();
    assert!(prompt.starts_with(LUCIDOS_PROMPT) && prompt.ends_with(&ws.mirror().render()));
}

/// Claude Code freezes a session's system prompt, so a block appended on resume
/// is never read. A session that started unmoved must keep Claude Code's
/// loading, even when its key was verified in between.
#[cfg(unix)]
#[test]
fn a_resumed_session_that_started_unmoved_never_moves() {
    let ws = StartupWorkspace::new();
    ws.mark(super::startup_instructions::Verdict::Verified);
    let bin = ws.bin();
    let mut args = ws.args(&bin);
    args.resume_session_id = Some("sess-unmoved");
    let command = build_command(&args, None).unwrap();
    let argv = collect_args(&command.cmd);
    let prompt = flag_value(&argv, "--append-system-prompt-file").unwrap();
    assert_eq!(std::fs::read_to_string(prompt).unwrap(), LUCIDOS_PROMPT);
    assert!(command.session_settings_file.is_none());
    assert!(command.startup_follow_up.is_none());
}

/// A session recorded as moved resumes moved, whatever its key says now, so
/// its files never load twice or go missing.
#[cfg(unix)]
#[test]
fn a_resumed_session_that_started_moved_moves_again() {
    let ws = StartupWorkspace::new();
    super::startup_instructions::StartupLedger::record_moved_session(
        ws.workspace.path(),
        "sess-moved",
    );
    let bin = ws.bin();
    let mut args = ws.args(&bin);
    args.resume_session_id = Some("sess-moved");
    let command = build_command(&args, None).unwrap();
    let argv = collect_args(&command.cmd);
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(flag_value(&argv, "--settings").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        settings["claudeMdExcludes"],
        serde_json::json!(ws.mirror().excluded_paths())
    );
    assert!(matches!(
        command.startup_follow_up,
        Some(StartupFollowUp::Moved { .. })
    ));
}

/// `claudeMdExcludes` holds glob patterns. A path a pattern might misread
/// would leave the file loaded twice, so such a worktree keeps Claude Code's
/// loading.
#[cfg(unix)]
#[test]
fn a_worktree_path_with_glob_syntax_never_moves() {
    let ws = StartupWorkspace::new();
    let odd = ws.worktree.path().join("old (copy)");
    std::fs::create_dir_all(&odd).unwrap();
    std::fs::write(odd.join("CLAUDE.md"), "# Rules\n").unwrap();
    let bin = ws.bin();
    let mut args = ws.args(&bin);
    args.worktree_path = &odd;
    let mirror =
        super::startup_instructions::StartupInstructions::mirror(&odd, ws.config.path(), None);
    super::startup_instructions::StartupLedger::record(
        ws.workspace.path(),
        &mirror.key("7.7.7 (Claude Code)"),
        super::startup_instructions::Verdict::Verified,
    );
    let command = build_command(&args, None).unwrap();
    assert!(command.session_settings_file.is_none());
}
