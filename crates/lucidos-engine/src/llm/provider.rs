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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}
