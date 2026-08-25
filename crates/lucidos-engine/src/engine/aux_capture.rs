//! `ContextCaptured` for an *auxiliary model call*.
//!
//! An auxiliary model call is one the engine makes for itself rather than as
//! an agent's turn: a thread title, an image description, a memory call, an
//! image generation. Each costs real tokens, so each emits a capture and
//! token accounting stops undercounting.
//!
//! This module is the only place that builds one. It pairs
//! [`ContextProducer::Auxiliary`] with the purpose, so the two cannot
//! disagree. `core::aux_context_backfill` reuses it to write a reconstructed
//! row in the same shape.

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::{ApiUsage, ContextProducer, ContextPurpose, ContextRole, ContextSection};
use crate::llm::provider::LlmResponse;
use uuid::Uuid;

/// Build the `ContextCaptured` for one auxiliary call.
///
/// `request_chars` is the true size of what was sent, counted by every caller
/// with `chars().count()`. It becomes the capture's one body-less section,
/// which keeps the "sections sum to the estimate" invariant true.
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
    })
}

/// The usage block for an image call, from what the image provider reported.
///
/// `None` when it reported no input tokens, which is Imagen: it prices per
/// image, so a zeroed block would read as a call that cost nothing. Image
/// endpoints have no prompt cache, so both cache counters are zero.
pub(crate) fn usage_from_image(
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
) -> Option<ApiUsage> {
    input_tokens.map(|input_tokens| ApiUsage {
        input_tokens,
        output_tokens: output_tokens.unwrap_or(0),
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
    })
}

/// A thread to attach auxiliary captures to, and the job they belong to.
///
/// Held by the call sites, which pass it down as `Option<&AuxCapture>`. `None`
/// means there is no thread to anchor to, and then nothing is emitted: a
/// memory rebuild and artifact indexing both extract facts with no thread in
/// scope, and inventing an id would be worse than the missing row.
///
/// Owns its `EventBus` clone so a spawned task can hold one. Image
/// description and title generation both run detached from the turn.
#[derive(Clone)]
pub(crate) struct AuxCapture {
    bus: EventBus,
    thread_id: Uuid,
    purpose: ContextPurpose,
}

impl AuxCapture {
    pub(crate) fn new(bus: &EventBus, thread_id: Uuid, purpose: ContextPurpose) -> Self {
        Self {
            bus: bus.clone(),
            thread_id,
            purpose,
        }
    }

    /// `None` when no thread anchors the call, so a caller holding an
    /// `Option<Uuid>` does not have to branch.
    pub(crate) fn for_thread(
        bus: &EventBus,
        thread_id: Option<Uuid>,
        purpose: ContextPurpose,
    ) -> Option<Self> {
        thread_id.map(|id| Self::new(bus, id, purpose))
    }

    /// Record one round trip. Called per attempt, so a resample or a retry is
    /// counted: both spent tokens, whatever the caller did with the answer.
    ///
    /// Never fails the call it observes. The emit is fire-and-forget, because
    /// losing a title over a bookkeeping row would be the worse trade.
    pub(crate) async fn record(&self, model: &str, request_chars: usize, response: &LlmResponse) {
        self.record_usage(model, request_chars, usage_from_response(response))
            .await;
    }

    /// Record a call whose usage came from somewhere other than an
    /// `LlmResponse`, such as an image provider.
    pub(crate) async fn record_usage(
        &self,
        model: &str,
        request_chars: usize,
        usage: Option<ApiUsage>,
    ) {
        self.bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id: self.thread_id,
                    event: auxiliary_capture(self.purpose, model, request_chars, usage, false),
                    meta: EventMeta::NONE,
                },
                "[AuxCapture] ContextCaptured",
            )
            .await;
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
        for purpose in [
            ContextPurpose::Title,
            ContextPurpose::ImageDescribe,
            ContextPurpose::Memory,
            ContextPurpose::ConversationSummary,
            ContextPurpose::ImageGen,
        ] {
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
            unknown_sse_dropped: 0,
            model_only_text: None,
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
}
