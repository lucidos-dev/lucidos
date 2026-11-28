use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Write};

use crate::ask_user_question_hook::{card_not_shown_reason, post_question, QUIET_ANSWER_NOTE};
use crate::http::{permission_prompt_client, ConnectRetry};
use crate::workspace::{resolve_from_env, BoxError};

const TOOL_APPROVE: &str = "approve";
const TOOL_ASK_USER_QUESTION: &str = "ask_user_question";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolSet {
    All,
    PermissionOnly,
}

impl ToolSet {
    fn advertises(self, tool: &str) -> bool {
        match self {
            ToolSet::All => true,
            ToolSet::PermissionOnly => tool == TOOL_APPROVE,
        }
    }
}

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

pub fn run(tools: ToolSet) -> Result<(), BoxError> {
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

        if let Some(resp) = handle(&req, &client, &base_url, &thread_id, tools) {
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
    tools: ToolSet,
) -> Option<JsonRpcResponse> {
    let id = req.id.clone()?;
    let result = match req.method.as_str() {
        "initialize" => Ok(serde_json::json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "lucidos-perm", "version": env!("CARGO_PKG_VERSION") }
        })),
        "tools/list" => Ok(serde_json::json!({ "tools": tool_definitions(tools) })),
        "tools/call" => match req.params.get("name").and_then(|v| v.as_str()) {
            Some(TOOL_APPROVE) | None => call_approve(&req.params, client, base_url, thread_id),
            Some(TOOL_ASK_USER_QUESTION) if tools.advertises(TOOL_ASK_USER_QUESTION) => {
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

fn tool_definitions(tools: ToolSet) -> Vec<Value> {
    let mut out = Vec::new();
    if tools.advertises(TOOL_APPROVE) {
        out.push(serde_json::json!({
            "name": TOOL_APPROVE,
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
        }));
    }
    if tools.advertises(TOOL_ASK_USER_QUESTION) {
        out.push(serde_json::json!({
            "name": TOOL_ASK_USER_QUESTION,
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
                        "items": {
                            "anyOf": [
                                { "type": "string" },
                                {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" },
                                        "description": {
                                            "type": "string",
                                            "description": "Markdown under the label. A \
                                                picture or a widget embed draws here: \
                                                ![label](app:<id>?params={...})"
                                        },
                                        "preview": { "type": "string" },
                                        "quiet": {
                                            "type": "boolean",
                                            "description": "True for an option that \
                                                needs nothing more from you, such as \
                                                \"Leave it as is\" or \"Not now\". \
                                                Picking it ends your turn. Never for \
                                                an option that promises work, and \
                                                never on every option."
                                        },
                                        "quiet_reply": {
                                            "type": "string",
                                            "description": "Optional, quiet options \
                                                only: the fixed answer the thread \
                                                shows when this is picked, written \
                                                as your own reply."
                                        }
                                    },
                                    "required": ["label"]
                                }
                            ]
                        },
                        "description": "2-4 answers: a short label, or {label, description} \
                            to draw a picture or a widget in the option. Omit for free-text. \
                            NEVER include an \"Other\" / \"Something else\" / \"Let me type \
                            it\" label: there is no text-entry option, so tapping one just \
                            sends you that label back as the answer. The user can always \
                            type a reply in the prompt instead, or cancel the question."
                    },
                    "multi_select": {
                        "type": "boolean",
                        "description": "Allow picking several options"
                    }
                },
                "required": ["question"]
            }
        }));
    }
    out
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
        .send_with_connect_retry()
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
    session_id: &'a str,
    questions: Value,
}

#[derive(Deserialize)]
struct AskUserQuestionResponseBody {
    answers: Value,
    #[serde(default)]
    refusal: Option<String>,
    #[serde(default)]
    quiet: bool,
}

fn call_ask_user_question(
    params: &Value,
    client: &reqwest::blocking::Client,
    base_url: &str,
    thread_id: &str,
) -> Result<Value, String> {
    let endpoint = format!("{}/api/v1/internal/ask-user-question", base_url);
    let args = params.get("arguments").ok_or("missing arguments")?;
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
    let options = match parse_options(args.get("options")) {
        Ok(options) => options,
        Err(reason) => return Ok(tool_error_result(&reason)),
    };

    let questions = serde_json::json!([{
        "question": question,
        "header": "",
        "multiSelect": multi_select,
        "options": options,
    }]);

    let tool_use_id = format!("codex-q-{}", uuid::Uuid::new_v4());

    let body = AskUserQuestionRequestBody {
        thread_id,
        tool_use_id: &tool_use_id,
        session_id: "",
        questions,
    };
    Ok(
        match post_question::<AskUserQuestionResponseBody>(client, &endpoint, &body) {
            Ok(resp) => ask_tool_result(&resp, question),
            Err(cause) => card_not_shown_result(&cause),
        },
    )
}

fn card_not_shown_result(cause: &str) -> Value {
    tool_error_result(&card_not_shown_reason(TOOL_ASK_USER_QUESTION, cause))
}

fn tool_error_result(text: &str) -> Value {
    serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "isError": true
    })
}

fn parse_options(raw: Option<&Value>) -> Result<Vec<Value>, String> {
    let decoded;
    let entries = match raw {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::String(text)) if text.trim().is_empty() => return Ok(Vec::new()),
        Some(Value::Array(entries)) => entries,
        Some(Value::String(text)) => {
            decoded = serde_json::from_str::<Value>(text)
                .ok()
                .and_then(|value| match value {
                    Value::Array(entries) => Some(entries),
                    _ => None,
                })
                .ok_or(OPTIONS_SHAPE_ERROR)?;
            &decoded
        }
        Some(_) => return Err(OPTIONS_SHAPE_ERROR.to_string()),
    };
    entries
        .iter()
        .map(|entry| {
            engine_option(entry).ok_or_else(|| {
                format!("invalid option {entry}: each option needs a string `label`. {OPTIONS_SHAPE_ERROR}")
            })
        })
        .collect()
}

const OPTIONS_SHAPE_ERROR: &str =
    "`options` must be an array of labels or {label, description} objects, or omitted for a free-text question";

fn engine_option(option: &Value) -> Option<Value> {
    if let Some(label) = option.as_str() {
        return Some(serde_json::json!({ "label": label }));
    }
    let label = option.get("label")?.as_str()?;
    let mut out = serde_json::json!({ "label": label });
    for field in ["description", "preview"] {
        if let Some(text) = option.get(field).and_then(Value::as_str) {
            out[field] = Value::from(text);
        }
    }
    if option.get("quiet").and_then(Value::as_bool) == Some(true) {
        out["quiet"] = Value::Bool(true);
    }
    if let Some(reply) = option.get("quiet_reply").and_then(Value::as_str) {
        out["quiet_reply"] = Value::from(reply);
    }
    Some(out)
}

fn ask_tool_result(resp: &AskUserQuestionResponseBody, question: &str) -> Value {
    match &resp.refusal {
        Some(refusal) => tool_error_result(refusal),
        None => {
            let answer = extract_single_answer(&resp.answers, question);
            let text = if resp.quiet {
                format!("{answer}\n\n{QUIET_ANSWER_NOTE}")
            } else {
                answer
            };
            serde_json::json!({ "content": [{ "type": "text", "text": text }] })
        }
    }
}

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
    fn an_unreachable_engine_returns_retry_guidance_as_a_tool_error() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let params = serde_json::json!({
            "arguments": { "question": "Approve the plan?", "options": ["Approve", "Request changes"] }
        });
        let result = call_ask_user_question(
            &params,
            &dummy_client(),
            &format!("http://127.0.0.1:{port}"),
            "tid",
        )
        .expect("a failed request is still a tool result");
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("call ask_user_question again"), "{text}");
        assert!(text.contains("Do not ask in prose"), "{text}");
    }

    #[test]
    fn initialize_returns_protocol_version_and_server_info() {
        let req = JsonRpcRequest {
            id: Some(Value::from(1)),
            method: "initialize".to_string(),
            params: Value::Null,
        };
        let resp = handle(&req, &dummy_client(), "http://unused", "tid", ToolSet::All).unwrap();
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
        let resp = handle(&req, &dummy_client(), "http://unused", "tid", ToolSet::All).unwrap();
        let tools = &resp.result.unwrap()["tools"];
        assert_eq!(tools[0]["name"], "approve");
        assert_eq!(tools[0]["inputSchema"]["required"][0], "tool_name");
        assert_eq!(tools[1]["name"], "ask_user_question");
        assert_eq!(tools[1]["inputSchema"]["required"][0], "question");
        let kinds = &tools[1]["inputSchema"]["properties"]["options"]["items"]["anyOf"];
        assert_eq!(kinds[0]["type"], "string");
        assert_eq!(kinds[1]["type"], "object");
        assert!(kinds[1]["properties"]["description"]["description"]
            .as_str()
            .unwrap()
            .contains("app:<id>"));
    }

    #[test]
    fn permission_only_hides_the_question_tool_from_claude_code() {
        let req = JsonRpcRequest {
            id: Some(Value::from(2)),
            method: "tools/list".to_string(),
            params: Value::Null,
        };
        let resp = handle(
            &req,
            &dummy_client(),
            "http://unused",
            "tid",
            ToolSet::PermissionOnly,
        )
        .unwrap();
        let tools = resp.result.unwrap()["tools"].clone();
        let names: Vec<&str> = tools
            .as_array()
            .expect("tools must be an array")
            .iter()
            .map(|t| t["name"].as_str().expect("every tool is named"))
            .collect();
        assert_eq!(
            names,
            vec!["approve"],
            "CC must see the permission tool and nothing else"
        );
    }

    #[test]
    fn permission_only_refuses_a_question_tool_call() {
        let req = JsonRpcRequest {
            id: Some(Value::from(9)),
            method: "tools/call".to_string(),
            params: serde_json::json!({
                "name": "ask_user_question",
                "arguments": { "question": "Deploy now?" }
            }),
        };
        let resp = handle(
            &req,
            &dummy_client(),
            "http://unused",
            "tid",
            ToolSet::PermissionOnly,
        )
        .unwrap();
        assert!(resp.result.is_none());
        assert!(resp.error.unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("ask_user_question"));
    }

    #[test]
    fn ask_user_question_options_schema_bans_a_text_entry_escape_label() {
        let req = JsonRpcRequest {
            id: Some(Value::from(3)),
            method: "tools/list".to_string(),
            params: Value::Null,
        };
        let resp = handle(&req, &dummy_client(), "http://unused", "tid", ToolSet::All).unwrap();
        let options_desc = resp.result.unwrap()["tools"][1]["inputSchema"]["properties"]["options"]
            ["description"]
            .as_str()
            .expect("options must document itself")
            .to_string();
        for needle in ["NEVER include an \"Other\"", "no text-entry option"] {
            assert!(
                options_desc.contains(needle),
                "options description must ban the escape-hatch label (missing: {needle:?}): \
                 {options_desc}"
            );
        }
    }

    #[test]
    fn tools_call_with_unknown_name_returns_error() {
        let req = JsonRpcRequest {
            id: Some(Value::from(7)),
            method: "tools/call".to_string(),
            params: serde_json::json!({ "name": "frobnicate", "arguments": {} }),
        };
        let resp = handle(&req, &dummy_client(), "http://unused", "tid", ToolSet::All).unwrap();
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert!(
            err["message"].as_str().unwrap().contains("frobnicate"),
            "error must name the unknown tool"
        );
    }

    #[test]
    fn ask_user_question_requires_question_text() {
        let req = JsonRpcRequest {
            id: Some(Value::from(8)),
            method: "tools/call".to_string(),
            params: serde_json::json!({
                "name": "ask_user_question",
                "arguments": { "question": "   " }
            }),
        };
        let resp = handle(&req, &dummy_client(), "http://unused", "tid", ToolSet::All).unwrap();
        let err = resp.error.unwrap();
        assert!(err["message"].as_str().unwrap().contains("question"));
    }

    #[test]
    fn an_object_option_keeps_its_label_description_preview_quiet_and_reply() {
        let options: Vec<Value> = [
            serde_json::json!("Ship it"),
            serde_json::json!({ "label": "Not now", "quiet": true }),
            serde_json::json!({ "label": "Later", "quiet": false }),
            serde_json::json!({ "label": "Fine", "quiet": true, "quiet_reply": "All set." }),
            serde_json::json!({
                "label": "D",
                "description": "Split by a dot",
                "preview": "![D](artifacts/d.png)",
                "extra": 1
            }),
            serde_json::json!({ "description": "no label" }),
            serde_json::json!(7),
        ]
        .iter()
        .filter_map(engine_option)
        .collect();
        assert_eq!(
            options,
            vec![
                serde_json::json!({ "label": "Ship it" }),
                serde_json::json!({ "label": "Not now", "quiet": true }),
                serde_json::json!({ "label": "Later" }),
                serde_json::json!({ "label": "Fine", "quiet": true, "quiet_reply": "All set." }),
                serde_json::json!({
                    "label": "D",
                    "description": "Split by a dot",
                    "preview": "![D](artifacts/d.png)"
                }),
            ]
        );
    }

    #[test]
    fn options_absent_null_blank_or_empty_mean_a_free_text_question() {
        assert!(parse_options(None).unwrap().is_empty());
        for free_text in [
            Value::Null,
            serde_json::json!([]),
            serde_json::json!(""),
            serde_json::json!("  "),
            serde_json::json!("[]"),
        ] {
            assert!(
                parse_options(Some(&free_text)).unwrap().is_empty(),
                "{free_text}"
            );
        }
    }

    #[test]
    fn malformed_options_come_back_as_a_tool_error_the_model_reads() {
        let params = serde_json::json!({
            "arguments": { "question": "Approve the plan?", "options": "Approve, Request changes" }
        });
        let result = call_ask_user_question(&params, &dummy_client(), "http://127.0.0.1:9", "tid")
            .expect("a malformed call is still a tool result");
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("`options` must be an array"), "{text}");
    }

    #[test]
    fn a_json_encoded_options_array_still_yields_buttons() {
        let encoded = Value::from(r#"["Approve", {"label": "Request changes"}]"#);
        assert_eq!(
            parse_options(Some(&encoded)).unwrap(),
            vec![
                serde_json::json!({ "label": "Approve" }),
                serde_json::json!({ "label": "Request changes" }),
            ]
        );
    }

    #[test]
    fn malformed_options_are_refused_instead_of_dropped() {
        for bad in [
            serde_json::json!("Approve, Request changes"),
            serde_json::json!({ "label": "Approve" }),
            serde_json::json!(7),
            serde_json::json!(["Approve", 7]),
            serde_json::json!([{ "description": "no label" }]),
        ] {
            let err = parse_options(Some(&bad)).unwrap_err();
            assert!(err.contains("`options` must be an array"), "{bad}: {err}");
        }
    }

    #[test]
    fn extract_single_answer_prefers_exact_question_key() {
        let answers = serde_json::json!({ "Deploy now?": "Yes" });
        assert_eq!(extract_single_answer(&answers, "Deploy now?"), "Yes");
    }

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
    fn a_refused_card_is_an_error_result_carrying_the_reason() {
        let resp: AskUserQuestionResponseBody = serde_json::from_value(serde_json::json!({
            "questions": [],
            "answers": {},
            "refusal": "Question card not shown. Answer first."
        }))
        .unwrap();
        let result = ask_tool_result(&resp, "Deploy now?");
        assert_eq!(result["isError"], true);
        assert_eq!(
            result["content"][0]["text"],
            "Question card not shown. Answer first."
        );

        let answered: AskUserQuestionResponseBody =
            serde_json::from_value(serde_json::json!({ "answers": { "Deploy now?": "Yes" } }))
                .unwrap();
        let result = ask_tool_result(&answered, "Deploy now?");
        assert!(result.get("isError").is_none());
        assert_eq!(result["content"][0]["text"], "Yes");
    }

    #[test]
    fn a_quiet_answer_tells_the_agent_to_end_its_turn() {
        let resp: AskUserQuestionResponseBody = serde_json::from_value(serde_json::json!({
            "answers": { "Draft it?": "Leave it" },
            "quiet": true
        }))
        .unwrap();
        let result = ask_tool_result(&resp, "Draft it?");
        assert_eq!(
            result["content"][0]["text"],
            format!("Leave it\n\n{QUIET_ANSWER_NOTE}")
        );
    }

    #[test]
    fn notifications_produce_no_response() {
        let req = JsonRpcRequest {
            id: None,
            method: "notifications/initialized".to_string(),
            params: Value::Null,
        };
        assert!(handle(&req, &dummy_client(), "http://unused", "tid", ToolSet::All).is_none());
    }

    #[test]
    fn unknown_method_returns_error() {
        let req = JsonRpcRequest {
            id: Some(Value::from(3)),
            method: "totally/unknown".to_string(),
            params: Value::Null,
        };
        let resp = handle(&req, &dummy_client(), "http://unused", "tid", ToolSet::All).unwrap();
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err["code"], -32603);
    }
}
