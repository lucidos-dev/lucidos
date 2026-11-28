mod capture;
pub use crate::engine::event_bus::spend_cap;

pub(crate) use {capture::model_call_failed, capture::usage_from_response};
pub use {capture::AuxCapture, capture::CallDetail};

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
    pub async fn chat<P: LlmProvider + ?Sized>(
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

    pub(crate) async fn prefill<P: LlmProvider + ?Sized>(
        &self,
        provider: &P,
        body: &crate::llm::prefill::PrefillBody,
    ) -> Result<LlmResponse, BoxError> {
        metered::prefill(self, provider, body).await
    }

    pub async fn judge<J: JudgmentProvider + ?Sized>(
        &self,
        judge: &J,
        state: serde_json::Value,
        questions: Vec<(String, Question)>,
    ) -> Result<Judgment, BoxError> {
        metered::judge(self, judge, state, questions).await
    }

    /// INVARIANT: Generate one image and record what it cost.
    ///
    /// Imagen prices per image and reports no tokens, so its row carries no
    /// usage. The row exists either way: an image the engine paid for must
    /// not be missing from the ledger. It names the serving model, never the
    /// provider's display label, since a cost breakdown groups on the model.
    pub async fn generate<I: ImageProvider + ?Sized>(
        &self,
        provider: &I,
        prompt: &str,
        input_images: Vec<Vec<u8>>,
        size: ImageSize,
    ) -> Result<ImageResult, BoxError> {
        metered::generate(self, provider, prompt, input_images, size).await
    }

    pub async fn search(
        &self,
        chain: &WebSearchChain,
        query: &str,
        max_results: usize,
    ) -> Result<String, BoxError> {
        metered::search(self, chain, query, max_results).await
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn turn_chat(
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
    // INVARIANT: The turn's row sizes itself from its context sections, so the size
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

#[derive(Default)]
pub struct TurnReport {
    pub usage: Option<ApiUsage>,
    pub detail: CallDetail,
    pub stable_prefix_fingerprint: Option<String>,
}

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
    fn spend_cap_policy(
        &self,
    ) -> Option<(
        std::sync::Arc<dyn crate::llm::spend_cap::SpendCapPolicy>,
        bool,
    )> {
        Some((std::sync::Arc::new(self.bus.clone()), true))
    }
    async fn record(&self, cost: CallCost<'_>) {
        let response = match cost.reported {
            Reported::Chat(response) => Some(response),
            _ => None,
        };
        let detail = CallDetail::of(&cost);
        *self.call.lock().unwrap_or_else(|p| p.into_inner()) = TurnCall {
            model: cost.model.to_string(),
            report: TurnReport {
                usage: response.and_then(usage_from_response),
                stable_prefix_fingerprint: detail.stable_prefix_fingerprint.clone(),
                detail,
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

/// INVARIANT: The row one turn round owes.
///
/// The loop finishes it with its context sections. One that goes out of scope
/// unfinished, on an early return or a panic, still records its usage from
/// `Drop`. A forgotten finish then costs the row its detail, never the row.
pub struct TurnCapture {
    bus: EventBus,
    thread_id: Uuid,
    meta: EventMeta,
    model: String,
    report: Option<TurnReport>,
}

impl TurnCapture {
    pub fn usage(&self) -> Option<ApiUsage> {
        self.report.as_ref().and_then(|report| report.usage)
    }

    pub async fn finish(mut self, build: impl FnOnce(TurnReport) -> ThreadEvent) {
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
            crate::log_legacy!("[ModelCall] A turn row was dropped with no runtime to record it");
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
            stable_prefix_fingerprint: None,
            ttft_ms: report.detail.ttft_ms,
            request_id: report.detail.request_id,
            telemetry_owned_by_cli: false,
            session: None,
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
