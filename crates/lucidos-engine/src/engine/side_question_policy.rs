pub const SIDE_QUESTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

pub const SIDE_QUESTION_MAX_TURNS: usize = 3;

pub const SIDE_QUESTION_INSTRUCTIONS: &str = "This is a side question from the user, \
asked beside the main conversation. Answer it directly in one reply, from what you already \
know. You cannot use tools here: every tool call is refused. Do not continue the main task \
and do not promise any action. If the answer needs a tool, say so. Then suggest asking in the \
main conversation. The main task may still be running: a last step that reads as interrupted \
was not stopped.";

pub const SIDE_QUESTION_TOOL_REFUSAL: &str =
    "Side questions cannot use tools. Answer from what you already know.";

pub fn kept_reaching_for_tools(agent: &str) -> String {
    format!("{agent} kept reaching for tools instead of answering. Ask it in the main conversation instead.")
}

pub fn side_question_timeout_message(agent: &str) -> String {
    format!(
        "{agent} did not answer within {} seconds",
        SIDE_QUESTION_TIMEOUT.as_secs()
    )
}
