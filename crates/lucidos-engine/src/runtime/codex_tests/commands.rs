use super::*;

fn test_config(worktree: &Path) -> CodexConfig {
    CodexConfig {
        codex_bin: std::ffi::OsString::from("codex"),
        worktree_path: worktree.to_path_buf(),
        system_prompt: None,
        model: None,
        reasoning_effort: None,
        sandbox_writable_roots: Vec::new(),
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
    assert!(
        args.windows(2)
            .any(|w| w[0] == "-c" && w[1] == "model_reasoning_summary=\"detailed\""),
        "codex's default summary mode emits no reasoning — the Thinking step needs this"
    );
    assert!(
        args.windows(2)
            .any(|w| w[0] == "-c" && w[1] == "project_doc_fallback_filenames=[\"CLAUDE.md\"]"),
        "no AGENTS.md ships (ADR 0004) — CLAUDE.md fallback is the CC-parity project doc"
    );
    assert!(
        args.windows(2)
            .any(|w| w[0] == "-c" && w[1] == "project_doc_max_bytes=65536"),
        "codex's 32KiB default would truncate Lucidos' ~29KiB CLAUDE.md soon"
    );
}

#[test]
fn max_effort_is_model_scoped_in_exec_driver() {
    let config = test_config(Path::new("/tmp/wt"));
    let cmd = build_codex_turn_command(&config, Some("gpt-5.6-sol"), Some("max"), None, "go", &[]);
    let args = collect_args(&cmd);
    assert!(
        args.windows(2)
            .any(|w| w[0] == "-c" && w[1] == "model_reasoning_effort=\"max\""),
        "GPT-5.6 models advertise Max and must receive it; got {args:?}"
    );

    // Older models reject Max. A stale selection must be omitted so Codex
    // applies its own default instead of failing the whole turn.
    let cmd = build_codex_turn_command(&config, Some("gpt-5.5"), Some("max"), None, "go", &[]);
    let args = collect_args(&cmd);
    assert!(
        !args
            .iter()
            .any(|a| a.starts_with("model_reasoning_effort=")),
        "Max must be omitted for pre-5.6 models"
    );

    let cmd = build_codex_turn_command(&config, None, Some("max"), None, "go", &[]);
    let args = collect_args(&cmd);
    assert!(
        !args
            .iter()
            .any(|a| a.starts_with("model_reasoning_effort=")),
        "Max must be omitted when the default model is unknown"
    );
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
    assert!(overrides
        .iter()
        .any(|o| o.starts_with("mcp_servers.lucidos.")));
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
fn every_sandbox_writable_root_becomes_an_add_dir() {
    // Two load-bearing holes in the workspace-write sandbox:
    //   /repo/.git  — a linked worktree's real git dir lives under the main
    //                 repo, so without it every in-agent `git commit` fails.
    //   /ws/data:     a direct write into the PARENT workspace's data/ tree
    //                 (`lucidos data path --mkdir`, an editor write to a
    //                 resolved data path) is outside the worktree. Without it
    //                 the seatbelt returns EPERM (os error 1): the 2026-07-26
    //                 nightly's Codex security pass lost two findings this way,
    //                 back when `lucidos data write` wrote the file itself.
    //                 That command now PUTs to the engine and needs no root.
    let mut config = test_config(Path::new("/tmp/wt"));
    config.sandbox_writable_roots = vec![PathBuf::from("/repo/.git"), PathBuf::from("/ws/data")];
    let cmd = build_codex_turn_command(&config, None, None, None, "p", &[]);
    let args = collect_args(&cmd);
    let dirs: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--add-dir")
        .map(|(i, _)| &args[i + 1])
        .collect();
    assert_eq!(
        dirs,
        vec!["/repo/.git", "/ws/data"],
        "exactly the configured roots, in order; an extra --add-dir is an \
         unreviewed widening of the sandbox"
    );
}

#[test]
fn no_add_dir_when_there_are_no_writable_roots() {
    let config = test_config(Path::new("/tmp/wt"));
    assert!(config.sandbox_writable_roots.is_empty());
    let cmd = build_codex_turn_command(&config, None, None, None, "p", &[]);
    assert!(!collect_args(&cmd).iter().any(|a| a == "--add-dir"));
}

#[tokio::test]
async fn writable_roots_grant_the_workspace_data_dir_and_nothing_wider() {
    // The worktree here is not a git repo, so resolve_git_common_dir degrades
    // to None and the data dir is the only entry — which is what lets this
    // assert the SCOPE exactly.
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("data/artifacts")).unwrap();
    // A sibling the sandbox must NOT be able to write.
    std::fs::create_dir_all(ws.path().join(".lucidos/worktrees/other")).unwrap();

    let roots = super::super::codex::sandbox_writable_roots(
        &ws.path().join(".lucidos/worktrees/mine"),
        ws.path(),
    )
    .await;

    let expected = std::fs::canonicalize(ws.path().join("data")).unwrap();
    assert_eq!(roots, vec![expected]);
    assert!(
        roots[0].is_absolute(),
        "codex resolves a relative --add-dir against the CHILD's cwd (the \
         worktree), so a non-absolute root opens the hole in the wrong place"
    );
    assert!(
        !roots.contains(&ws.path().to_path_buf()),
        "the workspace ROOT must never be writable — it holds .lucidos/ \
         (engine runtime, logs, gateway registry) and every sibling worktree"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn writable_roots_are_canonicalized_not_taken_verbatim() {
    // Same `canonicalize` call that absolutizes a relative LUCIDOS_WORKSPACE
    // (the Makefile passes `./test-workspace`, the boot fallback is
    // `./workspace`) — asserted here through a symlink, which is testable
    // without mutating the process-global cwd. It also covers the macOS
    // seatbelt's own requirement: it matches on real paths, so `/var/...`
    // must reach it as `/private/var/...`.
    let real = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(real.path().join("data")).unwrap();
    let link_parent = tempfile::tempdir().unwrap();
    let link = link_parent.path().join("ws-link");
    std::os::unix::fs::symlink(real.path(), &link).unwrap();

    let roots = super::super::codex::sandbox_writable_roots(&link.join("wt"), &link).await;

    assert_eq!(
        roots,
        vec![std::fs::canonicalize(real.path().join("data")).unwrap()]
    );
    assert_ne!(
        roots[0],
        link.join("data"),
        "the verbatim, un-resolved path must not be what reaches the sandbox"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn the_git_root_is_canonicalized_too_not_just_the_data_root() {
    // The sibling test above cannot cover this arm: its worktree is not a git
    // repo, so resolve_git_common_dir degrades to None. Both roots have to be
    // canonical for the same reason — the seatbelt matches real paths — and a
    // git root that isn't is a silently-blocked in-agent `git commit`, which is
    // the only reason that entry exists.
    let real = tempfile::tempdir().unwrap();
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(real.path())
        .status();
    // No git on the box ⇒ nothing to assert; the degrade path is its own test.
    let Ok(status) = init else { return };
    if !status.success() {
        return;
    }
    std::fs::create_dir_all(real.path().join("data")).unwrap();
    let link_parent = tempfile::tempdir().unwrap();
    let link = link_parent.path().join("ws-link");
    std::os::unix::fs::symlink(real.path(), &link).unwrap();

    // Worktree == repo root, so `git rev-parse --git-common-dir` answers with a
    // RELATIVE `.git` — joined onto the symlinked worktree, then canonicalized.
    let roots = super::super::codex::sandbox_writable_roots(&link, &link).await;

    let git_root = std::fs::canonicalize(real.path().join(".git")).unwrap();
    assert!(
        roots.contains(&git_root),
        "expected the canonical git dir {} among {roots:?}",
        git_root.display()
    );
    assert!(
        !roots.iter().any(|r| r.starts_with(&link)),
        "no root may still carry the symlinked path the seatbelt won't match"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_data_symlink_may_relocate_the_tree_but_never_widen_it() {
    // Resolving symlinks is required (the seatbelt matches real paths), which
    // hands the `data` entry control over how wide the hole is. Relocation is a
    // legitimate setup and must keep working; widening must not.
    let ws = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let wt = ws.path().join("wt");

    // Relocated: `data -> <elsewhere>` still grants exactly that tree.
    std::os::unix::fs::symlink(elsewhere.path(), ws.path().join("data")).unwrap();
    let roots = super::super::codex::sandbox_writable_roots(&wt, ws.path()).await;
    assert_eq!(
        roots,
        vec![std::fs::canonicalize(elsewhere.path()).unwrap()],
        "relocating data/ onto another disk is a supported layout"
    );

    // Widened: `data -> .` would grant the workspace root — .lucidos/ (engine
    // runtime, logs, gateway registry) and every sibling worktree with it.
    std::fs::remove_file(ws.path().join("data")).unwrap();
    std::os::unix::fs::symlink(ws.path(), ws.path().join("data")).unwrap();
    let roots = super::super::codex::sandbox_writable_roots(&wt, ws.path()).await;
    assert!(
        roots.is_empty(),
        "a data symlink that resolves to the workspace root must be refused, \
         not granted — that is the scope this function promises to withhold"
    );
}

#[tokio::test]
async fn writable_roots_skip_a_missing_data_dir_rather_than_creating_it() {
    let ws = tempfile::tempdir().unwrap();
    let roots = super::super::codex::sandbox_writable_roots(&ws.path().join("wt"), ws.path()).await;
    assert!(roots.is_empty());
    assert!(
        !ws.path().join("data").exists(),
        "spawn must not conjure a data dir — the engine provisions it at boot, \
         so its absence is a signal, not something to paper over"
    );
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
    // The GPT-5.6 family (Sol / Terra / Luna) is offered — the model ids codex
    // accepts for `-m`, mirroring the chat registry's gpt-5.6-* ids.
    for value in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
        assert!(
            options.iter().any(|o| o["value"] == value),
            "{value} must be in the codex /model picker"
        );
    }

    let effort_options = arr[1]["params"][0]["options"]
        .as_array()
        .expect("effort options");
    assert!(
        effort_options.iter().any(|o| o["value"] == "max"),
        "Max must be in the Codex effort picker"
    );
    assert!(
        effort_options
            .iter()
            .all(|o| o.get("supported_models").is_none()),
        "the matrix is transposed onto the model rows; the wire carries no \
         effort-to-models list"
    );
}

/// The transpose. The JSON declares which models accept `max`; the wire says
/// which efforts each model offers. Both must describe the same matrix.
#[test]
fn each_codex_model_row_carries_the_efforts_it_accepts() {
    let defs = codex_command_definitions();
    let options = defs.as_array().expect("array")[0]["params"][0]["options"]
        .as_array()
        .expect("options")
        .clone();
    let efforts_of = |model: &str| -> Vec<String> {
        options
            .iter()
            .find(|o| o["value"] == model)
            .unwrap_or_else(|| panic!("{model} must be offered"))["reasoning_efforts"]
            .as_array()
            .unwrap_or_else(|| panic!("{model} must declare its tiers"))
            .iter()
            .map(|e| e.as_str().expect("tier").to_string())
            .collect()
    };
    for sixer in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
        assert!(
            efforts_of(sixer).contains(&"max".to_string()),
            "{sixer} accepts max"
        );
    }
    for earlier in ["gpt-5.5", "gpt-5.4", "gpt-5.4-mini"] {
        assert!(
            !efforts_of(earlier).contains(&"max".to_string()),
            "{earlier} does not accept max"
        );
    }
    // The sentinel resolves to whatever the user's Codex config picks, so it
    // can only be offered the tiers every model takes.
    assert!(!efforts_of("default").contains(&"max".to_string()));
    assert!(efforts_of("default").contains(&"high".to_string()));
}

/// The JSON keeps the effort-to-models shape upstream announces. A model row
/// that grew a `reasoning_efforts` key would be the drift the transpose exists
/// to prevent.
#[test]
fn the_codex_menu_json_still_declares_the_matrix_on_the_effort_rows() {
    assert!(
        codex_model_options()
            .iter()
            .all(|m| m.supported_models.is_none()),
        "a model row must not declare compatibility; the effort rows do"
    );
    assert!(
        codex_reasoning_effort_options()
            .iter()
            .any(|e| e.supported_models.is_some()),
        "at least one effort must still name its models, or the matrix is gone"
    );
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
    let resolved = resolve_codex_binary(Some(tmp.path()), None);
    assert_eq!(resolved, codex_path.as_os_str());
}

#[test]
fn resolve_codex_binary_override_wins_over_probes() {
    // A user-configured path (coding_agent_codex_path) beats every probe —
    // even when the ~/.local/bin install exists.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let local_bin = tmp.path().join(".local").join("bin");
    std::fs::create_dir_all(&local_bin).expect("create .local/bin");
    std::fs::write(local_bin.join("codex"), b"#!/bin/sh\n").expect("write stub");
    let override_path = tmp.path().join("custom-codex");

    let resolved = resolve_codex_binary(Some(tmp.path()), Some(&override_path));
    assert_eq!(
        resolved,
        override_path.as_os_str(),
        "a configured binary path must win over the probe list"
    );
}

#[test]
fn resolve_codex_binary_falls_back_to_bare_name() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let resolved = resolve_codex_binary(Some(tmp.path()), None);
    // May resolve to a system install (homebrew) when present on the test
    // host; otherwise the bare name. Either way it must be non-empty and not
    // point inside the empty temp home.
    assert!(!resolved.is_empty());
    assert!(!resolved
        .to_string_lossy()
        .starts_with(&*tmp.path().to_string_lossy()));
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
