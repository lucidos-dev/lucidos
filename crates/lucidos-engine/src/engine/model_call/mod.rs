//! The *model call service*: the one path every engine model call takes, and
//! the `ContextCaptured` row it always leaves (ADR 0242).
//!
//! **Only the service can call a model.** Each billable provider method takes
//! a `CallToken`, and only `llm::metered` can make one. Each metered function
//! hands its cost to a `CostSink`, and the sinks are here. So a model call
//! that records nothing does not compile, wherever it is written.
//!
//! - An auxiliary call goes through [`AuxCapture::chat`], [`AuxCapture::judge`],
//!   [`AuxCapture::generate`] or [`AuxCapture::search`].
//! - The agent's own turn goes through [`turn_chat`]. The loop finishes its
//!   richer row with [`TurnCapture::finish`], where the event order needs it.
//! - Spend that arrives outside a provider call (a voice talker turn, a coding
//!   agent's side question, a proxied call) records through
//!   [`AuxCapture::record_usage`].
//!
//! Every row lands on a thread: the caller's, or the home thread when no
//! thread caused the call. See [`AuxCapture`].

mod capture;

pub(crate) use capture::{
    auxiliary_capture, model_call_failed, usage_from_provider, usage_from_response, AuxCapture,
    CallDetail,
};

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::{ApiUsage, ContextProducer, ContextPurpose};
use crate::llm::image::{ImageProvider, ImageResult, ImageSize};
use crate::llm::judgment::{Judgment, JudgmentProvider, Question};
use crate::llm::metered::{self, CallCost, CallFailed, CostSink, Reported};
use crate::llm::provider::{
    LlmProvider, LlmResponse, Message, ModelSelection, TokenCallback, ToolDefinition,
};
use crate::llm::web_search::WebSearchChain;
use uuid::Uuid;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

impl AuxCapture {
    /// Make one chat call and record what it cost, with the tier and time it
    /// took. A failed call records a `ModelCallFailed`, never a capture.
    pub(crate) async fn chat<P: LlmProvider + ?Sized>(
        &self,
        provider: &P,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
    ) -> Result<LlmResponse, BoxError> {
        let request_chars =
            crate::engine::context::request_chars(system_prompt.unwrap_or(""), &messages, &tools);
        metered::chat(
            self,
            provider,
            request_chars,
            messages,
            tools,
            selection,
            system_prompt,
            on_token,
        )
        .await
    }

    /// Send one *prefill beat* and record what it cost.
    pub(crate) async fn prefill<P: LlmProvider + ?Sized>(
        &self,
        provider: &P,
        body: &crate::llm::prefill::PrefillBody,
    ) -> Result<LlmResponse, BoxError> {
        metered::prefill(self, provider, body).await
    }

    /// Ask one typed judgment and record what it cost.
    pub(crate) async fn judge<J: JudgmentProvider + ?Sized>(
        &self,
        judge: &J,
        state: serde_json::Value,
        questions: Vec<(String, Question)>,
    ) -> Result<Judgment, BoxError> {
        metered::judge(self, judge, state, questions).await
    }

    /// Generate one image and record what it cost.
    ///
    /// Imagen prices per image and reports no tokens, so its row carries no
    /// usage. The row exists either way: an image the engine paid for must
    /// not be missing from the ledger. It names the serving model, never the
    /// provider's display label, since a cost breakdown groups on the model.
    pub(crate) async fn generate<I: ImageProvider + ?Sized>(
        &self,
        provider: &I,
        prompt: &str,
        input_images: Vec<Vec<u8>>,
        size: ImageSize,
    ) -> Result<ImageResult, BoxError> {
        metered::generate(self, provider, prompt, input_images, size).await
    }

    /// Run one web search and record the backend that answered.
    pub(crate) async fn search(
        &self,
        chain: &WebSearchChain,
        query: &str,
        max_results: usize,
    ) -> Result<String, BoxError> {
        metered::search(self, chain, query, max_results).await
    }
}

/// Make the agent's own chat call for one round of a turn.
///
/// Returns the response and its [`TurnCapture`], which the loop finishes with
/// the round's context sections once its repairs have run. A failed call
/// records a `ModelCallFailed` instead. Both rows carry `meta`, so each keeps
/// the turn's `request_event_id`. `round` counts from 1. Only round 1 keeps
/// the stable-prefix fingerprint, since every later round of the turn sends
/// the same prefix.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn turn_chat(
    bus: &EventBus,
    thread_id: Uuid,
    meta: EventMeta,
    round: usize,
    provider: &dyn LlmProvider,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
    selection: ModelSelection<'_>,
    system_prompt: Option<&str>,
    on_token: Option<TokenCallback>,
) -> Result<(LlmResponse, TurnCapture), BoxError> {
    let slot = TurnSlot::new(bus, thread_id, meta.clone());
    // The turn's row sizes itself from its context sections, so the size
    // the call reports is not read.
    let response = metered::chat(
        &slot,
        provider,
        0,
        messages,
        tools,
        selection,
        system_prompt,
        on_token,
    )
    .await?;
    let TurnCall { model, mut report } = slot.call.into_inner().unwrap_or_else(|p| p.into_inner());
    if round != 1 {
        report.stable_prefix_fingerprint = None;
    }
    let capture = TurnCapture {
        bus: bus.clone(),
        thread_id,
        meta,
        model,
        report: Some(report),
    };
    Ok((response, capture))
}

#[derive(Default)]
struct TurnCall {
    model: String,
    report: TurnReport,
}

/// What a turn's call reported, held until [`TurnCapture`] records it.
#[derive(Default)]
pub(crate) struct TurnReport {
    pub(crate) usage: Option<ApiUsage>,
    pub(crate) detail: CallDetail,
    pub(crate) stable_prefix_fingerprint: Option<String>,
}

/// Holds what a turn round's call reported, and records the call if it fails.
struct TurnSlot {
    bus: EventBus,
    thread_id: Uuid,
    meta: EventMeta,
    call: std::sync::Mutex<TurnCall>,
}

impl TurnSlot {
    fn new(bus: &EventBus, thread_id: Uuid, meta: EventMeta) -> Self {
        Self {
            bus: bus.clone(),
            thread_id,
            meta,
            call: std::sync::Mutex::default(),
        }
    }
}

#[async_trait::async_trait]
impl CostSink for TurnSlot {
    async fn record(&self, cost: CallCost<'_>) {
        let response = match cost.reported {
            Reported::Chat(response) => Some(response),
            _ => None,
        };
        *self.call.lock().unwrap_or_else(|p| p.into_inner()) = TurnCall {
            model: cost.model.to_string(),
            report: TurnReport {
                usage: response.and_then(usage_from_response),
                detail: CallDetail::of(&cost),
                stable_prefix_fingerprint: response
                    .and_then(|response| response.stable_prefix_fingerprint.clone()),
            },
        };
    }

    async fn record_failure(&self, failed: CallFailed<'_>) {
        self.bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id: self.thread_id,
                    event: model_call_failed(ContextPurpose::Turn, &failed),
                    meta: self.meta.clone(),
                },
                "[ModelCall] ModelCallFailed",
            )
            .await;
    }
}

/// The row one turn round owes.
///
/// The loop finishes it with its context sections. One that goes out of scope
/// unfinished, on an early return or a panic, still records its usage from
/// `Drop`. A forgotten finish then costs the row its detail, never the row.
pub(crate) struct TurnCapture {
    bus: EventBus,
    thread_id: Uuid,
    /// The turn's meta, which the finished row and the `Drop` fallback carry.
    meta: EventMeta,
    model: String,
    /// Taken by whichever records the row: [`Self::finish`] or `Drop`.
    report: Option<TurnReport>,
}

impl TurnCapture {
    /// What the provider reported for this round.
    pub(crate) fn usage(&self) -> Option<ApiUsage> {
        self.report.as_ref().and_then(|report| report.usage)
    }

    /// Emit the round's row. `build` receives what the call reported and
    /// returns the `ContextCaptured` to persist.
    pub(crate) async fn finish(mut self, build: impl FnOnce(TurnReport) -> ThreadEvent) {
        let event = build(self.report.take().unwrap_or_default());
        self.bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id: self.thread_id,
                    event,
                    meta: self.meta.clone(),
                },
                "[AgenticLoop] ContextCaptured",
            )
            .await;
    }
}

impl Drop for TurnCapture {
    fn drop(&mut self) {
        let Some(report) = self.report.take() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            crate::log!("[ModelCall] A turn row was dropped with no runtime to record it");
            return;
        };
        let bus = self.bus.clone();
        let thread_id = self.thread_id;
        let meta = self.meta.clone();
        let event = ThreadEvent::ContextCaptured {
            producer: ContextProducer::MainLlm,
            model: std::mem::take(&mut self.model),
            context_window: 0,
            sections: Vec::new(),
            tools: Vec::new(),
            estimated_total_tokens: 0,
            usage: report.usage,
            trimmed: false,
            trim_passes: Vec::new(),
            purpose: ContextPurpose::Turn,
            reconstructed: false,
            parent_tool_use_id: None,
            api_call_id: None,
            reasoning_effort: report.detail.reasoning_effort,
            duration_ms: report.detail.duration_ms,
            served_model: report.detail.served_model,
            provider: report.detail.provider,
            // The round is unknown here, and only round 1 carries one.
            stable_prefix_fingerprint: None,
            ttft_ms: report.detail.ttft_ms,
            request_id: report.detail.request_id,
        };
        runtime.spawn(async move {
            bus.emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event,
                    meta,
                },
                "[ModelCall] unfinished turn row",
            )
            .await;
        });
    }
}

#[cfg(test)]
#[path = "model_call_tests.rs"]
mod tests;
