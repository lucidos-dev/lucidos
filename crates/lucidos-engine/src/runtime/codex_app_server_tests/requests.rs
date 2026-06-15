use super::*;
use crate::runtime::codex::CodexConfig;
use crate::runtime::codex_app_server_parse::parse_approval_request;
use std::path::PathBuf;

fn test_config() -> CodexConfig {
    CodexConfig {
        codex_bin: std::ffi::OsString::from("codex"),
        worktree_path: PathBuf::from("/tmp/wt"),
        system_prompt: Some("SYSPROMPT".into()),
        model: None,
        reasoning_effort: None,
        git_common_dir: None,
        env: Vec::new(),
    }
}

#[test]
fn fresh_thread_request_shape() {
    let (method, params) = build_thread_request(&test_config(), None);
    assert_eq!(method, "thread/start");
    assert_eq!(params["cwd"], "/tmp/wt");
    assert_eq!(params["sandbox"], "workspace-write");
    assert_eq!(
        params["approvalPolicy"], "on-request",
        "on-request is the point of this driver — sandbox escalations raise a card"
    );
    assert_eq!(params["developerInstructions"], "SYSPROMPT");
    assert_eq!(
        params["config"]["sandbox_workspace_write"]["network_access"], true,
        "coding tasks need cargo/npm network access inside the sandbox"
    );
    assert_eq!(
        params["config"]["mcp_servers"]["lucidos"]["enabled_tools"][0],
        "ask_user_question",
        "the question tool must ride the app-server protocol too"
    );
    assert!(params.get("threadId").is_none());
    assert!(params.get("model").is_none(), "no model param when unset");
}

#[test]
fn resume_thread_request_targets_the_stored_thread() {
    let (method, params) = build_thread_request(&test_config(), Some("sid-9"));
    assert_eq!(method, "thread/resume");
    assert_eq!(params["threadId"], "sid-9");
    // Same instruction-recovery semantics as CC's --append-system-prompt on
    // resume: the developer instructions ride every spawn.
    assert_eq!(params["developerInstructions"], "SYSPROMPT");
    assert_eq!(params["approvalPolicy"], "on-request");
}

#[test]
fn git_common_dir_becomes_writable_root() {
    // The app-server analog of the exec driver's --add-dir: without it the
    // workspace-write sandbox blocks every `git commit` in a linked worktree.
    let mut config = test_config();
    config.git_common_dir = Some(PathBuf::from("/repo/.git"));
    let (_, params) = build_thread_request(&config, None);
    assert_eq!(
        params["config"]["sandbox_workspace_write"]["writable_roots"][0],
        "/repo/.git"
    );
}

#[test]
fn model_default_sentinel_is_omitted() {
    let mut config = test_config();
    config.model = Some("default".into());
    let (_, params) = build_thread_request(&config, None);
    assert!(params.get("model").is_none());

    config.model = Some("gpt-5.5".into());
    let (_, params) = build_thread_request(&config, None);
    assert_eq!(params["model"], "gpt-5.5");
}

#[test]
fn turn_start_params_carry_text_images_model_effort() {
    let params = build_turn_start_params(
        "t-1",
        "do the thing",
        &[PathBuf::from("/tmp/a.png")],
        Some("gpt-5.5"),
        Some("xhigh"),
    );
    assert_eq!(params["threadId"], "t-1");
    assert_eq!(params["input"][0]["type"], "text");
    assert_eq!(params["input"][0]["text"], "do the thing");
    assert_eq!(params["input"][1]["type"], "localImage");
    assert_eq!(params["input"][1]["path"], "/tmp/a.png");
    assert_eq!(params["model"], "gpt-5.5");
    assert_eq!(params["effort"], "xhigh");
}

#[test]
fn turn_start_params_omit_empty_text_and_default_model() {
    let params = build_turn_start_params(
        "t-1",
        "",
        &[PathBuf::from("/tmp/a.png")],
        Some("default"),
        None,
    );
    // Image-only turn — no empty text block.
    assert_eq!(params["input"].as_array().unwrap().len(), 1);
    assert_eq!(params["input"][0]["type"], "localImage");
    assert!(params.get("model").is_none());
    assert!(params.get("effort").is_none());
}

#[test]
fn frame_serializers_produce_newline_terminated_jsonrpc() {
    let req = request_line(7, "turn/start", serde_json::json!({"threadId": "t"}));
    assert!(req.ends_with('\n'), "line-delimited framing");
    let parsed: serde_json::Value = serde_json::from_str(req.trim()).unwrap();
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["id"], 7);
    assert_eq!(parsed["method"], "turn/start");

    let notif = notification_line("initialized");
    let parsed: serde_json::Value = serde_json::from_str(notif.trim()).unwrap();
    assert_eq!(parsed["method"], "initialized");
    assert!(parsed.get("id").is_none(), "notifications carry no id");

    let resp = response_line(
        &serde_json::json!("req-1"),
        serde_json::json!({"decision": "accept"}),
    );
    let parsed: serde_json::Value = serde_json::from_str(resp.trim()).unwrap();
    assert_eq!(parsed["id"], "req-1");
    assert_eq!(parsed["result"]["decision"], "accept");

    let err = error_response_line(&serde_json::json!(5), -32601, "nope");
    let parsed: serde_json::Value = serde_json::from_str(err.trim()).unwrap();
    assert_eq!(parsed["error"]["code"], -32601);
}

#[test]
fn approval_requests_parse_into_backend_shaped_tools() {
    let cmd = parse_approval_request(
        "item/commandExecution/requestApproval",
        &serde_json::json!({
            "threadId": "t", "turnId": "u", "itemId": "i9",
            "command": "sudo rm -rf /x", "cwd": "/wt", "startedAtMs": 1
        }),
    )
    .expect("command approval parses");
    assert_eq!(cmd.item_id, "i9");
    assert_eq!(cmd.tool_name, "command_execution");
    assert_eq!(cmd.input["command"], "sudo rm -rf /x");
    assert_eq!(cmd.input["cwd"], "/wt");

    let fc = parse_approval_request(
        "item/fileChange/requestApproval",
        &serde_json::json!({
            "threadId": "t", "turnId": "u", "itemId": "i10",
            "reason": "writes outside worktree", "grantRoot": "/etc", "startedAtMs": 1
        }),
    )
    .expect("file-change approval parses");
    assert_eq!(fc.tool_name, "file_change");
    assert_eq!(fc.input["reason"], "writes outside worktree");
    assert_eq!(fc.input["grant_root"], "/etc");

    assert!(
        parse_approval_request("mcpServer/elicitation/request", &serde_json::json!({})).is_none(),
        "unknown server requests must be declined with a JSON-RPC error, not bridged"
    );
}

/// The exec driver's `-c` overrides are DERIVED from the app-server config
/// JSON (one source, two encodings) — pin that every config key reaches the
/// flag form and the values stay TOML-parseable JSON literals.
#[test]
fn mcp_server_config_overrides_derive_from_the_json() {
    let json = crate::runtime::codex::lucidos_mcp_server_config_json();
    let overrides = crate::runtime::codex::lucidos_mcp_server_config_overrides();
    let keys = json.as_object().unwrap();
    assert_eq!(overrides.len(), keys.len());
    for (key, value) in keys {
        let expected = format!(
            "mcp_servers.lucidos.{key}={}",
            serde_json::to_string(value).unwrap()
        );
        assert!(
            overrides.contains(&expected),
            "missing derived override {expected}; got {overrides:?}"
        );
    }
    // The load-bearing values themselves.
    assert_eq!(json["command"], "lucidos");
    assert_eq!(json["enabled_tools"][0], "ask_user_question");
    assert_eq!(json["tool_timeout_sec"], 86400);
}
