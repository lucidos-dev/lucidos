//! Gemini request defaults shared by the Vertex client and the SDK.

use crate::llm::model_registry::ProviderKind;
use serde::Serialize;

/// Build the `generationConfig.thinkingConfig` for a Gemini call. Gemini 3.x and 4
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

/// Map the unified `reasoning_effort` to a Gemini `thinkingLevel`, clamped to
/// the levels the specific model accepts: Gemini 3 Flash supports
/// `minimal`/`low`/`medium`/`high`; Gemini 3 Pro supports only `low`/`high`
/// (sending Pro `minimal` or `medium` 400s). Gemini 3 can't fully disable
/// thinking, so `none` maps to the model's floor: `minimal` on Flash, `low` on
/// Pro and on a model that always reasons (Gemini 3.8 Flash answers `minimal`
/// with a 400). Neon supports `xhigh` and maps `max` to it. Other Gemini
/// models cap at `high`. Lowercase per Google's REST docs.
pub(super) fn gemini_thinking_level(model: &str, effort: &str) -> &'static str {
    let flash = model.contains("flash");
    match effort {
        // Gemini 3 can't be fully disabled; use the model's floor.
        "none" => {
            if flash && !crate::llm::reasoning::always_reasons(model) {
                "minimal"
            } else {
                "low"
            }
        }
        "low" => "low",
        // Gemini 4 and Flash accept medium; Gemini 3 Pro rounds up.
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
    /// Accepted levels depend on the model. See `gemini_thinking_level`.
    /// Gemini 3.x can reject a budget or return worse results. Flash can go
    /// as low as `minimal`. With `None`, the model uses its default level.
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_level: Option<&'static str>,
    /// When true, Gemini returns its deliberation as `thought:true` parts that
    /// `build_gemini_llm_response` strips — otherwise Flash narrates its
    /// reasoning in the ordinary answer text.
    include_thoughts: bool,
}
