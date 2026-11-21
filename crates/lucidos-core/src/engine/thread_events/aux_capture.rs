//! Build the persisted context capture for an auxiliary model call.

use crate::llm::api_usage::ApiUsage;
use crate::llm::metered::{CallCost, Reported};

use super::{ContextProducer, ContextPurpose, ContextRole, ContextSection, ThreadEvent};

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
pub fn auxiliary_capture(
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
/// `AuxCapture::chat` reports both, from the selection and its own clock.
#[derive(Clone, Debug, Default)]
pub struct CallDetail {
    /// The reasoning tier the call ran at.
    pub reasoning_effort: Option<String>,
    /// How long the call took, wall clock.
    pub duration_ms: Option<u64>,
    /// The model the reply named, when it named one.
    pub served_model: Option<String>,
    /// The backend the router sent the call to.
    pub provider: Option<String>,
    /// The reporting CLI process owns telemetry for this call.
    pub telemetry_owned_by_cli: bool,
    /// Time to the first content delta, from the start `duration_ms` counts from.
    pub ttft_ms: Option<u64>,
    /// The provider's id for the response that answered.
    pub request_id: Option<String>,
    pub stable_prefix_fingerprint: Option<String>,
}

impl CallDetail {
    /// What `cost` says about its call beyond the usage.
    pub fn of(cost: &CallCost<'_>) -> Self {
        Self {
            reasoning_effort: cost.reasoning_effort.map(str::to_string),
            duration_ms: cost.duration_ms,
            served_model: cost.reported.served_model().map(str::to_string),
            provider: cost
                .reported
                .provider()
                .or(cost.provider.map(|kind| kind.as_str()))
                .map(str::to_string),
            telemetry_owned_by_cli: false,
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
pub fn auxiliary_capture_detailed(
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
        estimated_total_tokens: crate::llm::context_tokens::estimate_tokens_from_chars(
            request_chars,
        ),
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
        telemetry_owned_by_cli: detail.telemetry_owned_by_cli,
        session: None,
    }
}
