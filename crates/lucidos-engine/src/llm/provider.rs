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
                    ContentBlock::Text { text } | ContentBlock::EngineTail { text } => {
                        Some(text.as_str())
                    }
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
    /// A block the engine appended at the tail of a context-mode round: the
    /// context panel, or the rendered working understanding.
    ///
    /// Every provider renders it as an ordinary text block, so nothing changes
    /// on the wire. The variant exists so the two passes that treat these
    /// blocks differently can ask WHO WROTE IT rather than what it starts
    /// with. Both used to match the displayed prefix, which made a user
    /// message opening with `[CONTEXT PANEL]` collapse into the
    /// superseded-panel note and vanish from the request.
    ///
    /// One pass is `chat::process::context_panel::collapse_tail_blocks`, which
    /// rewrites a superseded block. The other is Anthropic's cache marker. It
    /// anchors in front of the tail, so next round's rewrite does not re-price
    /// the results the block rides on.
    #[serde(rename = "engine_tail")]
    EngineTail { text: String },
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
    /// Text the model wrote that the USER must not see and the MODEL must.
    ///
    /// Gemini narrates its plan in ordinary text parts beside a `functionCall`.
    /// That is working notes, not an answer, so `content` stays `None` and
    /// keeps it off the screen. This field carries the same text back into the
    /// assistant turn, so the model does not re-enter the next round having
    /// forgotten it. Anthropic and OpenAI keep their text in `content`, so they
    /// leave this `None`.
    ///
    /// Never set alongside `content`: [`LlmResponse::history_text`] reads one
    /// or the other, so setting both would send the same text twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_only_text: Option<String>,
}

impl LlmResponse {
    /// The model's own words for the next request, for the assistant turn the
    /// agentic loop rebuilds.
    ///
    /// `content` is the user-facing half and is not interchangeable. The two
    /// diverge wherever a provider emits text that is real context but not a
    /// printable answer.
    pub fn history_text(&self) -> Option<&str> {
        self.content.as_deref().or(self.model_only_text.as_deref())
    }
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
