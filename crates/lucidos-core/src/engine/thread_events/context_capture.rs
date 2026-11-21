use serde::{Deserialize, Serialize};

/// `content` is `Option` so the modal can render section *shape* (name + the
/// two sizes) without persisting the body when the `capture_context`
/// preference is off. Old DB rows always have content and deserialize as
/// `Some(_)`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextSection {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Chars this section ADDS to the request, beyond what other sections
    /// already count. Sum it over a capture and you get the budget the
    /// request spent.
    ///
    /// On most sections nothing else counts their bytes, so the delta happens
    /// to equal `content_chars`. `Conversation` is where the two part: every
    /// other section is already concatenated into `messages[0]`, so its delta
    /// is what the tool loop added on top.
    ///
    /// The alias is honest. The stored value always was the delta, and only
    /// the name lied. Two read paths serve `sections` verbatim rather than
    /// through serde, so they rename the stored key themselves: see
    /// `api::threads::events_snapshot::rename_legacy_section_size`.
    #[serde(alias = "char_count")]
    pub budget_delta_chars: usize,
    /// True `chars().count()` of this section's own content, whatever the
    /// persisted `content` shows. A dropped body (`capture_context` off) and a
    /// head-and-tail truncation both leave it untouched.
    ///
    /// `None` means the row predates this field, never a zero-size section.
    #[serde(default)]
    pub content_chars: Option<usize>,
    /// API role this section is sent under. Defaults to "user" for backward
    /// compat with persisted ContextCaptured events from before this change.
    /// Every previous section was actually part of either the system prompt
    /// (System Instructions) or the user message. The system row gets
    /// migrated by name in the viewer fallback.
    #[serde(default = "default_context_role")]
    pub role: ContextRole,
    /// Inner-group label used by the viewer to nest sections within the
    /// user-message role. None for system-role sections, prior-message rows,
    /// and legacy events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// API role bucket a `ContextSection` belongs to. Mirrors the three buckets
/// in the LLM API call: the system prompt, prior messages (verbatim resume
/// tool blocks), and this turn's user message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRole {
    System,
    PriorMessage,
    User,
}

fn default_context_role() -> ContextRole {
    ContextRole::User
}

/// CC can't expose its system prompt body or tool schemas via the
/// stream-json envelope, only their token cost. The frontend uses this
/// discriminant to know whether to expect a section breakdown body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextProducer {
    MainLlm,
    ClaudeCode,
    Codex,
    /// An *auxiliary model call*: one the engine makes for itself rather than
    /// as an agent's turn (a thread title, an image description, a memory
    /// call, an image generation). It has its own producer so a cost
    /// breakdown keyed on `producer` does not file background spend under the
    /// agent that happened to be running. Like the coding-agent producers it
    /// carries no section breakdown body, only the call's cost.
    Auxiliary,
}

impl ContextProducer {
    /// Producer for a `ContextCaptured` emitted from a coding-agent session.
    pub fn from_coding_agent(agent: crate::engine::thread_events::CodingAgent) -> Self {
        match agent {
            crate::engine::thread_events::CodingAgent::ClaudeCode => Self::ClaudeCode,
            crate::engine::thread_events::CodingAgent::Codex => Self::Codex,
        }
    }
}

/// Which model call a `ContextCaptured` records.
///
/// `Turn` is an agent's own round trip and is the only value the two agentic
/// loops emit. Every other variant is an *auxiliary model call*, and always
/// pairs with [`ContextProducer::Auxiliary`]: `AuxCapture` is the single
/// constructor for both fields, so the pair cannot diverge.
///
/// Serialized only when it is not `Turn`, so a turn row keeps the wire shape
/// it had before this field existed. A stored row without the key reads back
/// as `Turn`, which is what the 600k rows written before it rely on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPurpose {
    #[default]
    Turn,
    Title,
    ImageDescribe,
    Memory,
    /// History summarisation, which used to be stamped [`Self::Memory`] along
    /// with fact extraction and query classification. One purpose over three
    /// jobs left a 94,903-char summariser call and a 6,800-char extraction
    /// indistinguishable on the wire. Diagnosing the summariser then meant
    /// inferring it from payload size.
    ///
    /// Standing invariant: **one purpose per auxiliary model preference**. This
    /// one owns `model_conversation_summary`, and `Memory` is fact extraction
    /// alone now that [`Self::QueryClassification`] has its own key too.
    ConversationSummary,
    /// The three yes/no questions in front of memory retrieval.
    ///
    /// It owns `model_query_classification`, which follows no other key. Split
    /// out for the reason the summariser was, plus one of its own. The user can point it at a different BACKEND
    /// (`judgment_query_classification`, ADR 0220), and a backend is chosen in
    /// the same control the model is.
    QueryClassification,
    ImageGen,
    /// One reply from a voice session's rented *talker* (ADR 0149).
    ///
    /// It owns `model_voice_talker`, per the standing invariant above. Unlike
    /// every sibling here it is not one HTTP call: the talker speaks over a
    /// live socket, so a row is written per reply and the purpose's
    /// `AuxBudget` is never asked for.
    Voice,
    /// The *command guard*'s judge over the ambiguous middle (ADR 0002).
    ///
    /// It owns `model_command_judge` and `reasoning_command_judge`, per the
    /// standing invariant above. Both backends stamp this one purpose. The user
    /// can point the judge at Jev with `judgment_command_guard`, and a switch of
    /// backend must not read as a switch of job.
    CommandJudge,
    /// One call of the agent's own `judge` tool.
    ///
    /// It owns no model preference, because the tool is Jev or nothing. See
    /// `aux_purpose`, which names that case rather than exempting it.
    JudgeTool,
    /// Retired: one round of the removed `execute_intent` sub-loop.
    ///
    /// Nothing produces it now. It stays so old `ContextCaptured` rows still
    /// read, and so a cost filter on `purpose: "intent_loop"` still matches
    /// them.
    IntentLoop,
    /// The verdict behind `correct_memory`, which asks which stored entries
    /// carry the fact the user rejected. Runs the agent's own chat model.
    MemoryCorrection,
    /// The summary written for one file the `import_file` tool imported. Runs
    /// the agent's own chat model, once per imported file.
    ArtifactSummary,
    /// One *side question*'s model call, on either agent: a Lucidos Agent round
    /// on its own chat model, or a Claude Code session copy's summed usage.
    /// It runs beside the thread and is never a turn.
    SideQuestion,
    /// The *change summary*: one line saying what a change of several commits
    /// does. It owns `model_change_summary`, falling back to the title model.
    ChangeSummary,
    /// One *compactor* call: a *summary tree* node written from its source
    /// lines (ADR 0362). It owns `model_summary_compaction`, whose unset
    /// default is resolved against the configured providers.
    SummaryCompaction,
    /// One batch of the recall tool's `find`: a judgment provider judging
    /// summary lines against a query (ADR 0362). It owns `model_memory_find`,
    /// which follows no other key.
    MemoryFind,
    /// One `web_search` call, on whichever backend answered. Each backend
    /// fixes its own search model, so it owns no preference.
    WebSearch,
    /// One model call an app or script made through the credentialed proxy.
    /// The caller names the model in its own request, so it owns no
    /// preference.
    Proxy,
    /// The turn-end gate's forced *read decision*, for a turn that ended
    /// without one (ADR 0417). It owns `model_read_decision`, which follows
    /// no other key.
    ReadDecision,
    /// The turn-end gate's forced *wake decision*, for a child turn that ended
    /// without one. It owns `model_wake_decision`, which follows no other key.
    /// The user can point it at a System One model with
    /// `judgment_wake_decision`.
    WakeDecision,
    /// The turn-end *expected wake decision*, for a turn that left its thread
    /// waiting and stated no expected wake (ADR 0603). It owns
    /// `model_expected_wake_decision`, which follows no other key. The user can
    /// point it at a System One model with `judgment_expected_wake_decision`.
    ExpectedWakeDecision,
    /// The judgment that reads the owner's spoken words against an
    /// open *permission request* (ADR 0664). It owns `model_permission_answer`,
    /// which follows no other key. The user can point it at a System One model
    /// with `judgment_permission_answer`.
    PermissionAnswer,
    /// One *copy beat* of the cache keep-alive: a Claude Code session copy
    /// asked for one word while its session is parked on a card. It runs the
    /// session's own model. Its `duration_ms` dates the request that refreshed
    /// the cache.
    CacheKeepAlive,
    /// Retired: one call of the removed prefill byte-order probe, a synthetic
    /// turn or beat on the Claude gateway (ADR 0581).
    ///
    /// Nothing produces it now. It stays so old `ContextCaptured` rows still
    /// read, and so a cost filter on `purpose: "prefill_probe"` still matches
    /// them.
    PrefillProbe,
}

/// SQL predicate over an `events` row: true for an *auxiliary capture*, a
/// `ContextCaptured` whose payload names a purpose other than `turn`. A turn
/// writes no key ([`ContextPurpose::is_turn`]). The store's page tests pin it
/// against the serializer for every purpose. The transcript's twin is
/// `isAuxiliaryCapture` in the app's `store/thread-events/exchange-grouping.ts`.
pub(crate) const AUXILIARY_CAPTURE_SQL: &str = "(event_type = 'ContextCaptured' \
     AND payload->>'purpose' IS NOT NULL AND payload->>'purpose' <> 'turn')";

impl ContextPurpose {
    /// Skip-serializing predicate: a turn capture writes no `purpose` key.
    pub(crate) fn is_turn(&self) -> bool {
        matches!(self, Self::Turn)
    }

    /// How the event payload spells `purpose`, for SQL to match on.
    pub fn wire(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// Section name for the one body-less section an auxiliary capture
    /// carries. Names the request so a reader of the raw payload can tell
    /// which call the estimate belongs to.
    pub fn section_name(&self) -> &'static str {
        match self {
            Self::Turn => "Request",
            Self::Title => "Title Request",
            Self::ImageDescribe => "Image Description Request",
            Self::Memory => "Memory Request",
            Self::ConversationSummary => "Conversation Summary Request",
            Self::QueryClassification => "Query Classification Request",
            Self::ImageGen => "Image Generation Request",
            Self::Voice => "Voice Reply",
            Self::CommandJudge => "Command Judge Request",
            Self::JudgeTool => "Judge Tool Request",
            Self::IntentLoop => "Intent Loop Request",
            Self::MemoryCorrection => "Memory Correction Request",
            Self::ArtifactSummary => "Artifact Summary Request",
            Self::SideQuestion => "Side Question Request",
            Self::ChangeSummary => "Change Summary Request",
            Self::SummaryCompaction => "Summary Compaction Request",
            Self::MemoryFind => "Memory Find Request",
            Self::WebSearch => "Web Search Request",
            Self::Proxy => "Proxied Request",
            Self::ReadDecision => "Read Decision Request",
            Self::WakeDecision => "Wake Decision Request",
            Self::ExpectedWakeDecision => "Expected Wake Decision Request",
            Self::PermissionAnswer => "Permission Answer Request",
            Self::CacheKeepAlive => "Cache Keep-Alive Request",
            Self::PrefillProbe => "Prefill Probe Request",
        }
    }
}

/// Every purpose the enum has, so a new variant fails the tests below until it
/// declares what it reads and how long it may take.
#[cfg(any(test, feature = "test-support"))]
pub const ALL_PURPOSES: &[ContextPurpose] = &[
    ContextPurpose::Turn,
    ContextPurpose::Title,
    ContextPurpose::ImageDescribe,
    ContextPurpose::Memory,
    ContextPurpose::ConversationSummary,
    ContextPurpose::QueryClassification,
    ContextPurpose::ImageGen,
    ContextPurpose::Voice,
    ContextPurpose::CommandJudge,
    ContextPurpose::JudgeTool,
    ContextPurpose::IntentLoop,
    ContextPurpose::MemoryCorrection,
    ContextPurpose::ArtifactSummary,
    ContextPurpose::SideQuestion,
    ContextPurpose::ChangeSummary,
    ContextPurpose::SummaryCompaction,
    ContextPurpose::MemoryFind,
    ContextPurpose::WebSearch,
    ContextPurpose::Proxy,
    ContextPurpose::ReadDecision,
    ContextPurpose::WakeDecision,
    ContextPurpose::ExpectedWakeDecision,
    ContextPurpose::PermissionAnswer,
    ContextPurpose::CacheKeepAlive,
    ContextPurpose::PrefillProbe,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows a retired purpose wrote still read, and still write back the same
    /// purpose a cost filter matches on. The literals pin the stored wire form.
    #[test]
    fn a_retired_purpose_capture_still_round_trips() {
        for (wire, retired) in [
            ("intent_loop", ContextPurpose::IntentLoop),
            ("prefill_probe", ContextPurpose::PrefillProbe),
        ] {
            let purpose: ContextPurpose = serde_json::from_value(serde_json::json!(wire))
                .unwrap_or_else(|e| panic!("an old {wire} row reads: {e}"));
            assert_eq!(purpose, retired);
            assert_eq!(
                serde_json::to_value(purpose).unwrap(),
                serde_json::json!(wire)
            );
        }
    }

    #[test]
    fn context_section_legacy_payload_deserializes_with_default_role_and_no_group() {
        // Pre-change ContextCaptured events have no `role` or `group` fields.
        // They must still deserialize cleanly: `role` defaults to `User`, and
        // `group` stays `None`.
        let json = r#"{"name":"X","char_count":42}"#;
        let section: ContextSection =
            serde_json::from_str(json).expect("legacy payload must deserialize");
        assert_eq!(section.name, "X");
        assert_eq!(section.budget_delta_chars, 42);
        assert!(section.content.is_none());
        assert_eq!(section.role, ContextRole::User);
        assert!(section.group.is_none());
    }

    /// Months of stored rows spell the delta `char_count`. They must land in
    /// the field that holds a delta, and they must not claim a content size
    /// nobody measured.
    #[test]
    fn a_stored_char_count_is_the_budget_delta_and_leaves_the_content_size_unknown() {
        let json = r#"{"name":"Conversation","char_count":418291}"#;
        let section: ContextSection =
            serde_json::from_str(json).expect("legacy payload must deserialize");
        assert_eq!(section.budget_delta_chars, 418_291);
        assert_eq!(
            section.content_chars, None,
            "absent means unmeasured, not zero"
        );
    }

    /// The rename has no shim, so nothing writes the old key. A reader summing
    /// `char_count` off a fresh row must find nothing there.
    #[test]
    fn a_written_section_spells_both_sizes_and_never_the_old_name() {
        let section = ContextSection {
            name: "System Instructions".to_string(),
            content: None,
            budget_delta_chars: 90,
            content_chars: Some(90),
            role: ContextRole::System,
            group: None,
        };
        let json = serde_json::to_value(&section).expect("serializes");
        assert_eq!(json["budget_delta_chars"], 90);
        assert_eq!(json["content_chars"], 90);
        assert!(json.get("char_count").is_none());
    }
}
