use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub type TokenCallback = Box<dyn Fn(&str) + Send + Sync>;

pub(crate) type SharedCallback = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

pub(crate) fn forward(shared: &Option<SharedCallback>) -> Option<TokenCallback> {
    shared
        .clone()
        .map(|cb| Box::new(move |token: &str| cb(token)) as TokenCallback)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    pub fn as_text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text }
                    | ContentBlock::EngineTail { text }
                    | ContentBlock::MemoryView { text }
                    | ContentBlock::StableContext { text } => Some(text.as_str()),
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
        /// INVARIANT: Opaque encrypted reasoning state Gemini 3 attaches to the first
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
        source_type: String,
        media_type: String,
        data: String,
    },
    /// INVARIANT: A block the engine appended at the tail of a context-mode round: the
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
    #[serde(rename = "memory_view")]
    MemoryView { text: String },
    /// INVARIANT: The *stable context block* of a turn's user message: the sections that
    /// change only when the user changes something (profile, accounts, thread
    /// context, loaded knowhow bodies).
    ///
    /// Every provider renders it as ordinary text, and only the Anthropic wire
    /// marks it. The cache reads a marked block whole, so it must stay a block
    /// of its own and hold identical bytes from turn to turn.
    #[serde(rename = "stable_context")]
    StableContext { text: String },
}

pub fn replace_image_blocks(
    blocks: &mut [ContentBlock],
    placeholder: impl Fn() -> String,
) -> usize {
    let mut stripped = 0usize;
    for block in blocks.iter_mut() {
        if let ContentBlock::Image { data, .. } = block {
            stripped += data.len();
            *block = ContentBlock::Text {
                text: placeholder(),
            };
        }
    }
    stripped
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::llm::api_usage::ApiUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served_model: Option<String>,
    #[serde(skip)]
    pub provider: Option<crate::llm::ProviderKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_prefix_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_chars: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_blocks: Option<usize>,
    #[serde(default)]
    pub unknown_sse_dropped: u32,
    /// INVARIANT: Text the model wrote that the USER must not see and the MODEL must.
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub progress_notes: Vec<String>,
    #[serde(skip)]
    pub prefill: Option<crate::llm::prefill::PrefillPair>,
}

impl LlmResponse {
    pub fn history_text(&self) -> Option<&str> {
        self.content.as_deref().or(self.model_only_text.as_deref())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ModelSelection<'a> {
    /// INVARIANT: The model to run, or `None` for the provider's own default.
    pub model: Option<&'a str>,
    pub provider: Option<crate::llm::model_registry::ProviderKind>,
    pub reasoning_effort: Option<&'a str>,
    pub attempt_timeout: Option<std::time::Duration>,
    pub forced_tool: Option<&'a str>,
    pub service_tier: Option<ServiceTier>,
    pub cache_mode: CacheMode,
    /// INVARIANT: How far from `model` the router may move within its *lineage*. A
    /// default selection sets `NamedOrNewer`, so it never steps back.
    pub generation_rule: crate::llm::model_registry::GenerationRule,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CacheMode {
    #[default]
    Implicit,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTier {
    Flex,
}

impl ServiceTier {
    pub fn from_wire(value: &str) -> Option<Self> {
        (value == "flex").then_some(Self::Flex)
    }

    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Flex => "flex",
        }
    }
}

impl<'a> ModelSelection<'a> {
    pub fn model(model: &'a str) -> Self {
        Self {
            model: Some(model),
            ..Self::default()
        }
    }

    pub fn with_effort(mut self, effort: Option<&'a str>) -> Self {
        self.reasoning_effort = effort;
        self
    }

    pub fn with_attempt_timeout(mut self, attempt_timeout: Option<std::time::Duration>) -> Self {
        self.attempt_timeout = attempt_timeout;
        self
    }

    pub fn with_forced_tool(mut self, forced_tool: Option<&'a str>) -> Self {
        self.forced_tool = forced_tool;
        self
    }

    pub fn with_service_tier(mut self, service_tier: Option<ServiceTier>) -> Self {
        self.service_tier = service_tier;
        self
    }

    pub fn with_cache_mode(mut self, cache_mode: CacheMode) -> Self {
        self.cache_mode = cache_mode;
        self
    }

    pub fn with_generation_rule(
        mut self,
        generation_rule: crate::llm::model_registry::GenerationRule,
    ) -> Self {
        self.generation_rule = generation_rule;
        self
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// INVARIANT: Resolve the backend before a metered background call checks its hold.
    fn resolved_provider_kind(
        &self,
        _selection: &ModelSelection<'_>,
    ) -> Result<Option<crate::llm::ProviderKind>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(None)
    }

    /// INVARIANT: One billable call. `call` comes from the model call service, which
    /// records it (`engine::model_call`). A wrapper forwards the token it got.
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>>;

    async fn prefill(
        &self,
        body: &crate::llm::prefill::PrefillBody,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        Err(crate::llm::prefill::cannot_prefill(body.provider))
    }

    fn default_model(&self) -> &str {
        ""
    }

    fn effort_sent(&self, selection: &ModelSelection<'_>) -> Option<String> {
        selection.reasoning_effort.map(str::to_string)
    }

    fn is_configured(&self) -> bool {
        true
    }

    fn configured_providers(&self) -> Option<Vec<crate::llm::model_registry::ProviderKind>> {
        None
    }

    async fn route_available(
        &self,
        _provider: crate::llm::ProviderKind,
        _model: &str,
        _wire_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        Ok(true)
    }

    fn not_served_ledger(&self) -> Option<crate::llm::not_served::NotServedLedger> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}
