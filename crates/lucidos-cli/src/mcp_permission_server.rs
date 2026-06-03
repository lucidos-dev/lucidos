//! MCP stdio server invoked by CC as `--permission-prompt-tool mcp__lucidos_perm__approve`.
//! Forwards each permission request to the parent engine's
//! `/api/v1/internal/permission-prompt` endpoint and returns the user's decision
//! (or denial on timeout/error) as an MCP `text` content block.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Write};

use crate::http::permission_prompt_client;
use crate::workspace::{resolve_from_env, BoxError};

#[derive(Deserialize)]
struct JsonRpcRequest {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

#[derive(Serialize)]
struct PermissionRequestBody<'a> {
    thread_id: &'a str,
    tool_use_id: &'a str,
    tool_name: &'a str,
    input: &'a Value,
}

#[derive(Deserialize)]
struct PermissionResponseBody {
    allowed: bool,
    #[serde(default)]
    reason: Option<String>,
}

pub fn run() -> Result<(), BoxError> {
    let workspace = resolve_from_env()?;
    let thread_id = std::env::var("LUCIDOS_THREAD_ID")
        .map_err(|_| "LUCIDOS_THREAD_ID env var required for permission-prompt server")?;
    let endpoint = format!("{}/api/v1/internal/permission-prompt", workspace.base_url());

    let client = permission_prompt_client()?;

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                // Reply with a JSON-RPC parse error so the client doesn't hang
                // waiting for a response. id=null per spec when the request id
                // can't be recovered.
                eprintln!("[lucidos-mcp-perm] bad jsonrpc: {}", e);
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0",
                    id: serde_json::Value::Null,
                    result: None,
                    error: Some(serde_json::json!({
                        "code": -32700,
                        "message": format!("Parse error: {}", e),
                    })),
                };
                writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
                stdout.flush()?;
                continue;
            }
        };

        if let Some(resp) = handle(&req, &client, &endpoint, &thread_id) {
            writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn handle(
    req: &JsonRpcRequest,
    client: &reqwest::blocking::Client,
    endpoint: &str,
    thread_id: &str,
) -> Option<JsonRpcResponse> {
    let id = req.id.clone()?;
    let result = match req.method.as_str() {
        "initialize" => Ok(serde_json::json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "lucidos-perm", "version": env!("CARGO_PKG_VERSION") }
        })),
        "tools/list" => Ok(serde_json::json!({
            "tools": [{
                "name": "approve",
                "description": "Surfaces a permission prompt to the Lucidos user",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "tool_name": { "type": "string" },
                        "input": { "type": "object" },
                        "tool_use_id": { "type": "string" }
                    },
                    "required": ["tool_name", "input", "tool_use_id"]
                }
            }]
        })),
        "tools/call" => call_approve(&req.params, client, endpoint, thread_id),
        other => Err(format!("Method not supported: {}", other)),
    };

    Some(match result {
        Ok(r) => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(r),
            error: None,
        },
        Err(msg) => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(serde_json::json!({ "code": -32603, "message": msg })),
        },
    })
}

fn call_approve(
    params: &Value,
    client: &reqwest::blocking::Client,
    endpoint: &str,
    thread_id: &str,
) -> Result<Value, String> {
    let args = params.get("arguments").ok_or("missing arguments")?;
    let tool_use_id = args
        .get("tool_use_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_name = args
        .get("tool_name")
        .and_then(|v| v.as_str())
        .ok_or("missing tool_name")?;
    let input = args.get("input").unwrap_or(&Value::Null);

    let body = PermissionRequestBody {
        thread_id,
        tool_use_id,
        tool_name,
        input,
    };
    let resp = client
        .post(endpoint)
        .json(&body)
        .send()
        .map_err(|e| format!("HTTP failed: {}", e))?
        .error_for_status()
        .map_err(|e| format!("HTTP status: {}", e))?
        .json::<PermissionResponseBody>()
        .map_err(|e| format!("HTTP body parse: {}", e))?;

    let payload = if resp.allowed {
        serde_json::json!({
            "behavior": "allow",
            "updatedInput": input,
        })
    } else {
        serde_json::json!({
            "behavior": "deny",
            "message": resp
                .reason
                .unwrap_or_else(|| "User denied this permission request".to_string()),
        })
    };

    Ok(serde_json::json!({
        "content": [{ "type": "text", "text": payload.to_string() }]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_client() -> reqwest::blocking::Client {
        reqwest::blocking::Client::new()
    }

    #[test]
    fn initialize_returns_protocol_version_and_server_info() {
        let req = JsonRpcRequest {
            id: Some(Value::from(1)),
            method: "initialize".to_string(),
            params: Value::Null,
        };
        let resp = handle(&req, &dummy_client(), "http://unused", "tid").unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2025-06-18");
        assert_eq!(result["serverInfo"]["name"], "lucidos-perm");
    }

    #[test]
    fn tools_list_advertises_approve() {
        let req = JsonRpcRequest {
            id: Some(Value::from(2)),
            method: "tools/list".to_string(),
            params: Value::Null,
        };
        let resp = handle(&req, &dummy_client(), "http://unused", "tid").unwrap();
        let tools = &resp.result.unwrap()["tools"];
        assert_eq!(tools[0]["name"], "approve");
        assert_eq!(tools[0]["inputSchema"]["required"][0], "tool_name");
    }

    #[test]
    fn notifications_produce_no_response() {
        let req = JsonRpcRequest {
            id: None,
            method: "notifications/initialized".to_string(),
            params: Value::Null,
        };
        assert!(handle(&req, &dummy_client(), "http://unused", "tid").is_none());
    }

    #[test]
    fn unknown_method_returns_error() {
        let req = JsonRpcRequest {
            id: Some(Value::from(3)),
            method: "totally/unknown".to_string(),
            params: Value::Null,
        };
        let resp = handle(&req, &dummy_client(), "http://unused", "tid").unwrap();
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err["code"], -32603);
    }
}
