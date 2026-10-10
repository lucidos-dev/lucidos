use crate::llm::provider::{
    ContentBlock, LlmProvider, LlmResponse, Message, MessageContent, ModelSelection, TokenCallback,
    ToolCall, ToolDefinition,
};
use async_trait::async_trait;
use std::time::Duration;

/// The `LUCIDOS_MODEL` value that selects this provider: the explicit e2e
/// opt-in. It is also the provider's model id.
pub const MOCK_MODEL: &str = "mock";

// ~100 words — long enough for streaming/cancel tests to have content to work with.
pub const MOCK_RESPONSE: &str = "\
The quick brown fox jumps over the lazy dog. A pangram is a sentence that \
contains every letter of the alphabet at least once. The five boxing wizards \
jump quickly across the moonlit field. Pack my box with five dozen liquor jugs. \
How vexingly quick daft zebras jump. The jay, pig, fox, zebra and my wolves \
quack. Crazy Frederick bought many very exquisite opal jewels. We promptly \
judged antique ivory buckles for the next prize. Sixty zippers were quickly \
picked from the woven jute bag. A large fawn jumped quickly over white zinc \
boxes on the shelf.";

/// Sentinel that makes the mock issue one `await_event` call instead of plain
/// text, so an E2E test can drive a real subscription without a real model.
///
/// A tool call is the one thing the mock cannot infer from a prompt, and
/// `await_event` is the one behaviour that CANNOT be reproduced by seeding
/// events (the way `load_knowhow_dedup_test` does): the whole feature is what
/// the engine does at the moment the tool call arrives, so nothing downstream
/// of it exists to seed. Hence a sentinel rather than a second fake provider.
pub const MOCK_AWAIT_EVENT_SENTINEL: &str = "MOCK_SUBSCRIBE_ON:";

/// Sentinel that makes the mock issue a `run_python` call carrying the rest
/// of the line as its `code`.
///
/// Exists for the same reason as the one above: a tool call is what the mock
/// cannot infer from a prompt, and the *command guard* is a pre-dispatch gate,
/// so everything it does (classify the lane, snapshot the workspace, emit or
/// suppress `CommandCheckpointed`) happens only when a real bash/python call
/// arrives. None of that can be reached by seeding events: seeding the card is
/// seeding the outcome, which is exactly what leaves the guard untested.
pub const MOCK_RUN_PYTHON_SENTINEL: &str = "MOCK_RUN_PYTHON:";

/// Sentinel that makes the mock issue one `read_file` call per path, all in
/// one response. The paths are whitespace-separated to the end of the line.
///
/// A batch of reads is what the agent loop runs as a parallel run (ADR 0246),
/// and only a real multi-call response reaches that path.
pub const MOCK_READ_FILES_SENTINEL: &str = "MOCK_READ_FILES:";

/// Sentinel that makes the mock issue one call to any tool:
/// `MOCK_TOOL_CALL: <tool name> <JSON arguments>`, to the end of the line.
///
/// For a test whose subject is what one tool does when a real model calls
/// it, such as where `write_file` lands a path. The sentinels above stay
/// because each also shapes the turn around its call.
pub const MOCK_TOOL_CALL_SENTINEL: &str = "MOCK_TOOL_CALL:";

/// On the request line, the reply carries its own read decision beside the
/// text: `request_read` with `read: true`, once. The turn then ends on that
/// text without another round.
pub const MOCK_READ_BESIDE_REPLY_SENTINEL: &str = "MOCK_READ_BESIDE_REPLY";

/// A message carrying this makes a forced `request_read` answer `read: true`.
/// Without it the forced decision is `read: false`, as for most replies.
pub const MOCK_READ_YES_SENTINEL: &str = "MOCK_READ_YES";

/// The arguments the mock answers a forced call to `tool` with.
fn forced_arguments(tool: &str, messages: &[Message]) -> serde_json::Value {
    if tool != crate::llm::tool_names::REQUEST_READ {
        return serde_json::json!({});
    }
    let read = messages.iter().any(|m| match &m.content {
        MessageContent::Text(text) => text.contains(MOCK_READ_YES_SENTINEL),
        MessageContent::Blocks(blocks) => blocks.iter().any(
            |b| matches!(b, ContentBlock::Text { text, .. } if text.contains(MOCK_READ_YES_SENTINEL)),
        ),
    });
    serde_json::json!({ crate::llm::tools::READ_ARG: read })
}

/// Sentinel that makes the mock end its reply on a question typed as prose:
/// `MOCK_ASK_IN_PROSE: <question>`, to the end of the line. The engine then
/// sends the turn back once, and the mock declines with
/// [`MOCK_PROSE_NUDGE_REASON`], as a model does for an open-ended question.
pub const MOCK_ASK_IN_PROSE_SENTINEL: &str = "MOCK_ASK_IN_PROSE:";

/// What the mock says when sent back for a prose question. The user must never
/// see it, so a test can search the transcript for it.
pub const MOCK_PROSE_NUDGE_REASON: &str = "Open-ended, so any options would be guesses.";

/// What the mock says on any turn whose message array already carries the
/// `await_event` call: the iteration right after it subscribes, and the
/// re-entered turn later. Distinct from [`MOCK_RESPONSE`] so a test can tell those from a
/// turn that never subscribed, without counting events.
pub const MOCK_REENTRY_RESPONSE: &str = "Picked the watch back up and finished the work.";

/// A deterministic LLM provider for E2E testing.
///
/// Returns a fixed text response, streamed word-by-word with a small delay
/// between tokens. Activate with `LUCIDOS_MODEL=mock`.
///
/// It issues a tool call only when scripted to, and only to a tool the request
/// offers, behind
/// [`MOCK_AWAIT_EVENT_SENTINEL`] (see [`scripted_await_event`]),
/// [`MOCK_RUN_PYTHON_SENTINEL`] (see [`scripted_run_python`]),
/// [`MOCK_READ_FILES_SENTINEL`] (see [`scripted_read_files`]) and
/// [`MOCK_TOOL_CALL_SENTINEL`] (see [`scripted_tool_call`]).
pub struct MockProvider {
    default_model: String,
}

/// The line the chat prompt assembler puts the user's own words on, always
/// last in the assembled message (`chat/process/run.rs`).
///
/// The mock reads the sentinel from AFTER this marker and nowhere else, which
/// is the whole reason the marker is referenced here. Every earlier section of
/// that same message quotes other turns and other threads: `[MEMORY]` is a
/// vector search over the workspace and `[CONVERSATION HISTORY]` is this
/// thread's past. A scan of the raw text therefore finds a sentinel that some
/// unrelated test typed minutes ago, and parks a thread that never asked to
/// wait on an event it never named. That is not a hypothetical: it parked eight
/// threads across three test files on the run that first exercised this.
const REQUEST_LINE_MARKER: &str = "Request:";

/// Decide whether this turn should subscribe, and to what.
///
/// Returns the event type to watch when THIS turn's request asks for it and the
/// conversation has not already made the call. Both halves matter:
///
/// * the sentinel is read only from the current request line, per
///   [`REQUEST_LINE_MARKER`], so a quoted one cannot subscribe an unrelated
///   thread;
/// * and the call must not repeat. `await_event` returns rather than ending the
///   turn, so the very next loop iteration sees its own tool_use block, and the
///   WOKEN turn later sees the same request again in its history. Keying the
///   second half on that block is what makes both terminate: once it exists the
///   mock returns plain text and the turn finishes. A prompt-only rule would
///   subscribe forever and trip the recent-subscription cap.
pub fn scripted_await_event(messages: &[Message]) -> Option<String> {
    if already_called(messages, crate::llm::tool_names::AWAIT_EVENT) {
        return None;
    }
    // Last message only: that is this turn's assembled prompt. Anything
    // earlier is history, which by definition already had its chance to park.
    let last = messages.last()?;
    match &last.content {
        MessageContent::Text(text) => sentinel_event_type(text),
        MessageContent::Blocks(blocks) => blocks.iter().find_map(|b| match b {
            ContentBlock::Text { text } => sentinel_event_type(text),
            _ => None,
        }),
    }
}

/// Pull the event name off `MOCK_SUBSCRIBE_ON:<EventType>` on the request line.
/// Whitespace-delimited, so the sentinel can sit inside an ordinary sentence.
fn sentinel_event_type(text: &str) -> Option<String> {
    // After the LAST request marker: the assembler appends it once at the end,
    // and a quoted "Request:" inside history must not shadow the real one.
    let request = text.rsplit(REQUEST_LINE_MARKER).next()?;
    let rest = request.split(MOCK_AWAIT_EVENT_SENTINEL).nth(1)?;
    let name = rest.split_whitespace().next()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// Decide whether this turn should run python, and what code.
///
/// Same two halves as [`scripted_await_event`], for the same two reasons: the
/// sentinel is read only from the current request line, and the call must not
/// repeat once the array carries it.
///
/// Unlike the event sentinel this takes the rest of the LINE rather than the
/// next whitespace-delimited word, because the payload is python source and
/// contains spaces. So it has to be the last thing on its line.
pub fn scripted_run_python(messages: &[Message]) -> Option<String> {
    if already_called(messages, crate::llm::tool_names::RUN_PYTHON) {
        return None;
    }
    sentinel_line(messages, MOCK_RUN_PYTHON_SENTINEL)
}

/// Decide whether this turn should read files, and which. Same two halves as
/// [`scripted_await_event`]: the request line only, and never twice.
pub fn scripted_read_files(messages: &[Message]) -> Option<Vec<String>> {
    if already_called(messages, crate::llm::tool_names::READ_FILE) {
        return None;
    }
    let line = sentinel_line(messages, MOCK_READ_FILES_SENTINEL)?;
    Some(line.split_whitespace().map(str::to_string).collect())
}

/// Decide whether this turn should call a named tool, and with what arguments.
/// Same two halves as [`scripted_await_event`]: the request line only, and
/// never twice. A line whose arguments are not a JSON object scripts nothing.
pub fn scripted_tool_call(messages: &[Message]) -> Option<(String, serde_json::Value)> {
    let line = sentinel_line(messages, MOCK_TOOL_CALL_SENTINEL)?;
    let (name, args) = line.split_once(char::is_whitespace)?;
    if already_called(messages, name) {
        return None;
    }
    let args: serde_json::Value = serde_json::from_str(args.trim()).ok()?;
    args.is_object().then(|| (name.to_string(), args))
}

/// What this turn's reply ends on, when it was scripted to end on a question.
///
/// The question itself on the first round. On the round the engine sent back,
/// the decline: the turn's assembled prompt is then an earlier message, with
/// the drafted question after it.
pub fn scripted_prose_question(messages: &[Message]) -> Option<String> {
    if let Some(question) = sentinel_line(messages, MOCK_ASK_IN_PROSE_SENTINEL) {
        return Some(question);
    }
    let (_, earlier) = messages.split_last()?;
    let asked = earlier
        .iter()
        .position(|m| sentinel_line_in(m, MOCK_ASK_IN_PROSE_SENTINEL).is_some())?;
    earlier[asked + 1..]
        .iter()
        .any(|m| m.role == "assistant")
        .then(|| MOCK_PROSE_NUDGE_REASON.to_string())
}

/// The rest of the request line after `sentinel`, read from this turn's
/// assembled prompt only.
fn sentinel_line(messages: &[Message], sentinel: &str) -> Option<String> {
    sentinel_line_in(messages.last()?, sentinel)
}

/// Whether this turn's request line carries `sentinel`, for a sentinel that
/// takes no arguments.
fn request_line_has(messages: &[Message], sentinel: &str) -> bool {
    let has = |text: &str| {
        text.rsplit(REQUEST_LINE_MARKER)
            .next()
            .is_some_and(|request| request.contains(sentinel))
    };
    messages.last().is_some_and(|m| match &m.content {
        MessageContent::Text(text) => has(text),
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if has(text))),
    })
}

fn sentinel_line_in(message: &Message, sentinel: &str) -> Option<String> {
    let from_text = |text: &str| {
        let request = text.rsplit(REQUEST_LINE_MARKER).next()?;
        let rest = request.split(sentinel).nth(1)?;
        let line = rest.lines().next()?.trim();
        (!line.is_empty()).then(|| line.to_string())
    };
    match &message.content {
        MessageContent::Text(text) => from_text(text),
        MessageContent::Blocks(blocks) => blocks.iter().find_map(|b| match b {
            ContentBlock::Text { text } => from_text(text),
            _ => None,
        }),
    }
}

/// True when the array already carries a call to `tool_name`: this turn has
/// made it (in this iteration or an earlier one) and must now say something and
/// finish. Without this the mock would re-issue the same call every iteration
/// and the turn would never end.
fn already_called(messages: &[Message], tool_name: &str) -> bool {
    messages.iter().any(|m| match &m.content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { name, .. } if name == tool_name)),
        MessageContent::Text(_) => false,
    })
}

impl MockProvider {
    pub fn new(model: String) -> Self {
        Self {
            default_model: model,
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        _system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        // Small initial delay to simulate network round-trip
        tokio::time::sleep(Duration::from_millis(50)).await;

        // A forced call answers with its one tool, as a real provider must.
        if let Some(tool) = selection.forced_tool {
            return Ok(LlmResponse {
                tool_calls: vec![ToolCall {
                    id: format!("toolu_mock_forced_{tool}"),
                    name: tool.to_string(),
                    arguments: forced_arguments(tool, &messages),
                    thought_signature: None,
                }],
                stop_reason: Some("tool_use".to_string()),
                ..LlmResponse::default()
            });
        }

        // A real model can only call a tool the request offers. Callers that
        // offer none (the compactor, the title writer) quote thread messages,
        // sentinels included, and must still get text back.
        let offers = |name: &str| tools.iter().any(|t| t.name == name);

        if let Some(event_type) =
            scripted_await_event(&messages).filter(|_| offers(crate::llm::tool_names::AWAIT_EVENT))
        {
            // No streaming: the subscribing iteration emits no assistant text,
            // and streaming one here would leave a `TextStreamed` the
            // transcript has to explain.
            return Ok(LlmResponse {
                content: None,
                tool_calls: vec![ToolCall {
                    id: "toolu_mock_await_event".to_string(),
                    name: crate::llm::tool_names::AWAIT_EVENT.to_string(),
                    arguments: serde_json::json!({
                        "on": [{ "event_type": event_type }],
                        "timeout_secs": 300,
                        "reason": format!("scripted mock park on {event_type}"),
                    }),
                    thought_signature: None,
                }],
                stop_reason: Some("tool_use".to_string()),
                output_tokens: None,
                input_tokens: None,
                cache_creation_tokens: None,
                cache_read_tokens: None,
                served_model: None,
                thinking_chars: None,
                thinking_blocks: None,
                unknown_sse_dropped: 0,
                model_only_text: None,
                progress_notes: Vec::new(),
            });
        }

        if let Some(code) =
            scripted_run_python(&messages).filter(|_| offers(crate::llm::tool_names::RUN_PYTHON))
        {
            return Ok(LlmResponse {
                content: None,
                tool_calls: vec![ToolCall {
                    id: "toolu_mock_run_python".to_string(),
                    name: crate::llm::tool_names::RUN_PYTHON.to_string(),
                    arguments: serde_json::json!({ "code": code }),
                    thought_signature: None,
                }],
                stop_reason: Some("tool_use".to_string()),
                output_tokens: None,
                input_tokens: None,
                cache_creation_tokens: None,
                cache_read_tokens: None,
                served_model: None,
                thinking_chars: None,
                thinking_blocks: None,
                unknown_sse_dropped: 0,
                model_only_text: None,
                progress_notes: Vec::new(),
            });
        }

        if let Some(paths) =
            scripted_read_files(&messages).filter(|_| offers(crate::llm::tool_names::READ_FILE))
        {
            return Ok(LlmResponse {
                content: None,
                tool_calls: paths
                    .iter()
                    .enumerate()
                    .map(|(i, path)| ToolCall {
                        id: format!("toolu_mock_read_file_{i}"),
                        name: crate::llm::tool_names::READ_FILE.to_string(),
                        arguments: serde_json::json!({ "path": path }),
                        thought_signature: None,
                    })
                    .collect(),
                stop_reason: Some("tool_use".to_string()),
                output_tokens: None,
                input_tokens: None,
                cache_creation_tokens: None,
                cache_read_tokens: None,
                served_model: None,
                thinking_chars: None,
                thinking_blocks: None,
                unknown_sse_dropped: 0,
                model_only_text: None,
                progress_notes: Vec::new(),
            });
        }

        if let Some((name, arguments)) =
            scripted_tool_call(&messages).filter(|(name, _)| offers(name))
        {
            return Ok(LlmResponse {
                content: None,
                tool_calls: vec![ToolCall {
                    id: format!("toolu_mock_{name}"),
                    name,
                    arguments,
                    thought_signature: None,
                }],
                stop_reason: Some("tool_use".to_string()),
                output_tokens: None,
                input_tokens: None,
                cache_creation_tokens: None,
                cache_read_tokens: None,
                served_model: None,
                thinking_chars: None,
                thinking_blocks: None,
                unknown_sse_dropped: 0,
                model_only_text: None,
                progress_notes: Vec::new(),
            });
        }

        let beside_reply = request_line_has(&messages, MOCK_READ_BESIDE_REPLY_SENTINEL)
            && offers(crate::llm::tool_names::REQUEST_READ)
            && !already_called(&messages, crate::llm::tool_names::REQUEST_READ);

        // Once the array carries an `await_event` call, the turn says its piece
        // and ends: this is the "subscribe, then finish" shape the real tool
        // description asks for.
        let body = if let Some(text) = scripted_prose_question(&messages) {
            text
        } else if already_called(&messages, crate::llm::tool_names::AWAIT_EVENT) {
            MOCK_REENTRY_RESPONSE.to_string()
        } else {
            MOCK_RESPONSE.to_string()
        };

        if let Some(cb) = &on_token {
            for (i, word) in body.split_whitespace().enumerate() {
                if i > 0 {
                    cb(" ");
                }
                cb(word);
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }

        if beside_reply {
            return Ok(LlmResponse {
                content: Some(body),
                tool_calls: vec![ToolCall {
                    id: "toolu_mock_request_read".to_string(),
                    name: crate::llm::tool_names::REQUEST_READ.to_string(),
                    arguments: serde_json::json!({ crate::llm::tools::READ_ARG: true }),
                    thought_signature: None,
                }],
                stop_reason: Some("tool_use".to_string()),
                ..LlmResponse::default()
            });
        }

        Ok(LlmResponse {
            content: Some(body),
            tool_calls: vec![],
            stop_reason: Some("end_turn".to_string()),
            output_tokens: None,
            input_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            served_model: None,
            thinking_chars: None,
            thinking_blocks: None,
            unknown_sse_dropped: 0,
            model_only_text: None,
            progress_notes: Vec::new(),
        })
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The beside-reply sentinel takes no arguments, so it may end its line.
    #[test]
    fn the_beside_reply_sentinel_counts_at_the_end_of_the_request_line() {
        let msgs = assembled("", "marker MOCK_READ_BESIDE_REPLY");
        assert!(request_line_has(&msgs, MOCK_READ_BESIDE_REPLY_SENTINEL));
        let quoted = assembled("earlier: MOCK_READ_BESIDE_REPLY", "hello");
        assert!(!request_line_has(&quoted, MOCK_READ_BESIDE_REPLY_SENTINEL));
    }

    /// A forced call answers with its one tool: `read: false` by default, and
    /// `read: true` when the turn carries the sentinel.
    #[tokio::test]
    async fn a_forced_call_answers_with_its_tool() {
        let provider = MockProvider::new(MOCK_MODEL.to_string());
        let decide = |text: &str| {
            let messages = vec![Message {
                role: "user".into(),
                content: MessageContent::Text(text.into()),
            }];
            let provider = &provider;
            async move {
                provider
                    .chat(
                        messages,
                        vec![crate::llm::tools::request_read_tool()],
                        ModelSelection::default()
                            .with_forced_tool(Some(crate::llm::tool_names::REQUEST_READ)),
                        None,
                        None,
                        crate::llm::metered::CallToken::for_test(),
                    )
                    .await
                    .unwrap()
            }
        };
        let no = decide("thanks").await;
        assert_eq!(no.tool_calls.len(), 1);
        assert_eq!(no.tool_calls[0].name, "request_read");
        assert_eq!(
            no.tool_calls[0].arguments[crate::llm::tools::READ_ARG],
            false
        );
        assert!(no.content.is_none());
        let yes = decide(&format!("write the report {MOCK_READ_YES_SENTINEL}")).await;
        assert_eq!(
            yes.tool_calls[0].arguments[crate::llm::tools::READ_ARG],
            true
        );
    }

    #[tokio::test]
    async fn mock_returns_fixed_response() {
        let provider = MockProvider::new("mock".to_string());
        let resp = provider
            .chat(
                vec![],
                vec![],
                ModelSelection::default(),
                None,
                None,
                crate::llm::metered::CallToken::for_test(),
            )
            .await
            .unwrap();
        assert!(resp.content.is_some());
        assert!(resp.content.as_ref().unwrap().len() > 10);
        assert!(resp.tool_calls.is_empty());
    }

    /// The assembled prompt the engine actually sends: context sections that
    /// quote OTHER turns, then this turn's request line last.
    fn assembled(memory: &str, request: &str) -> Vec<Message> {
        vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(format!(
                "[MEMORY]\n{memory}\n[END MEMORY]\n\nRequest: {request}"
            )),
        }]
    }

    #[test]
    fn the_sentinel_parks_when_this_turn_asks_for_it() {
        let msgs = assembled(
            "nothing relevant",
            "please MOCK_SUBSCRIBE_ON:ReleasePublished now",
        );
        assert_eq!(
            scripted_await_event(&msgs),
            Some("ReleasePublished".to_string())
        );
    }

    /// The regression that parked eight unrelated threads: `[MEMORY]` is a
    /// vector search over the whole workspace, so another test's sentinel
    /// lands in a prompt that never asked to wait for anything.
    #[test]
    fn a_sentinel_quoted_from_memory_parks_nothing() {
        let msgs = assembled(
            "earlier: someone said MOCK_SUBSCRIBE_ON:SomeOtherEvent",
            "just answer normally",
        );
        assert_eq!(scripted_await_event(&msgs), None);
    }

    #[test]
    fn a_prose_question_is_asked_then_declined_when_sent_back() {
        let mut msgs = assembled("", "MOCK_ASK_IN_PROSE: What are we working on?");
        assert_eq!(
            scripted_prose_question(&msgs).as_deref(),
            Some("What are we working on?")
        );
        for (role, text) in [("assistant", "What are we working on?"), ("user", "nudge")] {
            msgs.push(Message {
                role: role.to_string(),
                content: MessageContent::Text(text.to_string()),
            });
        }
        assert_eq!(
            scripted_prose_question(&msgs).as_deref(),
            Some(MOCK_PROSE_NUDGE_REASON)
        );
        assert_eq!(
            scripted_prose_question(&assembled("", "hello")),
            None,
            "an ordinary turn is not scripted"
        );
    }

    #[test]
    fn this_turns_request_wins_over_a_quoted_one() {
        let msgs = assembled(
            "earlier: MOCK_SUBSCRIBE_ON:StaleEvent",
            "MOCK_SUBSCRIBE_ON:FreshEvent please",
        );
        assert_eq!(scripted_await_event(&msgs), Some("FreshEvent".to_string()));
    }

    /// Termination, and it has to hold twice: the loop iteration right after
    /// the call sees the block, and so does the woken turn (which sees its own
    /// request again in history). Without this the mock would re-subscribe
    /// forever and trip the recent-subscription cap.
    #[test]
    fn a_turn_that_already_subscribed_does_not_subscribe_again() {
        let mut msgs = assembled("", "MOCK_SUBSCRIBE_ON:ReleasePublished");
        msgs.push(Message {
            role: "assistant".to_string(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "toolu_mock_await_event".to_string(),
                name: "await_event".to_string(),
                input: serde_json::json!({}),
                thought_signature: None,
            }]),
        });
        assert_eq!(scripted_await_event(&msgs), None);
        assert!(already_called(&msgs, "await_event"));
    }

    fn offered(names: &[&str]) -> Vec<ToolDefinition> {
        names
            .iter()
            .map(|name| ToolDefinition {
                name: name.to_string(),
                description: String::new(),
                parameters: serde_json::json!({}),
            })
            .collect()
    }

    /// The summary-tree compactor and the title writer offer no tools, yet
    /// quote thread messages that carry sentinels. A call there came back with
    /// no text, and the compactor retried that node forever.
    #[tokio::test]
    async fn a_sentinel_in_a_request_offering_no_tools_answers_in_text() {
        let provider = MockProvider::new("mock".to_string());
        for line in [
            format!("{MOCK_AWAIT_EVENT_SENTINEL}ReleasePublished"),
            format!("{MOCK_RUN_PYTHON_SENTINEL} print(1)"),
            format!("{MOCK_READ_FILES_SENTINEL} a.txt"),
            format!(
                r#"{MOCK_TOOL_CALL_SENTINEL} write_file {{"path":"artifacts/x.md","content":"x"}}"#
            ),
        ] {
            let resp = provider
                .chat(
                    vec![Message {
                        role: "user".to_string(),
                        content: MessageContent::Text(format!("Summarise:\n{line}")),
                    }],
                    vec![],
                    ModelSelection::default(),
                    None,
                    None,
                    crate::llm::metered::CallToken::for_test(),
                )
                .await
                .unwrap();
            assert!(resp.tool_calls.is_empty(), "{line}");
            assert_eq!(resp.content.as_deref(), Some(MOCK_RESPONSE), "{line}");
        }
    }

    #[tokio::test]
    async fn a_scripted_call_needs_its_own_tool_offered() {
        let provider = MockProvider::new("mock".to_string());
        let resp = provider
            .chat(
                assembled(
                    "",
                    &format!(r#"{MOCK_TOOL_CALL_SENTINEL} write_file {{"path":"a.md"}}"#),
                ),
                offered(&[crate::llm::tool_names::READ_FILE]),
                ModelSelection::default(),
                None,
                None,
                crate::llm::metered::CallToken::for_test(),
            )
            .await
            .unwrap();
        assert!(resp.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn mock_parks_with_a_single_await_event_call() {
        let provider = MockProvider::new("mock".to_string());
        let resp = provider
            .chat(
                assembled("", "MOCK_SUBSCRIBE_ON:ReleasePublished"),
                offered(&[crate::llm::tool_names::AWAIT_EVENT]),
                ModelSelection::default(),
                None,
                None,
                crate::llm::metered::CallToken::for_test(),
            )
            .await
            .unwrap();
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "await_event");
        assert_eq!(
            resp.tool_calls[0].arguments["on"][0]["event_type"],
            "ReleasePublished"
        );
        assert!(
            resp.content.is_none(),
            "a park emits no assistant text to explain"
        );
    }

    /// One response carrying every read is what makes it a batch. The second
    /// iteration sees the calls in its array and answers in prose instead.
    #[tokio::test]
    async fn mock_reads_every_path_in_one_response_and_only_once() {
        let provider = MockProvider::new("mock".to_string());
        let mut msgs = assembled("", "MOCK_READ_FILES: a.txt b.txt c.txt");
        let resp = provider
            .chat(
                msgs.clone(),
                offered(&[crate::llm::tool_names::READ_FILE]),
                ModelSelection::default(),
                None,
                None,
                crate::llm::metered::CallToken::for_test(),
            )
            .await
            .unwrap();
        let paths: Vec<&str> = resp
            .tool_calls
            .iter()
            .map(|c| c.arguments["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["a.txt", "b.txt", "c.txt"]);
        assert!(resp.tool_calls.iter().all(|c| c.name == "read_file"));

        msgs.push(Message {
            role: "assistant".to_string(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "toolu_mock_read_file_0".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({ "path": "a.txt" }),
                thought_signature: None,
            }]),
        });
        assert_eq!(scripted_read_files(&msgs), None);
    }

    #[test]
    fn a_scripted_tool_call_carries_its_name_and_arguments_once() {
        let mut msgs = assembled(
            "earlier: MOCK_TOOL_CALL: delete_file {\"path\":\"artifacts/x.md\"}",
            r#"MOCK_TOOL_CALL: write_file {"path":"themes/a.json","content":"{\"name\":\"A\"}"}"#,
        );
        let (name, args) = scripted_tool_call(&msgs).expect("scripted");
        assert_eq!(name, "write_file");
        assert_eq!(args["path"], "themes/a.json");
        assert_eq!(args["content"], r#"{"name":"A"}"#);

        msgs.push(Message {
            role: "assistant".to_string(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "toolu_mock_write_file".to_string(),
                name: "write_file".to_string(),
                input: args,
                thought_signature: None,
            }]),
        });
        assert_eq!(scripted_tool_call(&msgs), None, "never twice");
    }

    #[test]
    fn a_tool_call_line_without_object_arguments_scripts_nothing() {
        for line in [
            "MOCK_TOOL_CALL: write_file",
            "MOCK_TOOL_CALL: write_file not json",
            "MOCK_TOOL_CALL: write_file [1, 2]",
        ] {
            assert_eq!(scripted_tool_call(&assembled("", line)), None, "{line}");
        }
    }

    #[tokio::test]
    async fn mock_streams_tokens() {
        let provider = MockProvider::new("mock".to_string());
        let tokens = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let tokens_clone = tokens.clone();
        let cb: TokenCallback = Box::new(move |token: &str| {
            tokens_clone.lock().unwrap().push(token.to_string());
        });
        let resp = provider
            .chat(
                vec![],
                vec![],
                ModelSelection::default(),
                None,
                Some(cb),
                crate::llm::metered::CallToken::for_test(),
            )
            .await
            .unwrap();
        assert!(resp.content.is_some());
        let collected = tokens.lock().unwrap();
        assert!(collected.len() > 10, "should stream many tokens");
    }
}
