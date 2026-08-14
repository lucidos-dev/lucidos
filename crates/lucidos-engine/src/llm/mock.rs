use crate::llm::provider::{
    ContentBlock, LlmProvider, LlmResponse, Message, MessageContent, TokenCallback, ToolCall,
    ToolDefinition,
};
use async_trait::async_trait;
use std::time::Duration;

// ~100 words — long enough for streaming/cancel tests to have content to work with.
pub(crate) const MOCK_RESPONSE: &str = "\
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
/// It issues a tool call only when scripted to, behind
/// [`MOCK_AWAIT_EVENT_SENTINEL`] (see [`scripted_await_event`]) and
/// [`MOCK_RUN_PYTHON_SENTINEL`] (see [`scripted_run_python`]).
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
///   subscribe forever and trip the consecutive-subscription cap.
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
    let last = messages.last()?;
    match &last.content {
        MessageContent::Text(text) => sentinel_python_code(text),
        MessageContent::Blocks(blocks) => blocks.iter().find_map(|b| match b {
            ContentBlock::Text { text } => sentinel_python_code(text),
            _ => None,
        }),
    }
}

/// Pull the code off `MOCK_RUN_PYTHON:<code to end of line>` on the request
/// line.
fn sentinel_python_code(text: &str) -> Option<String> {
    let request = text.rsplit(REQUEST_LINE_MARKER).next()?;
    let rest = request.split(MOCK_RUN_PYTHON_SENTINEL).nth(1)?;
    let code = rest.lines().next()?.trim();
    (!code.is_empty()).then(|| code.to_string())
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
        _tools: Vec<ToolDefinition>,
        _model_override: Option<&str>,
        _system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        _reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        // Small initial delay to simulate network round-trip
        tokio::time::sleep(Duration::from_millis(50)).await;

        if let Some(event_type) = scripted_await_event(&messages) {
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
                thinking_chars: None,
                unknown_sse_dropped: 0,
            });
        }

        if let Some(code) = scripted_run_python(&messages) {
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
                thinking_chars: None,
                unknown_sse_dropped: 0,
            });
        }

        // Once the array carries an `await_event` call, the turn says its piece
        // and ends: this is the "subscribe, then finish" shape the real tool
        // description asks for.
        let body = if already_called(&messages, crate::llm::tool_names::AWAIT_EVENT) {
            MOCK_REENTRY_RESPONSE
        } else {
            MOCK_RESPONSE
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

        Ok(LlmResponse {
            content: Some(body.to_string()),
            tool_calls: vec![],
            stop_reason: Some("end_turn".to_string()),
            output_tokens: None,
            input_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            thinking_chars: None,
            unknown_sse_dropped: 0,
        })
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_returns_fixed_response() {
        let provider = MockProvider::new("mock".to_string());
        let resp = provider
            .chat(vec![], vec![], None, None, None, None)
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
    /// forever and trip the consecutive-subscription cap.
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

    #[tokio::test]
    async fn mock_parks_with_a_single_await_event_call() {
        let provider = MockProvider::new("mock".to_string());
        let resp = provider
            .chat(
                assembled("", "MOCK_SUBSCRIBE_ON:ReleasePublished"),
                vec![],
                None,
                None,
                None,
                None,
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

    #[tokio::test]
    async fn mock_streams_tokens() {
        let provider = MockProvider::new("mock".to_string());
        let tokens = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let tokens_clone = tokens.clone();
        let cb: TokenCallback = Box::new(move |token: &str| {
            tokens_clone.lock().unwrap().push(token.to_string());
        });
        let resp = provider
            .chat(vec![], vec![], None, None, Some(cb), None)
            .await
            .unwrap();
        assert!(resp.content.is_some());
        let collected = tokens.lock().unwrap();
        assert!(collected.len() > 10, "should stream many tokens");
    }
}
