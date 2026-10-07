//! The **preference catalog**: the one place every preference key, its
//! allowed values and its default are written.
//!
//! Lucidos "Settings" spans several backing stores (preferences, models,
//! credentials, MCP servers, repositories). This catalog covers ONLY the
//! `preferences` key-value table. Each [`PrefSpec`] declares a key's access,
//! scope, allowed values, default, and any write side effect.
//!
//! - **Readers** go through a typed [`Pref`] handle, whose reads in
//!   `core::preferences` resolve an unset preference to this default. A caller
//!   cannot pass its own (ADR 0368).
//! - **The agent** reads and writes the [`PrefAccess::Agent`] entries through
//!   `get_preferences` / `set_preference`. The rest are refused with a hint
//!   naming where the user changes them. So the agent cannot disable its own
//!   command guard or raise the backstop over its own loop.
//! - **The frontend** reads a generated copy,
//!   `packages/lucidos-sdk/src/generated/preference-catalog.ts`.
//! - **Secrets** never live in `preferences` (they go through credentials).
//!
//! `system-knowhow/preferences.md` is kept in lockstep: a sync test fails when
//! a key or its default drifts (`.claude/rules/system-knowhow.md`).

/// Where a preference is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefScope {
    /// One workspace-wide value (`device_id IS NULL`).
    Global,
    /// A per-device override (keyed by the caller's device id) that wins over the
    /// global value. `set_preference` auto-resolves the caller's device id for
    /// these; the agent never has to pass it.
    Device,
}

/// A side-effect a write triggers beyond the plain store upsert + the persisted
/// `PreferencesChanged` broadcast. Lets the one write chokepoint keep the
/// engine's in-memory caches and the legacy per-key events honest regardless of
/// which entry path (LLM tool or HTTP `PUT /preferences`) made the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefSideEffect {
    /// Plain preference — upsert + emit `PreferencesChanged`.
    None,
    /// `language` — also refresh the engine's in-memory `user_language` and emit
    /// `LanguageSet` (the frontend live-applies on it).
    Language,
    /// `timezone` — also refresh in-memory `user_timezone` and emit
    /// `TimezoneSet`. (Value is IANA-validated by [`PrefValue::IanaTimezone`].)
    Timezone,
    /// `push_notifications` — also sync the `devices.push_enabled` column and let
    /// the tool layer drive the `[PUSH_NOTIFICATION_REQUEST]` permission
    /// handshake.
    Push,
    /// `home_thread_enabled`: turning it on creates the home thread unless one
    /// is already marked, so off and on again brings the same thread back.
    HomeThread,
}

/// The allowed values for a preference — drives both validation and the
/// agent-facing description.
#[derive(Debug, Clone, Copy)]
pub enum PrefValue {
    /// `"true"` / `"false"`.
    Bool,
    /// One of a fixed set of strings.
    Enum(&'static [&'static str]),
    /// A number in `[min, max]` inclusive (parsed as f64; integers accepted).
    Number { min: f64, max: f64 },
    /// An IANA timezone name (validated via `chrono_tz`).
    IanaTimezone,
    /// Free-form non-empty text (e.g. a model id, a URL, a language name).
    Text,
    /// A theme id: well formed, and never a theme mode value. An unknown id
    /// is accepted and paints the default theme.
    ThemeId,
    /// Comma-separated `model=bytes` pairs, each a model id and a byte count.
    ModelBytes,
    /// `theme`, a font catalog id, or a well-formed workspace font id. Whether
    /// the workspace font exists is asked where it is used, which falls back
    /// when it does not (ADR 0308).
    FontFamily,
}

/// Who may write a preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefAccess {
    /// The Lucidos Agent may write it through `set_preference`.
    Agent,
    /// A real setting the user changes in Settings. The agent must not write
    /// it, and `set_preference` refuses it with this hint pointing at where
    /// the user changes it.
    Human { hint: &'static str },
    /// Engine bookkeeping rather than a setting. Refused to the agent like
    /// [`PrefAccess::Human`], and written WITHOUT announcing
    /// `PreferencesChanged` (see [`is_silent_key`]).
    Engine { hint: &'static str },
}

/// What a preference resolves to while no row is stored.
#[derive(Debug, Clone, Copy)]
pub enum PrefDefault {
    /// A real value, valid for the spec's [`PrefValue`].
    Value(&'static str),
    /// No value. The text says what an unset preference means.
    Unset(&'static str),
    /// The resolved value of another preference.
    Inherits(&'static PrefSpec),
}

/// One preference: its key, who may write it, its values and its default.
///
/// The ONLY place a preference default is written. Readers reach a spec
/// through its typed [`Pref`] handle, which resolves the default from here, so
/// a caller has no way to supply its own.
#[derive(Debug)]
pub struct PrefSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub access: PrefAccess,
    pub scope: PrefScope,
    pub value: PrefValue,
    pub default: PrefDefault,
    pub description: &'static str,
    pub side_effect: PrefSideEffect,
}

impl PrefSpec {
    /// The default as the agent and the docs show it.
    pub fn default_label(&self) -> String {
        match self.default {
            PrefDefault::Value(value) => value.to_string(),
            PrefDefault::Unset(meaning) => format!("(unset: {meaning})"),
            PrefDefault::Inherits(other) => format!("(the {} value)", other.key),
        }
    }

    /// The default value, following [`PrefDefault::Inherits`] to the spec
    /// that holds one. `None` for an [`PrefDefault::Unset`] default.
    pub const fn default_value(&self) -> Option<&'static str> {
        match self.default {
            PrefDefault::Value(value) => Some(value),
            PrefDefault::Unset(_) => None,
            PrefDefault::Inherits(other) => other.default_value(),
        }
    }
}

/// The value kind of a [`Pref`] handle. Each kind fixes the type a read
/// returns, and its constructor refuses a spec of another kind at compile time.
pub mod kind {
    /// A `true` / `false` switch with a default.
    pub enum Flag {}
    /// A number with a default, clamped to the spec's bounds.
    pub enum Number {}
    /// Text with a default value or an inherited one.
    pub enum Text {}
    /// Text with no default: a read returns `None` while unset.
    pub enum Optional {}
}
pub use kind::{Flag, Number, Optional, Text};

/// A typed handle on one catalog spec.
///
/// The reads live in `core::preferences`, and every one resolves an unset
/// preference to the spec's default. A defaulted kind returns `T`, never
/// `Option<T>`, so there is nothing to `unwrap_or`.
///
/// ```compile_fail
/// use lucidos_engine::core::preference_catalog::*;
/// // A flag handle on a text spec does not compile.
/// const WRONG: Pref<Flag> = Pref::flag(PrefSpec {
///     key: "example",
///     label: "Example",
///     access: PrefAccess::Agent,
///     scope: PrefScope::Global,
///     value: PrefValue::Text,
///     default: PrefDefault::Value("x"),
///     description: "",
///     side_effect: PrefSideEffect::None,
/// });
/// ```
pub struct Pref<K> {
    pub spec: PrefSpec,
    kind: std::marker::PhantomData<K>,
}

impl<K> Pref<K> {
    pub const fn key(&self) -> &'static str {
        self.spec.key
    }
}

impl Pref<Flag> {
    pub const fn flag(spec: PrefSpec) -> Self {
        assert!(
            matches!(spec.value, PrefValue::Bool),
            "a flag handle needs PrefValue::Bool"
        );
        assert!(
            matches!(spec.default, PrefDefault::Value(_)),
            "a flag handle needs a default value"
        );
        Self {
            spec,
            kind: std::marker::PhantomData,
        }
    }

    /// The default, parsed.
    pub fn default_flag(&self) -> bool {
        self.spec
            .default_value()
            .and_then(parse_flag)
            .expect("a flag's default is checked by `every_default_is_a_valid_value`")
    }
}

impl Pref<Number> {
    pub const fn number(spec: PrefSpec) -> Self {
        assert!(
            matches!(spec.value, PrefValue::Number { .. }),
            "a number handle needs PrefValue::Number"
        );
        assert!(
            matches!(spec.default, PrefDefault::Value(_)),
            "a number handle needs a default value"
        );
        Self {
            spec,
            kind: std::marker::PhantomData,
        }
    }

    /// The default, parsed.
    pub fn default_number(&self) -> f64 {
        self.spec
            .default_value()
            .and_then(|value| value.parse().ok())
            .expect("a number's default is checked by `every_default_is_a_valid_value`")
    }

    /// The spec's inclusive bounds, `(min, max)`.
    pub fn bounds(&self) -> (f64, f64) {
        match self.spec.value {
            PrefValue::Number { min, max } => (min, max),
            _ => unreachable!("the constructor admits only PrefValue::Number"),
        }
    }
}

impl Pref<Text> {
    pub const fn text(spec: PrefSpec) -> Self {
        assert!(
            !matches!(spec.value, PrefValue::Bool | PrefValue::Number { .. }),
            "a text handle needs a text-shaped PrefValue"
        );
        assert!(
            !matches!(spec.default, PrefDefault::Unset(_)),
            "a text handle needs a default; use Pref::optional"
        );
        Self {
            spec,
            kind: std::marker::PhantomData,
        }
    }

    /// The default, following an inherited one to its source.
    pub const fn default_text(&self) -> &'static str {
        self.spec.default_value().expect(
            "an inherited text default ends in a value (`inherited_defaults_end_in_a_value`)",
        )
    }
}

impl Pref<Optional> {
    pub const fn optional(spec: PrefSpec) -> Self {
        assert!(
            matches!(spec.default, PrefDefault::Unset(_)),
            "an optional handle needs an unset default"
        );
        Self {
            spec,
            kind: std::marker::PhantomData,
        }
    }
}

/// How a stored switch reads: one vocabulary for every flag, and for the env
/// switches that layer over some of them. `None` is neither on nor off, so a
/// flag read falls back to its default.
/// Case-insensitive, as stored switches always were.
pub fn parse_flag(value: &str) -> Option<bool> {
    let value = value.trim().to_ascii_lowercase();
    if FLAG_ON_VALUES.contains(&value.as_str()) {
        Some(true)
    } else if FLAG_OFF_VALUES.contains(&value.as_str()) {
        Some(false)
    } else {
        None
    }
}

/// An engine env switch spelled exactly as one of [`FLAG_ON_VALUES`].
///
/// Exact on purpose: these switches include `LUCIDOS_BIND_ALL` and
/// `LUCIDOS_PERMISSIVE_CORS`, which never read another case. A spelling they
/// never honoured must not start opening them.
pub fn env_switch_is_on(value: &str) -> bool {
    FLAG_ON_VALUES.contains(&value.trim())
}

/// The spellings [`parse_flag`] reads as on, in lowercase.
pub const FLAG_ON_VALUES: &[&str] = &["1", "true", "yes", "on"];

/// The spellings [`parse_flag`] reads as off, in lowercase.
pub const FLAG_OFF_VALUES: &[&str] = &["0", "false", "no", "off"];

/// The unified reasoning vocabulary, straight from `llm::reasoning`, which also
/// decides which of these tiers a given model supports and clamps a request
/// onto the closest one. The catalog only checks membership: a value that is a
/// tier but unavailable on the user's current model is accepted here and
/// snapped at the routing chokepoint, so changing the model can never make a
/// stored preference unwritable.
pub(crate) const REASONING_EFFORTS: &[&str] = crate::llm::EFFORT_LADDER;
/// The frontend reads these lists from the generated catalog.
const IMAGE_MODELS: &[&str] = &[
    "auto",
    "imagen-4",
    "gpt-image-1",
    "gpt-image-1.5",
    "gpt-image-2",
];
/// The routing lives in `crates/lucidos-app/src/utils/openExternalUrl.ts`.
const EXTERNAL_LINK_TARGETS: &[&str] = &["safari", "ask", "in-app"];
const THEME_MODES: &[&str] = &["light", "dark", "system"];
const MOTION_PREFS: &[&str] = &["system", "reduce", "full"];
const THEME_EFFECTS_PREFS: &[&str] = &["system", "reduce", "full"];

/// The default model of every background task that has its own one.
/// `lucidos-eval` pins the memory tasks to it, so its runs stay comparable.
pub const BACKGROUND_MODEL: &str = "gemini-3-flash-preview";

/// What an unset background model resolves to when no configured provider
/// serves its default. The list is `engine::aux_purpose::AUX_FALLBACKS`, and
/// `the_description_names_the_lead_and_every_fallback` pins the two together.
macro_rules! aux_fallback_note {
    () => {
        " While unset and its default unreachable, the engine runs the first of gemini-3-flash-preview, gpt-5.4-mini and claude-haiku-4-5 that a configured provider serves, else the chat model. GET /api/v1/models/background says which model runs."
    };
}

// ---- Agent-settable ----

// ---- Locale (global, side-effecting) ----
pub const LANGUAGE: Pref<Optional> = Pref::optional(PrefSpec {
    key: "language",
    label: "Language",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("detected from the conversation"),
    description: "Preferred language for the Lucidos Agent's responses and session summaries (e.g. 'English', 'Norwegian', 'Spanish'). A voice call speaks it too, and pins its transcriber to the matching ISO-639-1 code.",
    side_effect: PrefSideEffect::Language,
});

pub const TIMEZONE: Pref<Optional> = Pref::optional(PrefSpec {
    key: "timezone",
    label: "Timezone",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::IanaTimezone,
    default: PrefDefault::Unset("ask the user; triggers and the clock run in UTC meanwhile"),
    description: "IANA timezone (e.g. 'Europe/Oslo', 'America/New_York', 'Asia/Tokyo'). Used for trigger scheduling and time display. Set this before creating triggers.",
    side_effect: PrefSideEffect::Timezone,
});

// ---- Models (global) ----
pub const CHAT_MODEL: Pref<Text> = Pref::text(PrefSpec {
    key: "chat_model",
    label: "Chat model",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value("claude-opus-5"),
    description: "Default chat model id for NEW Lucidos Agent threads. A thread that's already running reuses its own last-used model, so changing this does NOT switch the current/running thread on its next turn (use the thread's in-thread picker for that). Must be an enabled model id from the registry: call get_preferences or manage_models(action='list') to see the options (e.g. 'claude-opus-5', 'claude-fable-5').",
    side_effect: PrefSideEffect::None,
});

pub const CHAT_REASONING_EFFORT: Pref<Text> = Pref::text(PrefSpec {
    key: "chat_reasoning_effort",
    label: "Reasoning effort",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(REASONING_EFFORTS),
    default: PrefDefault::Value("high"),
    description: "Default thinking budget for NEW Lucidos Agent threads. A thread that's already running reuses its own last-used effort, so changing this does NOT change the current/running thread on its next turn (use the thread's in-thread picker for that). Not every model supports every tier; the engine clamps per model.",
    side_effect: PrefSideEffect::None,
});

pub const RESPONSE_STYLE: Pref<Text> = Pref::text(PrefSpec {
    key: "response_style",
    label: "Response style",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value(crate::core::response_style::STANDARD_ID),
    description: "The shape of a chat or trigger answer (how much comes back, and what it is for): the id of a style in the library. 'standard' is the default and adds nothing to the prompt, so answers come back as they always have. 'concise', 'minimal' (the outcome, not the process) and 'learning' (explains the why as it goes) are shipped, and the user may edit any of them or add their own in Settings > Models > Response style, so the set is open and this is not a closed enum. Read 'response_styles' or GET /api/v1/response-styles for the ids that exist here. When the user asks for shorter or longer answers, SET THIS: it is what makes the request stick, where saying it in chat lasts one thread. An id nothing defines falls back to 'standard'. A change applies from the next message, because a turn builds its prompt once at the start.",
    side_effect: PrefSideEffect::None,
});

pub const RESPONSE_STYLES: Pref<Optional> = Pref::optional(PrefSpec {
    key: "response_styles",
    label: "Style library",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("the shipped styles only"),
    description: "The user's own response styles, as a JSON array of {id, label, instruction} objects. It holds ONLY what they changed: an entry whose id is 'concise', 'minimal' or 'learning' overrides that shipped style, one with a fresh kebab-case id adds a style, and removing an entry restores the shipped text. 'standard' is the off switch and is REFUSED here. THIS KEY REPLACES THE WHOLE ARRAY, so read the current value first and send it back with your edit applied, or you will delete every style the user wrote. Bounds, all refused rather than trimmed: 40 chars of id, 40 of label, 1000 of instruction, 20 entries. The instruction is injected verbatim under a 'RESPONSE STYLE:' heading, and the engine appends a rule that keeps warnings and caveats in whatever the style asks for.",
    side_effect: PrefSideEffect::None,
});

pub const TECHNICAL_LITERACY: Pref<Text> = Pref::text(PrefSpec {
    key: "technical_literacy",
    label: "Technical literacy",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(crate::core::technical_literacy::SETTABLE_IDS),
    default: PrefDefault::Value(crate::core::technical_literacy::NOT_SET_ID),
    description: "How technical the words are in every answer: the response style's second part, beside 'response_style' (the shape of an answer). 'non-technical' (shown as Keep it plain) means plain words, no jargon or code, and no question they cannot answer. 'technical' means technical terms need no explanation. 'developer' means engineering terms, code and ids need no explanation. A level sets which words to use, never how much to say: that is 'response_style'. Chat and triggers read it from the next message, and a coding-agent session or voice call from its next start. Store ONLY a level the user stated or picked, never one you inferred from how they write. When they say 'I am not technical' or 'I am a developer', SET THIS. 'not-set', the default, adds nothing; set it to clear the level.",
    side_effect: PrefSideEffect::None,
});

pub const IMAGE_MODEL: Pref<Text> = Pref::text(PrefSpec {
    key: "image_model",
    label: "Image model",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(IMAGE_MODELS),
    default: PrefDefault::Value("auto"),
    description: "Model used by generate_image. 'auto' lets the engine choose.",
    side_effect: PrefSideEffect::None,
});

pub const MODEL_TITLE: Pref<Text> = Pref::text(PrefSpec {
    key: "model_title",
    label: "Title model",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value(BACKGROUND_MODEL),
    description: concat!(
        "Background model that generates thread titles. A cheap, fast model is appropriate.",
        aux_fallback_note!()
    ),
    side_effect: PrefSideEffect::None,
});

pub const REASONING_TITLE: Pref<Text> = Pref::text(PrefSpec {
    key: "reasoning_title",
    label: "Title reasoning",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(REASONING_EFFORTS),
    default: PrefDefault::Value("none"),
    description: "Thinking budget for thread-title generation. Naming a thread needs none of it; raise this only if titles are poor.",
    side_effect: PrefSideEffect::None,
});

pub const MODEL_IMAGE_DESCRIPTION: Pref<Text> = Pref::text(PrefSpec {
    key: "model_image_description",
    label: "Image-description model",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value(BACKGROUND_MODEL),
    description: concat!(
        "Background model that describes uploaded images for search/memory. It must have the vision flag (manage_models vision), or no description runs.",
        aux_fallback_note!()
    ),
    side_effect: PrefSideEffect::None,
});

pub const REASONING_IMAGE_DESCRIPTION: Pref<Text> = Pref::text(PrefSpec {
    key: "reasoning_image_description",
    label: "Image-description reasoning",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(REASONING_EFFORTS),
    default: PrefDefault::Value("none"),
    description: "Thinking budget for describing an uploaded image. Captioning is a perception task, so the default spends nothing on deliberation.",
    side_effect: PrefSideEffect::None,
});

pub const MODEL_CHANGE_SUMMARY: Pref<Text> = Pref::text(PrefSpec {
    key: "model_change_summary",
    label: "Change-summary model",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Inherits(&MODEL_TITLE.spec),
    description: "Background model that writes a change summary: one line saying what a coding-agent change of several commits does. It heads the change card, the change toasts and the Changes panel. A single-commit change makes no call, since its commit subject is already the line. Inherits model_title until you set this one.",
    side_effect: PrefSideEffect::None,
});

pub const REASONING_CHANGE_SUMMARY: Pref<Text> = Pref::text(PrefSpec {
    key: "reasoning_change_summary",
    label: "Change-summary reasoning",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(REASONING_EFFORTS),
    default: PrefDefault::Inherits(&REASONING_TITLE.spec),
    description: "Thinking budget for the change summary. It condenses a list of commit subjects into one line, so the default spends nothing. Inherits reasoning_title until you set this one.",
    side_effect: PrefSideEffect::None,
});

pub const MODEL_SUMMARY_COMPACTION: Pref<Optional> = Pref::optional(PrefSpec {
    key: "model_summary_compaction",
    label: "Summary-compaction model",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("the first of GPT-6.1 Sol, Gemini 3.8 Flash and Sonnet 5.5 a configured provider serves, else the chat model"),
    description: "Background model the compactor writes summary tree lines with: one line per message too long to keep verbatim, then one line per pair of lines. While unset, the engine picks the first of gpt-6.1-sol, gemini-3.8-flash and claude-sonnet-5-5 that a configured provider serves, and falls back to the chat model. GET /api/v1/models/background says which model that resolves to.",
    side_effect: PrefSideEffect::None,
});

pub const REASONING_SUMMARY_COMPACTION: Pref<Optional> = Pref::optional(PrefSpec {
    key: "reasoning_summary_compaction",
    label: "Summary-compaction reasoning",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(REASONING_EFFORTS),
    default: PrefDefault::Unset("the tier the compactor's model runs at by default: low"),
    description: "Thinking budget for the compactor. A summary line has to fit 512 bytes and keep the user's words, which a little thinking helps with. While unset, the compactor runs its model at the tier its default names, which is low for every default model.",
    side_effect: PrefSideEffect::None,
});

pub const MODEL_MEMORY_FIND: Pref<Text> = Pref::text(PrefSpec {
    key: "model_memory_find",
    label: "Memory-find model",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value(BACKGROUND_MODEL),
    description: concat!(
        "Background model the recall tool's find asks its yes/no questions on, when judgment_memory_find leaves it on chat. Each call judges about 40 summary lines. It follows no other key.",
        aux_fallback_note!()
    ),
    side_effect: PrefSideEffect::None,
});

pub const REASONING_MEMORY_FIND: Pref<Text> = Pref::text(PrefSpec {
    key: "reasoning_memory_find",
    label: "Memory-find reasoning",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(REASONING_EFFORTS),
    default: PrefDefault::Value("none"),
    description: "Thinking budget for find. It answers short yes/no questions, so the default spends nothing.",
    side_effect: PrefSideEffect::None,
});

pub const MODEL_MEMORY: Pref<Text> = Pref::text(PrefSpec {
    key: "model_memory",
    label: "Memory model",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value(BACKGROUND_MODEL),
    description: concat!(
        "Background model that extracts facts from a turn for long-term memory. While unset, the engine runs gpt-5.6-luna when a configured provider serves it, else this default. It binds extraction only: the conversation summary, query classification and find have keys of their own.",
        aux_fallback_note!()
    ),
    side_effect: PrefSideEffect::None,
});

pub const REASONING_MEMORY: Pref<Text> = Pref::text(PrefSpec {
    key: "reasoning_memory",
    label: "Memory reasoning",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(REASONING_EFFORTS),
    default: PrefDefault::Value("low"),
    description: "Thinking budget for fact extraction. GPT-5.6 Luna and GPT-5.4 mini lost coverage at none, and Gemini 3 Flash measured the same at low as at none.",
    side_effect: PrefSideEffect::None,
});

pub const MODEL_QUERY_CLASSIFICATION: Pref<Text> = Pref::text(PrefSpec {
    key: "model_query_classification",
    label: "Query-classification model",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value(BACKGROUND_MODEL),
    description: concat!(
        "Background model that decides what a turn needs retrieved: long-term memory, the file list, credentials. It follows no other key. Settings offers TypeSafe (Jev) in the same control, which writes judgment_query_classification instead.",
        aux_fallback_note!()
    ),
    side_effect: PrefSideEffect::None,
});

pub const REASONING_QUERY_CLASSIFICATION: Pref<Text> = Pref::text(PrefSpec {
    key: "reasoning_query_classification",
    label: "Query-classification reasoning",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(REASONING_EFFORTS),
    default: PrefDefault::Value("none"),
    description: "Thinking budget for query classification. It answers three yes/no questions in front of every turn, so the default spends nothing.",
    side_effect: PrefSideEffect::None,
});

pub const MODEL_CONVERSATION_SUMMARY: Pref<Text> = Pref::text(PrefSpec {
    key: "model_conversation_summary",
    label: "Conversation-summary model",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value(BACKGROUND_MODEL),
    description: concat!(
        "Background model that writes a thread's conversation summary: the one paragraph standing in for its older assistant turns. The input can be 80k tokens, far larger than any other background call. While unset, the engine runs the first of gpt-6.1-sol, gemini-3.8-flash and claude-sonnet-5-5 that a configured provider serves, the compactor's list, else this default. It follows no other key.",
        aux_fallback_note!()
    ),
    side_effect: PrefSideEffect::None,
});

pub const VOICE_ENABLED: Pref<Flag> = Pref::flag(PrefSpec {
    key: "voice_enabled",
    label: "Voice (experimental)",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("false"),
    description: "Off by default. When 'true', the home thread can be called and spoken to: its composer grows a call control and /api/v1/voice accepts a socket. A call runs on the home thread alone, so it also needs home_thread_enabled. Experimental. It rents a speech-to-speech talker from OpenAI and needs that provider configured, and every spoken utterance starts an ordinary agent turn, so a short call can cost several turns. With this off nothing voice-shaped is reachable and the other voice keys do nothing.",
    side_effect: PrefSideEffect::None,
});

pub const HOME_THREAD_ENABLED: Pref<Flag> = Pref::flag(PrefSpec {
    key: "home_thread_enabled",
    label: "Home thread (experimental)",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("false"),
    description: "Off by default. When 'true', the workspace has a home thread: one chat thread that never ends, opened from a Home entry in the header or the Lucidos menu rather than from the thread drawer. It can follow up any thread, and it presses an owner button only when the user's words in that turn ask for it. Voice calls run there alone. Turning it on creates the home thread if none exists. Turning it off hides it and turns those powers off; the thread and its history stay, and come back when it is turned on again.",
    side_effect: PrefSideEffect::HomeThread,
});

pub const MODEL_VOICE_TALKER: Pref<Text> = Pref::text(PrefSpec {
    key: "model_voice_talker",
    label: "Voice talker model",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value("gpt-realtime-2.1"),
    description: "Speech-to-speech model a voice session speaks through. It holds the conversation and nothing else: it can change nothing, so every action still goes through the ordinary agent. Two families, and this id picks which protocol the call opens on. A 'gpt-realtime-*' model bills by the token, and the caller can settle a card or ring off out loud. 'gpt-live-1' listens while it speaks, so it takes an interruption better, but it holds no tools: a card is settled by tapping it and a call is ended on the button. It bills by the minute, which the usage rollup does not show. Not a chat-model registry row, because neither family can serve an ordinary turn.",
    side_effect: PrefSideEffect::None,
});

pub const MODEL_VOICE_TRANSCRIBER: Pref<Text> = Pref::text(PrefSpec {
    key: "model_voice_transcriber",
    label: "Voice transcriber model",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value("gpt-4o-mini-transcribe"),
    description: "Model turning the caller's speech into text inside a voice session, and only on a realtime one. A 'gpt-live-1' talker transcribes the caller itself and reads no id here, so this key does nothing for it. The second and last model in the voice loop: nothing translates and nothing summarises. Which language it is pinned to comes from 'language', not from here.",
    side_effect: PrefSideEffect::None,
});

pub const VOICE_TALKER_VOICE: Pref<Text> = Pref::text(PrefSpec {
    key: "voice_talker_voice",
    label: "Spoken voice",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value("marin"),
    description: "The voice a call is spoken in, as the provider's own name for one. Not a model, and not the language: 'language' decides what is spoken, and this decides who speaks it. A name the provider does not know refuses the call.",
    side_effect: PrefSideEffect::None,
});

pub const VOICE_RESIDENT_SECTIONS: Pref<Text> = Pref::text(PrefSpec {
    key: "voice_resident_sections",
    label: "Voice resident sections",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value("who-and-where,this-thread,workspace-shape"),
    description: "Comma-separated ids of what a voice session loads at the start of a call. That block is the whole of what voice answers with no wait, since the talker cannot look anything up. Ids today: who-and-where, this-thread, workspace-shape. An unknown id is ignored. A row that exists means exactly what it lists; only an absent row means the three above. Turning every section off writes an empty value, which you cannot: it is a Settings-only state, reached by the toggles in Settings > Models > Voice.",
    side_effect: PrefSideEffect::None,
});

pub const REASONING_CONVERSATION_SUMMARY: Pref<Text> = Pref::text(PrefSpec {
    key: "reasoning_conversation_summary",
    label: "Conversation-summary reasoning",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(REASONING_EFFORTS),
    default: PrefDefault::Value("low"),
    description: "Thinking budget for the conversation summary. Measured output length does not track this setting, so raising it is unlikely to help; the summariser's failures are calls that never complete.",
    side_effect: PrefSideEffect::None,
});

// ---- Providers (global) ----
pub const VERTEX_REGION: Pref<Text> = Pref::text(PrefSpec {
    key: "vertex_region",
    label: "Vertex region",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value("europe-west1"),
    description: "Google Vertex AI region for vertex-provider models (e.g. 'europe-west1', 'us-central1', 'global').",
    side_effect: PrefSideEffect::None,
});

pub const OPENCODE_FREE_ENABLED: Pref<Flag> = Pref::flag(PrefSpec {
    key: "opencode_free_enabled",
    label: "OpenCode Free models",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("false"),
    description: "Off by default. When 'true', the keyless OpenCode Free tier is available and its models appear in the picker. Requests go anonymously to a third-party relay with no API key and no account, and several of those free models may train on what they receive. Turn it on only if the user asked for free models and accepts that.",
    side_effect: PrefSideEffect::None,
});

pub const PROXY_TIMEOUT_SECS: Pref<Number> = Pref::number(PrefSpec {
    key: "proxy_timeout_secs",
    label: "Proxy timeout (seconds)",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Number {
        min: crate::api::proxy_timeout::MIN_SECS as f64,
        max: crate::api::proxy_timeout::MAX_SECS as f64,
    },
    default: PrefDefault::Value("30"),
    description: "How long the engine proxy waits on one upstream request before it answers 504, in seconds. It covers every proxied call: `lucidos proxy`, `lucidos.proxy` in an app, the proxy_request tool, and the builtin model routes such as vertex and openai. A streamed reply counts in full, because the proxy reads the whole body before it answers. Raise it when a long model call through the proxy times out. An apis.json entry's own `timeout_secs` wins over this for that entry. Applies from the next call.",
    side_effect: PrefSideEffect::None,
});

// ---- Behavior (global) ----
pub const NOTIFICATIONS_FILTER: Pref<Text> = Pref::text(PrefSpec {
    key: "notifications_filter",
    label: "Notifications filter",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(&["all", "unread"]),
    default: PrefDefault::Value("all"),
    description: "Which notifications the bell shows.",
    side_effect: PrefSideEffect::None,
});

pub const NOTIFICATION_TOASTS: Pref<Flag> = Pref::flag(PrefSpec {
    key: "notification_toasts",
    label: "In-app toasts",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("true"),
    description: "Whether a notification pops an in-app toast on a device the user is looking at. 'false' silences the pop-up: the notification still counts on the bell badge and waits in the Notifications panel, and no OS push arrives in its place, because a device that is present never gets one. Unlike push_notifications this covers every device, so one write silences them all.",
    side_effect: PrefSideEffect::None,
});

pub const MOBILE_DYNAMIC_BARS: Pref<Flag> = Pref::flag(PrefSpec {
    key: "mobile_dynamic_bars",
    label: "Dynamic bars",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("false"),
    description: "On a phone, slide the header and the prompt away while scrolling down and bring them back on scroll up ('true'), instead of keeping both always visible ('false').",
    side_effect: PrefSideEffect::None,
});

pub const EXTERNAL_LINK_TARGET: Pref<Text> = Pref::text(PrefSpec {
    key: "external_link_target",
    label: "External links open in",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(EXTERNAL_LINK_TARGETS),
    default: PrefDefault::Value("safari"),
    description: "Where an external http(s) link goes when tapped in an INSTALLED iOS PWA (no effect on desktop, Android, or a normal Safari tab, which all open a new tab). 'safari' hands it to the Safari app; 'ask' opens the OS share sheet so iOS offers every installed browser, including the user's real default; 'in-app' keeps it in the PWA's in-app web view, which has no address bar and no shared Safari session.",
    side_effect: PrefSideEffect::None,
});

pub const WELCOME_SUGGESTIONS_DISMISSED: Pref<Flag> = Pref::flag(PrefSpec {
    key: "welcome_suggestions_dismissed",
    label: "Welcome message dismissed",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("false"),
    description: "Whether the new-workspace welcome message + starter suggestions are hidden. Set 'false' to SHOW the welcome message again, 'true' to hide it. The app also sets it to 'true' by itself after a send once the user has started 3 threads.",
    side_effect: PrefSideEffect::None,
});

// The context-mode eval reads this file as TEXT: it scans for `key: "…"` to
// decide whether the engine knows the flag. Keep the key a literal.
// `the_context_mode_key_is_spelled_out_for_the_eval_scan` pins it.
pub const SELF_CURATED_CONTEXT_MODE: Pref<Flag> = Pref::flag(PrefSpec {
    key: "self_curated_context_mode",
    label: "Self-curated context mode",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("false"),
    description: "EXPERIMENTAL, off by default. When 'true', a chat or trigger thread runs the self-curated context mode. Tool results are then swept away in batches: every ten rounds the sweep takes everything more than five rounds old, and the call that made each one leaves with it. Nothing stands in their place, and doing nothing holds a result until the sweep. The agent keeps its picture of the job in a working understanding it writes as ordinary text in its own reply, and holds one item longer by naming its evt-<hex> address under a [KEEP OPEN] heading there. A context panel rides at the tail of every round, stating how full the prompt is, what each item costs and how long it has left. Everything else rides as it always did, except that the previous turn's tool calls are not re-sent, the conversation summariser does not run, and `todo_write` is withdrawn because the checklist moved into the same block. Coding-agent threads are unaffected. The risk is a re-fetch: a result the agent needed and did not write down costs a round to read back. Leave it off unless the user asked for it.",
    side_effect: PrefSideEffect::None,
});

pub const SELF_CURATED_CONTEXT_EXPIRE_AFTER_ROUNDS: Pref<Number> = Pref::number(PrefSpec {
    key: "self_curated_context_expire_after_rounds",
    label: "Context expiry age (rounds)",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Number { min: 1.0, max: 1000.0 },
    default: PrefDefault::Value("5"),
    description: "How old a tool result has to be before a sweep may take it, in rounds. Only read when `self_curated_context_mode` is 'true'. A sweep takes everything past this age, so with the default sweep interval an item lives 6 to 15 rounds and averages ten. The number is provisional and the eval sweeps it, so the prompt and the panel both quote whatever is in force.",
    side_effect: PrefSideEffect::None,
});

pub const SELF_CURATED_CONTEXT_SWEEP_EVERY_ROUNDS: Pref<Number> = Pref::number(PrefSpec {
    key: "self_curated_context_sweep_every_rounds",
    label: "Context sweep interval (rounds)",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Number { min: 1.0, max: 1000.0 },
    default: PrefDefault::Value("10"),
    description: "How often the sweep runs, in rounds. Only read when `self_curated_context_mode` is 'true'. Removing a pair from the middle of the request invalidates every cached byte after it, so the pass is scheduled rather than run every round: nine rounds in ten are pure appends and keep the cache discount. Setting it to 1 restores a per-round drop and pays that cost on every round.",
    side_effect: PrefSideEffect::None,
});

// ---- Memory module (global, ADR 0362) ----
pub const MEMORY_MODULE: Pref<Text> = Pref::text(PrefSpec {
    key: "memory_module",
    label: "Memory module",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(&["classic", "tree"]),
    default: PrefDefault::Value("classic"),
    description: "How a turn gets its past. 'classic' is the conversation summariser, memory recall and memory search. 'tree' is the summary trees: each turn reads a workspace memory view and a thread memory view, and the agent gets the recall tool (zoom, find, search, date). Choosing 'tree' starts the compactor, which summarises the workspace once in the background. Turns stay on Classic until the workspace tree and the threads active in the last 7 days are built; older threads fill in after. Applies from the next turn.",
    side_effect: PrefSideEffect::None,
});

pub const WORKSPACE_VIEW_BYTES_HOME: Pref<Number> = Pref::number(PrefSpec {
    key: "workspace_view_bytes_home",
    label: "Workspace memory view on the home thread (bytes)",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Number { min: 0.0, max: 262_144.0 },
    default: PrefDefault::Value("65536"),
    description: "How many bytes of the workspace memory view a home thread turn reads, under the 'tree' memory module. 0 turns the view off there.",
    side_effect: PrefSideEffect::None,
});

pub const WORKSPACE_VIEW_BYTES_CHAT: Pref<Number> = Pref::number(PrefSpec {
    key: "workspace_view_bytes_chat",
    label: "Workspace memory view in a chat thread (bytes)",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Number { min: 0.0, max: 262_144.0 },
    default: PrefDefault::Value("16384"),
    description: "How many bytes of the workspace memory view a chat thread turn reads, under the 'tree' memory module. 0 turns the view off there.",
    side_effect: PrefSideEffect::None,
});

pub const WORKSPACE_VIEW_BYTES_TRIGGER: Pref<Number> = Pref::number(PrefSpec {
    key: "workspace_view_bytes_trigger",
    label: "Workspace memory view in a trigger run (bytes)",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Number { min: 0.0, max: 262_144.0 },
    default: PrefDefault::Value("16384"),
    description: "How many bytes of the workspace memory view a trigger run reads, under the 'tree' memory module. Trigger runs are frequent, so this is the size that costs most. 0 turns the view off there.",
    side_effect: PrefSideEffect::None,
});

pub const WORKSPACE_VIEW_BYTES_CODING_AGENT: Pref<Number> = Pref::number(PrefSpec {
    key: "workspace_view_bytes_coding_agent",
    label: "Workspace memory view for a coding agent (bytes)",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Number { min: 0.0, max: 262_144.0 },
    default: PrefDefault::Value("0"),
    description: "How many bytes of the workspace memory view a coding-agent session gets in its system prompt at session start, under the 'tree' memory module. Default is 0, so a coding agent starts with no memory, like Classic. Set a positive value to opt in.",
    side_effect: PrefSideEffect::None,
});

pub const THREAD_VIEW_BYTES: Pref<Number> = Pref::number(PrefSpec {
    key: "thread_view_bytes",
    label: "Thread memory view (bytes)",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Number { min: 0.0, max: 262_144.0 },
    default: PrefDefault::Value("65536"),
    description: "The most bytes of a thread's own past a turn reads, under the 'tree' memory module. Recent entries read verbatim and older ones as summary lines.",
    side_effect: PrefSideEffect::None,
});

pub const MEMORY_VIEW_MODEL_CAPS: Pref<Optional> = Pref::optional(PrefSpec {
    key: "memory_view_model_caps",
    label: "Memory view caps per model",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::ModelBytes,
    default: PrefDefault::Unset("no caps"),
    description: "Caps both memory views for named models, as 'model=bytes' pairs separated by commas, for example 'claude-haiku-4-5=8192'. A turn on a listed model reads at most that many bytes of each view. Unlisted models use the surface sizes alone.",
    side_effect: PrefSideEffect::None,
});

pub const CODING_AGENT_DEFAULT: Pref<Text> = Pref::text(PrefSpec {
    key: "coding_agent_default",
    label: "Default coding agent",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(&["claude-code", "codex"]),
    default: PrefDefault::Value("claude-code"),
    description:
        "Default coding agent the compose destination picker pre-selects for coding targets.",
    side_effect: PrefSideEffect::None,
});

pub const CODING_AGENT_CLAUDE_PATH: Pref<Optional> = Pref::optional(PrefSpec {
    key: "coding_agent_claude_path",
    label: "Claude Code binary path",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("auto-detected"),
    description: "Absolute path to the `claude` CLI used for Claude Code threads. Leave unset to auto-detect (~/.local/bin, ~/.claude/local, Homebrew, PATH). A set path that doesn't exist fails the spawn with an error naming this setting — it never silently falls back.",
    side_effect: PrefSideEffect::None,
});

pub const CODING_AGENT_CODEX_PATH: Pref<Optional> = Pref::optional(PrefSpec {
    key: "coding_agent_codex_path",
    label: "Codex binary path",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("auto-detected"),
    description: "Absolute path to the `codex` CLI used for Codex threads. Leave unset to auto-detect (~/.local/bin, Homebrew, PATH). A set path that doesn't exist fails the spawn with an error naming this setting — it never silently falls back.",
    side_effect: PrefSideEffect::None,
});

pub const CODING_AGENT_CLAUDE_PERMISSION_MODE: Pref<Text> = Pref::text(PrefSpec {
    key: "coding_agent_claude_permission_mode",
    label: "Claude Code permission mode",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(&["accept-edits", "auto"]),
    default: PrefDefault::Value("accept-edits"),
    description: "Which of Claude Code's own permission modes coding-agent threads run in. 'accept-edits' is the default and cards anything outside the session's working directories. 'auto' lets Claude Code's safety classifier approve routine actions instead, which reaches shapes no allowlist can (a `cd` combined with a redirect, a write, or git). Four costs: it ignores a bare `Bash` entry in `cc-allowed-tools`, it denies rather than cards when the classifier is unreachable, a denial streak falls back to prompting, and each gated call pays a classifier round-trip. Applies to new sessions.",
    side_effect: PrefSideEffect::None,
});

// ---- Appearance (device-scoped) ----
pub const THEME_MODE: Pref<Text> = Pref::text(PrefSpec {
    key: "theme-mode",
    label: "Theme mode",
    access: PrefAccess::Agent,
    scope: PrefScope::Device,
    value: PrefValue::Enum(THEME_MODES),
    default: PrefDefault::Value("system"),
    description: "Light or dark for THIS device. Defaults to 'system', which follows the OS light/dark setting; 'light' and 'dark' pin it. Device-scoped, so it overrides the global value on the device that set it.",
    side_effect: PrefSideEffect::None,
});

pub const FONT_FAMILY: Pref<Text> = Pref::text(PrefSpec {
    key: "font-family",
    label: "Font family",
    access: PrefAccess::Agent,
    scope: PrefScope::Device,
    value: PrefValue::FontFamily,
    default: PrefDefault::Value(crate::core::fonts::FOLLOW_THEME),
    description: "UI font for THIS device. The default 'theme' paints the active theme's font, else 'fira-code'. A font id wins over the theme. Every font is bundled, on the device, or a workspace font ('ws-<slug>') (GET /api/v1/fonts), so none loads from the internet.",
    side_effect: PrefSideEffect::None,
});

pub const UI_SCALE: Pref<Number> = Pref::number(PrefSpec {
    key: "ui-scale",
    label: "UI scale",
    access: PrefAccess::Agent,
    scope: PrefScope::Device,
    value: PrefValue::Number { min: 75.0, max: 200.0 },
    default: PrefDefault::Value("100"),
    description: "UI scale percent for THIS device (75–200, snapped to 12.5 steps; 100 = default). Device-scoped.",
    side_effect: PrefSideEffect::None,
});

pub const MOTION: Pref<Text> = Pref::text(PrefSpec {
    key: "motion",
    label: "Motion",
    access: PrefAccess::Agent,
    scope: PrefScope::Device,
    value: PrefValue::Enum(MOTION_PREFS),
    default: PrefDefault::Value("system"),
    description: "Whether animations are reduced on THIS device. Defaults to 'system', which follows the OS reduce-motion setting. 'reduce' calms the app (no slides, pulses or spinners) whatever the OS says; 'full' keeps every animation even when the OS asks to reduce. Device-scoped.",
    side_effect: PrefSideEffect::None,
});

pub const THEME_EFFECTS: Pref<Text> = Pref::text(PrefSpec {
    key: "theme-effects",
    label: "Theme effects",
    access: PrefAccess::Agent,
    scope: PrefScope::Device,
    value: PrefValue::Enum(THEME_EFFECTS_PREFS),
    default: PrefDefault::Value("system"),
    description: "Whether the shadows, filters and scanlines a theme puts on its parts show on THIS device. 'system' drops them when the OS asks for more contrast or less transparency. 'reduce' always drops them and keeps part colours, letter-spacing, the caret shape and borders; 'full' always shows them. Device-scoped.",
    side_effect: PrefSideEffect::None,
});

pub const THEME: Pref<Text> = Pref::text(PrefSpec {
    key: "theme",
    label: "Theme",
    access: PrefAccess::Agent,
    scope: PrefScope::Device,
    value: PrefValue::ThemeId,
    default: PrefDefault::Value(crate::core::themes::DEFAULT_THEME_ID),
    description: "The theme (colours, header and focus styles) on THIS device, by id: a built-in like 'nord' or a workspace theme at data/themes/<id>.json. Device-scoped. See the `themes` knowhow.",
    side_effect: PrefSideEffect::None,
});

// ---- Typing (device-scoped) ----
pub const AUTOCORRECT: Pref<Flag> = Pref::flag(PrefSpec {
    key: "autocorrect",
    label: "Autocorrect",
    access: PrefAccess::Agent,
    scope: PrefScope::Device,
    value: PrefValue::Bool,
    default: PrefDefault::Value("true"),
    description: "Whether text fields autocorrect as the user types on THIS device. Device-scoped. On an iPhone or iPad, iOS autocorrect can swallow the tap on Send or Submit while it holds a correction. The button then does nothing until the keyboard closes. Closing the keyboard and tapping again gets through; offer 'false' if it keeps happening. Spell-check underlines and sentence capitals stay either way.",
    side_effect: PrefSideEffect::None,
});

// ---- Push (device-scoped, side-effecting) ----
pub const PUSH_NOTIFICATIONS: Pref<Optional> = Pref::optional(PrefSpec {
    key: "push_notifications",
    label: "Push notifications",
    access: PrefAccess::Agent,
    scope: PrefScope::Device,
    value: PrefValue::Enum(&["enabled", "declined"]),
    default: PrefDefault::Unset("ask the user"),
    description: "Push notifications for THIS device. Setting 'enabled' triggers the browser/OS permission prompt; 'declined' suppresses it so the user isn't asked again. Device-scoped.",
    side_effect: PrefSideEffect::Push,
});

// ---- Backup (global) ----
// A write re-registers the backup cron through the scheduler's
// PreferencesChanged subscriber, with no engine restart. A scheduled backup
// uploads only once the provider's account is connected in Settings, Accounts.
// The Backup page has no account UI. An enabled schedule generates an
// encryption key on its first run.
pub const BACKUP_SCHEDULE: Pref<Text> = Pref::text(PrefSpec {
    key: "backup_schedule",
    label: "Backup schedule",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value("off"),
    description: "Automatic backup schedule as a 6-field cron expression in the user's timezone (second minute hour day-of-month month day-of-week), or 'off' to disable. Examples: '0 0 3 * * *' = daily 03:00, '0 0 3 * * 0' = weekly Sunday 03:00, '0 0 */12 * * *' = every 12h. Requires backup_provider set AND its account connected (see that key).",
    side_effect: PrefSideEffect::None,
});

pub const BACKUP_PROVIDER: Pref<Optional> = Pref::optional(PrefSpec {
    key: "backup_provider",
    label: "Backup provider",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Enum(crate::core::backup::PROVIDER_IDS),
    default: PrefDefault::Unset("ask the user"),
    description: "Cloud destination for backups, independent of backup_schedule: it stays set with the schedule off, and the Backup page's provider dropdown both opens on it and writes it. Setting this connects NOTHING: the account is connected separately with connect_oauth_account, or by the user in Settings → Accounts (never on the Backup page, which has no account UI). Until then backups run and the upload fails. Check get_backup_status, which reports whether the account is connected.",
    side_effect: PrefSideEffect::None,
});

pub const BACKUP_RETENTION: Pref<Number> = Pref::number(PrefSpec {
    key: "backup_retention",
    label: "Backup retention",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Number { min: 1.0, max: 1000.0 },
    default: PrefDefault::Value("5"),
    description: "How many of the most recent backups to keep in the provider; older ones are pruned after each successful backup.",
    side_effect: PrefSideEffect::None,
});

pub const BACKUP_REMINDER_DISMISSED: Pref<Optional> = Pref::optional(PrefSpec {
    key: "backup_reminder_dismissed",
    label: "Backup reminder dismissed",
    access: PrefAccess::Agent,
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("the reminder shows"),
    description: "Dismissal state of the app-shell banner shown while backup is off (no active backup_schedule with a backup_provider). Three values: unset or empty means never dismissed, so the banner shows; an RFC 3339 instant means dismissed then, hidden for 30 days from that instant; 'forever' means dismissed a second time, hidden permanently. Set it to empty to bring the reminder back. The banner is only ever shown while backup is off, so enabling a schedule hides it whatever this says.",
    side_effect: PrefSideEffect::None,
});

// ---- Human-only: real settings the agent must not write ----

pub const COMMAND_GUARD: Pref<Flag> = Pref::flag(PrefSpec {
    key: "command_guard",
    label: "Command guard",
    access: PrefAccess::Human {
        hint: "the command guard is the safety gate over the agent's own bash/python — toggle it in Settings → Permissions, not via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("false"),
    description: "Master switch for the command guard (ADR 0002), the pre-dispatch safety gate over the Lucidos Agent's bash and python tools. Ships dark: a workspace opts in.",
    side_effect: PrefSideEffect::None,
});

pub const COMMAND_GUARD_JUDGE: Pref<Flag> = Pref::flag(PrefSpec {
    key: "command_guard_judge",
    label: "Command-guard judge",
    access: PrefAccess::Human {
        hint: "managed in Settings → Permissions (Command safety)",
    },
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("true"),
    description: "Whether the LLM judge classifies the commands the command guard's static lists do not settle. 'false' falls back to the static dangerous list for the ask lane. Read only while command_guard is on.",
    side_effect: PrefSideEffect::None,
});

pub const MODEL_COMMAND_JUDGE: Pref<Text> = Pref::text(PrefSpec {
    key: "model_command_judge",
    label: "Command-judge model",
    access: PrefAccess::Human {
        hint: "managed in Settings → Permissions (Command safety)",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value("claude-haiku-4-5"),
    description: concat!(
        "The model the command-guard judge runs on.",
        aux_fallback_note!()
    ),
    side_effect: PrefSideEffect::None,
});

pub const REASONING_COMMAND_JUDGE: Pref<Text> = Pref::text(PrefSpec {
    key: "reasoning_command_judge",
    label: "Command-judge reasoning",
    access: PrefAccess::Human {
        hint: "the judge's thinking budget is part of the command guard, so it is managed in Settings → Permissions (Command safety), not via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Enum(REASONING_EFFORTS),
    default: PrefDefault::Value("none"),
    description: "The effort the command-guard judge runs at. It answers with a short JSON verdict in front of every ambiguous command, so the default spends nothing.",
    side_effect: PrefSideEffect::None,
});

pub const JUDGMENT_COMMAND_GUARD: Pref<Optional> = Pref::optional(PrefSpec {
    key: "judgment_command_guard",
    label: "Command-guard backend",
    access: PrefAccess::Human {
        hint: "which backend classifies the agent's own commands (chat, or a System One model such as jev or clef) is part of the command guard, so it is managed in Settings → Permissions (Command safety), not via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("the judge runs on its chat model"),
    description: "The System One model that classifies the agent's commands, by id. Unset asks the judge's chat model.",
    side_effect: PrefSideEffect::None,
});

pub const JUDGMENT_QUERY_CLASSIFICATION: Pref<Optional> = Pref::optional(PrefSpec {
    key: "judgment_query_classification",
    label: "Query-classification backend",
    access: PrefAccess::Human {
        hint: "which backend decides whether a message needs memory (chat, or a System One model such as jev or clef): infrastructure routing over your own retrieval, managed in Settings → Models (Background tasks), not via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("the classifier runs on its chat model"),
    description: "The System One model that classifies a turn's query, by id. Unset asks model_query_classification.",
    side_effect: PrefSideEffect::None,
});

pub const JUDGMENT_MEMORY_FIND: Pref<Optional> = Pref::optional(PrefSpec {
    key: "judgment_memory_find",
    label: "Memory-find backend",
    access: PrefAccess::Human {
        hint: "which backend walks the memory tree for find (chat, or a System One model): picking a System One model sends memory lines to that vendor, so only the user decides, in Settings, never via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("find runs on its chat model"),
    description: "The System One model the recall tool's find asks, by id. Unset asks model_memory_find.",
    side_effect: PrefSideEffect::None,
});

pub const MAX_TOOL_CALLS: Pref<Number> = Pref::number(PrefSpec {
    key: "max_tool_calls",
    label: "Tool calls per turn",
    access: PrefAccess::Human {
        hint: "the per-turn tool-call cap is the backstop over your own agentic loop, so you must not raise your own limit; the user changes it in Settings → Models → Chat & triggers",
    },
    scope: PrefScope::Global,
    value: PrefValue::Number { min: 1.0, max: u32::MAX as f64 },
    default: PrefDefault::Value("500"),
    description: "How many tool calls the Lucidos Agent may make in one chat or trigger turn before the loop stops with its [ENGINE-LIMIT] terminator. The floor of 1 rules out the one broken value: a cap of 0 would stop the turn before its first call.",
    side_effect: PrefSideEffect::None,
});

pub const CAPTURE_CONTEXT: Pref<Flag> = Pref::flag(PrefSpec {
    key: "capture_context",
    label: "Context capture",
    access: PrefAccess::Human {
        hint: "a debug-only context-capture toggle; change it in Settings if you really need to",
    },
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("false"),
    description: "Whether ContextCaptured keeps each LLM call's section bodies. Off keeps only each section's name and its two sizes.",
    side_effect: PrefSideEffect::None,
});

pub const KEYBINDINGS: Pref<Optional> = Pref::optional(PrefSpec {
    key: "keybindings",
    label: "Keyboard shortcuts",
    access: PrefAccess::Human {
        hint: "keyboard shortcuts are managed in Settings → Keyboard Shortcuts",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("the shipped shortcuts"),
    description: "The user's keyboard shortcut overrides, as JSON.",
    side_effect: PrefSideEffect::None,
});

pub const VOICE_INPUT_DEVICE: Pref<Optional> = Pref::optional(PrefSpec {
    key: "voice_input_device",
    label: "Voice input device",
    access: PrefAccess::Human {
        hint: "which microphone a call opens on one device: a browser's own opaque handle for it, which means nothing anywhere else. The user picks it by holding the call control, and you cannot know what the ids on their machine stand for",
    },
    scope: PrefScope::Device,
    value: PrefValue::Text,
    default: PrefDefault::Unset("the system default microphone"),
    description: "The microphone a call opens on this device, as the browser's device id.",
    side_effect: PrefSideEffect::None,
});

pub const NETWORK_BIND: Pref<Optional> = Pref::optional(PrefSpec {
    key: "network_bind",
    label: "Engine network bind",
    access: PrefAccess::Human {
        hint: "the per-workspace engine network bind (loopback / all interfaces / a specific tailnet IP): a security setting changed in Settings → Access → Network access, not via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("the bind network.toml and the environment resolve"),
    description: "The per-workspace engine bind: loopback, all interfaces, or one tailnet IP.",
    side_effect: PrefSideEffect::None,
});

pub const ENGINE_SWITCH_DISMISSED_BUILD: Pref<Optional> = Pref::optional(PrefSpec {
    key: "engine_switch_dismissed_build",
    label: "Deferred engine switch",
    access: PrefAccess::Human {
        hint: "internal UI state — the on-disk engine build id the user deferred the 'Switch to new version' toast for (workspace-global so a dismiss defers on every device); managed by the version-update toast, not a setting",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("never deferred"),
    description: "The engine build id the user deferred the 'Switch to new version' toast for.",
    side_effect: PrefSideEffect::None,
});

pub const CLIENT_REFRESH_DISMISSED_BUILD: Pref<Optional> = Pref::optional(PrefSpec {
    key: "client_refresh_dismissed_build",
    label: "Deferred client refresh",
    access: PrefAccess::Human {
        hint: "internal UI state — the served client build id the user deferred the 'refresh to sync' toast for (workspace-global so a dismiss defers on every device); managed by the version-update toast, not a setting",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("never deferred"),
    description: "The client build id the user deferred the 'refresh to sync' toast for.",
    side_effect: PrefSideEffect::None,
});

pub const PROVIDER_ENABLED_VERTEX: Pref<Flag> = Pref::flag(PrefSpec {
    key: "provider_enabled_vertex",
    label: "Vertex provider",
    access: PrefAccess::Human {
        hint: "whether the Vertex provider is switched on: the user decides in Settings → Models → Providers, and you must not switch off a provider that may be serving this turn",
    },
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("true"),
    description: "Whether the Vertex provider is switched on. A veto over a configured provider: off parks it and keeps its credential.",
    side_effect: PrefSideEffect::None,
});

pub const PROVIDER_ENABLED_ANTHROPIC: Pref<Flag> = Pref::flag(PrefSpec {
    key: "provider_enabled_anthropic",
    label: "Anthropic provider",
    access: PrefAccess::Human {
        hint: "whether the direct Anthropic provider is switched on: managed in Settings → Models → Providers, never via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("true"),
    description: "Whether the Anthropic provider is switched on. A veto over a configured provider: off parks it and keeps its credential.",
    side_effect: PrefSideEffect::None,
});

pub const PROVIDER_ENABLED_OPENAI: Pref<Flag> = Pref::flag(PrefSpec {
    key: "provider_enabled_openai",
    label: "OpenAI provider",
    access: PrefAccess::Human {
        hint: "whether the direct OpenAI provider is switched on: managed in Settings → Models → Providers, never via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("true"),
    description: "Whether the OpenAI provider is switched on. A veto over a configured provider: off parks it and keeps its credential.",
    side_effect: PrefSideEffect::None,
});

pub const PROVIDER_ENABLED_OPENROUTER: Pref<Flag> = Pref::flag(PrefSpec {
    key: "provider_enabled_openrouter",
    label: "OpenRouter provider",
    access: PrefAccess::Human {
        hint: "whether the OpenRouter provider is switched on: managed in Settings → Models → Providers, never via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("true"),
    description: "Whether the OpenRouter provider is switched on. A veto over a configured provider: off parks it and keeps its credential.",
    side_effect: PrefSideEffect::None,
});

pub const PROVIDER_ENABLED_XAI: Pref<Flag> = Pref::flag(PrefSpec {
    key: "provider_enabled_xai",
    label: "xAI provider",
    access: PrefAccess::Human {
        hint: "whether the xAI provider is switched on: managed in Settings → Models → Providers, never via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("true"),
    description: "Whether the xAI provider is switched on. A veto over a configured provider: off parks it and keeps its credential.",
    side_effect: PrefSideEffect::None,
});

pub const PROVIDER_ENABLED_LOCAL: Pref<Flag> = Pref::flag(PrefSpec {
    key: "provider_enabled_local",
    label: "local provider",
    access: PrefAccess::Human {
        hint: "whether the local OpenAI-compatible provider is switched on: managed in Settings → Models → Providers, never via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("true"),
    description: "Whether the local provider is switched on. A veto over a configured provider: off parks it and keeps its credential.",
    side_effect: PrefSideEffect::None,
});

pub const PROVIDER_ENABLED_TYPESAFE: Pref<Flag> = Pref::flag(PrefSpec {
    key: "provider_enabled_typesafe",
    label: "TypeSafe provider",
    access: PrefAccess::Human {
        hint: "whether TypeSafe (Jev) is switched on at all: the master switch above the judgment_* keys, managed in Settings → Models → Providers. Never via set_preference: switching it off moves the command guard's backend, which you must not do",
    },
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("true"),
    description: "Whether the TypeSafe provider is switched on. A veto over a configured provider: off parks it and keeps its credential.",
    side_effect: PrefSideEffect::None,
});

pub const PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI: Pref<Flag> = Pref::flag(PrefSpec {
    key: "provider_enabled_cloudflare_workers_ai",
    label: "Cloudflare Workers AI provider",
    access: PrefAccess::Human {
        hint: "whether Cloudflare Workers AI (Clef and Clef-flash) is switched on at all: the master switch above the judgment_* keys, managed in Settings → Models → Providers. Never via set_preference: switching it off moves the command guard's backend, which you must not do",
    },
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("true"),
    description: "Whether the Cloudflare Workers AI provider is switched on. A veto over a configured provider: off parks it and keeps its credential.",
    side_effect: PrefSideEffect::None,
});

pub const PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM: Pref<Flag> = Pref::flag(PrefSpec {
    key: "provider_enabled_system_one_custom",
    label: "custom System One provider",
    access: PrefAccess::Human {
        hint: "whether the custom System One endpoint is switched on at all: the master switch above the judgment_* keys, managed in Settings → Models → Providers. Never via set_preference: switching it off moves the command guard's backend, which you must not do",
    },
    scope: PrefScope::Global,
    value: PrefValue::Bool,
    default: PrefDefault::Value("true"),
    description: "Whether the custom System One provider is switched on. A veto over a configured provider: off parks it and keeps its credential.",
    side_effect: PrefSideEffect::None,
});

pub const LOCAL_BASE_URL: Pref<Text> = Pref::text(PrefSpec {
    key: "local_base_url",
    label: "Local model URL",
    access: PrefAccess::Human {
        hint: "the local provider's base URL, which receives every prompt and answers with the tool calls you run: the user sets it in Settings → Models → Providers, never via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Value("http://localhost:11434/v1"),
    description: "Where local-model chat goes: an OpenAI-compatible base URL. The env var LUCIDOS_LOCAL_BASE_URL layers between a stored value and this default.",
    side_effect: PrefSideEffect::None,
});

pub const SYSTEM_ONE_CUSTOM_URL: Pref<Optional> = Pref::optional(PrefSpec {
    key: "system_one_custom_url",
    label: "Custom System One URL",
    access: PrefAccess::Human {
        hint: "the custom System One endpoint's URL, which receives every state a judgment site sends it and can answer the command guard: the user sets it in Settings → Models → Providers, never via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("no custom endpoint"),
    description: "The custom System One endpoint's URL.",
    side_effect: PrefSideEffect::None,
});

pub const SYSTEM_ONE_CUSTOM_MODEL: Pref<Optional> = Pref::optional(PrefSpec {
    key: "system_one_custom_model",
    label: "Custom System One model",
    access: PrefAccess::Human {
        hint: "the model the custom System One endpoint is asked for: set beside its URL in Settings → Models → Providers, never via set_preference",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("no custom model"),
    description: "The model the custom System One endpoint is asked for.",
    side_effect: PrefSideEffect::None,
});

// ---- Engine bookkeeping: refused to the agent, written silently ----

pub const VAPID_KEYS: Pref<Optional> = Pref::optional(PrefSpec {
    key: "vapid_keys",
    label: "Web Push keys",
    access: PrefAccess::Engine {
        hint: "the workspace's Web Push signing keypair, generated once on demand; a transport secret, not a setting",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("not written yet"),
    description: "the workspace's Web Push signing keypair, generated once on demand; a transport secret, not a setting",
    side_effect: PrefSideEffect::None,
});

pub const BACKUP_LAST_RUN: Pref<Optional> = Pref::optional(PrefSpec {
    key: "backup_last_run",
    label: "Last backup",
    access: PrefAccess::Engine {
        hint: "internal backup state, not a setting: the backup job writes the outcome of its last run so the Settings page survives a restart",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("not written yet"),
    description: "internal backup state, not a setting: the backup job writes the outcome of its last run so the Settings page survives a restart",
    side_effect: PrefSideEffect::None,
});

pub const RELEASE_NOTICE_CURSOR: Pref<Optional> = Pref::optional(PrefSpec {
    key: "release_notice_cursor",
    label: "Release notice cursor",
    access: PrefAccess::Engine {
        hint: "internal UI state: the id of the last release notice this workspace answered, which is what makes that sequence ordered and one-time. The user answers a notice in its own modal, and clearing this by hand re-shows every notice they have already read",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("not written yet"),
    description: "internal UI state: the id of the last release notice this workspace answered, which is what makes that sequence ordered and one-time. The user answers a notice in its own modal, and clearing this by hand re-shows every notice they have already read",
    side_effect: PrefSideEffect::None,
});

pub const BACKFILL_TRIGGER_ID_FROM_EVENTS_DONE: Pref<Optional> = Pref::optional(PrefSpec {
    key: "backfill_trigger_id_from_events_done",
    label: "Backfill marker",
    access: PrefAccess::Engine {
        hint: "a one-shot migration marker, so a completed backfill is not re-run",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("not written yet"),
    description: "a one-shot migration marker, so a completed backfill is not re-run",
    side_effect: PrefSideEffect::None,
});

pub const BACKFILL_TRIGGER_ID_V5_TO_CONFIG_ID_DONE: Pref<Optional> = Pref::optional(PrefSpec {
    key: "backfill_trigger_id_v5_to_config_id_done",
    label: "Backfill marker",
    access: PrefAccess::Engine {
        hint: "a one-shot migration marker, so a completed backfill is not re-run",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("not written yet"),
    description: "a one-shot migration marker, so a completed backfill is not re-run",
    side_effect: PrefSideEffect::None,
});

pub const BACKFILL_REPO_NAMES_FROM_CHANGES_DONE: Pref<Optional> = Pref::optional(PrefSpec {
    key: "backfill_repo_names_from_changes_done",
    label: "Backfill marker",
    access: PrefAccess::Engine {
        hint: "a one-shot migration marker, so a completed backfill is not re-run",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("not written yet"),
    description: "a one-shot migration marker, so a completed backfill is not re-run",
    side_effect: PrefSideEffect::None,
});

pub const BACKFILL_CC_REPO_ID_TO_DETERMINISTIC_DONE: Pref<Optional> = Pref::optional(PrefSpec {
    key: "backfill_cc_repo_id_to_deterministic_done",
    label: "Backfill marker",
    access: PrefAccess::Engine {
        hint: "a one-shot migration marker, so a completed backfill is not re-run",
    },
    scope: PrefScope::Global,
    value: PrefValue::Text,
    default: PrefDefault::Unset("not written yet"),
    description: "a one-shot migration marker, so a completed backfill is not re-run",
    side_effect: PrefSideEffect::None,
});

/// Every preference the engine reads or writes, in one list. The agent-facing
/// tools show only the [`PrefAccess::Agent`] entries.
pub const CATALOG: &[&PrefSpec] = &[
    &LANGUAGE.spec,
    &TIMEZONE.spec,
    &CHAT_MODEL.spec,
    &CHAT_REASONING_EFFORT.spec,
    &RESPONSE_STYLE.spec,
    &RESPONSE_STYLES.spec,
    &TECHNICAL_LITERACY.spec,
    &IMAGE_MODEL.spec,
    &MODEL_TITLE.spec,
    &REASONING_TITLE.spec,
    &MODEL_IMAGE_DESCRIPTION.spec,
    &REASONING_IMAGE_DESCRIPTION.spec,
    &MODEL_CHANGE_SUMMARY.spec,
    &REASONING_CHANGE_SUMMARY.spec,
    &MODEL_SUMMARY_COMPACTION.spec,
    &REASONING_SUMMARY_COMPACTION.spec,
    &MODEL_MEMORY_FIND.spec,
    &REASONING_MEMORY_FIND.spec,
    &MODEL_MEMORY.spec,
    &REASONING_MEMORY.spec,
    &MODEL_QUERY_CLASSIFICATION.spec,
    &REASONING_QUERY_CLASSIFICATION.spec,
    &MODEL_CONVERSATION_SUMMARY.spec,
    &VOICE_ENABLED.spec,
    &HOME_THREAD_ENABLED.spec,
    &MODEL_VOICE_TALKER.spec,
    &MODEL_VOICE_TRANSCRIBER.spec,
    &VOICE_TALKER_VOICE.spec,
    &VOICE_RESIDENT_SECTIONS.spec,
    &REASONING_CONVERSATION_SUMMARY.spec,
    &VERTEX_REGION.spec,
    &OPENCODE_FREE_ENABLED.spec,
    &PROXY_TIMEOUT_SECS.spec,
    &NOTIFICATIONS_FILTER.spec,
    &NOTIFICATION_TOASTS.spec,
    &MOBILE_DYNAMIC_BARS.spec,
    &EXTERNAL_LINK_TARGET.spec,
    &WELCOME_SUGGESTIONS_DISMISSED.spec,
    &SELF_CURATED_CONTEXT_MODE.spec,
    &SELF_CURATED_CONTEXT_EXPIRE_AFTER_ROUNDS.spec,
    &SELF_CURATED_CONTEXT_SWEEP_EVERY_ROUNDS.spec,
    &MEMORY_MODULE.spec,
    &WORKSPACE_VIEW_BYTES_HOME.spec,
    &WORKSPACE_VIEW_BYTES_CHAT.spec,
    &WORKSPACE_VIEW_BYTES_TRIGGER.spec,
    &WORKSPACE_VIEW_BYTES_CODING_AGENT.spec,
    &THREAD_VIEW_BYTES.spec,
    &MEMORY_VIEW_MODEL_CAPS.spec,
    &CODING_AGENT_DEFAULT.spec,
    &CODING_AGENT_CLAUDE_PATH.spec,
    &CODING_AGENT_CODEX_PATH.spec,
    &CODING_AGENT_CLAUDE_PERMISSION_MODE.spec,
    &THEME_MODE.spec,
    &FONT_FAMILY.spec,
    &UI_SCALE.spec,
    &MOTION.spec,
    &THEME_EFFECTS.spec,
    &THEME.spec,
    &AUTOCORRECT.spec,
    &PUSH_NOTIFICATIONS.spec,
    &BACKUP_SCHEDULE.spec,
    &BACKUP_PROVIDER.spec,
    &BACKUP_RETENTION.spec,
    &BACKUP_REMINDER_DISMISSED.spec,
    &COMMAND_GUARD.spec,
    &COMMAND_GUARD_JUDGE.spec,
    &MODEL_COMMAND_JUDGE.spec,
    &REASONING_COMMAND_JUDGE.spec,
    &JUDGMENT_COMMAND_GUARD.spec,
    &JUDGMENT_QUERY_CLASSIFICATION.spec,
    &JUDGMENT_MEMORY_FIND.spec,
    &MAX_TOOL_CALLS.spec,
    &CAPTURE_CONTEXT.spec,
    &KEYBINDINGS.spec,
    &VOICE_INPUT_DEVICE.spec,
    &NETWORK_BIND.spec,
    &ENGINE_SWITCH_DISMISSED_BUILD.spec,
    &CLIENT_REFRESH_DISMISSED_BUILD.spec,
    &PROVIDER_ENABLED_VERTEX.spec,
    &PROVIDER_ENABLED_ANTHROPIC.spec,
    &PROVIDER_ENABLED_OPENAI.spec,
    &PROVIDER_ENABLED_OPENROUTER.spec,
    &PROVIDER_ENABLED_XAI.spec,
    &PROVIDER_ENABLED_LOCAL.spec,
    &PROVIDER_ENABLED_TYPESAFE.spec,
    &PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI.spec,
    &PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM.spec,
    &LOCAL_BASE_URL.spec,
    &SYSTEM_ONE_CUSTOM_URL.spec,
    &SYSTEM_ONE_CUSTOM_MODEL.spec,
    &VAPID_KEYS.spec,
    &BACKUP_LAST_RUN.spec,
    &RELEASE_NOTICE_CURSOR.spec,
    &BACKFILL_TRIGGER_ID_FROM_EVENTS_DONE.spec,
    &BACKFILL_TRIGGER_ID_V5_TO_CONFIG_ID_DONE.spec,
    &BACKFILL_REPO_NAMES_FROM_CHANGES_DONE.spec,
    &BACKFILL_CC_REPO_ID_TO_DETERMINISTIC_DONE.spec,
];

/// The spec for any catalogued key, whoever may write it.
pub fn spec(key: &str) -> Option<&'static PrefSpec> {
    CATALOG.iter().copied().find(|s| s.key == key)
}

/// Find the catalog spec for an agent-settable key.
pub fn lookup(key: &str) -> Option<&'static PrefSpec> {
    spec(key).filter(|s| s.access == PrefAccess::Agent)
}

/// If `key` is a known-but-not-agent-settable preference, return the hint
/// explaining where the human changes it.
pub fn internal_hint(key: &str) -> Option<&'static str> {
    match spec(key)?.access {
        PrefAccess::Agent => None,
        PrefAccess::Human { hint } | PrefAccess::Engine { hint } => Some(hint),
    }
}

/// The keys the agent must not write: every [`PrefAccess::Human`] and
/// [`PrefAccess::Engine`] spec.
pub fn internal_specs() -> impl Iterator<Item = &'static PrefSpec> {
    CATALOG
        .iter()
        .copied()
        .filter(|s| s.access != PrefAccess::Agent)
}

/// Whether a preference key is engine bookkeeping rather than a setting, and so
/// writes WITHOUT announcing `PreferencesChanged`.
///
/// The `preferences` table holds two different things: settings the user
/// changes, which every device must learn about, and engine bookkeeping that
/// needs a durable home. Announcing is the default. `PreferenceStore::set_silent`
/// rejects any key this refuses, so silence has to be asked for by name: a
/// [`PrefAccess::Engine`] spec.
pub fn is_silent_key(key: &str) -> bool {
    spec(key).is_some_and(|s| matches!(s.access, PrefAccess::Engine { .. }))
}

/// Validate `value` against a spec's [`PrefValue`]. Returns a human-readable
/// error (suitable to hand back to the agent) on rejection.
pub fn validate(spec: &PrefSpec, value: &str) -> Result<(), String> {
    match spec.value {
        PrefValue::Bool => {
            if value == "true" || value == "false" {
                Ok(())
            } else {
                Err(format!(
                    "'{}' must be 'true' or 'false' (got '{}')",
                    spec.key, value
                ))
            }
        }
        PrefValue::Enum(allowed) => {
            if allowed.contains(&value) {
                Ok(())
            } else {
                Err(format!(
                    "'{}' must be one of [{}] (got '{}')",
                    spec.key,
                    allowed.join(", "),
                    value
                ))
            }
        }
        PrefValue::Number { min, max } => match value.parse::<f64>() {
            Ok(n) if n >= min && n <= max => Ok(()),
            Ok(n) => Err(format!(
                "'{}' must be a number between {} and {} (got {})",
                spec.key, min, max, n
            )),
            Err(_) => Err(format!("'{}' must be a number (got '{}')", spec.key, value)),
        },
        PrefValue::IanaTimezone => {
            if value.parse::<chrono_tz::Tz>().is_ok() {
                Ok(())
            } else {
                Err(format!(
                    "'{}' must be an IANA timezone name like 'Europe/Oslo' or 'America/New_York' (got '{}')",
                    spec.key, value
                ))
            }
        }
        PrefValue::Text => {
            if value.trim().is_empty() {
                Err(format!("'{}' must not be empty", spec.key))
            } else {
                Ok(())
            }
        }
        PrefValue::ThemeId => {
            crate::core::themes::validate_id(value).map_err(|e| format!("'{}': {}", spec.key, e))
        }
        PrefValue::ModelBytes => crate::engine::summary_tree::module::parse_model_caps(value)
            .map(|_| ())
            .map_err(|e| format!("'{}': {}", spec.key, e)),
        PrefValue::FontFamily => {
            if crate::core::fonts::FONT_PREFERENCE_VALUES.contains(&value)
                || crate::core::workspace_fonts::is_workspace_id(value)
            {
                Ok(())
            } else {
                Err(format!(
                    "'{}' must be one of [{}] (got '{}')",
                    spec.key,
                    allowed_values_hint(spec),
                    value
                ))
            }
        }
    }
}

/// Human-readable summary of a spec's allowed values, used in the agent-facing
/// description for both `set_preference`'s error path and `get_preferences`.
pub fn allowed_values_hint(spec: &PrefSpec) -> String {
    match spec.value {
        PrefValue::Bool => "true | false".to_string(),
        PrefValue::Enum(allowed) => allowed.join(" | "),
        PrefValue::Number { min, max } => format!("number {}–{}", min, max),
        PrefValue::IanaTimezone => "IANA timezone name".to_string(),
        PrefValue::Text => "text".to_string(),
        PrefValue::ThemeId => "a theme id".to_string(),
        PrefValue::ModelBytes => "model=bytes, model=bytes".to_string(),
        PrefValue::FontFamily => format!(
            "{} | ws-<slug> (a workspace font)",
            crate::core::fonts::FONT_PREFERENCE_VALUES.join(" | ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_keys_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for spec in CATALOG {
            assert!(seen.insert(spec.key), "duplicate catalog key: {}", spec.key);
        }
    }

    /// A handle const missing from `CATALOG` would be readable but invisible
    /// to `spec()`, the agent's tools, the doc check and the TS catalog.
    #[test]
    fn every_handle_is_listed_in_the_catalog() {
        let source = include_str!("preference_catalog.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        let declared = [
            "Pref::flag(",
            "Pref::number(",
            "Pref::text(",
            "Pref::optional(",
        ]
        .iter()
        .map(|ctor| {
            production
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .filter(|line| line.contains(&format!("= {ctor}")))
                .count()
        })
        .sum::<usize>();
        assert_eq!(
            declared,
            CATALOG.len(),
            "a Pref const is missing from CATALOG"
        );
    }

    #[test]
    fn command_guard_is_not_settable_but_has_a_hint() {
        assert!(lookup(COMMAND_GUARD.key()).is_none());
        assert!(internal_hint(COMMAND_GUARD.key()).is_some());
    }

    /// The local model host reads every prompt and writes the replies the
    /// agentic loop runs. The agent choosing it would let a prompt injection
    /// move the agent onto a model the attacker controls.
    #[test]
    fn the_local_model_host_is_not_agent_settable() {
        assert!(lookup(LOCAL_BASE_URL.key()).is_none());
        let hint = internal_hint(LOCAL_BASE_URL.key()).expect("internal");
        assert!(hint.contains("Settings → Models → Providers"), "{hint}");
    }

    /// The per-turn tool-call cap is the backstop over the agent's own loop, so
    /// the agent must not raise it. It still gets an actionable hint rather
    /// than "unknown key".
    #[test]
    fn max_tool_calls_is_not_settable_but_has_a_hint() {
        assert!(lookup(MAX_TOOL_CALLS.key()).is_none());
        let hint = internal_hint(MAX_TOOL_CALLS.key()).expect("must carry a hint");
        assert!(
            hint.contains("Settings"),
            "the hint must point at the Settings surface, got: {hint}"
        );
    }

    /// Only engine bookkeeping writes silently; a setting always announces.
    #[test]
    fn only_engine_specs_are_silent() {
        for spec in CATALOG {
            assert_eq!(
                is_silent_key(spec.key),
                matches!(spec.access, PrefAccess::Engine { .. }),
                "{}",
                spec.key
            );
        }
    }

    #[test]
    fn bool_validation() {
        let spec = &WELCOME_SUGGESTIONS_DISMISSED.spec;
        assert!(validate(spec, "true").is_ok());
        assert!(validate(spec, "false").is_ok());
        assert!(validate(spec, "yes").is_err());
    }

    #[test]
    fn enum_validation() {
        let spec = &THEME_MODE.spec;
        assert!(validate(spec, "dark").is_ok());
        assert!(validate(spec, "system").is_ok());
        assert!(validate(spec, "blue").is_err());
    }

    #[test]
    fn number_validation_respects_bounds() {
        let spec = &UI_SCALE.spec;
        assert!(validate(spec, "100").is_ok());
        assert!(validate(spec, "137.5").is_ok());
        assert!(validate(spec, "50").is_err());
        assert!(validate(spec, "300").is_err());
        assert!(validate(spec, "big").is_err());
    }

    #[test]
    fn timezone_validation() {
        let spec = &TIMEZONE.spec;
        assert!(validate(spec, "Europe/Oslo").is_ok());
        assert!(validate(spec, "Not/AZone").is_err());
    }

    #[test]
    fn font_family_takes_a_catalog_font_theme_or_a_workspace_font() {
        let spec = &FONT_FAMILY.spec;
        for value in [
            "theme",
            "fira-code",
            "inter",
            "ws-brand-sans",
            FONT_FAMILY.default_text(),
        ] {
            assert!(validate(spec, value).is_ok(), "{value}");
        }
        for value in ["", "comic-sans", "ws-", "ws-Brand", "ws-a/b", "url(x)"] {
            assert!(validate(spec, value).is_err(), "{value}");
        }
    }

    /// A default the value type does not allow is a default nothing can ever
    /// hold: the agent reports it, the user asks for it, and `set_preference`
    /// rejects it. It is also what the handles' typed default accessors parse.
    #[test]
    fn every_default_is_a_valid_value() {
        for spec in CATALOG {
            if let PrefDefault::Value(value) = spec.default {
                assert!(
                    validate(spec, value).is_ok(),
                    "'{}' defaults to '{}', which its own validation refuses",
                    spec.key,
                    value
                );
            }
        }
    }

    /// An inherited default resolves through the other key, so the chain must
    /// end in a value of the same shape, and never loop.
    #[test]
    fn inherited_defaults_end_in_a_value() {
        for spec in CATALOG {
            if !matches!(spec.default, PrefDefault::Inherits(_)) {
                continue;
            }
            let mut at: &PrefSpec = spec;
            for _ in 0..CATALOG.len() {
                match at.default {
                    PrefDefault::Inherits(other) => {
                        assert_eq!(
                            std::mem::discriminant(&at.value),
                            std::mem::discriminant(&other.value),
                            "{} inherits {}, which holds another kind of value",
                            at.key,
                            other.key
                        );
                        at = other;
                    }
                    PrefDefault::Value(_) => break,
                    PrefDefault::Unset(_) => panic!("{} inherits an unset default", spec.key),
                }
            }
            assert!(
                matches!(at.default, PrefDefault::Value(_)),
                "{}'s inheritance loops",
                spec.key
            );
        }
    }

    /// The mode's flag, in the exact shape its eval reads.
    ///
    /// The harness resolves whether the engine implements the mode by scanning
    /// this file for `key: "…"` (`lucidos_eval::manipulation::declared_keys`).
    /// A constant in the spec literal would read as a missing flag and refuse a
    /// lean run, while the engine itself still compiled and worked.
    #[test]
    fn the_context_mode_key_is_spelled_out_for_the_eval_scan() {
        let source = include_str!("preference_catalog.rs");
        assert!(
            source.contains(&format!("key: \"{}\"", SELF_CURATED_CONTEXT_MODE.key())),
            "the catalog must spell the context-mode key out as a literal"
        );
        let spec = lookup(SELF_CURATED_CONTEXT_MODE.key())
            .expect("the context-mode flag is agent-settable");
        assert_eq!(spec.scope, PrefScope::Global);
        assert!(
            !SELF_CURATED_CONTEXT_MODE.default_flag(),
            "the mode ships dark"
        );
    }

    /// The default has to name a real row, or the picker opens on nothing.
    #[test]
    fn the_response_style_default_is_a_shipped_style() {
        use crate::core::response_style;

        assert_eq!(RESPONSE_STYLE.spec.scope, PrefScope::Global);
        // Text rather than an enum, deliberately: the user may add styles, so
        // the set is open. `chat_model` is Text for the same reason.
        assert!(matches!(RESPONSE_STYLE.spec.value, PrefValue::Text));
        let library = response_style::merge(&[]);
        assert!(library
            .iter()
            .any(|s| s.id == RESPONSE_STYLE.default_text()));
    }

    /// The description quotes the bounds the write gate enforces. A number that
    /// drifts here is the agent being told a limit that is not the limit, and
    /// finding out by having its write refused.
    #[test]
    fn the_style_library_description_states_the_real_bounds() {
        use crate::core::response_style;

        let spec = &RESPONSE_STYLES.spec;
        assert_eq!(spec.scope, PrefScope::Global);
        for bound in [
            response_style::MAX_ID_CHARS,
            response_style::MAX_LABEL_CHARS,
            response_style::MAX_INSTRUCTION_CHARS,
            response_style::MAX_STYLES,
        ] {
            assert!(
                spec.description.contains(&bound.to_string()),
                "the description omits the bound {bound}"
            );
        }
        assert!(
            spec.description.contains(response_style::STANDARD_ID),
            "the description must say the off switch is refused here"
        );
    }

    #[test]
    fn theme_mode_is_device_scoped_and_language_is_global() {
        assert_eq!(THEME_MODE.spec.scope, PrefScope::Device);
        assert_eq!(LANGUAGE.spec.scope, PrefScope::Global);
    }

    /// Motion is a comfort setting for one screen, so it must not reach another
    /// device.
    #[test]
    fn motion_is_a_device_scoped_three_way_choice() {
        let spec = &MOTION.spec;
        assert_eq!(spec.scope, PrefScope::Device);
        for value in ["system", "reduce", "full"] {
            assert!(validate(spec, value).is_ok(), "{value} must be accepted");
        }
        assert!(validate(spec, "off").is_err());
        assert!(validate(spec, "true").is_err());
    }

    /// Theme effects are a comfort and battery setting for one screen, like
    /// motion (ADR 0307).
    #[test]
    fn theme_effects_is_a_device_scoped_three_way_choice() {
        let spec = &THEME_EFFECTS.spec;
        assert_eq!(spec.scope, PrefScope::Device);
        for value in ["system", "reduce", "full"] {
            assert!(validate(spec, value).is_ok(), "{value} must be accepted");
        }
        assert!(validate(spec, "off").is_err());
    }

    /// The dead-Send bug belongs to one keyboard, so turning autocorrect off on
    /// the phone must not reach another device.
    #[test]
    fn autocorrect_is_a_device_scoped_switch() {
        let spec = &AUTOCORRECT.spec;
        assert_eq!(spec.scope, PrefScope::Device);
        assert!(validate(spec, "true").is_ok());
        assert!(validate(spec, "false").is_ok());
        assert!(validate(spec, "off").is_err());
    }

    /// Drift guard (`.claude/rules/system-knowhow.md`): every catalogued key is
    /// documented in `system-knowhow/preferences.md`, and every default the doc
    /// states is the catalog's. The doc is prose the agent reads raw, so it
    /// keeps its own copy, pinned here (ADR 0368).
    ///
    /// - A table row's Default column holds the value in backticks, the key it
    ///   inherits in backticks, or a parenthesised note for an unset default.
    /// - A number row's Allowed column states the catalog's bounds.
    /// - A bullet that says "Default `X`" names the catalog's value.
    #[test]
    fn doc_lists_every_key_with_its_catalog_default() {
        let doc = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../system-knowhow/preferences.md"
        ))
        .expect("system-knowhow/preferences.md must exist");
        let stated_default = regex::Regex::new(r"Default `([^`]+)`").unwrap();
        for spec in CATALOG {
            let name = format!("`{}`", spec.key);
            assert!(
                doc.contains(&name),
                "preference '{}' is not documented in system-knowhow/preferences.md",
                spec.key
            );
            let row_start = format!("| {name} |");
            if let Some(row) = doc.lines().find(|line| line.starts_with(&row_start)) {
                if let PrefValue::Number { min, max } = spec.value {
                    let allowed = row.split(" | ").nth(2).unwrap_or_default();
                    assert!(
                        allowed.contains(&format!("{min}–{max}")),
                        "preferences.md states '{allowed}' for {}, but the catalog bounds are {min}–{max}",
                        spec.key
                    );
                }
                let column = row.split(" | ").nth(3).unwrap_or_default();
                let holds = match spec.default {
                    PrefDefault::Value(value) => column.contains(&format!("`{value}`")),
                    PrefDefault::Inherits(other) => column.contains(&format!("`{}`", other.key)),
                    PrefDefault::Unset(_) => column.starts_with('('),
                };
                assert!(
                    holds,
                    "preferences.md states '{}' for {}, but the catalog default is {}",
                    column,
                    spec.key,
                    spec.default_label()
                );
            }
            let bullet_start = format!("- {name}");
            if let Some(at) = doc.find(&bullet_start) {
                let bullet = &doc[at..];
                let bullet = &bullet[..bullet[2..].find("\n- ").map_or(bullet.len(), |n| n + 2)];
                if let Some(stated) = stated_default.captures(bullet) {
                    assert_eq!(
                        Some(&stated[1]),
                        spec.default_value(),
                        "preferences.md states another default for {}",
                        spec.key
                    );
                }
            }
        }
    }

    /// The one-definition guard (ADR 0368): no Rust source outside this
    /// file spells a preference key. A reader names a key only through its
    /// handle, so its default can only come from here.
    ///
    /// A key with a `_` or `-` is flagged anywhere it appears as a whole string
    /// literal. A plain-word key such as `language` is also an ordinary word
    /// and a wire field. It is flagged only on a line that names
    /// `PreferenceStore` or a `key`. No allowlist: a hit is fixed at its
    /// source.
    #[test]
    fn no_preference_key_is_spelled_outside_the_catalog() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let catalog = std::path::Path::new(file!()).file_name().unwrap();
        let literal = regex::Regex::new(r#""([a-z][a-z0-9_\-]*)""#).unwrap();
        let context = regex::Regex::new(r"PreferenceStore|\bkey\b").unwrap();
        let mut scanned = 0;
        let mut offenders = Vec::new();
        let mut stack = vec![crates];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let name = entry.file_name();
                if path.is_dir() {
                    if !matches!(name.to_str(), Some("target" | "node_modules" | "dist")) {
                        stack.push(path);
                    }
                    continue;
                }
                let is_catalog =
                    name == catalog && path.parent().is_some_and(|p| p.ends_with("src/core"));
                if path.extension().is_none_or(|ext| ext != "rs") || is_catalog {
                    continue;
                }
                scanned += 1;
                let source = std::fs::read_to_string(&path).unwrap();
                for (n, line) in source.lines().enumerate() {
                    for capture in literal.captures_iter(line) {
                        let word = &capture[1];
                        let Some(spec) = spec(word) else { continue };
                        let distinctive = spec.key.contains(['_', '-']);
                        if distinctive || context.is_match(line) {
                            offenders.push(format!("{}:{}: {word}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        assert!(scanned > 500, "the walk found only {scanned} Rust files");
        assert!(
            offenders.is_empty(),
            "a preference key is spelled outside the catalog; read it through its \
             `prefs::` handle instead:\n{}",
            offenders.join("\n")
        );
    }

    /// The catalog's bounds are the proxy's own.
    #[test]
    fn the_proxy_timeout_entry_takes_the_proxy_bounds() {
        use crate::api::proxy_timeout::{MAX_SECS, MIN_SECS};
        let spec = &PROXY_TIMEOUT_SECS.spec;
        assert_eq!(spec.scope, PrefScope::Global);
        assert!(validate(spec, &MIN_SECS.to_string()).is_ok());
        assert!(validate(spec, &MAX_SECS.to_string()).is_ok());
        let err = validate(spec, &(MAX_SECS + 1).to_string()).unwrap_err();
        assert!(
            err.contains(PROXY_TIMEOUT_SECS.key()) && err.contains(&MAX_SECS.to_string()),
            "the refusal must name the key and the maximum: {err}"
        );
        assert!(validate(spec, &(MIN_SECS - 1).to_string()).is_err());
        assert!(validate(spec, "soon").is_err());
    }
}

#[cfg(test)]
#[path = "preference_catalog_codegen_tests.rs"]
mod codegen_tests;
