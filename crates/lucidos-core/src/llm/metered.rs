use async_trait::async_trait;
use std::sync::Arc;

use crate::llm::spend_cap::SpendCapPolicy;

use crate::llm::call_trace::{self, CallFailure};
use crate::llm::image::{ImageProvider, ImageResult, ImageSize};
use crate::llm::judgment::{Judgment, JudgmentProvider, Question};
use crate::llm::prefill::PrefillBody;
use crate::llm::provider::{
    LlmProvider, LlmResponse, Message, ModelSelection, TokenCallback, ToolDefinition,
};
use crate::llm::web_search::{WebSearchChain, WebSearchProvider, WebSearchResult};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Copy, Debug)]
pub struct CallToken {
    _sealed: (),
}

impl CallToken {
    fn mint() -> Self {
        Self { _sealed: () }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test() -> Self {
        Self::mint()
    }

    /// INVARIANT: A token for `lucidos-eval`, which scores models outside any workspace
    /// and has no event bus to record on. No engine source may call this:
    /// `no_engine_source_mints_the_eval_token` holds that.
    #[doc(hidden)]
    pub fn for_eval_harness() -> Self {
        Self::mint()
    }
}

pub enum Reported<'a> {
    Chat(&'a LlmResponse),
    Judgment(&'a Judgment),
    Image {
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
    },
    Search(&'a WebSearchResult),
}

impl Reported<'_> {
    pub(crate) fn served_model(&self) -> Option<&str> {
        match self {
            Self::Chat(response) => response.served_model.as_deref(),
            Self::Judgment(judgment) => judgment.served_model.as_deref(),
            Self::Image { .. } | Self::Search(_) => None,
        }
    }

    pub(crate) fn provider(&self) -> Option<&'static str> {
        match self {
            Self::Chat(response) => response.provider.map(|kind| kind.as_str()),
            Self::Search(result) => result.provider.map(|provider| provider.id()),
            Self::Judgment(_) | Self::Image { .. } => None,
        }
    }
}

pub struct CallCost<'a> {
    pub model: &'a str,
    /// INVARIANT: The size of what was sent. A chat call is sized by
    /// `engine::context::request_chars`, the same count a turn's budget uses.
    pub request_chars: usize,
    pub reported: Reported<'a>,
    pub(crate) reasoning_effort: Option<&'a str>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) ttft_ms: Option<u64>,
    pub(crate) request_id: Option<&'a str>,
    pub(crate) provider: Option<crate::llm::ProviderKind>,
}

/// INVARIANT: How a metered chat call failed, as facts. Never the error message: a
/// provider's error body can echo request content.
pub struct CallFailed<'a> {
    pub model: &'a str,
    pub reasoning_effort: Option<&'a str>,
    pub duration_ms: u64,
    pub attempts: u32,
    pub failure: CallFailure,
    pub status_code: Option<u16>,
    pub request_id: Option<&'a str>,
    pub provider: Option<crate::llm::ProviderKind>,
}

#[async_trait]
pub trait CostSink: Send + Sync {
    fn spend_cap_policy(&self) -> Option<(Arc<dyn SpendCapPolicy>, bool)> {
        None
    }
    async fn record(&self, cost: CallCost<'_>);

    async fn record_failure(&self, _failure: CallFailed<'_>) {}

    /// INVARIANT: When the provider must have answered. It bounds the provider call
    /// alone, so a call that answered in time is never lost to its record.
    fn deadline(&self) -> Option<tokio::time::Instant> {
        None
    }
}

tokio::task_local! {
    static SPEND_POLICY: (Arc<dyn SpendCapPolicy>, bool, std::sync::Mutex<Option<crate::llm::ProviderKind>>);
}

/// INVARIANT: Every actual route checks here, including a fallback inside one call.
pub(crate) async fn check_provider(provider: crate::llm::ProviderKind) -> Result<(), BoxError> {
    if let Ok((policy, interactive)) =
        SPEND_POLICY.try_with(|(policy, interactive, _)| (policy.clone(), *interactive))
    {
        policy.check(Some(provider), interactive).await?;
        SPEND_POLICY.with(|(_, _, selected)| *selected.lock().unwrap() = Some(provider));
    }
    Ok(())
}

/// INVARIANT: Observe a refusal before a search or routing fallback can consume it.
pub(crate) async fn http_spend_cap(
    provider: crate::llm::ProviderKind,
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body: &str,
) -> Option<crate::llm::spend_cap::SpendCapReached> {
    let cap = crate::llm::spend_cap::SpendCapReached::from_http(provider, status, headers, body)?;
    if let Ok(policy) = SPEND_POLICY.try_with(|(policy, _, _)| policy.clone()) {
        policy.failed(&cap).await;
    }
    Some(cap)
}

async fn within<T>(
    sink: &dyn CostSink,
    call: impl std::future::Future<Output = Result<T, BoxError>>,
) -> Result<T, BoxError> {
    let bounded = async {
        match sink.deadline() {
            Some(deadline) => tokio::time::timeout_at(deadline, call)
                .await
                .map_err(|elapsed| Box::new(elapsed) as BoxError)?,
            None => call.await,
        }
    };
    let Some((policy, interactive)) = sink.spend_cap_policy() else {
        return bounded.await;
    };
    SPEND_POLICY
        .scope(
            (policy.clone(), interactive, std::sync::Mutex::new(None)),
            async {
                let result = bounded.await;
                match &result {
                    Ok(_) => {
                        let provider =
                            SPEND_POLICY.with(|(_, _, selected)| *selected.lock().unwrap());
                        policy.succeeded(provider).await;
                    }
                    Err(error) => policy.failed(error.as_ref()).await,
                }
                result
            },
        )
        .await
}

/// INVARIANT: Make one chat call and record what it cost.
///
/// The cost names the model the selection pins, else the provider's default,
/// and the tier the call went out at and the time it took. Both are read after
/// the call, since an auxiliary call can move off a not-served model. A failed
/// call reported no cost, so it records a [`CostSink::record_failure`] and
/// returns its error untouched.
#[allow(clippy::too_many_arguments)]
pub async fn chat<P: LlmProvider + ?Sized>(
    sink: &dyn CostSink,
    provider: &P,
    request_chars: usize,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
    selection: ModelSelection<'_>,
    system_prompt: Option<&str>,
    on_token: Option<TokenCallback>,
) -> Result<LlmResponse, BoxError> {
    // INVARIANT: Route failures must pass through the call's normal failure recording.
    let resolved = if sink.spend_cap_policy().is_some() {
        provider.resolved_provider_kind(&selection).unwrap_or(None)
    } else {
        None
    };
    if let Some((policy, interactive)) = sink.spend_cap_policy() {
        policy.check(resolved, interactive).await?;
    }
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
            if let Some((policy, _)) = sink.spend_cap_policy() {
                policy.failed(error.as_ref()).await;
            }
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
    if let Some((policy, _)) = sink.spend_cap_policy() {
        policy
            .succeeded(response.provider.or(trace.provider).or(resolved))
            .await;
    }
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
        provider: trace.provider,
    })
    .await;
    Ok(response)
}

pub async fn prefill<P: LlmProvider + ?Sized>(
    sink: &dyn CostSink,
    provider: &P,
    body: &PrefillBody,
) -> Result<LlmResponse, BoxError> {
    if let Some((policy, interactive)) = sink.spend_cap_policy() {
        policy.check(body.provider, interactive).await?;
    }
    let started = std::time::Instant::now();
    let call = provider.prefill(body, CallToken::mint());
    let (outcome, trace) = call_trace::scoped(within(sink, call)).await;
    let effort = body.reasoning_effort.as_deref();
    let mut response = match outcome {
        Ok(response) => response,
        Err(error) => {
            if let Some((policy, _)) = sink.spend_cap_policy() {
                policy.failed(error.as_ref()).await;
            }
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
    if let Some((policy, _)) = sink.spend_cap_policy() {
        policy.succeeded(body.provider).await;
    }
    sink.record(CallCost {
        model: &body.model,
        request_chars: body.request_chars(),
        reported: Reported::Chat(&response),
        reasoning_effort: effort,
        duration_ms: Some(started.elapsed().as_millis() as u64),
        ttft_ms: None,
        request_id: trace.request_id.as_deref(),
        provider: trace.provider,
    })
    .await;
    Ok(response)
}

pub async fn judge<J: JudgmentProvider + ?Sized>(
    sink: &dyn CostSink,
    judge: &J,
    state: serde_json::Value,
    questions: Vec<(String, Question)>,
) -> Result<Judgment, BoxError> {
    let started = std::time::Instant::now();
    let call = within(sink, judge.ask(state, questions, CallToken::mint()));
    let (outcome, trace) = call_trace::scoped(call).await;
    let judgment = outcome?;
    sink.record(CallCost {
        model: judgment.model.as_deref().unwrap_or_default(),
        request_chars: judgment.request_chars,
        reported: Reported::Judgment(&judgment),
        reasoning_effort: None,
        duration_ms: Some(elapsed_ms(started)),
        ttft_ms: None,
        request_id: trace.request_id.as_deref(),
        provider: trace.provider,
    })
    .await;
    Ok(judgment)
}

pub async fn generate<I: ImageProvider + ?Sized>(
    sink: &dyn CostSink,
    provider: &I,
    prompt: &str,
    input_images: Vec<Vec<u8>>,
    size: ImageSize,
) -> Result<ImageResult, BoxError> {
    let request_chars = prompt.chars().count() + input_images.iter().map(Vec::len).sum::<usize>();
    let started = std::time::Instant::now();
    let call = within(
        sink,
        provider.generate(prompt, input_images, size, CallToken::mint()),
    );
    let (outcome, trace) = call_trace::scoped(call).await;
    let result = outcome?;
    sink.record(CallCost {
        model: &result.model,
        request_chars,
        reported: Reported::Image {
            input_tokens: result.input_tokens,
            output_tokens: result.output_tokens,
        },
        reasoning_effort: None,
        duration_ms: Some(elapsed_ms(started)),
        ttft_ms: None,
        request_id: trace.request_id.as_deref(),
        provider: trace.provider,
    })
    .await;
    Ok(result)
}

pub async fn search(
    sink: &dyn CostSink,
    chain: &WebSearchChain,
    query: &str,
    max_results: usize,
) -> Result<String, BoxError> {
    let started = std::time::Instant::now();
    let call = within(sink, chain.search(query, max_results, CallToken::mint()));
    let (outcome, trace) = call_trace::scoped(call).await;
    let result = outcome?;
    sink.record(CallCost {
        model: &result.model,
        request_chars: query.chars().count(),
        reported: Reported::Search(&result),
        reasoning_effort: None,
        duration_ms: Some(elapsed_ms(started)),
        ttft_ms: None,
        request_id: trace.request_id.as_deref(),
        provider: None,
    })
    .await;
    Ok(result.text)
}

fn elapsed_ms(started: std::time::Instant) -> u64 {
    started.elapsed().as_millis() as u64
}
