//! `ContextCaptured` for an *auxiliary model call*, and where it lands.
//!
//! An auxiliary model call is one that is not an agent's turn: a thread title,
//! an image description, a memory call, an image generation, a typed judgment,
//! or a call an app made through the proxy. Each costs real tokens, so each
//! emits a capture and token accounting stops undercounting.
//!
//! This module builds every one. It pairs [`ContextProducer::Auxiliary`] with
//! the purpose, so the two cannot disagree. `core::aux_context_backfill`
//! reuses it to write a reconstructed row in the same shape. The calls that
//! emit them live in the parent module, the model call service.

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::{ApiUsage, ContextProducer, ContextPurpose, ContextRole, ContextSection};
use crate::llm::metered::{CallCost, CallFailed, CostSink, Reported};
use crate::llm::provider::LlmResponse;
use uuid::Uuid;

/// Build the `ContextCaptured` for one auxiliary call.
///
/// `request_chars` is the size of what was sent, as `llm::metered::CallCost`
/// defines it. It becomes the capture's one body-less section, which keeps the
/// "sections sum to the estimate" invariant true.
///
/// One section means nothing else counts those chars, so the section's budget
/// delta is its own size and the two sizes agree. The body is never persisted
/// here, which does not move `content_chars`: it measures what was sent.
///
/// `context_window` is 0. A single-shot auxiliary call spends against no
/// budget, so any other number would be a plausible-looking invention.
pub(crate) fn auxiliary_capture(
    purpose: ContextPurpose,
    model: &str,
    request_chars: usize,
    usage: Option<ApiUsage>,
    reconstructed: bool,
) -> ThreadEvent {
    auxiliary_capture_detailed(
        purpose,
        model,
        request_chars,
        usage,
        reconstructed,
        CallDetail::default(),
    )
}

/// What the service knows about one call beyond its usage. Every
/// [`AuxCapture::chat`] reports both, from the selection and its own clock.
#[derive(Clone, Debug, Default)]
pub(crate) struct CallDetail {
    /// The reasoning tier the call ran at.
    pub(crate) reasoning_effort: Option<String>,
    /// How long the call took, wall clock.
    pub(crate) duration_ms: Option<u64>,
    /// The model the reply named, when it named one.
    pub(crate) served_model: Option<String>,
    /// The backend the router sent the call to.
    pub(crate) provider: Option<String>,
    /// Time to the first content delta, from the start `duration_ms` counts from.
    pub(crate) ttft_ms: Option<u64>,
    /// The provider's id for the response that answered.
    pub(crate) request_id: Option<String>,
    pub(crate) stable_prefix_fingerprint: Option<String>,
}

impl CallDetail {
    /// What `cost` says about its call beyond the usage.
    pub(crate) fn of(cost: &CallCost<'_>) -> Self {
        Self {
            reasoning_effort: cost.reasoning_effort.map(str::to_string),
            duration_ms: cost.duration_ms,
            served_model: cost.reported.served_model().map(str::to_string),
            provider: cost
                .reported
                .provider()
                .or(cost.provider)
                .map(|kind| kind.as_str().to_string()),
            ttft_ms: cost.ttft_ms,
            request_id: cost.request_id.map(str::to_string),
            stable_prefix_fingerprint: match cost.reported {
                Reported::Chat(response) => response.stable_prefix_fingerprint.clone(),
                _ => None,
            },
        }
    }
}

/// [`auxiliary_capture`], with what the caller knows about the call.
pub(crate) fn auxiliary_capture_detailed(
    purpose: ContextPurpose,
    model: &str,
    request_chars: usize,
    usage: Option<ApiUsage>,
    reconstructed: bool,
    detail: CallDetail,
) -> ThreadEvent {
    let stable_prefix_fingerprint = detail
        .stable_prefix_fingerprint
        .filter(|_| purpose == ContextPurpose::CacheKeepAlive);
    ThreadEvent::ContextCaptured {
        producer: ContextProducer::Auxiliary,
        model: model.to_string(),
        context_window: 0,
        sections: vec![ContextSection {
            name: purpose.section_name().to_string(),
            content: None,
            budget_delta_chars: request_chars,
            content_chars: Some(request_chars),
            role: ContextRole::User,
            group: None,
        }],
        tools: vec![],
        estimated_total_tokens: crate::engine::context::estimate_tokens_from_chars(request_chars),
        usage,
        trimmed: false,
        // This path never runs the trimmer, so no pass can have fired.
        trim_passes: Vec::new(),
        purpose,
        reconstructed,
        parent_tool_use_id: None,
        api_call_id: None,
        reasoning_effort: detail.reasoning_effort,
        duration_ms: detail.duration_ms,
        served_model: detail.served_model,
        provider: detail.provider,
        stable_prefix_fingerprint,
        ttft_ms: detail.ttft_ms,
        request_id: detail.request_id,
        session: None,
    }
}

/// The `ModelCallFailed` for one failed call. It holds facts only, never the
/// provider's message.
pub(crate) fn model_call_failed(purpose: ContextPurpose, failed: &CallFailed<'_>) -> ThreadEvent {
    ThreadEvent::ModelCallFailed {
        model: failed.model.to_string(),
        purpose,
        provider: failed.provider.map(|kind| kind.as_str().to_string()),
        reasoning_effort: failed.reasoning_effort.map(str::to_string),
        duration_ms: failed.duration_ms,
        attempts: failed.attempts,
        failure: failed.failure,
        status_code: failed.status_code,
        request_id: failed.request_id.map(str::to_string),
    }
}

/// The usage a provider response reported. `None` when it named no tokens in
/// either direction, since a zeroed block would read as a call that cost
/// nothing. A zero prompt with output is kept: a local server answering from
/// its KV cache reports exactly that.
pub(crate) fn usage_from_response(response: &LlmResponse) -> Option<ApiUsage> {
    response
        .usage
        .filter(|u| u.input_tokens > 0 || u.output_tokens > 0)
}

/// `usage` unless it names no prompt tokens. A judgment backend reports an
/// absent prompt count as zero. Every judgment has a prompt, so zero is where
/// a report of nothing is drawn.
fn billed(usage: Option<ApiUsage>) -> Option<ApiUsage> {
    usage.filter(|u| u.input_tokens > 0)
}

/// The usage block for an image call, from what the image provider reported.
///
/// `None` when it reported no input tokens, which is Imagen: it prices per
/// image, so a zeroed block would read as a call that cost nothing. Image
/// endpoints have no prompt cache, so both cache counters are zero.
/// `modality` stays `None`: these endpoints report one total per direction.
pub(crate) fn usage_from_image(
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
) -> Option<ApiUsage> {
    input_tokens.map(|input_tokens| ApiUsage {
        input_tokens,
        output_tokens: output_tokens.unwrap_or(0),
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        ..ApiUsage::default()
    })
}

/// The thread auxiliary captures land on, and the job they belong to.
///
/// **Every capture lands on a thread.** A call made for a thread records
/// there. A call no thread caused records on the workspace's home thread
/// (ADR 0242): the compactor on an artifact leaf, the memory indexer on an
/// artifact, a `find` from outside any thread. Home never ends and nothing
/// can delete it. So the row has a stable place.
///
/// Owns its `EventBus` clone so a spawned task can hold one. Image
/// description and title generation both run detached from the turn.
#[derive(Clone)]
pub(crate) struct AuxCapture {
    sink: Sink,
    /// `None` records on the home thread.
    thread_id: Option<Uuid>,
    purpose: ContextPurpose,
    /// When each provider call must have answered. See [`Self::until`].
    deadline: Option<tokio::time::Instant>,
    /// The trigger whose fire made the call. See [`Self::for_trigger_fire`].
    emitting_trigger_id: Option<String>,
}

/// Where a capture goes. Only a test can make one that goes nowhere.
#[derive(Clone)]
enum Sink {
    Bus(EventBus),
    #[cfg(test)]
    Discard,
}

impl AuxCapture {
    pub(crate) fn new(bus: &EventBus, thread_id: Uuid, purpose: ContextPurpose) -> Self {
        Self::for_thread_or_home(bus, Some(thread_id), purpose)
    }

    /// Records on `thread_id`, or on the home thread when there is none.
    pub(crate) fn for_thread_or_home(
        bus: &EventBus,
        thread_id: Option<Uuid>,
        purpose: ContextPurpose,
    ) -> Self {
        Self {
            sink: Sink::Bus(bus.clone()),
            thread_id,
            purpose,
            deadline: None,
            emitting_trigger_id: None,
        }
    }

    /// This capture, with each provider call bounded by `deadline`. The bound
    /// covers the call alone: a call that answered in time keeps its answer
    /// and its row, however long the record then takes.
    pub(crate) fn until(&self, deadline: tokio::time::Instant) -> Self {
        Self {
            deadline: Some(deadline),
            ..self.clone()
        }
    }

    /// This capture, with its rows stamped as `trigger`'s fire. The record
    /// runs on its own task, which the fire's ambient marker never reaches.
    /// So a caller that knows the fire states it here.
    pub(crate) fn for_trigger_fire(self, trigger: Option<String>) -> Self {
        Self {
            emitting_trigger_id: trigger,
            ..self
        }
    }

    /// A capture that records nothing, for a test that drives a model call
    /// with no database behind it.
    #[cfg(test)]
    pub(crate) fn discarding(purpose: ContextPurpose) -> Self {
        Self {
            sink: Sink::Discard,
            thread_id: None,
            purpose,
            deadline: None,
            emitting_trigger_id: None,
        }
    }

    /// Emit one row on its thread.
    ///
    /// The emit runs as its own task, which this call awaits. A caller that
    /// a timeout or a cancel drops mid-record leaves it running. So a deadline
    /// around a model call never costs the call its row.
    ///
    /// The home thread resolves at record time, and is created if boot has not
    /// made it yet. A failed lookup drops this one row with a log
    /// line: a bookkeeping row never fails the call it observes.
    async fn emit(&self, event: ThreadEvent) {
        let bus = match &self.sink {
            Sink::Bus(bus) => bus.clone(),
            #[cfg(test)]
            Sink::Discard => return,
        };
        let thread_id = self.thread_id;
        let trigger = self.emitting_trigger_id.clone();
        let task = tokio::spawn(async move {
            let thread_id = match thread_id {
                Some(id) => id,
                None => {
                    match crate::engine::home_thread::ensure_home_thread(&bus, bus.pool()).await {
                        Ok(id) => id,
                        Err(e) => {
                            crate::log!("[AuxCapture] No home thread to record on: {}", e);
                            return;
                        }
                    }
                }
            };
            bus.emit_or_log_as_trigger(
                BusEvent::Thread {
                    thread_id,
                    event,
                    meta: EventMeta::NONE,
                },
                "[AuxCapture] row",
                trigger,
            )
            .await;
        });
        if let Err(e) = task.await {
            crate::log!("[AuxCapture] The capture task failed: {}", e);
        }
    }

    /// Record spend reported outside a provider call: a voice talker turn, a
    /// coding agent's side question, a call the proxy forwarded.
    pub(crate) async fn record_usage(
        &self,
        model: &str,
        request_chars: usize,
        usage: Option<ApiUsage>,
    ) {
        self.record_usage_detailed(model, request_chars, usage, CallDetail::default())
            .await;
    }

    /// [`Self::record_usage`], with what the caller knows about the call.
    pub(crate) async fn record_usage_detailed(
        &self,
        model: &str,
        request_chars: usize,
        usage: Option<ApiUsage>,
        detail: CallDetail,
    ) {
        self.emit(auxiliary_capture_detailed(
            self.purpose,
            model,
            request_chars,
            usage,
            false,
            detail,
        ))
        .await;
    }
}

#[async_trait::async_trait]
impl CostSink for AuxCapture {
    /// One row per call, before its caller reads the answer.
    async fn record(&self, cost: CallCost<'_>) {
        let detail = CallDetail::of(&cost);
        let usage = match cost.reported {
            Reported::Chat(response) => usage_from_response(response),
            Reported::Judgment(judgment) => billed(judgment.usage),
            Reported::Image {
                input_tokens,
                output_tokens,
            } => usage_from_image(input_tokens, output_tokens),
            Reported::Usage(usage) => usage,
        };
        self.emit(auxiliary_capture_detailed(
            self.purpose,
            cost.model,
            cost.request_chars,
            usage,
            false,
            detail,
        ))
        .await;
    }

    async fn record_failure(&self, failed: CallFailed<'_>) {
        self.emit(model_call_failed(self.purpose, &failed)).await;
    }

    fn deadline(&self) -> Option<tokio::time::Instant> {
        self.deadline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture_fields(event: &ThreadEvent) -> (ContextProducer, ContextPurpose) {
        match event {
            ThreadEvent::ContextCaptured {
                producer, purpose, ..
            } => (*producer, *purpose),
            other => panic!("expected ContextCaptured, got {:?}", other),
        }
    }

    /// The pairing this module exists to guarantee. A `main_llm` row carrying
    /// a title purpose would file background spend under the agent's line.
    #[test]
    fn every_purpose_pairs_with_the_auxiliary_producer() {
        for &purpose in crate::engine::aux_purpose::ALL_PURPOSES {
            if purpose == ContextPurpose::Turn {
                continue;
            }
            let event = auxiliary_capture(purpose, "gemini-3-flash-preview", 100, None, false);
            let (producer, stamped) = capture_fields(&event);
            assert_eq!(producer, ContextProducer::Auxiliary);
            assert_eq!(stamped, purpose);
        }
    }

    /// The section is what keeps `ContextCaptured`'s documented invariant
    /// true: the sections sum to the chars behind the headline estimate.
    #[test]
    fn the_single_section_accounts_for_the_whole_estimate() {
        let event = auxiliary_capture(ContextPurpose::Title, "gpt-5.4", 2500, None, false);
        let ThreadEvent::ContextCaptured {
            sections,
            estimated_total_tokens,
            context_window,
            ..
        } = &event
        else {
            panic!("expected ContextCaptured");
        };
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].budget_delta_chars, 2500);
        assert_eq!(
            sections[0].content_chars,
            Some(2500),
            "one section, so the delta is its own size and the two agree"
        );
        assert!(sections[0].content.is_none(), "no body on an aux section");
        assert_eq!(
            *estimated_total_tokens,
            crate::engine::context::estimate_tokens_from_chars(2500)
        );
        assert_eq!(*context_window, 0, "an aux call spends against no budget");
    }

    /// The usage block carries every field that says how long the write lives.
    /// So a capture's TTL is the one the response reported.
    #[test]
    fn the_usage_carries_the_write_split_and_requested_ttl() {
        use crate::engine::cache_ttl::{CacheLifetime, CacheTtl};
        let base = ApiUsage {
            input_tokens: 16_500,
            output_tokens: 9,
            cache_creation_tokens: 16_000,
            ..ApiUsage::default()
        };
        let usage_of = |usage: ApiUsage| {
            usage_from_response(&LlmResponse {
                usage: Some(usage),
                ..LlmResponse::default()
            })
            .expect("usage")
        };
        let thirty = usage_of(ApiUsage {
            cache_creation_30m_tokens: Some(1_000),
            cache_creation_12h_tokens: Some(15_000),
            ..base
        });
        assert_eq!(thirty.cache_creation_30m_tokens, Some(1_000));
        assert_eq!(thirty.cache_creation_12h_tokens, Some(15_000));
        assert_eq!(
            CacheTtl::of_write(&thirty),
            CacheTtl::Reported(CacheLifetime::ThirtyMinutes)
        );
        let twelve = usage_of(ApiUsage {
            cache_creation_12h_tokens: Some(16_000),
            ..base
        });
        assert_eq!(
            CacheTtl::of_write(&twelve),
            CacheTtl::Reported(CacheLifetime::TwelveHours)
        );
        let requested = usage_of(ApiUsage {
            cache_creation_requested_ttl: Some(CacheLifetime::FiveMinutes),
            ..base
        });
        assert_eq!(
            CacheTtl::of_write(&requested),
            CacheTtl::Requested(CacheLifetime::FiveMinutes)
        );
        assert_eq!(CacheTtl::of_write(&usage_of(base)), CacheTtl::Unknown);
    }

    /// A provider that reports no usage, or only zero counts, yields no usage
    /// block. A zero block would read as a free call.
    #[test]
    fn a_provider_reporting_nothing_yields_no_usage() {
        assert!(usage_from_response(&LlmResponse::default()).is_none());
        let zeroed = LlmResponse {
            usage: Some(ApiUsage::default()),
            ..LlmResponse::default()
        };
        assert!(usage_from_response(&zeroed).is_none());
    }

    /// A zero prompt with output still bills its output tokens.
    #[test]
    fn a_zero_prompt_with_output_keeps_its_usage() {
        let response = LlmResponse {
            usage: Some(ApiUsage {
                output_tokens: 250,
                ..ApiUsage::default()
            }),
            ..LlmResponse::default()
        };
        assert_eq!(
            usage_from_response(&response).map(|u| u.output_tokens),
            Some(250)
        );
    }

    /// OpenAI's image endpoints report tokens, so an image call is accounted
    /// in the same units as every other row.
    #[test]
    fn an_image_provider_that_reports_tokens_yields_usage() {
        let usage = usage_from_image(Some(1_536), Some(4_160)).expect("usage present");
        assert_eq!(usage.input_tokens, 1_536);
        assert_eq!(usage.output_tokens, 4_160);
        assert_eq!(
            usage.cache_read_tokens, 0,
            "image calls have no prompt cache"
        );
    }

    /// Imagen prices per image and reports nothing. The row is still emitted
    /// by the caller, but it must not claim a zero-token call.
    #[test]
    fn an_image_provider_that_reports_nothing_yields_no_usage() {
        assert!(usage_from_image(None, None).is_none());
    }

    /// A backend that reported nothing must not read as a free call. The wire
    /// gives an absent count as zero, so zero is where that line is drawn.
    #[test]
    fn a_judgment_reporting_nothing_yields_no_usage() {
        assert!(billed(None).is_none());
        assert!(billed(Some(ApiUsage::default())).is_none());
    }
}
