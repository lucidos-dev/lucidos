use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::{ApiUsage, ContextPurpose};
use crate::llm::metered::{CallCost, CallFailed, CostSink, Reported};
use crate::llm::provider::LlmResponse;
use uuid::Uuid;

#[cfg(test)]
use crate::engine::thread_events::aux_capture::auxiliary_capture;
pub(crate) use crate::engine::thread_events::aux_capture::auxiliary_capture_detailed;
pub use crate::engine::thread_events::aux_capture::CallDetail;

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

pub(crate) fn usage_from_response(response: &LlmResponse) -> Option<ApiUsage> {
    response
        .usage
        .filter(|u| u.input_tokens > 0 || u.output_tokens > 0)
}

fn billed(usage: Option<ApiUsage>) -> Option<ApiUsage> {
    usage.filter(|u| u.input_tokens > 0)
}

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

#[derive(Clone)]
pub struct AuxCapture {
    sink: Sink,
    thread_id: Option<Uuid>,
    purpose: ContextPurpose,
    deadline: Option<tokio::time::Instant>,
    emitting_trigger_id: Option<String>,
}

#[derive(Clone)]
enum Sink {
    Bus(EventBus),
    #[cfg(any(test, feature = "test-support"))]
    Discard,
}

impl AuxCapture {
    pub fn new(bus: &EventBus, thread_id: Uuid, purpose: ContextPurpose) -> Self {
        Self::for_thread_or_home(bus, Some(thread_id), purpose)
    }

    pub fn for_thread_or_home(
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

    /// INVARIANT: This capture, with each provider call bounded by `deadline`. The bound
    /// covers the call alone: a call that answered in time keeps its answer
    /// and its row, however long the record then takes.
    pub fn until(&self, deadline: tokio::time::Instant) -> Self {
        Self {
            deadline: Some(deadline),
            ..self.clone()
        }
    }

    /// INVARIANT: This capture, with its rows stamped as `trigger`'s fire. The record
    /// runs on its own task, which the fire's ambient marker never reaches.
    /// So a caller that knows the fire states it here.
    pub fn for_trigger_fire(self, trigger: Option<String>) -> Self {
        Self {
            emitting_trigger_id: trigger,
            ..self
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn discarding(purpose: ContextPurpose) -> Self {
        Self {
            sink: Sink::Discard,
            thread_id: None,
            purpose,
            deadline: None,
            emitting_trigger_id: None,
        }
    }

    /// INVARIANT: Emit one row on its thread.
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
            #[cfg(any(test, feature = "test-support"))]
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
                            crate::log_legacy!("[AuxCapture] No home thread to record on: {}", e);
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
            crate::log_legacy!("[AuxCapture] The capture task failed: {}", e);
        }
    }

    pub async fn record_usage(&self, model: &str, request_chars: usize, usage: Option<ApiUsage>) {
        self.record_usage_detailed(model, request_chars, usage, CallDetail::default())
            .await;
    }

    pub async fn record_usage_detailed(
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
    fn spend_cap_policy(
        &self,
    ) -> Option<(
        std::sync::Arc<dyn crate::llm::spend_cap::SpendCapPolicy>,
        bool,
    )> {
        match &self.sink {
            Sink::Bus(bus) => Some((std::sync::Arc::new(bus.clone()), false)),
            #[cfg(any(test, feature = "test-support"))]
            Sink::Discard => None,
        }
    }

    async fn record(&self, cost: CallCost<'_>) {
        let detail = CallDetail::of(&cost);
        let usage = match cost.reported {
            Reported::Chat(response) => usage_from_response(response),
            Reported::Judgment(judgment) => billed(judgment.usage),
            Reported::Image {
                input_tokens,
                output_tokens,
            } => usage_from_image(input_tokens, output_tokens),
            Reported::Search(result) => result.usage,
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
    use crate::engine::ContextProducer;

    fn capture_fields(event: &ThreadEvent) -> (ContextProducer, ContextPurpose) {
        match event {
            ThreadEvent::ContextCaptured {
                producer, purpose, ..
            } => (*producer, *purpose),
            other => panic!("expected ContextCaptured, got {:?}", other),
        }
    }

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

    #[test]
    fn a_provider_reporting_nothing_yields_no_usage() {
        assert!(usage_from_response(&LlmResponse::default()).is_none());
        let zeroed = LlmResponse {
            usage: Some(ApiUsage::default()),
            ..LlmResponse::default()
        };
        assert!(usage_from_response(&zeroed).is_none());
    }

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

    #[test]
    fn an_image_provider_that_reports_nothing_yields_no_usage() {
        assert!(usage_from_image(None, None).is_none());
    }

    #[test]
    fn a_judgment_reporting_nothing_yields_no_usage() {
        assert!(billed(None).is_none());
        assert!(billed(Some(ApiUsage::default())).is_none());
    }
}
