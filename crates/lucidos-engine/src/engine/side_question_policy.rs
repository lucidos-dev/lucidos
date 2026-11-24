//! Shared limits, instructions and failure messages for side-question runners.

/// How long a side question may take, on any agent. A Claude Code copy
/// resumes the whole transcript before its model call starts.
pub const SIDE_QUESTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Model turns a side question may take, on any agent. A refused tool call
/// costs one, so this leaves room to answer after a stray attempt.
pub const SIDE_QUESTION_MAX_TURNS: usize = 3;

/// What the model reads before the question, on any agent. The Claude Code
/// copy resumes a saved transcript, whose running step reads as interrupted.
pub const SIDE_QUESTION_INSTRUCTIONS: &str = "This is a side question from the user, \
asked beside the main conversation. Answer it directly in one reply, from what you already \
know. You cannot use tools here: every tool call is refused. Do not continue the main task \
and do not promise any action. If the answer needs a tool, say so. Then suggest asking in the \
main conversation. The main task may still be running: a last step that reads as interrupted \
was not stopped.";

/// What the model reads when it calls a tool anyway, on any agent.
pub const SIDE_QUESTION_TOOL_REFUSAL: &str =
    "Side questions cannot use tools. Answer from what you already know.";

/// What the asker reads when every turn reached for a tool.
pub fn kept_reaching_for_tools(agent: &str) -> String {
    format!("{agent} kept reaching for tools instead of answering. Ask it in the main conversation instead.")
}

/// What the asker reads when a side question runs past its deadline.
pub fn side_question_timeout_message(agent: &str) -> String {
    format!(
        "{agent} did not answer within {} seconds",
        SIDE_QUESTION_TIMEOUT.as_secs()
    )
}
