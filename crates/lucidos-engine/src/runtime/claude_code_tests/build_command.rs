use super::*;

fn collect_envs(
    cmd: &tokio::process::Command,
) -> std::collections::HashMap<std::ffi::OsString, std::ffi::OsString> {
    cmd.as_std()
        .get_envs()
        .filter_map(|(k, v)| v.map(|v| (k.to_owned(), v.to_owned())))
        .collect()
}

fn test_spawn_args<'a>(
    worktree: &'a Path,
    workspace: &'a Path,
    thread_id: uuid::Uuid,
) -> SpawnArgs<'a> {
    SpawnArgs {
        worktree_path: worktree,
        workspace_path: workspace,
        allowed_tools: None,
        system_prompt: None,
        resume_session_id: None,
        model: None,
        reasoning_effort: None,
        thread_id,
        spawning_event_id: None,
        repo_name: None,
        interactive: false,
        continuation: false,
        user_env_vars: &[],
        claude_config_dir: None,
        binary_override: None,
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
        workspace_path: workspace,
        allowed_tools: None,
        system_prompt: None,
        resume_session_id: None,
        model: None,
        reasoning_effort: None,
        thread_id,
        spawning_event_id,
        repo_name: None,
        interactive: false,
        continuation: false,
        user_env_vars: &[],
        claude_config_dir: None,
        binary_override: None,
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
        workspace_path: workspace,
        allowed_tools: None,
        system_prompt: None,
        resume_session_id: None,
        model: None,
        reasoning_effort: None,
        thread_id,
        spawning_event_id: None,
        repo_name,
        interactive: false,
        continuation: false,
        user_env_vars: &[],
        claude_config_dir: None,
        binary_override: None,
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
        workspace_path: p,
        allowed_tools: None,
        system_prompt: None,
        resume_session_id: None,
        model: None,
        reasoning_effort: None,
        thread_id,
        spawning_event_id: None,
        repo_name: Some("engine-repo"),
        interactive: false,
        continuation: false,
        user_env_vars: &user_env,
        claude_config_dir: None,
        binary_override: None,
    };
    let cmd = build_command(&args, None);
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
    // pinned value (SpawnArgs.claude_config_dir) is set AFTER apply_lucidos_env's
    // user-env loop, so it wins the collision — the fix for dev/bf997e21.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let user_env = vec![(
        "CLAUDE_CONFIG_DIR".to_string(),
        "/home/u/.claude-personal".to_string(),
    )];
    let mut args = test_spawn_args(p, p, thread_id);
    args.user_env_vars = &user_env;
    args.claude_config_dir = Some("/home/u/.claude");
    let cmd = build_command(&args, None);
    let env = collect_envs(&cmd);
    assert_eq!(
        env.get(std::ffi::OsStr::new("CLAUDE_CONFIG_DIR"))
            .map(|v| v.as_os_str()),
        Some(std::ffi::OsStr::new("/home/u/.claude")),
        "pinned config dir must override the user's live CLAUDE_CONFIG_DIR on resume"
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
    // claude_config_dir left None (fresh session)
    let cmd = build_command(&args, None);
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
    let cmd = build_command(&test_spawn_args(p, p, thread_id), None);
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
    let cmd = build_command(&test_spawn_args(worktree, workspace, thread_id), None);
    let env = collect_envs(&cmd);
    assert_eq!(
        env.get(std::ffi::OsStr::new("LUCIDOS_WORKSPACE"))
            .map(|v| v.as_os_str()),
        Some(workspace.as_os_str())
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
    let cmd = build_command(
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
    let cmd = build_command(&test_spawn_args_with_event(p, p, thread_id, None), None);
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
    let cmd = build_command(&args, None);
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
    let cmd = build_command(&args, None);
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
    let cmd = build_command(
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
    let cmd = build_command(&test_spawn_args_with_repo(p, p, thread_id, None), None);
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
    let cmd = build_command(&test_spawn_args(p, p, thread_id), Some(cli_dir));
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
    let cmd = build_command(&test_spawn_args(p, p, thread_id), Some(cli.path()));
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
        cfg["mcpServers"]["lucidos_perm"]["args"][0],
        "mcp-permission-server"
    );

    assert!(
        args.iter().any(|a| a == "--strict-mcp-config"),
        "--strict-mcp-config keeps the permission server isolated from the user's global MCP config"
    );
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
    let cmd = build_command(&test_spawn_args(worktree, workspace, thread_id), None);
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

#[test]
fn build_command_sets_permission_mode_accept_edits() {
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = build_command(&test_spawn_args(p, p, thread_id), None);
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

    let fresh = build_command(&test_spawn_args(p, p, thread_id), None);
    assert!(
        collect_args(&fresh)
            .iter()
            .any(|a| a == "--include-partial-messages"),
        "fresh session must request partial messages"
    );

    let mut resumed_args = test_spawn_args(p, p, thread_id);
    resumed_args.resume_session_id = Some("sess-1");
    let resumed = build_command(&resumed_args, None);
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
    let cmd = build_command(&test_spawn_args(p, p, thread_id), None);
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
    let cmd = build_command(&test_spawn_args(p, p, thread_id), None);
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
        "coding_agent_claude_path",
    )
    .expect_err("nonexistent override must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("coding_agent_claude_path"),
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
        "coding_agent_claude_path",
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
    let cmd = build_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    assert!(
        !env.contains_key(std::ffi::OsStr::new("PATH")),
        "PATH must not be set when lucidos CLI binary is missing — \
         otherwise we'd shadow the inherited PATH with an empty value"
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
    let cmd = build_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    assert_eq!(
        env.get(std::ffi::OsStr::new("LUCIDOS_HOST_PID"))
            .map(|v| v.as_os_str()),
        Some(std::ffi::OsStr::new(&std::process::id().to_string())),
    );
}

#[test]
fn build_command_gates_rustc_wrapper_on_sccache_presence() {
    // RUSTC_WRAPPER must ALWAYS be set — to "sccache" when it's on PATH, else to
    // "" (empty). The empty value is load-bearing: the Lucidos repo's tracked
    // .cargo/config.toml sets `build.rustc-wrapper = "sccache"`, and cargo falls
    // back to that config when RUSTC_WRAPPER is merely unset; only an explicit
    // empty value forces a plain build on a host without sccache. Leaving it
    // unset (the original bug) would still hard-fail Lucidos-repo builds. Pin the
    // wiring by comparing against the same predicate build_command consults, so
    // the assertion is deterministic whether or not the test host has sccache.
    let thread_id = uuid::Uuid::new_v4();
    let p = std::path::Path::new("/tmp");
    let cmd = build_command(&test_spawn_args(p, p, thread_id), None);
    let env = collect_envs(&cmd);
    let wrapper = env.get(std::ffi::OsStr::new("RUSTC_WRAPPER")).expect(
        "RUSTC_WRAPPER must always be set (sccache, or empty to override .cargo/config.toml)",
    );
    let expected =
        if crate::runtime::spawn_env::sccache_on_path(std::env::var_os("PATH").as_deref()) {
            std::ffi::OsString::from("sccache")
        } else {
            std::ffi::OsString::from("")
        };
    assert_eq!(
        wrapper.as_os_str(),
        expected.as_os_str(),
        "RUSTC_WRAPPER must be \"sccache\" when on PATH, else \"\" to disable the .cargo/config.toml fallback"
    );
}

#[test]
fn format_user_input_text_only() {
    let input = AgentInput {
        text: "hello".into(),
        images: vec![],
    };
    let line = format_user_input(&input, Some("sess-1"));
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(parsed["type"], "user");
    assert_eq!(parsed["message"]["role"], "user");
    assert_eq!(parsed["message"]["content"], "hello");
    assert_eq!(parsed["session_id"], "sess-1");
}

#[test]
fn format_user_input_with_images_uses_blocks() {
    let input = AgentInput {
        text: "describe".into(),
        images: vec![crate::api::ChatImage {
            base64: "deadbeef".into(),
            mime_type: "image/png".into(),
        }],
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
