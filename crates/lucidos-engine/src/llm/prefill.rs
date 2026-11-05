//! A *prefill beat*'s request (ADR 0556). It sends the start of a thread's
//! last model request again. The provider reads it from the prompt cache. A
//! Messages beat hangs up before the model answers, and an OpenAI beat caps
//! its output.
//!
//! A provider that can prefill hands back a [`PrefillPair`] with each reply.
//! The cache keep-alive remembers the pair and later sends one of its bodies
//! through [`crate::llm::LlmProvider::prefill`].

use crate::llm::model_registry::ProviderKind;
use crate::llm::provider::LlmResponse;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The two cuts of one request a beat can send.
#[derive(Clone, Debug)]
pub struct PrefillPair {
    /// Through the last cache marker of the *stable prefix*: what the next
    /// turn sends again, for a turn-end wait.
    pub stable: PrefillBody,
    /// The whole request, for a wait on the thread's own question card,
    /// where the answer extends this very request.
    pub whole: PrefillBody,
}

impl PrefillPair {
    /// Stamp both bodies with the backend that served the turn and the model
    /// the turn resolved to. Only the router knows either.
    pub(crate) fn routed(&mut self, provider: ProviderKind, model: &str) {
        for body in [&mut self.stable, &mut self.whole] {
            body.provider = Some(provider);
            body.model = model.to_string();
        }
    }
}

/// One request body a beat sends, as the provider's own wire bytes.
#[derive(Clone)]
pub struct PrefillBody {
    /// The backend that served the turn. `None` from a provider called
    /// outside the router, which then has nowhere to send the beat.
    pub provider: Option<ProviderKind>,
    /// The model the beat's cost is recorded under.
    pub model: String,
    /// The effort the turn went out at, for the cost record.
    pub reasoning_effort: Option<String>,
    /// The stable prefix this cut actually sends, when its wire supports it.
    pub stable_prefix_fingerprint: Option<String>,
    /// The most output tokens this body allows. A beat that reports more
    /// was not a prefill (ADR 0556, the first-beat guard). `None` for a beat
    /// that hangs up before the model answers, whose usage block says nothing
    /// of its output.
    pub output_cap: Option<u32>,
    /// How the provider answers this body.
    pub(crate) reply: ReplyShape,
    /// The model id the leaf provider was asked for, which it resolves
    /// again on send.
    pub(crate) wire_model: String,
    /// The betas the turn sent as the `anthropic-beta` header.
    pub(crate) betas: Vec<&'static str>,
    pub(crate) body: bytes::Bytes,
}

/// How a provider answers a beat, which decides what [`send`] reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ReplyShape {
    /// One JSON reply, read whole.
    Json,
    /// A stream, read up to `message_start`, then dropped.
    MessageStart,
}

/// What a wire adds to the turn's request to make its beat.
pub(crate) struct BeatShape {
    pub(crate) output_cap: Option<u32>,
    pub(crate) reply: ReplyShape,
    /// A content part appended after the last marker of both cuts, so it is
    /// part of no cache key. It goes to the last item, which must be the
    /// user's.
    pub(crate) closing_part: Option<serde_json::Value>,
}

impl BeatShape {
    /// A request recast to answer one JSON reply of at most `output_cap`
    /// tokens, with nothing added to it.
    pub(crate) fn json_reply(output_cap: u32) -> Self {
        Self {
            output_cap: Some(output_cap),
            reply: ReplyShape::Json,
            closing_part: None,
        }
    }
}

impl PrefillBody {
    /// The size of what a beat sends, for its cost record.
    pub fn request_chars(&self) -> usize {
        self.body.len()
    }
}

/// Never the body: it holds the whole conversation.
impl std::fmt::Debug for PrefillBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrefillBody")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("output_cap", &self.output_cap)
            .field("bytes", &self.body.len())
            .finish()
    }
}

/// What the turn asked its leaf provider for, carried into both bodies.
pub(crate) struct TurnRoute<'a> {
    pub(crate) wire_model: &'a str,
    pub(crate) reasoning_effort: Option<&'a str>,
    pub(crate) betas: &'a [&'static str],
}

/// Where a stable cut ends, in the request's list of messages or items.
#[derive(Debug, PartialEq)]
pub(crate) enum StableEnd {
    /// After content part `part` of item `item`: the last marked part of the
    /// stable prefix, where the turn wrote an entry.
    Part { item: usize, part: usize },
    /// After the first `keep` items, then one unmarked user message of a
    /// single character, which a closing part may follow. A request refuses
    /// an empty list, and the character sits after the last marker, so it is
    /// billed as plain input.
    Placeholder { keep: usize },
}

/// One item of the list a stable cut shortens: a message or an input item.
pub(crate) trait CutItem {
    fn role(&self) -> Option<&str>;
    fn content_mut(&mut self) -> Option<&mut serde_json::Value>;
    /// An unmarked user message of a single character.
    fn placeholder() -> Self;
}

impl CutItem for serde_json::Value {
    fn role(&self) -> Option<&str> {
        self.get("role").and_then(serde_json::Value::as_str)
    }

    fn content_mut(&mut self) -> Option<&mut serde_json::Value> {
        self.get_mut("content")
    }

    fn placeholder() -> Self {
        serde_json::json!({"role": "user", "content": "."})
    }
}

/// The pair a beat on `request` sends. `request` is the turn's request in
/// the form `shape` asks for, and `items` finds the list a stable cut
/// shortens to `end`.
///
/// Each body serializes `request`'s own type, so it keeps the turn's byte
/// order. A gateway may route on the raw bytes (ADR 0581).
pub(crate) fn pair_of<R, I>(
    request: &R,
    items: impl Fn(&mut R) -> Option<&mut Vec<I>>,
    end: StableEnd,
    shape: BeatShape,
    route: TurnRoute<'_>,
) -> Option<PrefillPair>
where
    R: Clone + serde::Serialize,
    I: CutItem,
{
    let closing = |items: &mut Vec<I>| match &shape.closing_part {
        Some(part) => append_part(items, part),
        None => Some(()),
    };
    let mut whole = request.clone();
    closing(items(&mut whole)?)?;
    let whole = serde_json::to_vec(&whole).ok()?;
    let mut stable = request.clone();
    let list = items(&mut stable)?;
    cut(list, end);
    closing(list)?;
    let stable = serde_json::to_vec(&stable).ok()?;
    let body = |bytes: Vec<u8>| PrefillBody {
        provider: None,
        model: route.wire_model.to_string(),
        reasoning_effort: route.reasoning_effort.map(str::to_string),
        stable_prefix_fingerprint: None,
        output_cap: shape.output_cap,
        reply: shape.reply,
        wire_model: route.wire_model.to_string(),
        betas: route.betas.to_vec(),
        body: bytes.into(),
    };
    Some(PrefillPair {
        stable: body(stable),
        whole: body(whole),
    })
}

fn cut<I: CutItem>(items: &mut Vec<I>, end: StableEnd) {
    match end {
        StableEnd::Part { item, part } => {
            items.truncate(item + 1);
            if let Some(parts) = items
                .get_mut(item)
                .and_then(CutItem::content_mut)
                .and_then(serde_json::Value::as_array_mut)
            {
                parts.truncate(part + 1);
            }
        }
        StableEnd::Placeholder { keep } => {
            items.truncate(keep);
            items.push(I::placeholder());
        }
    }
}

/// Append `part` to the last item's content, which must be the user's. A
/// string content becomes a list of text parts first. `None` otherwise.
fn append_part<I: CutItem>(items: &mut [I], part: &serde_json::Value) -> Option<()> {
    let last = items.last_mut()?;
    if last.role() != Some("user") {
        return None;
    }
    match last.content_mut()? {
        serde_json::Value::Array(parts) => parts.push(part.clone()),
        content => {
            let text = serde_json::json!({"type": "text", "text": content.take()});
            *content = serde_json::Value::Array(vec![text, part.clone()]);
        }
    }
    Some(())
}

/// What a beat cost, from the usage block of its reply or of its
/// `message_start` message. One with no usage block is a failed beat:
/// nothing says what it read.
pub(crate) fn reply_usage(reply: &serde_json::Value) -> Result<LlmResponse, BoxError> {
    let usage = crate::llm::usage_wire::from_json(reply)
        .ok_or("the prefill reply carried no usage block")?;
    let text = |key: &str| reply.get(key).and_then(|v| v.as_str()).map(str::to_string);
    Ok(LlmResponse {
        usage: Some(usage),
        stop_reason: text("stop_reason"),
        served_model: text("model"),
        ..LlmResponse::default()
    })
}

/// The longest a beat may take. A prefill answers in seconds, and a beat is
/// worth nothing once its window has passed.
const PREFILL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Send one prefill, once. A refused beat is not retried: the next tick
/// decides afresh. `label` names the provider in the error.
///
/// A [`ReplyShape::MessageStart`] beat streams like the turn did. This reads
/// until `message_start` and drops the response, so it reads no content block.
pub(crate) async fn send(
    builder: reqwest::RequestBuilder,
    body: &PrefillBody,
    label: &str,
) -> Result<LlmResponse, BoxError> {
    use crate::llm::call_trace::{self, CallFailure};
    call_trace::attempt_began();
    let response = builder
        .header("Content-Type", "application/json")
        .timeout(PREFILL_TIMEOUT)
        .body(body.body.clone())
        .send()
        .await
        .inspect_err(|error| {
            call_trace::transport_failed(if error.is_timeout() {
                CallFailure::Timeout
            } else {
                CallFailure::Connection
            });
        })?;
    let status = response.status();
    let request_id = response
        .headers()
        .get("request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    call_trace::response_received(status.as_u16(), request_id.as_deref());
    if !status.is_success() {
        let reply = response.text().await?;
        if PrefillRefused::is_refusal(status.as_u16()) {
            return Err(Box::new(PrefillRefused::new(format!(
                "{label} refused the prefill ({status}): {reply}"
            ))));
        }
        return Err(format!("{label} could not serve the prefill ({status}): {reply}").into());
    }
    let message = match body.reply {
        ReplyShape::Json => serde_json::from_str(&response.text().await?)?,
        ReplyShape::MessageStart => message_start(response).await?,
    };
    reply_usage(&message)
}

/// The `message` of the stream's `message_start` event. An `error` event
/// first, or a stream that ends without one, is a failed beat.
async fn message_start(response: reqwest::Response) -> Result<serde_json::Value, BoxError> {
    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    while let Some(chunk) = stream.next().await {
        pending.extend_from_slice(&chunk?);
        while let Some(end) = pending.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let Some(data) = line.trim_end().strip_prefix("data: ") else {
                continue;
            };
            let mut event: serde_json::Value = serde_json::from_str(data)?;
            match event["type"].as_str() {
                Some("message_start") => return Ok(event["message"].take()),
                Some("error") => return Err(format!("the prefill stream failed: {data}").into()),
                _ => {}
            }
        }
    }
    Err("the prefill stream ended before message_start".into())
}

/// A provider that refused a prefill outright, with a 4xx other than a
/// timeout or a rate limit. The first-beat guard marks such a provider
/// unable to prefill, so it is its own type (ADR 0556).
#[derive(Debug)]
pub struct PrefillRefused {
    message: String,
}

impl PrefillRefused {
    pub(crate) fn new(message: String) -> Self {
        Self { message }
    }

    fn is_refusal(status: u16) -> bool {
        (400..500).contains(&status) && status != 408 && status != 429
    }
}

impl std::fmt::Display for PrefillRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PrefillRefused {}

/// The error a provider that cannot prefill answers with.
pub(crate) fn cannot_prefill(provider: Option<ProviderKind>) -> BoxError {
    match provider {
        Some(kind) => format!("{} cannot prefill a request", kind.as_str()).into(),
        None => "this provider cannot prefill a request".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_reads_its_usage_and_never_its_stop_reason_as_failure() {
        let reply = serde_json::json!({
            "model": "claude-opus-5-5",
            "stop_reason": "refusal",
            "content": [],
            "usage": {
                "input_tokens": 3,
                "cache_read_input_tokens": 90_000,
                "cache_creation_input_tokens": 0,
                "output_tokens": 0
            }
        });
        let beat = reply_usage(&reply).expect("a usage block");
        let usage = beat.usage.expect("the usage");
        assert_eq!(usage.input_tokens, 90_003);
        assert_eq!(usage.cache_read_tokens, 90_000);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(beat.stop_reason.as_deref(), Some("refusal"));
        assert_eq!(beat.served_model.as_deref(), Some("claude-opus-5-5"));
    }

    /// A keep-alive beat that writes the cache keeps the write's lifetime.
    #[test]
    fn a_beat_keeps_the_write_split() {
        let reply = serde_json::json!({
            "usage": {
                "input_tokens": 3,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 4_000,
                "cache_creation": {"ephemeral_1h_input_tokens": 4_000},
                "output_tokens": 0
            }
        });
        let response = reply_usage(&reply).expect("a usage block");
        let usage = response.usage.expect("usage");
        assert_eq!(usage.cache_creation_1h_tokens, Some(4_000));
    }

    #[test]
    fn a_reply_with_no_usage_block_is_a_failed_beat() {
        let reply = serde_json::json!({"stop_reason": "content_filter"});
        assert!(reply_usage(&reply).is_err());
    }

    #[test]
    fn only_a_lasting_4xx_is_a_refusal() {
        for status in [400, 401, 403, 404, 422] {
            assert!(PrefillRefused::is_refusal(status), "{status}");
        }
        for status in [200, 408, 429, 500, 529] {
            assert!(!PrefillRefused::is_refusal(status), "{status}");
        }
    }

    const START: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-test\",\"stop_reason\":null,\"usage\":{\"input_tokens\":2,\"cache_read_input_tokens\":500,\"cache_creation_input_tokens\":0,\"output_tokens\":1}}}\n\n";

    fn streamed_body() -> PrefillBody {
        PrefillBody {
            provider: None,
            model: "m".into(),
            reasoning_effort: None,
            stable_prefix_fingerprint: None,
            output_cap: None,
            reply: ReplyShape::MessageStart,
            wire_model: "m".into(),
            betas: Vec::new(),
            body: bytes::Bytes::from_static(b"{}"),
        }
    }

    /// Serve `chunks` as one SSE response, then keep the connection open.
    async fn serve_then_stall(chunks: Vec<&'static str>) -> String {
        use futures::StreamExt;
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move || {
                let chunks = chunks.clone();
                async move {
                    let stream = futures::stream::iter(chunks)
                        .map(|chunk| Ok::<_, std::convert::Infallible>(bytes::Bytes::from(chunk)))
                        .chain(futures::stream::pending());
                    (
                        [("content-type", "text/event-stream")],
                        axum::body::Body::from_stream(stream),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/")
    }

    async fn beat_to(url: &str) -> Result<LlmResponse, BoxError> {
        send(reqwest::Client::new().post(url), &streamed_body(), "Test").await
    }

    /// The beat returns at `message_start` while the model's answer is still
    /// streaming: it never reads a content block.
    #[tokio::test]
    async fn a_beat_hangs_up_at_message_start() {
        let url = serve_then_stall(vec!["event: ping\ndata: {\"type\":\"ping\"}\n\n", START]).await;
        let beat = tokio::time::timeout(std::time::Duration::from_secs(10), beat_to(&url))
            .await
            .expect("the beat does not wait for the answer")
            .expect("message_start carries the usage");
        assert_eq!(beat.usage.map(|u| u.cache_read_tokens), Some(500));
        assert_eq!(beat.served_model.as_deref(), Some("claude-test"));
    }

    #[tokio::test]
    async fn an_error_event_before_message_start_is_a_failed_beat() {
        let url = serve_then_stall(vec![
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\"}}\n\n",
            START,
        ])
        .await;
        let error = beat_to(&url)
            .await
            .expect_err("an error event fails the beat");
        assert!(error.to_string().contains("overloaded_error"), "{error}");
    }

    #[tokio::test]
    async fn a_stream_with_no_message_start_is_a_failed_beat() {
        let app = axum::Router::new().route("/", axum::routing::post(|| async { "" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let error = beat_to(&format!("http://{addr}/"))
            .await
            .expect_err("an empty stream fails the beat");
        assert!(error.to_string().contains("message_start"), "{error}");
    }

    #[test]
    fn debug_never_prints_the_body() {
        let body = PrefillBody {
            provider: None,
            model: "m".into(),
            reasoning_effort: None,
            stable_prefix_fingerprint: None,
            output_cap: Some(0),
            reply: ReplyShape::Json,
            wire_model: "m".into(),
            betas: Vec::new(),
            body: bytes::Bytes::from_static(b"{\"secret\":1}"),
        };
        let printed = format!("{body:?}");
        assert!(!printed.contains("secret"), "{printed}");
        assert!(printed.contains("bytes: 12"), "{printed}");
    }
}
