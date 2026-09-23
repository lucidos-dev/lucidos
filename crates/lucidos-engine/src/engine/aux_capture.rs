//! `ContextCaptured` for an *auxiliary model call*.
//!
//! An auxiliary model call is one the engine makes for itself rather than as
//! an agent's turn: a thread title, an image description, a memory call, an
//! image generation, a typed judgment. Each costs real tokens, so each emits a
//! capture and token accounting stops undercounting.
//!
//! This module is the only place that builds one. It pairs
//! [`ContextProducer::Auxiliary`] with the purpose, so the two cannot
//! disagree. `core::aux_context_backfill` reuses it to write a reconstructed
//! row in the same shape.
//!
//! **Every model call the engine makes records what it cost** (ADR 0242).
//! [`every_file_that_calls_a_model_records_the_cost`] holds that over the
//! source tree. What it catches is a model call in a file nobody was reading,
//! which is the shape every unrecorded call so far has had.

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::{ApiUsage, ContextProducer, ContextPurpose, ContextRole, ContextSection};
use crate::llm::judgment::{Judgment, JudgmentUsage, JEV_DEFAULT_MODEL};
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
/// Both cache counters are zero. The endpoint has no prompt cache, and
/// `modality` stays `None`: it reports one total per direction.
pub(crate) fn usage_from_judgment(usage: &JudgmentUsage) -> Option<ApiUsage> {
    (usage.input_tokens > 0).then_some(ApiUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        modality: None,
    })
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

    /// Record one typed judgment.
    ///
    /// The provider names the model and sizes the request, so no call site
    /// restates either. [`JEV_DEFAULT_MODEL`] is the fallback for a response
    /// that named no model: it is the alias the request asked for, which stays
    /// true even where it is less precise than the version that answered.
    pub(crate) async fn record_judgment(&self, judgment: &Judgment) {
        self.record_usage(
            judgment.model.as_deref().unwrap_or(JEV_DEFAULT_MODEL),
            judgment.request_chars,
            usage_from_judgment(&judgment.usage),
        )
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

/// Source shapes that mean a model was called on this line.
///
/// One method per billable provider trait: `LlmProvider::chat`,
/// `JudgmentProvider::ask` and `ImageProvider::generate`.
///
/// **They are bare substrings, so they over-match**, and that is the trade a
/// tripwire wants. Any method with one of those names hits, which `voice`'s own
/// `ask` already does. A false hit is loud and costs a reader a minute; a miss
/// is spend nobody sees. The failure message names both ways out.
///
/// **`EmbeddingProvider::embed` is deliberately absent.** Embedding runs
/// in-process on fastembed, so there is nothing to bill and nothing to record.
/// What holds that true is [`the_only_embedders_run_in_process`], not this
/// list: a remote embedder fails that test, and whoever adds one decides about
/// capture then.
#[cfg(test)]
const MODEL_CALL_SHAPES: &[&str] = &[".chat(", ".ask(", ".generate("];

/// Paths the audit does not read, and why.
///
/// `llm/` is the provider layer itself, where a call is the implementation
/// rather than a caller. `test_support.rs` holds the crate's test doubles, and
/// `production_sources` does not drop it: its name matches no test convention,
/// and its inner `#![cfg(test)]` is not the marker that truncates a file.
///
/// `bin/` and every `*_tests.rs` need no entry, since `production_sources`
/// drops those itself.
#[cfg(test)]
const UNWALKED: &[&str] = &["llm/", "test_support.rs"];

/// A file that calls a model and records nothing, with the reason it may.
///
/// **Empty, and it should stay that way.** A new row means some real spend went
/// invisible to the Token Cost app again, which is the whole failure this
/// module exists to prevent. Adding one is a decision, not a formality.
#[cfg(test)]
const UNRECORDED: &[(&str, &str)] = &[];

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
            ContextPurpose::QueryClassification,
            ContextPurpose::ImageGen,
            ContextPurpose::CommandJudge,
            ContextPurpose::JudgeTool,
            ContextPurpose::IntentLoop,
            ContextPurpose::MemoryCorrection,
            ContextPurpose::ArtifactSummary,
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
            thinking_blocks: None,
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

    /// Jev reports two counts and no cache, so a row carries what it spent and
    /// zeroes it cannot know.
    #[test]
    fn a_judgment_maps_the_counts_the_backend_reported() {
        let usage = usage_from_judgment(&JudgmentUsage {
            input_tokens: 312,
            output_tokens: 48,
        })
        .expect("usage present");
        assert_eq!(usage.input_tokens, 312);
        assert_eq!(usage.output_tokens, 48);
        assert_eq!(usage.cache_read_tokens, 0, "no prompt cache on this wire");
    }

    /// A backend that reported nothing must not read as a free call. The wire
    /// gives an absent count as zero, so zero is where that line is drawn.
    #[test]
    fn a_judgment_reporting_nothing_yields_no_usage() {
        assert!(usage_from_judgment(&JudgmentUsage::default()).is_none());
    }

    // --- the audit ----------------------------------------------------------

    /// Every engine source the audit judges: production text only, minus the
    /// provider layer. `production_sources` supplies the rest of the rule.
    fn walked_sources() -> Vec<(String, String)> {
        crate::test_support::source_scan::production_sources()
            .into_iter()
            .filter(|(path, _)| !UNWALKED.iter().any(|skip| path.starts_with(skip)))
            .collect()
    }

    /// **A file that calls a model records what the call cost.**
    ///
    /// The standing invariant, checked over the source rather than at runtime.
    /// It is per FILE, so what it catches is a model call arriving somewhere
    /// nobody was reading. A second uncaptured call inside a file that already
    /// captures is left to review, where the missing line is in the diff.
    #[test]
    fn every_file_that_calls_a_model_records_the_cost() {
        let callers: Vec<(String, String)> = walked_sources()
            .into_iter()
            .filter(|(_, body)| MODEL_CALL_SHAPES.iter().any(|shape| body.contains(shape)))
            .collect();
        // A walker that found nothing would pass this test for the wrong
        // reason, so pin one caller everybody knows.
        assert!(
            callers
                .iter()
                .any(|(path, _)| path.ends_with("command_judge.rs")),
            "the walker found no known model caller, so it is auditing nothing"
        );

        let silent: Vec<String> = callers
            .into_iter()
            .filter(|(_, body)| !body.contains("AuxCapture") && !body.contains("ContextCaptured"))
            .map(|(path, _)| path)
            .filter(|path| !UNRECORDED.iter().any(|(declared, _)| declared == path))
            .collect();
        assert!(
            silent.is_empty(),
            "these look like they call a model and record nothing: {silent:?}. \
             If the call spends tokens, record it through AuxCapture under a \
             ContextPurpose of its own, or declare it in UNRECORDED with the \
             reason. If the name only LOOKS like a provider method, add the \
             file to UNWALKED instead: MODEL_CALL_SHAPES over-matches by design"
        );
    }

    /// The exemption table is the audit's escape hatch, and an empty one is the
    /// claim worth keeping. Deleting this test is easier than earning a row,
    /// which is the point of writing the count down.
    #[test]
    fn nothing_is_exempt_from_recording_its_spend() {
        assert!(
            UNRECORDED.is_empty(),
            "a model call was exempted from recording: {UNRECORDED:?}"
        );
    }

    /// What keeps `.embed(`'s absence from [`MODEL_CALL_SHAPES`] honest.
    ///
    /// Embedding costs nothing today because it runs in-process. A remote
    /// embedder would be billable spend arriving with no purpose and no row, so
    /// it fails here first and the decision gets made deliberately.
    #[test]
    fn the_only_embedders_run_in_process() {
        let found: Vec<String> = walked_sources()
            .into_iter()
            .filter(|(_, body)| body.contains("impl EmbeddingProvider for"))
            .map(|(path, _)| path)
            .collect();
        assert_eq!(
            found,
            vec!["memory/embedder_slot.rs", "memory/fastembed.rs"],
            "an embedding provider was added or moved. One calling a remote \
             endpoint spends real tokens, so it needs a ContextPurpose and \
             `.embed(` in MODEL_CALL_SHAPES"
        );
    }
}
