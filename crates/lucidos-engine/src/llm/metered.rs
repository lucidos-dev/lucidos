//! The metered call path: the only way to call a billable provider.
//!
//! Each billable provider method takes a [`CallToken`], and only this module
//! can make one. Every function here makes one call and hands what it cost to
//! a [`CostSink`] before returning the answer. So a model call whose cost goes
//! nowhere does not compile, wherever it is written.
//!
//! The engine's sinks live in `engine::model_call`, the *model call service*,
//! which turns each cost into a `ContextCaptured` row.

use async_trait::async_trait;

use crate::llm::call_trace::{self, CallFailure};
use crate::llm::image::{ImageProvider, ImageResult, ImageSize};
use crate::llm::judgment::{Judgment, JudgmentProvider, Question};
use crate::llm::prefill::PrefillBody;
use crate::llm::provider::{
    LlmProvider, LlmResponse, Message, ModelSelection, TokenCallback, ToolDefinition,
};
use crate::llm::usage_wire::ProviderUsage;
use crate::llm::web_search::{WebSearchChain, WebSearchProvider, WebSearchResult};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Permission to make one billable provider call.
///
/// A wrapper provider forwards the token it was handed to the provider it
/// wraps. Nothing outside this module can make one.
#[derive(Clone, Copy, Debug)]
pub struct CallToken {
    _sealed: (),
}

impl CallToken {
    fn mint() -> Self {
        Self { _sealed: () }
    }

    /// A token for a test that drives a provider directly.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self::mint()
    }

    /// A token for `lucidos-eval`, which scores models outside any workspace
    /// and has no event bus to record on. No engine source may call this:
    /// `no_engine_source_mints_the_eval_token` holds that.
    #[doc(hidden)]
    pub fn for_eval_harness() -> Self {
        Self::mint()
    }
}

/// What a provider reported for one call, in the shape it reported it.
pub(crate) enum Reported<'a> {
    Chat(&'a LlmResponse),
    Judgment(&'a Judgment),
    Image {
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
    },
    Usage(Option<ProviderUsage>),
}

impl Reported<'_> {
    /// The *served model* a chat reply named. Every other shape already
    /// reports the serving model as the call's `model`.
    pub(crate) fn served_model(&self) -> Option<&str> {
        match self {
            Self::Chat(response) => response.served_model.as_deref(),
            Self::Judgment(_) | Self::Image { .. } | Self::Usage(_) => None,
        }
    }

    /// The backend a routed chat call went to. Every other shape names none.
    pub(crate) fn provider(&self) -> Option<crate::llm::ProviderKind> {
        match self {
            Self::Chat(response) => response.provider,
            Self::Judgment(_) | Self::Image { .. } | Self::Usage(_) => None,
        }
    }
}

/// What one call cost.
pub(crate) struct CallCost<'a> {
    /// The model the call asked for. A chat reply names the one that served
    /// it in [`Reported::served_model`].
    pub(crate) model: &'a str,
    /// The size of what was sent. A chat call is sized by
    /// `engine::context::request_chars`, the same count a turn's budget uses.
    pub(crate) request_chars: usize,
    pub(crate) reported: Reported<'a>,
    /// The reasoning tier the call ran at, when the caller chose one.
    pub(crate) reasoning_effort: Option<&'a str>,
    /// How long the call took, wall clock.
    pub(crate) duration_ms: Option<u64>,
    /// Time to the first content delta of the stream that answered, from the
    /// same start as `duration_ms`. `None` when no delta was seen.
    pub(crate) ttft_ms: Option<u64>,
    /// The `request-id` header of the response that answered, when it had one.
    pub(crate) request_id: Option<&'a str>,
}

/// How a metered chat call failed, as facts. Never the error message: a
/// provider's error body can echo request content.
pub(crate) struct CallFailed<'a> {
    pub(crate) model: &'a str,
    pub(crate) reasoning_effort: Option<&'a str>,
    /// How long the call ran before it failed, wall clock.
    pub(crate) duration_ms: u64,
    pub(crate) attempts: u32,
    pub(crate) failure: CallFailure,
    pub(crate) status_code: Option<u16>,
    pub(crate) request_id: Option<&'a str>,
    /// The backend the router sent the call to, when one chose.
    pub(crate) provider: Option<crate::llm::ProviderKind>,
}

/// Where a call's cost goes. Every metered function takes one.
#[async_trait]
pub(crate) trait CostSink: Send + Sync {
    async fn record(&self, cost: CallCost<'_>);

    /// Note a chat call that failed. It spent nothing a rollup should bill, so
    /// a sink that keeps no failures leaves this as is.
    async fn record_failure(&self, _failure: CallFailed<'_>) {}

    /// When the provider must have answered. It bounds the provider call
    /// alone, so a call that answered in time is never lost to its record.
    fn deadline(&self) -> Option<tokio::time::Instant> {
        None
    }
}

/// Run one provider call under the sink's deadline. A call that runs out
/// returns a boxed [`tokio::time::error::Elapsed`] and reported nothing.
async fn within<T>(
    sink: &dyn CostSink,
    call: impl std::future::Future<Output = Result<T, BoxError>>,
) -> Result<T, BoxError> {
    match sink.deadline() {
        Some(deadline) => tokio::time::timeout_at(deadline, call)
            .await
            .map_err(|elapsed| Box::new(elapsed) as BoxError)?,
        None => call.await,
    }
}

/// Make one chat call and record what it cost.
///
/// The cost names the model the selection pins, else the provider's default,
/// and the tier the call went out at and the time it took. Both are read after
/// the call, since an auxiliary call can move off a not-served model. A failed
/// call reported no cost, so it records a [`CostSink::record_failure`] and
/// returns its error untouched.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn chat<P: LlmProvider + ?Sized>(
    sink: &dyn CostSink,
    provider: &P,
    request_chars: usize,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
    selection: ModelSelection<'_>,
    system_prompt: Option<&str>,
    on_token: Option<TokenCallback>,
) -> Result<LlmResponse, BoxError> {
    let started = std::time::Instant::now();
    let call = provider.chat(
        messages,
        tools,
        selection,
        system_prompt,
        on_token,
        CallToken::mint(),
    );
    let (outcome, trace) = call_trace::scoped(within(sink, call)).await;
    let model = selection
        .model
        .unwrap_or(provider.default_model())
        .to_string();
    let effort = provider.effort_sent(&selection);
    let response = match outcome {
        Ok(response) => response,
        Err(error) => {
            let timed_out = error.is::<tokio::time::error::Elapsed>();
            sink.record_failure(CallFailed {
                model: &model,
                reasoning_effort: effort.as_deref(),
                duration_ms: started.elapsed().as_millis() as u64,
                attempts: trace.attempts,
                failure: trace.failure(timed_out),
                status_code: trace.status,
                request_id: trace.request_id.as_deref(),
                provider: trace.provider,
            })
            .await;
            return Err(error);
        }
    };
    sink.record(CallCost {
        model: &model,
        request_chars,
        reported: Reported::Chat(&response),
        reasoning_effort: effort.as_deref(),
        duration_ms: Some(started.elapsed().as_millis() as u64),
        ttft_ms: trace
            .first_delta
            .map(|first| first.saturating_duration_since(started).as_millis() as u64),
        request_id: trace.request_id.as_deref(),
    })
    .await;
    Ok(response)
}

/// Send one *prefill beat* and record what it cost, under the model and
/// effort its turn went out at. A failed beat records a failure, as a chat
/// call does.
pub(crate) async fn prefill<P: LlmProvider + ?Sized>(
    sink: &dyn CostSink,
    provider: &P,
    body: &PrefillBody,
) -> Result<LlmResponse, BoxError> {
    let started = std::time::Instant::now();
    let call = provider.prefill(body, CallToken::mint());
    let (outcome, trace) = call_trace::scoped(within(sink, call)).await;
    let effort = body.reasoning_effort.as_deref();
    let mut response = match outcome {
        Ok(response) => response,
        Err(error) => {
            sink.record_failure(CallFailed {
                model: &body.model,
                reasoning_effort: effort,
                duration_ms: started.elapsed().as_millis() as u64,
                attempts: trace.attempts,
                failure: trace.failure(error.is::<tokio::time::error::Elapsed>()),
                status_code: trace.status,
                request_id: trace.request_id.as_deref(),
                provider: trace.provider,
            })
            .await;
            return Err(error);
        }
    };
    response.stable_prefix_fingerprint = body.stable_prefix_fingerprint.clone();
    sink.record(CallCost {
        model: &body.model,
        request_chars: body.request_chars(),
        reported: Reported::Chat(&response),
        reasoning_effort: effort,
        duration_ms: Some(started.elapsed().as_millis() as u64),
        ttft_ms: None,
        request_id: trace.request_id.as_deref(),
    })
    .await;
    Ok(response)
}

/// Ask one typed judgment and record what it cost. The provider names the
/// model and sizes the request.
pub(crate) async fn judge<J: JudgmentProvider + ?Sized>(
    sink: &dyn CostSink,
    judge: &J,
    state: serde_json::Value,
    questions: Vec<(String, Question)>,
) -> Result<Judgment, BoxError> {
    let judgment = within(sink, judge.ask(state, questions, CallToken::mint())).await?;
    sink.record(CallCost {
        model: judgment.model.as_deref().unwrap_or_default(),
        request_chars: judgment.request_chars,
        reported: Reported::Judgment(&judgment),
        reasoning_effort: None,
        duration_ms: None,
        ttft_ms: None,
        request_id: None,
    })
    .await;
    Ok(judgment)
}

/// Generate one image and record what it cost, sized by the prompt and the
/// input images.
pub(crate) async fn generate<I: ImageProvider + ?Sized>(
    sink: &dyn CostSink,
    provider: &I,
    prompt: &str,
    input_images: Vec<Vec<u8>>,
    size: ImageSize,
) -> Result<ImageResult, BoxError> {
    let request_chars = prompt.chars().count() + input_images.iter().map(Vec::len).sum::<usize>();
    let call = provider.generate(prompt, input_images, size, CallToken::mint());
    let result = within(sink, call).await?;
    sink.record(CallCost {
        model: &result.model,
        request_chars,
        reported: Reported::Image {
            input_tokens: result.input_tokens,
            output_tokens: result.output_tokens,
        },
        reasoning_effort: None,
        duration_ms: None,
        ttft_ms: None,
        request_id: None,
    })
    .await;
    Ok(result)
}

/// Run one web search and record the backend that answered.
pub(crate) async fn search(
    sink: &dyn CostSink,
    chain: &WebSearchChain,
    query: &str,
    max_results: usize,
) -> Result<String, BoxError> {
    let WebSearchResult { text, usage, model } =
        within(sink, chain.search(query, max_results, CallToken::mint())).await?;
    sink.record(CallCost {
        model: &model,
        request_chars: query.chars().count(),
        reported: Reported::Usage(usage),
        reasoning_effort: None,
        duration_ms: None,
        ttft_ms: None,
        request_id: None,
    })
    .await;
    Ok(text)
}
