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

/// The questions stay raw JSON: the engine's `parse_ask_user_question_inputs`
/// owns their schema. A typed copy here dropped every field it did not name,
/// which is how option pictures (`preview`) never reached the card.
#[derive(Debug, Deserialize)]
pub(crate) struct ToolInput {
    pub(crate) questions: serde_json::Value,
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
    /// Set when the engine refused the card, which the user never saw.
    #[serde(default)]
    refusal: Option<String>,
}

pub(crate) fn run() -> Result<(), BoxError> {
    let workspace = resolve_from_env()?;
    let thread_id = std::env::var("LUCIDOS_THREAD_ID")
        .map_err(|_| "LUCIDOS_THREAD_ID env var required for ask-user-question-hook")?;

    let mut stdin_buf = String::new();
    std::io::stdin().read_to_string(&mut stdin_buf)?;
    let payload = parse_hook_payload(&stdin_buf)?;

    let body = AskUserQuestionRequestBody {
        thread_id: &thread_id,
        tool_use_id: &payload.tool_use_id,
        session_id: &payload.session_id,
        questions: payload.tool_input.questions,
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

    let output = match &resp.refusal {
        Some(reason) => build_refusal_output(reason),
        None => build_hook_output(&resp.questions, &resp.answers),
    };
    println!("{output}");
    Ok(())
}

/// Deny the tool call: Claude Code hands the reason to the model as the tool
/// result, and no card was shown.
fn build_refusal_output(reason: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

/// Build the JSON Claude Code expects on a PreToolUse hook's stdout when the
/// hook satisfies the tool itself: `permissionDecision: allow` plus
/// `updatedInput` carrying the synthesized answers (CC then constructs a
/// matching `tool_result` for its session). Echoes the questions array
/// verbatim alongside the answers — both fields are required.
fn build_hook_output(questions: &serde_json::Value, answers: &serde_json::Value) -> String {
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
        assert_eq!(
            parsed.tool_input.questions.as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            parsed.tool_input.questions[0]["question"],
            "What is your favorite color?"
        );
    }

    /// A session put a picture in every option's `preview`, and the card showed
    /// none: this hook re-typed each option as label plus description and
    /// dropped the rest. The engine owns the question schema, so the hook
    /// forwards the questions exactly as Claude Code sent them.
    #[test]
    fn parse_hook_payload_keeps_every_option_field_for_the_engine() {
        let raw = r#"{
            "session_id": "sid-1",
            "tool_use_id": "toolu_abc",
            "tool_input": {
                "questions": [{
                    "question": "Which one?",
                    "multiSelect": false,
                    "options": [{
                        "label": "Thinner lines",
                        "description": "calmest",
                        "preview": "![Thinner lines](artifacts/thin.png)"
                    }]
                }]
            }
        }"#;
        let parsed = parse_hook_payload(raw).expect("valid payload");
        assert_eq!(
            parsed.tool_input.questions[0]["options"][0]["preview"],
            "![Thinner lines](artifacts/thin.png)"
        );
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
        assert_eq!(
            parsed["hookSpecificOutput"]["updatedInput"]["questions"],
            questions
        );
        assert_eq!(
            parsed["hookSpecificOutput"]["updatedInput"]["answers"],
            answers
        );
    }

    #[test]
    fn a_refusal_denies_the_call_with_the_reason() {
        let parsed: serde_json::Value =
            serde_json::from_str(&build_refusal_output("Question card not shown.")).unwrap();
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecisionReason"],
            "Question card not shown."
        );
        assert!(parsed["hookSpecificOutput"].get("updatedInput").is_none());
    }
}
