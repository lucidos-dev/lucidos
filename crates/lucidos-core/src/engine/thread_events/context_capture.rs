use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextSection {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// INVARIANT: Chars this section ADDS to the request, beyond what other sections
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
    /// INVARIANT: True `chars().count()` of this section's own content, whatever the
    /// persisted `content` shows. A dropped body (`capture_context` off) and a
    /// head-and-tail truncation both leave it untouched.
    ///
    /// `None` means the row predates this field, never a zero-size section.
    #[serde(default)]
    pub content_chars: Option<usize>,
    /// INVARIANT: API role this section is sent under. Defaults to "user" for backward
    /// compat with persisted ContextCaptured events from before this change.
    /// Every previous section was actually part of either the system prompt
    /// (System Instructions) or the user message. The system row gets
    /// migrated by name in the viewer fallback.
    #[serde(default = "default_context_role")]
    pub role: ContextRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextProducer {
    MainLlm,
    ClaudeCode,
    Codex,
    Auxiliary,
}

impl ContextProducer {
    pub fn from_coding_agent(agent: crate::engine::thread_events::CodingAgent) -> Self {
        match agent {
            crate::engine::thread_events::CodingAgent::ClaudeCode => Self::ClaudeCode,
            crate::engine::thread_events::CodingAgent::Codex => Self::Codex,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPurpose {
    #[default]
    Turn,
    Title,
    ImageDescribe,
    Memory,
    ConversationSummary,
    QueryClassification,
    ImageGen,
    Voice,
    CommandJudge,
    JudgeTool,
    /// INVARIANT: Retired: one round of the removed `execute_intent` sub-loop.
    ///
    /// Nothing produces it now. It stays so old `ContextCaptured` rows still
    /// read, and so a cost filter on `purpose: "intent_loop"` still matches
    /// them.
    IntentLoop,
    MemoryCorrection,
    ArtifactSummary,
    SideQuestion,
    ChangeSummary,
    SummaryCompaction,
    MemoryFind,
    WebSearch,
    Proxy,
    ReadDecision,
    WakeDecision,
    ExpectedWakeDecision,
    PermissionAnswer,
    CacheKeepAlive,
    PrefillProbe,
}

/// INVARIANT: SQL predicate over an `events` row: true for an *auxiliary capture*, a
/// `ContextCaptured` whose payload names a purpose other than `turn`. A turn
/// writes no key ([`ContextPurpose::is_turn`]). The store's page tests pin it
/// against the serializer for every purpose. The transcript's twin is
/// `isAuxiliaryCapture` in the app's `store/thread-events/exchange-grouping.ts`.
pub(crate) const AUXILIARY_CAPTURE_SQL: &str = "(event_type = 'ContextCaptured' \
     AND payload->>'purpose' IS NOT NULL AND payload->>'purpose' <> 'turn')";

impl ContextPurpose {
    pub(crate) fn is_turn(&self) -> bool {
        matches!(self, Self::Turn)
    }

    pub fn wire(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    }

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
        let json = r#"{"name":"X","char_count":42}"#;
        let section: ContextSection =
            serde_json::from_str(json).expect("legacy payload must deserialize");
        assert_eq!(section.name, "X");
        assert_eq!(section.budget_delta_chars, 42);
        assert!(section.content.is_none());
        assert_eq!(section.role, ContextRole::User);
        assert!(section.group.is_none());
    }

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
