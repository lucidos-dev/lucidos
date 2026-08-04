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
        params["config"]["mcp_servers"]["lucidos"]["enabled_tools"][0], "ask_user_question",
        "the question tool must ride the app-server protocol too"
    );
    assert_eq!(
        params["config"]["mcp_servers"]["lucidos"]["tools"]["ask_user_question"]["approval_mode"],
        "approve",
        "Codex otherwise rejects/cancels the MCP question call before Lucidos can render the card"
    );
    assert_eq!(
        params["config"]["mcp_servers"]["lucidos"]["env"]["LUCIDOS_THREAD_ID"],
        "00000000-0000-0000-0000-000000000123",
        "Codex does not inherit app-server env into stdio MCP children; without \
         explicit LUCIDOS_THREAD_ID the lucidos MCP server exits before initialize"
    );
    assert_eq!(
        params["config"]["mcp_servers"]["lucidos"]["env"]["LUCIDOS_WORKSPACE"],
        "/ws"
    );
    assert!(params.get("threadId").is_none());
    assert!(params.get("model").is_none(), "no model param when unset");
    assert_eq!(
        params["config"]["model_reasoning_summary"], "detailed",
        "codex's default summary mode emits NO reasoning notifications (verified \
         live on 0.142.5) — without this the Thinking step never renders"
    );
    assert_eq!(
        params["config"]["project_doc_fallback_filenames"][0], "CLAUDE.md",
        "no AGENTS.md ships (ADR 0004), so CLAUDE.md is the project doc Codex \
         must fall back to — CC parity for the repo working agreement"
    );
    assert_eq!(
        params["config"]["project_doc_max_bytes"], 65536,
        "codex's 32KiB default would truncate Lucidos' ~29KiB CLAUDE.md soon"
    );
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
fn sandbox_writable_roots_reach_the_thread_request() {
    // The app-server analog of the exec driver's --add-dir. Both entries are
    // load-bearing: without the shared git dir the workspace-write sandbox
    // blocks every `git commit` in a linked worktree, and without the
    // workspace's data/ it blocks a direct write into the parent workspace's
    // data/ tree (`lucidos data path --mkdir`, an editor write to a resolved
    // data path). That is the 2026-07-26 nightly's EPERM, which lost two
    // security findings back when `lucidos data write` wrote the file itself;
    // that command now PUTs to the engine and needs no writable root.
    let mut config = test_config();
    config.sandbox_writable_roots = vec![PathBuf::from("/repo/.git"), PathBuf::from("/ws/data")];
    let (_, params) = build_thread_request(&config, None);
    assert_eq!(
        params["config"]["sandbox_workspace_write"]["writable_roots"],
        serde_json::json!(["/repo/.git", "/ws/data"])
    );
}

#[test]
fn writable_roots_is_omitted_when_there_are_none() {
    // An empty list must not serialize as `writable_roots: []` — that reads as
    // "explicitly no extra roots" to codex rather than "unset", and it is the
    // shape the request had before the roots existed.
    let config = test_config();
    assert!(config.sandbox_writable_roots.is_empty());
    let (_, params) = build_thread_request(&config, None);
    assert!(params["config"]["sandbox_workspace_write"]
        .get("writable_roots")
        .is_none());
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
fn turn_start_params_scope_max_effort_to_gpt_5_6() {
    let params = build_turn_start_params("t-1", "go", &[], Some("gpt-5.6-luna"), Some("max"));
    assert_eq!(params["effort"], "max", "GPT-5.6 models support Max");

    // Older models reject Max. A stale selection is dropped so Codex applies
    // its own default instead of failing the whole turn.
    let params = build_turn_start_params("t-1", "go", &[], Some("gpt-5.5"), Some("max"));
    assert!(
        params.get("effort").is_none(),
        "Max must be omitted for pre-5.6 models"
    );

    let params = build_turn_start_params("t-1", "go", &[], None, Some("max"));
    assert!(
        params.get("effort").is_none(),
        "Max must be omitted when the default model is unknown"
    );

    let params = build_turn_start_params("t-1", "go", &[], None, Some("xhigh"));
    assert_eq!(params["effort"], "xhigh", "vocab values still pass through");
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
/// flag form. Most values are JSON-compatible TOML; env is rendered as a TOML
/// inline table because Codex rejects JSON object syntax for that key.
#[test]
fn mcp_server_config_overrides_derive_from_the_json() {
    let config = test_config();
    let json = crate::runtime::codex::lucidos_mcp_server_config_json(&config.env);
    let overrides = crate::runtime::codex::lucidos_mcp_server_config_overrides(&config.env);
    let keys = json.as_object().unwrap();
    assert_eq!(overrides.len(), keys.len());
    for (key, value) in keys {
        if key == "env" {
            let found = overrides
                .iter()
                .find(|o| o.starts_with("mcp_servers.lucidos.env={"))
                .expect("env override present");
            assert!(found.contains("LUCIDOS_WORKSPACE"));
            assert!(found.contains("LUCIDOS_THREAD_ID"));
            assert!(found.contains("LUCIDOS_API_BASE_URL"));
        } else {
            let expected = format!(
                "mcp_servers.lucidos.{key}={}",
                serde_json::to_string(value).unwrap()
            );
            assert!(
                overrides.contains(&expected),
                "missing derived override {expected}; got {overrides:?}"
            );
        }
    }
    // The load-bearing values themselves.
    assert_eq!(json["command"], "lucidos");
    assert_eq!(json["env"]["LUCIDOS_WORKSPACE"], "/ws");
    assert_eq!(
        json["env"]["LUCIDOS_THREAD_ID"],
        "00000000-0000-0000-0000-000000000123"
    );
    assert_eq!(json["enabled_tools"][0], "ask_user_question");
    assert_eq!(
        json["tools"]["ask_user_question"]["approval_mode"], "approve",
        "non-interactive Codex sessions must trust the Lucidos question tool"
    );
    assert_eq!(json["tool_timeout_sec"], 86400);
}
