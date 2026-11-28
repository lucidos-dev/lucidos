use crate::llm::model_registry::ProviderKind;
use serde::Serialize;

/// INVARIANT: Build the `generationConfig.thinkingConfig` for a Gemini call. Gemini 3.x and 4
/// models get a thinking config; everything else returns `None` (unchanged).
///
/// Gemini 3.x and 4 expect `thinkingLevel`. The deprecated `thinkingBudget`
/// can return 400 or degrade quality. With `includeThoughts`, the response
/// marks deliberation as `thought:true` so the reply mapper can strip it.
pub(super) fn gemini_generation_config(
    model: &str,
    reasoning_effort: Option<&str>,
) -> Option<VertexGenerationConfig> {
    if !model.starts_with("gemini-3")
        && crate::llm::model_registry::unified_output_limit(model).is_none()
    {
        return None;
    }
    Some(VertexGenerationConfig {
        max_output_tokens: crate::llm::model_registry::unified_output_limit(model),
        thinking_config: VertexThinkingConfig {
            thinking_level: reasoning_effort.map(|effort| gemini_thinking_level(model, effort)),
            include_thoughts: true,
        },
    })
}

pub fn sdk_request_defaults(model: &str, effort: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({});
    if let Some(config) = gemini_generation_config(model, effort) {
        body["generationConfig"] =
            serde_json::to_value(config).expect("generation config serializes");
    }
    serde_json::json!({ "body": body, "headers": {} })
}

pub(super) fn gemini_thinking_level(model: &str, effort: &str) -> &'static str {
    let flash = model.contains("flash");
    match effort {
        "none" => {
            if flash && !crate::llm::reasoning::always_reasons(model) {
                "minimal"
            } else {
                "low"
            }
        }
        "low" => "low",
        "medium" => {
            if flash || crate::llm::model_registry::unified_output_limit(model).is_some() {
                "medium"
            } else {
                "high"
            }
        }
        "xhigh" | "max"
            if crate::llm::reasoning::supported_efforts(ProviderKind::Vertex, model)
                .contains(&"xhigh") =>
        {
            "xhigh"
        }
        _ => "high",
    }
}

#[derive(Serialize)]
pub(super) struct VertexGenerationConfig {
    #[serde(rename = "maxOutputTokens", skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<usize>,
    #[serde(rename = "thinkingConfig")]
    thinking_config: VertexThinkingConfig,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VertexThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_level: Option<&'static str>,
    /// INVARIANT: When true, Gemini returns its deliberation as `thought:true` parts that
    /// `build_gemini_llm_response` strips — otherwise Flash narrates its
    /// reasoning in the ordinary answer text.
    include_thoughts: bool,
}

pub type LocationHandle = std::sync::Arc<std::sync::RwLock<String>>;

pub fn location_handle(initial: String) -> LocationHandle {
    std::sync::Arc::new(std::sync::RwLock::new(initial))
}

pub fn read_location(handle: &LocationHandle) -> String {
    handle.read().map(|g| g.clone()).unwrap_or_else(|e| {
        crate::log_legacy!(
            "[Vertex] location lock poisoned, falling back to global: {}",
            e
        );
        "global".to_string()
    })
}
