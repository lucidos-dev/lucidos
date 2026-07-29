//! PreToolUse hook for Claude Code's AskUserQuestion tool.

use serde::{Deserialize, Serialize};
use std::io::Read;

use crate::http::permission_prompt_client;
use crate::workspace::{resolve_from_env, BoxError};

#[derive(Debug, Deserialize)]
pub(crate) struct HookPayload {
    pub(crate) session_id: String,
    pub(crate) tool_use_id: String,
    pub(crate) tool_input: ToolInput,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolInput {
    pub(crate) questions: Vec<HookQuestion>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct HookQuestion {
    pub(crate) question: String,
    pub(crate) header: String,
    #[serde(rename = "multiSelect")]
    pub(crate) multi_select: bool,
    pub(crate) options: Vec<HookOption>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct HookOption {
    pub(crate) label: String,
    pub(crate) description: String,
}

pub(crate) fn parse_hook_payload(raw: &str) -> Result<HookPayload, BoxError> {
    serde_json::from_str(raw).map_err(Into::into)
}

#[derive(Serialize)]
struct AskUserQuestionRequestBody<'a> {
    thread_id: &'a str,
    tool_use_id: &'a str,
    session_id: &'a str,
    questions: serde_json::Value,
}

#[derive(Deserialize)]
struct AskUserQuestionResponseBody {
    questions: serde_json::Value,
    answers: serde_json::Value,
}

pub(crate) fn run() -> Result<(), BoxError> {
    let workspace = resolve_from_env()?;
    let thread_id = std::env::var("LUCIDOS_THREAD_ID")
        .map_err(|_| "LUCIDOS_THREAD_ID env var required for ask-user-question-hook")?;

    let mut stdin_buf = String::new();
    std::io::stdin().read_to_string(&mut stdin_buf)?;
    let payload = parse_hook_payload(&stdin_buf)?;

    let questions_json = serde_json::to_value(&payload.tool_input.questions)?;
    let body = AskUserQuestionRequestBody {
        thread_id: &thread_id,
        tool_use_id: &payload.tool_use_id,
        session_id: &payload.session_id,
        questions: questions_json,
    };

    let endpoint = format!("{}/api/v1/internal/ask-user-question", workspace.base_url());
    let resp: AskUserQuestionResponseBody = permission_prompt_client()?
        .post(&endpoint)
        .json(&body)
        .send()
        .map_err(|e| format!("hook HTTP failed: {}", e))?
        .error_for_status()
        .map_err(|e| format!("hook HTTP status: {}", e))?
        .json()
        .map_err(|e| format!("hook response parse: {}", e))?;

    let output = build_hook_output(&resp.questions, &resp.answers);
    println!("{output}");
    Ok(())
}

/// Build the JSON Claude Code expects on a PreToolUse hook's stdout when the
/// hook satisfies the tool itself: `permissionDecision: allow` plus
/// `updatedInput` carrying the synthesized answers (CC then constructs a
/// matching `tool_result` for its session). Echoes the questions array
/// verbatim alongside the answers — both fields are required.
fn build_hook_output(
    questions: &serde_json::Value,
    answers: &serde_json::Value,
) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": {
                "questions": questions,
                "answers": answers,
            }
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hook_payload_extracts_questions_and_tool_use_id() {
        let raw = r#"{
            "session_id": "sid-1",
            "tool_use_id": "toolu_abc",
            "tool_input": {
                "questions": [{
                    "question": "What is your favorite color?",
                    "header": "Fav color",
                    "multiSelect": false,
                    "options": [
                        {"label": "Red", "description": "warm"},
                        {"label": "Blue", "description": "cool"}
                    ]
                }]
            }
        }"#;
        let parsed = parse_hook_payload(raw).expect("valid payload");
        assert_eq!(parsed.tool_use_id, "toolu_abc");
        assert_eq!(parsed.session_id, "sid-1");
        assert_eq!(parsed.tool_input.questions.len(), 1);
        assert_eq!(parsed.tool_input.questions[0].question, "What is your favorite color?");
    }

    #[test]
    fn build_hook_output_echoes_questions_and_includes_answers() {
        let questions = serde_json::json!([{
            "question": "Q1?",
            "header": "h1",
            "multiSelect": false,
            "options": [{"label":"A","description":""}]
        }]);
        let answers = serde_json::json!({"Q1?": "A"});
        let output = build_hook_output(&questions, &answers);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(parsed["hookSpecificOutput"]["updatedInput"]["questions"], questions);
        assert_eq!(parsed["hookSpecificOutput"]["updatedInput"]["answers"], answers);
    }
}
