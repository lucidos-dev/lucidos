use crate::llm::api_usage::ApiUsage;
use crate::llm::metered::{CallCost, Reported};

use super::{ContextProducer, ContextPurpose, ContextRole, ContextSection, ThreadEvent};

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

#[derive(Clone, Debug, Default)]
pub struct CallDetail {
    pub reasoning_effort: Option<String>,
    pub duration_ms: Option<u64>,
    pub served_model: Option<String>,
    pub provider: Option<String>,
    pub telemetry_owned_by_cli: bool,
    pub ttft_ms: Option<u64>,
    pub request_id: Option<String>,
    pub stable_prefix_fingerprint: Option<String>,
}

impl CallDetail {
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
