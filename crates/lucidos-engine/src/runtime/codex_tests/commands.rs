use super::*;

fn test_config(worktree: &Path) -> CodexConfig {
    CodexConfig {
        codex_bin: std::ffi::OsString::from("codex"),
        worktree_path: worktree.to_path_buf(),
        system_prompt: None,
        model: None,
        reasoning_effort: None,
        git_common_dir: None,
        env: vec![
            (
                std::ffi::OsString::from("LUCIDOS_WORKSPACE"),
                std::ffi::OsString::from("/ws"),
            ),
            (
                std::ffi::OsString::from("LUCIDOS_THREAD_ID"),
                std::ffi::OsString::from("00000000-0000-0000-0000-000000000123"),
            ),
            (
                std::ffi::OsString::from("LUCIDOS_API_BASE_URL"),
                std::ffi::OsString::from("http://127.0.0.1:5173"),
            ),
        ],
    }
}

fn collect_args(cmd: &tokio::process::Command) -> Vec<String> {
    cmd.as_std()
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn fresh_turn_command_layout() {
    let config = test_config(Path::new("/tmp/wt"));
    let cmd = build_codex_turn_command(&config, None, None, None, "do the thing", &[]);
    let args = collect_args(&cmd);
    assert_eq!(args[0], "exec");
    assert!(
        args.contains(&"--json".to_string()),
        "JSONL output is the wire contract"
    );
    let sandbox_idx = args
        .iter()
        .position(|a| a == "--sandbox")
        .expect("--sandbox");
    assert_eq!(args[sandbox_idx + 1], "workspace-write");
    assert!(
        args.windows(2)
            .any(|w| w[0] == "-c" && w[1] == "sandbox_workspace_write.network_access=true"),
        "coding tasks need cargo/npm network access inside the sandbox"
    );
    assert_eq!(
        args.last().map(String::as_str),
        Some("do the thing"),
        "prompt must be the trailing positional"
    );
    assert!(
        !args.iter().any(|a| a == "resume"),
        "fresh turn must not resume"
    );
    assert!(!args.iter().any(|a| a == "-m"), "no model flag when unset");
}

#[test]
fn resume_turn_places_global_flags_before_subcommand() {
    // codex rejects exec-level flags after the `resume` subcommand
    // (`error: unexpected argument '--sandbox' found`) — pin the ordering.
    let config = test_config(Path::new("/tmp/wt"));
    let cmd = build_codex_turn_command(
        &config,
        Some("gpt-5.5"),
        Some("high"),
        Some("sid-123"),
        "follow up",
        &[],
    );
    let args = collect_args(&cmd);
    let resume_idx = args.iter().position(|a| a == "resume").expect("resume");
    for flag in ["--json", "--sandbox", "-m", "-c"] {
        let idx = args
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("{flag} present"));
        assert!(
            idx < resume_idx,
            "{flag} must precede the resume subcommand (codex rejects it after)"
        );
    }
    assert_eq!(args[resume_idx + 1], "sid-123");
    assert_eq!(args[resume_idx + 2], "follow up");
}

/// Every per-turn child must wire the lucidos MCP server so the model can
/// raise a QuestionCard via `ask_user_question`. The config keys travel
/// together: dropping `enabled_tools` leaks the CC-only `approve` tool to the
/// model; dropping `tools.ask_user_question.approval_mode` makes Codex reject
/// the MCP call before Lucidos can render the card; dropping
/// `tool_timeout_sec` re-introduces the "question times out while the user is
/// thinking" failure CC fixed with MCP_TIMEOUT.
#[test]
fn turn_command_wires_lucidos_mcp_server() {
    let config = test_config(Path::new("/tmp/wt"));
    let cmd = build_codex_turn_command(&config, None, None, None, "p", &[]);
    let args = collect_args(&cmd);
    for expected in lucidos_mcp_server_config_overrides(&config.env) {
        assert!(
            args.windows(2).any(|w| w[0] == "-c" && w[1] == expected),
            "missing -c {expected}; got {args:?}"
        );
    }
    assert!(
        args.windows(2).any(|w| w[0] == "-c"
            && w[1].starts_with("mcp_servers.lucidos.env={")
            && w[1].contains("LUCIDOS_WORKSPACE")
            && w[1].contains("LUCIDOS_THREAD_ID")
            && w[1].contains("LUCIDOS_API_BASE_URL")),
        "the lucidos MCP child must receive explicit env; Codex does not inherit \
         the app-server/exec process env into MCP subprocesses. got {args:?}"
    );
    assert!(
        args.windows(2).any(|w| w[0] == "-c"
            && w[1].starts_with("mcp_servers.lucidos.tools=")
            && w[1].contains("ask_user_question")
            && w[1].contains("approval_mode")
            && w[1].contains("approve")),
        "the question MCP tool must be pre-approved for non-interactive Codex. got {args:?}"
    );
}

/// The suppression gate in the engine's run loop and the MCP wire name must
/// agree — `mcp__<server>__<tool>` derives from the server name (`lucidos`)
/// and tool name (`ask_user_question`) in the config overrides above. A
/// rename on either side without the other double-renders the question card.
#[test]
fn ask_user_question_tool_name_matches_server_config() {
    let config = test_config(Path::new("/tmp/wt"));
    let overrides = lucidos_mcp_server_config_overrides(&config.env);
    assert!(
        overrides
            .iter()
            .any(|o| o.starts_with("mcp_servers.lucidos."))
    );
    assert!(overrides.iter().any(|o| o.contains("ask_user_question")));
    assert_eq!(
        CODEX_ASK_USER_QUESTION_TOOL,
        "mcp__lucidos__ask_user_question"
    );
}

#[test]
fn model_default_sentinel_is_omitted() {
    // "default" mirrors CC's sentinel — let the user's codex config decide.
    let config = test_config(Path::new("/tmp/wt"));
    let cmd = build_codex_turn_command(&config, Some("default"), None, None, "p", &[]);
    assert!(!collect_args(&cmd).iter().any(|a| a == "-m"));

    let cmd = build_codex_turn_command(&config, Some("gpt-5.4-mini"), None, None, "p", &[]);
    let args = collect_args(&cmd);
    let m_idx = args.iter().position(|a| a == "-m").expect("-m");
    assert_eq!(args[m_idx + 1], "gpt-5.4-mini");
}

#[test]
fn effort_maps_to_model_reasoning_effort_config() {
    let config = test_config(Path::new("/tmp/wt"));
    let cmd = build_codex_turn_command(&config, None, Some("xhigh"), None, "p", &[]);
    let args = collect_args(&cmd);
    assert!(
        args.windows(2)
            .any(|w| w[0] == "-c" && w[1] == "model_reasoning_effort=\"xhigh\""),
        "effort rides the -c config override; got {:?}",
        args
    );
}

#[test]
fn git_common_dir_becomes_add_dir() {
    // A linked worktree's real git dir lives under the main repo — without
    // --add-dir the workspace-write sandbox blocks every `git commit` the
    // agent runs.
    let mut config = test_config(Path::new("/tmp/wt"));
    config.git_common_dir = Some(PathBuf::from("/repo/.git"));
    let cmd = build_codex_turn_command(&config, None, None, None, "p", &[]);
    let args = collect_args(&cmd);
    let idx = args
        .iter()
        .position(|a| a == "--add-dir")
        .expect("--add-dir");
    assert_eq!(args[idx + 1], "/repo/.git");
}

#[test]
fn image_paths_become_i_flags() {
    let config = test_config(Path::new("/tmp/wt"));
    let imgs = vec![PathBuf::from("/tmp/a.png"), PathBuf::from("/tmp/b.jpg")];
    let cmd = build_codex_turn_command(&config, None, None, None, "p", &imgs);
    let args = collect_args(&cmd);
    let i_positions: Vec<usize> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "-i")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(i_positions.len(), 2);
    assert_eq!(args[i_positions[0] + 1], "/tmp/a.png");
    assert_eq!(args[i_positions[1] + 1], "/tmp/b.jpg");
}

#[test]
fn command_applies_config_env_and_null_stdin() {
    let config = test_config(Path::new("/tmp/wt"));
    let cmd = build_codex_turn_command(&config, None, None, None, "p", &[]);
    let envs: std::collections::HashMap<_, _> = cmd
        .as_std()
        .get_envs()
        .filter_map(|(k, v)| v.map(|v| (k.to_owned(), v.to_owned())))
        .collect();
    assert_eq!(
        envs.get(std::ffi::OsStr::new("LUCIDOS_WORKSPACE"))
            .map(|v| v.as_os_str()),
        Some(std::ffi::OsStr::new("/ws")),
        "the Lucidos env contract must reach every per-turn child"
    );
}

#[test]
fn compose_first_turn_prompt_prepends_instructions() {
    let composed = compose_first_turn_prompt(Some("Be careful."), "fix the bug");
    assert!(composed.starts_with("# Session instructions"));
    assert!(composed.contains("Be careful."));
    assert!(composed.ends_with("fix the bug"));

    assert_eq!(
        compose_first_turn_prompt(None, "fix the bug"),
        "fix the bug"
    );
    assert_eq!(compose_first_turn_prompt(Some(""), "x"), "x");
}

#[test]
fn codex_command_definitions_shape() {
    let defs = codex_command_definitions();
    let arr = defs.as_array().expect("array");
    let subtypes: Vec<&str> = arr.iter().map(|d| d["subtype"].as_str().unwrap()).collect();
    assert!(subtypes.contains(&"set_model"));
    assert!(subtypes.contains(&"set_reasoning_effort"));
    assert!(
        !subtypes.contains(&"set_permission_mode"),
        "codex has no permission protocol — the sandbox is the guard"
    );
    let model_def = &arr[0];
    let options = model_def["params"][0]["options"]
        .as_array()
        .expect("options");
    assert!(
        options.iter().any(|o| o["value"] == "default"),
        "default sentinel must be offered"
    );
    assert!(options.iter().any(|o| o["value"] == "gpt-5.5"));
}

#[test]
fn menu_options_json_parses() {
    // LazyLock would panic at first use in production — surface a malformed
    // codex_menu_options.json as a unit-test failure instead.
    assert!(!codex_model_options().is_empty());
    assert!(!codex_reasoning_effort_options().is_empty());
}

#[test]
fn resolve_codex_binary_prefers_local_bin() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let local_bin = tmp.path().join(".local").join("bin");
    std::fs::create_dir_all(&local_bin).expect("create .local/bin");
    let codex_path = local_bin.join("codex");
    std::fs::write(&codex_path, b"#!/bin/sh\n").expect("write stub");
    let resolved = resolve_codex_binary(Some(tmp.path()));
    assert_eq!(resolved, codex_path.as_os_str());
}

#[test]
fn resolve_codex_binary_falls_back_to_bare_name() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let resolved = resolve_codex_binary(Some(tmp.path()));
    // May resolve to a system install (homebrew) when present on the test
    // host; otherwise the bare name. Either way it must be non-empty and not
    // point inside the empty temp home.
    assert!(!resolved.is_empty());
    assert!(
        !resolved
            .to_string_lossy()
            .starts_with(&*tmp.path().to_string_lossy())
    );
}

#[test]
fn write_image_files_decodes_and_names_by_mime() {
    use base64::Engine as _;
    let png = crate::api::ChatImage {
        base64: base64::engine::general_purpose::STANDARD.encode(b"fakepng"),
        mime_type: "image/png".into(),
    };
    let jpg = crate::api::ChatImage {
        base64: base64::engine::general_purpose::STANDARD.encode(b"fakejpg"),
        mime_type: "image/jpeg".into(),
    };
    let bad = crate::api::ChatImage {
        base64: "!!!not-base64!!!".into(),
        mime_type: "image/png".into(),
    };
    let (paths, guards) = write_image_files(&[png, jpg, bad]);
    assert_eq!(paths.len(), 2, "undecodable image is dropped, not fatal");
    assert!(paths[0].to_string_lossy().ends_with(".png"));
    assert!(paths[1].to_string_lossy().ends_with(".jpg"));
    assert_eq!(std::fs::read(&paths[0]).unwrap(), b"fakepng");
    drop(guards);
    assert!(!paths[0].exists(), "temp files are cleaned up on drop");
}
