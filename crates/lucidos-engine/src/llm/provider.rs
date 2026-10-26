use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Callback invoked with each text token/chunk as it streams from the LLM.
pub type TokenCallback = Box<dyn Fn(&str) + Send + Sync>;

/// One callback shared by a call's first attempt and its retry.
pub(crate) type SharedCallback = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// A fresh [`TokenCallback`] forwarding to `shared`.
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
    /// Extract the plain text from this content, ignoring tool blocks.
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
    /// A view block that ends a cache prefix (ADR 0362): a *memory view*'s
    /// or a *compaction view*'s. A view goes out one block per 4 lines, and
    /// only the blocks that take a cache mark are memory view blocks: the
    /// snapshot's last whole block, the thread view's rungs, and a compaction
    /// view's trailing rungs.
    ///
    /// Every provider renders it as ordinary text. Anthropic puts a cache
    /// breakpoint on it while one of the four is free, and the GPT Responses
    /// wire puts one on it too. A later request then reads the prefix through
    /// it from the cache. A Classic turn builds none.
    #[serde(rename = "memory_view")]
    MemoryView { text: String },
    /// The *stable context block* of a turn's user message: the sections that
    /// change only when the user changes something (profile, accounts, thread
    /// context, loaded knowhow bodies).
    ///
    /// Every provider renders it as ordinary text, and only the Anthropic wire
    /// marks it. The cache reads a marked block whole, so it must stay a block
    /// of its own and hold identical bytes from turn to turn.
    #[serde(rename = "stable_context")]
    StableContext { text: String },
}

/// Replace every `ContentBlock::Image` in `blocks` with a text placeholder
/// produced by `placeholder()`. Returns total base64 bytes removed (used for
/// logging by callers).
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
    /// Opaque signature Gemini 3 returns on `functionCall` parts that the
    /// next request must echo back verbatim. Captured here so callers
    /// (agentic loop) can plumb it into the matching `ContentBlock::ToolUse`.
    /// `None` for providers / models that don't emit one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// Cache-write token count: Anthropic, OpenRouter's Anthropic models, and
    /// OpenAI from GPT-5.6 on. `None` on providers that report none. Together
    /// with `cache_read_tokens` lets the modal show hit/miss rate so the user
    /// can see why a turn was cheap or expensive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u32>,
    /// Part of `cache_creation_tokens`: the tokens written to the 1-hour
    /// cache. `None` when the provider split no write by lifetime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_1h_tokens: Option<u32>,
    /// Part of `cache_creation_tokens`: OpenAI Responses' 30-minute write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_30m_tokens: Option<u32>,
    /// Part of `cache_creation_tokens`: OpenAI Responses' 12-hour write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_12h_tokens: Option<u32>,
    /// The cache lifetime the request's markers asked for. A provider reports
    /// what it asked, and the router keeps it only on a route that honours it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_requested_ttl: Option<crate::llm::cache_lifetime::CacheLifetime>,
    /// Cache-read token count. `None` on providers that report none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u32>,
    /// The *served model*: the model id the provider's reply names, verbatim.
    /// `None` when the reply names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served_model: Option<String>,
    /// The backend that served the call, set by the router from the route it
    /// resolved. `None` from a provider called outside the router.
    #[serde(skip)]
    pub provider: Option<crate::llm::ProviderKind>,
    /// The *stable-prefix fingerprint* of the request this answers. Set by
    /// the Anthropic and Vertex Claude providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_prefix_fingerprint: Option<String>,
    /// The service tier the reply says served the call, when not the
    /// standard one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ServiceTier>,
    /// Total characters of thinking text. High value with empty content
    /// distinguishes "thought hard then gave up" from "said nothing".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_chars: Option<usize>,
    /// Count of thinking blocks the model produced. Proof it thought even when
    /// `thinking_chars` is 0: Claude returns empty thinking text unless asked
    /// to show it. `None` for providers that don't report blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_blocks: Option<usize>,
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
    /// The *progress notes* in `content`: the provider's short summaries of
    /// text the model wrote before a tool call, in place of that text. The
    /// user saw only these, and the full text never reached Lucidos.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub progress_notes: Vec<String>,
    /// What a *prefill beat* on this request sends. `None` from a provider
    /// that cannot prefill, and on a request no beat may copy.
    #[serde(skip)]
    pub prefill: Option<crate::llm::prefill::PrefillPair>,
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

/// The *model selection* for one turn: which model, on which backend, thinking
/// how hard.
///
/// Every field is optional, and absent means "resolve it". `RoutingProvider` is
/// the only implementor that reads more than `model`: the leaf providers each
/// serve one backend, so `provider` is already settled by the time a turn
/// reaches them.
///
/// The three travel together because they are resolved together. Carrying them
/// as three arguments put the provider six parameters deep on a trait whose
/// leaves all ignore it.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModelSelection<'a> {
    /// The model to run, or `None` for the provider's own default.
    pub model: Option<&'a str>,
    /// The backend to serve it. Honoured or refused, never substituted, so a
    /// pick naming an unconfigured backend errors rather than moving the turn
    /// to a different vendor. `None` takes the row's own preference, then its
    /// first configured route.
    pub provider: Option<crate::llm::model_registry::ProviderKind>,
    /// One of `llm::reasoning::EFFORT_LADDER`, snapped onto what the resolved
    /// backend supports. `None` runs the model at its *default effort*, and a
    /// model without one at its provider's own default.
    pub reasoning_effort: Option<&'a str>,
    /// Cap on one HTTP attempt, body included. An *auxiliary model call* sets
    /// it so its deadline can contain the provider's retries (ADR 0107). A turn
    /// leaves it `None`, so a long valid stream is never cut off.
    pub attempt_timeout: Option<std::time::Duration>,
    /// The one tool the model must call, for an engine decision such as the
    /// turn-end gate's read decision (ADR 0417). Each provider maps it to its
    /// own `tool_choice` form. On Claude a forced call runs without thinking,
    /// and a model that cannot turn thinking off is refused before sending.
    pub forced_tool: Option<&'a str>,
    /// A cheaper, slower tier to ask for. Only a backend that offers it sends
    /// it; every other backend serves the call at its standard tier.
    pub service_tier: Option<ServiceTier>,
    /// Where the prompt cache writes. Only OpenAI's Responses wire reads it.
    pub cache_mode: CacheMode,
    /// How far from `model` the router may move within its *lineage*. A
    /// default selection sets `NamedOrNewer`, so it never steps back.
    pub generation_rule: crate::llm::model_registry::GenerationRule,
}

/// Where a request writes the prompt cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CacheMode {
    /// At its memory view marks, and at the end of the latest message. The
    /// next request of a growing conversation reads that end.
    #[default]
    Implicit,
    /// At its memory view marks only. For a request whose tail no later
    /// request shares: the tail is then billed as plain input, not as a write.
    Explicit,
}

/// A provider's processing tier other than its standard one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTier {
    /// OpenAI's Flex processing: half the price, slower, and refused with a
    /// 429 when OpenAI has no capacity for it.
    Flex,
}

impl ServiceTier {
    /// The tier a provider's `service_tier` value names, `None` for its
    /// standard tier or one this enum does not know.
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
    /// A selection naming only the model, which is what most callers have.
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
    /// One billable call. `call` comes from the model call service, which
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

    /// Send one *prefill beat*: a body from a [`crate::llm::prefill::PrefillPair`]
    /// this provider returned. The reply carries only its usage. A provider
    /// that never returns a pair cannot prefill.
    async fn prefill(
        &self,
        body: &crate::llm::prefill::PrefillBody,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        Err(crate::llm::prefill::cannot_prefill(body.provider))
    }

    /// Returns the default model name for this provider.
    fn default_model(&self) -> &str {
        ""
    }

    /// The effort a chat with `selection` goes out at, for its cost record.
    /// The router fills and clamps it; a leaf sends what it is given.
    fn effort_sent(&self, selection: &ModelSelection<'_>) -> Option<String> {
        selection.reasoning_effort.map(str::to_string)
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

    /// The not-found answers this provider routes around, beside
    /// [`Self::configured_providers`]. `None` for a provider that keeps no
    /// ledger; only `RoutingProvider` keeps one.
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
