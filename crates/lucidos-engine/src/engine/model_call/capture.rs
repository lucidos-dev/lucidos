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
use crate::llm::judgment::JudgmentUsage;
use crate::llm::metered::{CallCost, CostSink, Reported};
use crate::llm::provider::LlmResponse;
use crate::llm::usage_wire::ProviderUsage;
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
    }
}

/// The usage block from a provider response, mapped exactly as the chat
/// agentic loop maps it. `None` when the provider reported no prompt tokens.
pub(crate) fn usage_from_response(response: &LlmResponse) -> Option<ApiUsage> {
    response.input_tokens.map(|input_tokens| ApiUsage {
        input_tokens,
        output_tokens: response.output_tokens.unwrap_or(0),
        cache_read_tokens: response.cache_read_tokens.unwrap_or(0),
        cache_creation_tokens: response.cache_creation_tokens.unwrap_or(0),
        // Chat providers report one blended total per direction.
        modality: None,
    })
}

/// The usage block from a typed judgment.
///
/// `None` when it reported no input tokens, for the reason
/// [`usage_from_response`] returns `None`: a zeroed block reads as a call that
/// cost nothing. The judgment wire reports an absent count as zero, so the two
/// cases arrive here indistinguishable and take the safer reading.
///
/// The cache counters pass through: zero from a System One endpoint, and what
/// a chat model reported from the chat provider. `modality` stays `None`: both
/// report one total per direction.
pub(crate) fn usage_from_judgment(usage: &JudgmentUsage) -> Option<ApiUsage> {
    (usage.input_tokens > 0).then_some(ApiUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        modality: None,
    })
}

/// The usage block a raw provider body reported, read by `llm::usage_wire`.
pub(crate) fn usage_from_provider(usage: ProviderUsage) -> ApiUsage {
    ApiUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        // Every raw shape it reads reports one blended total per direction.
        modality: None,
    }
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
        modality: None,
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

    /// A capture that records nothing, for a test that drives a model call
    /// with no database behind it.
    #[cfg(test)]
    pub(crate) fn discarding(purpose: ContextPurpose) -> Self {
        Self {
            sink: Sink::Discard,
            thread_id: None,
            purpose,
            deadline: None,
        }
    }

    /// Emit one capture on its thread.
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
            bus.emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event,
                    meta: EventMeta::NONE,
                },
                "[AuxCapture] ContextCaptured",
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
        self.emit(auxiliary_capture(
            self.purpose,
            model,
            request_chars,
            usage,
            false,
        ))
        .await;
    }
}

#[async_trait::async_trait]
impl CostSink for AuxCapture {
    /// One row per call, before its caller reads the answer.
    async fn record(&self, cost: CallCost<'_>) {
        let usage = match cost.reported {
            Reported::Chat(response) => usage_from_response(response),
            Reported::Judgment(judgment) => usage_from_judgment(&judgment.usage),
            Reported::Image {
                input_tokens,
                output_tokens,
            } => usage_from_image(input_tokens, output_tokens),
            Reported::Usage(usage) => usage.map(usage_from_provider),
        };
        let detail = CallDetail {
            reasoning_effort: cost.reasoning_effort.map(str::to_string),
            duration_ms: cost.duration_ms,
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

    /// A response reporting only the token counts the test cares about.
    fn response_reporting(
        input: Option<u32>,
        output: Option<u32>,
        cache_read: Option<u32>,
        cache_write: Option<u32>,
    ) -> LlmResponse {
        LlmResponse {
            content: None,
            tool_calls: vec![],
            stop_reason: None,
            output_tokens: output,
            input_tokens: input,
            cache_creation_tokens: cache_write,
            cache_read_tokens: cache_read,
            thinking_chars: None,
            thinking_blocks: None,
            unknown_sse_dropped: 0,
            model_only_text: None,
            progress_notes: Vec::new(),
        }
    }

    #[test]
    fn usage_maps_every_field_the_provider_reported() {
        let response = response_reporting(Some(1200), Some(30), Some(900), Some(100));
        let usage = usage_from_response(&response).expect("usage present");
        assert_eq!(usage.input_tokens, 1200);
        assert_eq!(usage.output_tokens, 30);
        assert_eq!(usage.cache_read_tokens, 900);
        assert_eq!(usage.cache_creation_tokens, 100);
    }

    /// A provider that reports no prompt tokens yields no usage block, rather
    /// than a zero one that would read as a free call.
    #[test]
    fn a_provider_reporting_nothing_yields_no_usage() {
        assert!(usage_from_response(&response_reporting(None, None, None, None)).is_none());
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

    /// Gemini reports prompt tokens but no cache counters. The absent ones
    /// must read as zero rather than suppressing the whole usage block.
    #[test]
    fn a_partial_report_still_yields_usage() {
        let usage = usage_from_response(&response_reporting(Some(400), Some(12), None, None))
            .expect("usage present");
        assert_eq!(usage.input_tokens, 400);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.cache_creation_tokens, 0);
    }

    /// Jev reports two counts and no cache, so a row carries what it spent and
    /// zeroes it cannot know.
    #[test]
    fn a_judgment_maps_the_counts_the_backend_reported() {
        let usage = usage_from_judgment(&JudgmentUsage {
            input_tokens: 312,
            output_tokens: 48,
            ..JudgmentUsage::default()
        })
        .expect("usage present");
        assert_eq!(usage.input_tokens, 312);
        assert_eq!(usage.output_tokens, 48);
        assert_eq!(usage.cache_read_tokens, 0, "no prompt cache on this wire");
    }

    /// A chat model answering a judgment may read from or write to its prompt
    /// cache, and the rollup prices both. They reach the row unchanged.
    #[test]
    fn a_chat_judgments_cache_counters_reach_the_row() {
        let usage = usage_from_judgment(&JudgmentUsage {
            input_tokens: 40,
            output_tokens: 12,
            cache_read_tokens: 2_048,
            cache_creation_tokens: 512,
        })
        .expect("usage present");
        assert_eq!(usage.cache_read_tokens, 2_048);
        assert_eq!(usage.cache_creation_tokens, 512);
    }

    /// A backend that reported nothing must not read as a free call. The wire
    /// gives an absent count as zero, so zero is where that line is drawn.
    #[test]
    fn a_judgment_reporting_nothing_yields_no_usage() {
        assert!(usage_from_judgment(&JudgmentUsage::default()).is_none());
    }
}
