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

/// Always exits 0 with a decision on stdout. Claude Code treats any other exit
/// as a non-blocking error and runs its native tool. In a headless session that
/// tool answers "the user did not answer" at once, and the agent asks in prose.
pub(crate) fn run() -> Result<(), BoxError> {
    println!("{}", output_for(ask()));
    Ok(())
}

fn output_for(asked: Result<AskUserQuestionResponseBody, BoxError>) -> String {
    match asked {
        Ok(AskUserQuestionResponseBody {
            refusal: Some(reason),
            ..
        }) => build_refusal_output(&reason),
        Ok(resp) => build_hook_output(&resp.questions, &resp.answers),
        Err(e) => build_refusal_output(&card_not_shown_reason("AskUserQuestion", &e.to_string())),
    }
}

/// The tool result for a question that got no answer back from the engine.
/// Shared with the Codex `ask_user_question` MCP tool.
pub(crate) fn card_not_shown_reason(tool: &str, cause: &str) -> String {
    format!(
        "The question tool failed, and no answer came back ({cause}). \
         Do not ask in prose instead: a question in your reply never reaches the user as a card. \
         Fix what the error names, wait about 30 seconds, then call {tool} again. \
         If the same error comes back twice, stop and tell the user the question tool is failing."
    )
}

/// POST a question to the engine and decode its answer. A non-2xx keeps the
/// engine's error text, which often says how to fix the question.
pub(crate) fn post_question<R: serde::de::DeserializeOwned>(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    body: &impl Serialize,
) -> Result<R, String> {
    let resp = client
        .post(endpoint)
        .json(body)
        .send()
        .map_err(|e| format!("request to the engine failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp
            .text()
            .unwrap_or_else(|e| format!("body unreadable: {e}"));
        return Err(format!("engine answered {status}: {}", text.trim()));
    }
    resp.json()
        .map_err(|e| format!("engine answer unreadable: {e}"))
}

fn ask() -> Result<AskUserQuestionResponseBody, BoxError> {
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
    Ok(post_question(
        &permission_prompt_client()?,
        &endpoint,
        &body,
    )?)
}

/// Deny the tool call: Claude Code hands the reason to the model as the tool
/// result.
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

    /// A failed request must deny the call, never fall through to Claude Code's
    /// native tool.
    #[test]
    fn a_failed_request_denies_the_call_instead_of_falling_through() {
        let output = output_for(Err("engine answered 500 Internal Server Error".into()));
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let hook = &parsed["hookSpecificOutput"];
        assert_eq!(hook["permissionDecision"], "deny");
        let reason = hook["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("500 Internal Server Error"), "{reason}");
        assert!(reason.contains("call AskUserQuestion again"), "{reason}");
        assert!(reason.contains("Do not ask in prose"), "{reason}");
        assert!(reason.contains("same error comes back twice"), "{reason}");
    }

    /// The engine's error text often says how to fix the question, so a
    /// non-2xx must carry it to the agent.
    #[test]
    fn post_question_keeps_the_engine_error_text() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            let body = "fill in the question text";
            let _ = write!(
                stream,
                "HTTP/1.1 500 Internal Server Error\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        });
        let err = post_question::<serde_json::Value>(
            &reqwest::blocking::Client::new(),
            &format!("http://127.0.0.1:{port}/x"),
            &serde_json::json!({}),
        )
        .expect_err("a 500 is an error");
        assert!(err.contains("500"), "{err}");
        assert!(err.contains("fill in the question text"), "{err}");
    }

    #[test]
    fn an_answer_allows_the_call_and_an_engine_refusal_denies_it() {
        let answered = output_for(Ok(AskUserQuestionResponseBody {
            questions: serde_json::json!([]),
            answers: serde_json::json!({"Q?": "A"}),
            refusal: None,
        }));
        let parsed: serde_json::Value = serde_json::from_str(&answered).unwrap();
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "allow");

        let refused = output_for(Ok(AskUserQuestionResponseBody {
            questions: serde_json::json!([]),
            answers: serde_json::json!({}),
            refusal: Some("Question card not shown.".into()),
        }));
        let parsed: serde_json::Value = serde_json::from_str(&refused).unwrap();
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecisionReason"],
            "Question card not shown."
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
