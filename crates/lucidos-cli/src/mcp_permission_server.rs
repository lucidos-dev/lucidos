//! MCP stdio server shared by both coding-agent backends.
//!
//! Claude Code spawns it as `--permission-prompt-tool mcp__lucidos_perm__approve`
//! (server name `lucidos_perm`): each `approve` call forwards the permission
//! request to the parent engine's `/api/v1/internal/permission-prompt` endpoint
//! and returns the user's decision as an MCP `text` content block.
//!
//! Codex spawns it as MCP server `lucidos` with `enabled_tools =
//! ["ask_user_question"]` (see `runtime/codex.rs::build_codex_turn_command`):
//! each `ask_user_question` call forwards the question to
//! `/api/v1/internal/ask-user-question` — the same blocking endpoint CC's
//! PreToolUse hook uses — and returns the user's answer as the tool result, so
//! the Codex turn continues in place once the user clicks the QuestionCard.

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
    let base_url = workspace.base_url();

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

        if let Some(resp) = handle(&req, &client, &base_url, &thread_id) {
            writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn handle(
    req: &JsonRpcRequest,
    client: &reqwest::blocking::Client,
    base_url: &str,
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
            }, {
                "name": "ask_user_question",
                "description": "Ask the Lucidos user one question and block until they answer. \
                    The Lucidos UI renders the options as clickable buttons. Use whenever you \
                    need the user's decision instead of guessing.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "question": {
                            "type": "string",
                            "description": "Full question text shown on the card"
                        },
                        "options": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "2-4 short answer labels; omit for free-text"
                        },
                        "multi_select": {
                            "type": "boolean",
                            "description": "Allow picking several options"
                        }
                    },
                    "required": ["question"]
                }
            }]
        })),
        "tools/call" => match req.params.get("name").and_then(|v| v.as_str()) {
            // CC's --permission-prompt-tool designation strips the server/tool
            // prefix before dispatch, so legacy callers arrive with no name —
            // route those to approve for back-compat.
            Some("approve") | None => call_approve(&req.params, client, base_url, thread_id),
            Some("ask_user_question") => {
                call_ask_user_question(&req.params, client, base_url, thread_id)
            }
            Some(other) => Err(format!("Unknown tool: {}", other)),
        },
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
    base_url: &str,
    thread_id: &str,
) -> Result<Value, String> {
    let endpoint = format!("{}/api/v1/internal/permission-prompt", base_url);
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
        .post(&endpoint)
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

#[derive(Serialize)]
struct AskUserQuestionRequestBody<'a> {
    thread_id: &'a str,
    tool_use_id: &'a str,
    /// The coding-agent session id paired with the question. The MCP server
    /// never learns the Codex thread id (the MCP protocol doesn't carry it),
    /// so this is empty for Codex-originated questions. The field is
    /// write-only on the engine side — recovery resumes via
    /// `CodingAgentIdled` / `CodingAgentSettingsChanged`, never this event.
    session_id: &'a str,
    questions: Value,
}

#[derive(Deserialize)]
struct AskUserQuestionResponseBody {
    answers: Value,
}

/// Translate one MCP `ask_user_question` call into the engine's question-walk
/// shape (the same `/api/v1/internal/ask-user-question` endpoint CC's
/// PreToolUse hook uses), block until the user answers, and return the answer
/// as the tool result. One question per call — the model calls again for the
/// next question.
fn call_ask_user_question(
    params: &Value,
    client: &reqwest::blocking::Client,
    base_url: &str,
    thread_id: &str,
) -> Result<Value, String> {
    let endpoint = format!("{}/api/v1/internal/ask-user-question", base_url);
    let args = params.get("arguments").ok_or("missing arguments")?;
    // Trimmed BEFORE sending: the engine keys its `{question: answer}` map
    // on the trimmed question text (`question_text` in the engine's
    // parser), so an untrimmed key here (models love trailing newlines)
    // would miss the lookup below and degrade the tool result to the
    // serialized whole map.
    let question = args
        .get("question")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or("missing question")?;
    let multi_select = args
        .get("multi_select")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let options: Vec<Value> = args
        .get("options")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|label| serde_json::json!({ "label": label, "description": "" }))
                .collect()
        })
        .unwrap_or_default();

    // Engine wire shape — same as CC's `tool_input.questions` (the engine's
    // `parse_ask_user_question_inputs` reads `question` / `options[].label` /
    // `multiSelect`).
    let questions = serde_json::json!([{
        "question": question,
        "header": "",
        "multiSelect": multi_select,
        "options": options,
    }]);

    // Fresh id per call: there is no stable Codex-side tool_use_id visible to
    // an MCP server, so each ask is its own question. A codex child killed
    // mid-question therefore re-asks on resume instead of replaying a stale
    // answer — acceptable; the user answers once more.
    let tool_use_id = format!("codex-q-{}", uuid::Uuid::new_v4());

    let body = AskUserQuestionRequestBody {
        thread_id,
        tool_use_id: &tool_use_id,
        session_id: "",
        questions,
    };
    let resp = client
        .post(&endpoint)
        .json(&body)
        .send()
        .map_err(|e| format!("HTTP failed: {}", e))?
        .error_for_status()
        .map_err(|e| format!("HTTP status: {}", e))?
        .json::<AskUserQuestionResponseBody>()
        .map_err(|e| format!("HTTP body parse: {}", e))?;

    let answer_text = extract_single_answer(&resp.answers, question);
    Ok(serde_json::json!({
        "content": [{ "type": "text", "text": answer_text }]
    }))
}

/// Pull the single question's answer out of the engine's
/// `{question_text: answer}` map. Falls back to the whole map serialized as
/// JSON if the key is missing (defensive — the engine keys the map on the
/// exact question text we sent).
fn extract_single_answer(answers: &Value, question: &str) -> String {
    match answers.get(question) {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => answers.to_string(),
    }
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
    fn tools_list_advertises_approve_and_ask_user_question() {
        let req = JsonRpcRequest {
            id: Some(Value::from(2)),
            method: "tools/list".to_string(),
            params: Value::Null,
        };
        let resp = handle(&req, &dummy_client(), "http://unused", "tid").unwrap();
        let tools = &resp.result.unwrap()["tools"];
        assert_eq!(tools[0]["name"], "approve");
        assert_eq!(tools[0]["inputSchema"]["required"][0], "tool_name");
        // Codex spawns this server with `enabled_tools = ["ask_user_question"]`
        // — the tool must be advertised or the filter yields an empty tool set
        // and Codex silently loses its question path.
        assert_eq!(tools[1]["name"], "ask_user_question");
        assert_eq!(tools[1]["inputSchema"]["required"][0], "question");
    }

    #[test]
    fn tools_call_with_unknown_name_returns_error() {
        let req = JsonRpcRequest {
            id: Some(Value::from(7)),
            method: "tools/call".to_string(),
            params: serde_json::json!({ "name": "frobnicate", "arguments": {} }),
        };
        let resp = handle(&req, &dummy_client(), "http://unused", "tid").unwrap();
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert!(
            err["message"].as_str().unwrap().contains("frobnicate"),
            "error must name the unknown tool"
        );
    }

    #[test]
    fn ask_user_question_requires_question_text() {
        // The engine rejects empty question text server-side; failing fast
        // here keeps the error close to the model instead of a 500 round-trip.
        let req = JsonRpcRequest {
            id: Some(Value::from(8)),
            method: "tools/call".to_string(),
            params: serde_json::json!({
                "name": "ask_user_question",
                "arguments": { "question": "   " }
            }),
        };
        let resp = handle(&req, &dummy_client(), "http://unused", "tid").unwrap();
        let err = resp.error.unwrap();
        assert!(err["message"].as_str().unwrap().contains("question"));
    }

    #[test]
    fn extract_single_answer_prefers_exact_question_key() {
        let answers = serde_json::json!({ "Deploy now?": "Yes" });
        assert_eq!(extract_single_answer(&answers, "Deploy now?"), "Yes");
    }

    /// The engine keys its answers map on the TRIMMED question text — the
    /// tool must send a trimmed question or the lookup degrades to the
    /// whole-map fallback for any model-emitted trailing newline.
    #[test]
    fn ask_user_question_trims_question_before_use() {
        let args = serde_json::json!({ "question": "  Deploy now?\n" });
        let q = args
            .get("question")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .unwrap();
        let answers = serde_json::json!({ "Deploy now?": "Yes" });
        assert_eq!(extract_single_answer(&answers, q), "Yes");
    }

    #[test]
    fn extract_single_answer_falls_back_to_full_map_on_key_miss() {
        let answers = serde_json::json!({ "Other question": "Maybe" });
        let text = extract_single_answer(&answers, "Deploy now?");
        assert!(
            text.contains("Other question") && text.contains("Maybe"),
            "fallback must surface the whole map so the model still sees the answer: {text}"
        );
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
