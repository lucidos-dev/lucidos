use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Callback invoked with each text token/chunk as it streams from the LLM.
pub type TokenCallback = Box<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Extract the plain text from this content, ignoring tool blocks.
    pub fn as_text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        /// Opaque encrypted reasoning state Gemini 3 attaches to the first
        /// `functionCall` part of every turn. Must be echoed back verbatim
        /// on the next request or the API rejects with HTTP 400
        /// INVALID_ARGUMENT "Function call is missing a thought_signature".
        /// `None` for providers / models that don't emit one (Claude,
        /// Gemini ≤ 2.5, OpenAI). Skipped on serialize so existing event
        /// payloads still deserialize.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    #[serde(rename = "image")]
    Image {
        source_type: String, // "base64"
        media_type: String,  // "image/png", "image/jpeg", etc.
        data: String,        // base64-encoded image data
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    /// Opaque signature Gemini 3 returns on `functionCall` parts that the
    /// next request must echo back verbatim. Captured here so callers
    /// (agentic loop) can plumb it into the matching `ContentBlock::ToolUse`.
    /// `None` for providers / models that don't emit one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    /// Provider stop_reason ("end_turn", "max_tokens", etc.). None when
    /// not captured. Distinguishes legitimate empty completions from
    /// truncation when content is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u32>,
    /// Real prompt-token count from the provider's `usage`. Sums uncached input,
    /// cache-write, and cache-read tokens — the full size the model processed.
    /// `None` for providers that don't report it. Lets the UI replace the
    /// chars/4 estimate (which over-counts base64 image bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u32>,
    /// Cache-write token count (Anthropic-only). `None` on providers without
    /// prompt caching. Together with `cache_read_tokens` lets the modal show
    /// hit/miss rate so the user can see why a turn was cheap or expensive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u32>,
    /// Cache-read token count (Anthropic-only). `None` on providers without
    /// prompt caching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u32>,
    /// Total characters of thinking text. High value with empty content
    /// distinguishes "thought hard then gave up" from "said nothing".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_chars: Option<usize>,
    /// Count of SSE shapes the provider parser saw but couldn't classify
    /// (unknown `content_block` types or unknown delta types, excluding
    /// known-quiet metadata). Non-zero with empty content + empty
    /// `tool_calls` means the provider's stream changed shape and the
    /// parser dropped the model's output — the empty-completion
    /// diagnostic surfaces this so it isn't reported as intentional
    /// silence. Defaults to 0 for providers that don't track it.
    #[serde(default)]
    pub unknown_sse_dropped: u32,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model_override: Option<&str>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>>;

    /// Returns the default model name for this provider.
    fn default_model(&self) -> &str {
        ""
    }

    /// Whether this provider can actually serve LLM calls. `true` for every
    /// real provider, `RoutingProvider`, and the deterministic `MockProvider`
    /// (an explicit `LUCIDOS_MODEL=mock` opt-in for E2E). `false` only for
    /// `UnconfiguredProvider` — the sentinel installed when a packaged build
    /// boots before the user has configured any provider. The `/health`
    /// endpoint surfaces this as `llm_configured` so the frontend can show
    /// first-run provider onboarding instead of letting the user chat into a
    /// guaranteed error.
    fn is_configured(&self) -> bool {
        true
    }

    /// Which provider backends are actually configured, for filtering the model
    /// picker to providers the user has set up. `None` means "don't filter"
    /// (the default, and what `MockProvider` returns so E2E sees every model);
    /// `Some(list)` enumerates the live backends — `RoutingProvider` reports the
    /// ones it holds, `UnconfiguredProvider` reports `Some(vec![])` (nothing).
    /// Surfaced via `/health.configured_providers`.
    fn configured_providers(&self) -> Option<Vec<crate::llm::model_registry::ProviderKind>> {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}
