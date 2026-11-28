mod default;
mod judge_model;
mod not_served_report;
mod reach;

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use crate::core::prefs::{self, Optional, Pref, Text};
use crate::engine::ContextPurpose;
use crate::llm::model_registry::{default_effort, Generation, ModelRegistry, ProviderKind};
use crate::llm::not_served::NotServedLedger;
use crate::llm::LlmProvider;

use crate::engine::event_bus::EventBus;
use crate::llm::AuxProvider;
pub(crate) use default::select as select_model;
pub use default::ModelSource;
pub(crate) use judge_model::unresolved_reason as judge_unresolved;
use not_served_report::NotServedRecorder;
pub(crate) use reach::{seed_not_served, Reach};
pub use {default::asks_no_model, default::recommended, default::recommended_effort};
pub use {judge_model::guarded_turn_refusal, judge_model::unresolved_judge};

pub(crate) struct AuxModelPrefs {
    pub(crate) model: &'static Pref<Text>,
    pub(crate) reasoning: Option<&'static Pref<Text>>,
    pub(crate) needs_vision: bool,
}

pub(crate) struct ProviderResolvedPrefs {
    pub(crate) model: &'static Pref<Optional>,
    pub(crate) reasoning: &'static Pref<Optional>,
}

pub(crate) const SUMMARY_COMPACTION_PREFS: ProviderResolvedPrefs = ProviderResolvedPrefs {
    model: &prefs::MODEL_SUMMARY_COMPACTION,
    reasoning: &prefs::REASONING_SUMMARY_COMPACTION,
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum GenerationStep {
    Successor { model: String },
    Predecessor { model: String },
    Held { successor: String },
}

impl GenerationStep {
    fn of(
        registry: &ModelRegistry,
        reach: &Reach,
        model: &str,
        source: ModelSource,
    ) -> Option<Self> {
        let resolved = reach.resolve(registry, model, source.generation_rule());
        match resolved.generation {
            Generation::Named => None,
            Generation::Successor => Some(Self::Successor {
                model: resolved.model,
            }),
            Generation::Predecessor => Some(Self::Predecessor {
                model: resolved.model,
            }),
            Generation::Held { successor } => Some(Self::Held { successor }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuxSelection {
    pub model: String,
    pub reasoning: Option<String>,
    pub source: ModelSource,
    pub reachable: bool,
    pub generation: Option<GenerationStep>,
    pub(crate) fallback: Option<String>,
    pub not_served: Vec<String>,
    pub needs_vision: bool,
    pub vision: bool,
}

impl AuxSelection {
    pub fn refusal(&self) -> Option<String> {
        if !self.needs_vision || self.vision {
            return None;
        }
        Some(match self.source {
            ModelSource::Preference => format!(
                "the chosen model '{}' cannot read images. Pick one that can under \
                 Settings → Models → Background tasks, or mark it as reading images \
                 under Manage models",
                self.model
            ),
            ModelSource::Default | ModelSource::ChatModel => format!(
                "no recommended model that reads images is reachable, and the chat \
                 model '{}' cannot read them. Pick one that can under Settings → \
                 Models → Background tasks, or set up a provider for a recommended one",
                self.model
            ),
        })
    }
}

pub(crate) enum AuxModelSource {
    Turn,
    /// INVARIANT: The backend fixes the model, so there is nothing to choose. The `judge`
    /// tool is Jev or nothing: unlike the judgment sites it has no chat model
    /// to fall back to. Each web search backend sends its own search model.
    BackendPinned,
    AgentModel,
    Preferences(AuxModelPrefs),
    ProviderResolved,
    CallerChosen,
}

#[cfg(any(test, feature = "test-support"))]
impl AuxModelSource {
    pub(crate) fn prefs(self) -> Option<AuxModelPrefs> {
        match self {
            Self::Preferences(pair) => Some(pair),
            _ => None,
        }
    }
}

pub(crate) fn needs_vision(purpose: ContextPurpose) -> bool {
    match model_source(purpose) {
        AuxModelSource::Preferences(pair) => pair.needs_vision,
        AuxModelSource::Turn
        | AuxModelSource::BackendPinned
        | AuxModelSource::AgentModel
        | AuxModelSource::ProviderResolved
        | AuxModelSource::CallerChosen => false,
    }
}

pub(crate) fn model_source(purpose: ContextPurpose) -> AuxModelSource {
    let pair = match purpose {
        ContextPurpose::Turn => return AuxModelSource::Turn,
        ContextPurpose::JudgeTool | ContextPurpose::WebSearch => {
            return AuxModelSource::BackendPinned
        }
        ContextPurpose::Proxy | ContextPurpose::PrefillProbe => {
            return AuxModelSource::CallerChosen
        }
        ContextPurpose::IntentLoop
        | ContextPurpose::MemoryCorrection
        | ContextPurpose::ArtifactSummary
        | ContextPurpose::SideQuestion
        | ContextPurpose::CacheKeepAlive => return AuxModelSource::AgentModel,
        ContextPurpose::Title => AuxModelPrefs {
            model: &prefs::MODEL_TITLE,
            reasoning: Some(&prefs::REASONING_TITLE),
            needs_vision: false,
        },
        ContextPurpose::ChangeSummary => AuxModelPrefs {
            model: &prefs::MODEL_CHANGE_SUMMARY,
            reasoning: Some(&prefs::REASONING_CHANGE_SUMMARY),
            needs_vision: false,
        },
        ContextPurpose::ImageDescribe => AuxModelPrefs {
            model: &prefs::MODEL_IMAGE_DESCRIPTION,
            reasoning: Some(&prefs::REASONING_IMAGE_DESCRIPTION),
            needs_vision: true,
        },
        ContextPurpose::Memory => AuxModelPrefs {
            model: &prefs::MODEL_MEMORY,
            reasoning: Some(&prefs::REASONING_MEMORY),
            needs_vision: false,
        },
        ContextPurpose::ConversationSummary => AuxModelPrefs {
            model: &prefs::MODEL_CONVERSATION_SUMMARY,
            reasoning: Some(&prefs::REASONING_CONVERSATION_SUMMARY),
            needs_vision: false,
        },
        ContextPurpose::SummaryCompaction => return AuxModelSource::ProviderResolved,
        ContextPurpose::MemoryFind => AuxModelPrefs {
            model: &prefs::MODEL_MEMORY_FIND,
            reasoning: Some(&prefs::REASONING_MEMORY_FIND),
            needs_vision: false,
        },
        ContextPurpose::ReadDecision => AuxModelPrefs {
            model: &prefs::MODEL_READ_DECISION,
            reasoning: Some(&prefs::REASONING_READ_DECISION),
            needs_vision: false,
        },
        ContextPurpose::WakeDecision => AuxModelPrefs {
            model: &prefs::MODEL_WAKE_DECISION,
            reasoning: Some(&prefs::REASONING_WAKE_DECISION),
            needs_vision: false,
        },
        ContextPurpose::ExpectedWakeDecision => AuxModelPrefs {
            model: &prefs::MODEL_EXPECTED_WAKE_DECISION,
            reasoning: Some(&prefs::REASONING_EXPECTED_WAKE_DECISION),
            needs_vision: false,
        },
        ContextPurpose::PermissionAnswer => AuxModelPrefs {
            model: &prefs::MODEL_PERMISSION_ANSWER,
            reasoning: Some(&prefs::REASONING_PERMISSION_ANSWER),
            needs_vision: false,
        },
        ContextPurpose::QueryClassification => AuxModelPrefs {
            model: &prefs::MODEL_QUERY_CLASSIFICATION,
            reasoning: Some(&prefs::REASONING_QUERY_CLASSIFICATION),
            needs_vision: false,
        },
        ContextPurpose::ImageGen => AuxModelPrefs {
            model: &prefs::IMAGE_MODEL,
            reasoning: None,
            needs_vision: false,
        },
        ContextPurpose::Voice => AuxModelPrefs {
            model: &prefs::MODEL_VOICE_TALKER,
            reasoning: None,
            needs_vision: false,
        },
        ContextPurpose::CommandJudge => AuxModelPrefs {
            model: &prefs::MODEL_COMMAND_JUDGE,
            reasoning: Some(&prefs::REASONING_COMMAND_JUDGE),
            needs_vision: false,
        },
    };
    AuxModelSource::Preferences(pair)
}

/// INVARIANT: Read `purpose`'s *model selection* and resolve it against the providers
/// `router` holds.
///
/// Total by construction. A missing row and a database error both resolve to
/// the default. A background call that refuses to run over a preference read
/// is strictly worse than one running at its default.
///
/// Read per call, never cached, so a credential added at runtime moves the
/// next call. A purpose reading no preference runs on the chat model.
pub(crate) async fn resolve_selection(
    pool: &PgPool,
    registry: &ModelRegistry,
    router: &dyn LlmProvider,
    purpose: ContextPurpose,
) -> AuxSelection {
    let reach = Reach::of(pool, router).await;
    let recommended = recommended(purpose, registry);
    let chat_model = prefs::CHAT_MODEL
        .stored(pool)
        .await
        .unwrap_or_else(|| router.default_model().to_string());
    let (model, reasoning, source) = match model_source(purpose) {
        AuxModelSource::ProviderResolved => {
            let compactor = crate::engine::summary_tree::compactor_selection(
                pool,
                registry,
                &reach,
                &chat_model,
            )
            .await;
            (compactor.model, compactor.effort, compactor.source)
        }
        AuxModelSource::Preferences(pair) => {
            let stored = pair
                .model
                .stored_or_inherited(pool)
                .await
                .filter(|model| model != "default");
            let (model, source) =
                default::select(stored, &recommended, &chat_model, registry, &reach);
            let reasoning = match pair.reasoning {
                Some(pref) => {
                    let stored = pref
                        .stored_or_inherited(pool)
                        .await
                        .filter(|tier| crate::llm::reasoning::tier(tier).is_some());
                    stored.or_else(|| task_effort(pref, &model, &recommended, registry))
                }
                None => None,
            };
            (model, reasoning, source)
        }
        AuxModelSource::Turn
        | AuxModelSource::BackendPinned
        | AuxModelSource::AgentModel
        | AuxModelSource::CallerChosen => (chat_model, None, ModelSource::ChatModel),
    };
    // INVARIANT: An engine on `LUCIDOS_MODEL=mock`, the e2e opt-in, runs these calls on
    // its chat model, the mock. A test then spends nothing, and the record
    // says so.
    if router.default_model() == crate::llm::MOCK_MODEL {
        return AuxSelection {
            model: crate::llm::MOCK_MODEL.to_string(),
            reasoning,
            source: ModelSource::ChatModel,
            reachable: true,
            generation: None,
            fallback: None,
            not_served: Vec::new(),
            needs_vision: needs_vision(purpose),
            vision: true,
        };
    }
    let fallback = match (source, model_source(purpose)) {
        (ModelSource::Default, AuxModelSource::Preferences(_)) => {
            default::next_reachable(&model, &recommended, registry, &reach)
        }
        _ => None,
    };
    AuxSelection {
        reachable: reach.serves(registry, &model, source.generation_rule()),
        generation: GenerationStep::of(registry, &reach, &model, source),
        not_served: default::passed_over(&model, source, &recommended, registry, &reach),
        fallback,
        needs_vision: needs_vision(purpose),
        vision: crate::llm::model_registry::reads_images(registry, &model),
        model,
        reasoning,
        source,
    }
}

fn task_effort(
    pref: &Pref<Text>,
    model: &str,
    recommended: &[String],
    registry: &ModelRegistry,
) -> Option<String> {
    if recommended.iter().any(|m| m == model) {
        Some(pref.default_text().to_string())
    } else {
        default_effort(registry, model).map(str::to_string)
    }
}

pub const BACKGROUND_ROWS: &[ContextPurpose] = &[
    ContextPurpose::Title,
    ContextPurpose::ChangeSummary,
    ContextPurpose::ImageDescribe,
    ContextPurpose::Memory,
    ContextPurpose::QueryClassification,
    ContextPurpose::ConversationSummary,
    ContextPurpose::SummaryCompaction,
    ContextPurpose::MemoryFind,
    ContextPurpose::CommandJudge,
    ContextPurpose::ReadDecision,
    ContextPurpose::WakeDecision,
    ContextPurpose::ExpectedWakeDecision,
    ContextPurpose::PermissionAnswer,
];

pub fn model_key(purpose: ContextPurpose) -> Option<&'static str> {
    match model_source(purpose) {
        AuxModelSource::Preferences(pair) => Some(pair.model.key()),
        AuxModelSource::ProviderResolved => Some(SUMMARY_COMPACTION_PREFS.model.key()),
        AuxModelSource::Turn
        | AuxModelSource::BackendPinned
        | AuxModelSource::AgentModel
        | AuxModelSource::CallerChosen => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuxBudget {
    pub deadline: Duration,
    pub attempt_timeout: Duration,
}

const SUMMARY_BUDGET: AuxBudget = AuxBudget {
    deadline: Duration::from_secs(90),
    attempt_timeout: Duration::from_secs(45),
};

const SHORT_CALL_BUDGET: AuxBudget = AuxBudget {
    deadline: Duration::from_secs(30),
    attempt_timeout: Duration::from_secs(20),
};

const JUDGE_TOOL_BUDGET: AuxBudget = AuxBudget {
    deadline: Duration::from_secs(75),
    attempt_timeout: Duration::from_secs(60),
};

const COMPACTION_BUDGET: AuxBudget = AuxBudget {
    deadline: Duration::from_secs(85),
    attempt_timeout: Duration::from_secs(45),
};

const MEMORY_FIND_BUDGET: AuxBudget = AuxBudget {
    deadline: Duration::from_secs(60),
    attempt_timeout: Duration::from_secs(45),
};

pub fn budget_for(purpose: ContextPurpose) -> AuxBudget {
    match purpose {
        ContextPurpose::ConversationSummary => SUMMARY_BUDGET,
        ContextPurpose::JudgeTool => JUDGE_TOOL_BUDGET,
        ContextPurpose::SummaryCompaction => COMPACTION_BUDGET,
        ContextPurpose::MemoryFind => MEMORY_FIND_BUDGET,
        ContextPurpose::Title
        | ContextPurpose::ChangeSummary
        | ContextPurpose::ImageDescribe
        | ContextPurpose::Memory
        | ContextPurpose::QueryClassification
        | ContextPurpose::ImageGen
        | ContextPurpose::CommandJudge
        | ContextPurpose::ReadDecision
        | ContextPurpose::WakeDecision
        | ContextPurpose::ExpectedWakeDecision
        | ContextPurpose::PermissionAnswer => SHORT_CALL_BUDGET,
        ContextPurpose::Turn
        | ContextPurpose::Voice
        | ContextPurpose::IntentLoop
        | ContextPurpose::MemoryCorrection
        | ContextPurpose::ArtifactSummary
        | ContextPurpose::SideQuestion
        | ContextPurpose::CacheKeepAlive
        | ContextPurpose::WebSearch
        | ContextPurpose::Proxy
        | ContextPurpose::PrefillProbe => SHORT_CALL_BUDGET,
    }
}

pub struct AuxCall {
    selection: AuxSelection,
    budget: AuxBudget,
    router: Arc<dyn LlmProvider>,
    report: Option<Arc<NotServedRecorder>>,
}

impl AuxCall {
    pub(crate) async fn resolve(
        pool: &PgPool,
        registry: &ModelRegistry,
        router: Arc<dyn LlmProvider>,
        purpose: ContextPurpose,
    ) -> Self {
        Self {
            selection: resolve_selection(pool, registry, router.as_ref(), purpose).await,
            budget: budget_for(purpose),
            router,
            report: None,
        }
    }

    pub(crate) fn reporting_to(
        mut self,
        bus: &EventBus,
        pool: &PgPool,
        registry: &ModelRegistry,
        purpose: ContextPurpose,
    ) -> Self {
        let configured = self
            .router
            .configured_providers()
            .unwrap_or_else(|| ProviderKind::ALL.to_vec());
        self.report = Some(Arc::new(NotServedRecorder {
            bus: bus.clone(),
            pool: pool.clone(),
            registry: registry.clone(),
            reach: Reach::new(configured, NotServedLedger::default()),
            purpose,
            source: self.selection.source,
        }));
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn over(provider: Arc<dyn LlmProvider>, purpose: ContextPurpose) -> Self {
        let reasoning = model_source(purpose)
            .prefs()
            .and_then(|p| p.reasoning)
            .map(|r| r.default_text().to_string());
        Self {
            selection: AuxSelection {
                model: provider.default_model().to_string(),
                reasoning,
                source: ModelSource::Default,
                reachable: true,
                generation: None,
                fallback: None,
                not_served: Vec::new(),
                needs_vision: needs_vision(purpose),
                vision: true,
            },
            budget: budget_for(purpose),
            router: provider,
            report: None,
        }
    }

    pub fn provider(&self) -> Arc<dyn LlmProvider> {
        let provider = AuxProvider::new(
            self.router.clone(),
            self.selection.model.clone(),
            self.budget.attempt_timeout,
        )
        .with_generation_rule(self.selection.source.generation_rule());
        Arc::new(match &self.report {
            Some(report) => provider.reporting(report.clone(), self.selection.fallback.clone()),
            None => provider,
        })
    }

    pub fn selection(&self) -> &AuxSelection {
        &self.selection
    }

    pub(crate) fn running_model(&self) -> &str {
        match &self.selection.generation {
            Some(GenerationStep::Successor { model } | GenerationStep::Predecessor { model }) => {
                model
            }
            Some(GenerationStep::Held { .. }) | None => &self.selection.model,
        }
    }

    pub fn reasoning(&self) -> Option<&str> {
        self.selection.reasoning.as_deref()
    }

    pub fn attempt_timeout(&self) -> Duration {
        self.budget.attempt_timeout
    }

    pub fn deadline(&self) -> Duration {
        self.budget.deadline
    }
}

#[cfg(test)]
pub(crate) use crate::engine::thread_events::ALL_PURPOSES;

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod resolve_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_purpose_list_covers_the_enum() {
        for purpose in ALL_PURPOSES {
            match purpose {
                ContextPurpose::Turn
                | ContextPurpose::Title
                | ContextPurpose::ImageDescribe
                | ContextPurpose::Memory
                | ContextPurpose::ConversationSummary
                | ContextPurpose::QueryClassification
                | ContextPurpose::ImageGen
                | ContextPurpose::Voice
                | ContextPurpose::CommandJudge
                | ContextPurpose::JudgeTool
                | ContextPurpose::IntentLoop
                | ContextPurpose::MemoryCorrection
                | ContextPurpose::ArtifactSummary
                | ContextPurpose::SideQuestion
                | ContextPurpose::ChangeSummary
                | ContextPurpose::SummaryCompaction
                | ContextPurpose::MemoryFind
                | ContextPurpose::WebSearch
                | ContextPurpose::Proxy
                | ContextPurpose::ReadDecision
                | ContextPurpose::WakeDecision
                | ContextPurpose::ExpectedWakeDecision
                | ContextPurpose::PermissionAnswer
                | ContextPurpose::CacheKeepAlive
                | ContextPurpose::PrefillProbe => {}
            }
        }
        assert_eq!(ALL_PURPOSES.len(), 25);
    }

    fn inherited_key(handle: &Pref<Text>) -> Option<&'static str> {
        match handle.spec.default {
            prefs::PrefDefault::Inherits(other) => Some(other.key),
            _ => None,
        }
    }

    #[test]
    fn every_purpose_owns_exactly_one_model_preference() {
        let mut seen: Vec<&'static str> = vec![];
        for purpose in ALL_PURPOSES {
            match model_source(*purpose) {
                AuxModelSource::Turn => assert_eq!(
                    *purpose,
                    ContextPurpose::Turn,
                    "a turn is the only thing that is not an auxiliary call"
                ),
                AuxModelSource::BackendPinned => assert!(
                    matches!(
                        purpose,
                        ContextPurpose::JudgeTool | ContextPurpose::WebSearch
                    ),
                    "{purpose:?} claims its backend fixes its model"
                ),
                AuxModelSource::CallerChosen => assert!(
                    matches!(
                        purpose,
                        ContextPurpose::Proxy | ContextPurpose::PrefillProbe
                    ),
                    "{purpose:?} claims its caller names the model"
                ),
                AuxModelSource::AgentModel => assert!(
                    matches!(
                        purpose,
                        ContextPurpose::IntentLoop
                            | ContextPurpose::MemoryCorrection
                            | ContextPurpose::ArtifactSummary
                            | ContextPurpose::SideQuestion
                            | ContextPurpose::CacheKeepAlive
                    ),
                    "{:?} claims to run the agent's own model",
                    purpose
                ),
                AuxModelSource::Preferences(pair) => {
                    assert!(
                        !seen.contains(&pair.model.key()),
                        "{:?} reuses the model preference {}",
                        purpose,
                        pair.model.key()
                    );
                    seen.push(pair.model.key());
                }
                AuxModelSource::ProviderResolved => {
                    assert_eq!(
                        *purpose,
                        ContextPurpose::SummaryCompaction,
                        "the compactor is the only provider-resolved purpose"
                    );
                    let key = SUMMARY_COMPACTION_PREFS.model.key();
                    assert!(!seen.contains(&key));
                    seen.push(key);
                }
            }
        }
    }

    fn reasoning_key(purpose: ContextPurpose) -> Option<&'static str> {
        match model_source(purpose) {
            AuxModelSource::Preferences(pair) => pair.reasoning.map(|r| r.key()),
            AuxModelSource::ProviderResolved => Some(SUMMARY_COMPACTION_PREFS.reasoning.key()),
            _ => None,
        }
    }

    #[test]
    fn only_image_description_needs_vision() {
        for purpose in ALL_PURPOSES {
            assert_eq!(
                needs_vision(*purpose),
                *purpose == ContextPurpose::ImageDescribe,
                "{purpose:?}"
            );
        }
    }

    #[test]
    fn every_reasoning_preference_belongs_to_one_purpose() {
        let mut seen: Vec<&'static str> = vec![];
        for purpose in ALL_PURPOSES {
            let Some(reasoning) = reasoning_key(*purpose) else {
                continue;
            };
            assert!(
                !seen.contains(&reasoning),
                "{:?} reuses the reasoning preference {}",
                purpose,
                reasoning
            );
            seen.push(reasoning);
        }
    }

    #[test]
    fn the_compactor_resolves_its_default_against_the_providers() {
        assert!(matches!(
            model_source(ContextPurpose::SummaryCompaction),
            AuxModelSource::ProviderResolved
        ));
    }

    #[test]
    fn every_reasoning_default_is_a_tier() {
        for purpose in ALL_PURPOSES {
            let Some(reasoning) = model_source(*purpose).prefs().and_then(|p| p.reasoning) else {
                continue;
            };
            assert!(
                crate::llm::EFFORT_LADDER.contains(&reasoning.default_text()),
                "{:?} defaults to {:?}, which is not a tier",
                purpose,
                reasoning.default_text()
            );
        }
    }

    #[test]
    fn image_generation_has_no_reasoning_half() {
        let pair = model_source(ContextPurpose::ImageGen)
            .prefs()
            .expect("image generation reads a model");
        assert!(pair.reasoning.is_none());
    }

    #[test]
    fn no_memory_task_inherits_another_key() {
        for purpose in [
            ContextPurpose::Memory,
            ContextPurpose::QueryClassification,
            ContextPurpose::ConversationSummary,
            ContextPurpose::MemoryFind,
        ] {
            let pair = model_source(purpose).prefs().expect("prefs");
            assert_eq!(inherited_key(pair.model), None, "{purpose:?} model");
            let reasoning = pair.reasoning.expect("it runs at an effort");
            assert_eq!(inherited_key(reasoning), None, "{purpose:?} effort");
        }
    }

    #[test]
    fn the_change_summary_falls_back_to_the_title_pair() {
        let pair = model_source(ContextPurpose::ChangeSummary)
            .prefs()
            .expect("prefs");
        assert_eq!(pair.model.key(), prefs::MODEL_CHANGE_SUMMARY.key());
        assert_eq!(inherited_key(pair.model), Some(prefs::MODEL_TITLE.key()));
        let reasoning = pair.reasoning.expect("it runs at an effort");
        assert_eq!(reasoning.key(), prefs::REASONING_CHANGE_SUMMARY.key());
        assert_eq!(inherited_key(reasoning), Some(prefs::REASONING_TITLE.key()));
    }

    #[test]
    fn fact_extraction_still_owns_the_memory_model() {
        let pair = model_source(ContextPurpose::Memory).prefs().expect("prefs");
        assert_eq!(pair.model.key(), prefs::MODEL_MEMORY.key());
    }

    #[test]
    fn the_judge_tool_owns_no_model_preference() {
        assert!(matches!(
            model_source(ContextPurpose::JudgeTool),
            AuxModelSource::BackendPinned
        ));
    }

    #[test]
    fn the_command_judge_declares_the_pair_it_reads() {
        let pair = model_source(ContextPurpose::CommandJudge)
            .prefs()
            .expect("the command judge reads a pair");
        assert_eq!(pair.model.key(), prefs::MODEL_COMMAND_JUDGE.key());
        assert_eq!(inherited_key(pair.model), None);
        let reasoning = pair.reasoning.expect("it runs at an effort");
        assert_eq!(reasoning.key(), prefs::REASONING_COMMAND_JUDGE.key());
    }

    #[test]
    fn the_judge_tool_keeps_its_generous_attempt_cap() {
        assert_eq!(
            budget_for(ContextPurpose::JudgeTool).attempt_timeout,
            Duration::from_secs(60)
        );
    }

    #[test]
    fn every_inherited_effort_names_an_effort_key() {
        let efforts: Vec<&'static str> = ALL_PURPOSES
            .iter()
            .filter_map(|p| {
                model_source(*p)
                    .prefs()
                    .and_then(|m| m.reasoning)
                    .map(|r| r.key())
            })
            .collect();
        for purpose in ALL_PURPOSES {
            let Some(reasoning) = model_source(*purpose).prefs().and_then(|p| p.reasoning) else {
                continue;
            };
            let Some(inherited) = inherited_key(reasoning) else {
                continue;
            };
            assert!(
                efforts.contains(&inherited),
                "{:?} inherits {}, which is not an effort key",
                purpose,
                inherited
            );
        }
    }

    #[test]
    fn a_deadline_holds_one_full_attempt_and_the_whole_backoff() {
        let backoff: Duration = (1..=crate::llm::MAX_RETRIES)
            .map(|attempt| crate::llm::retry_delay(attempt, 1))
            .sum();
        for purpose in ALL_PURPOSES {
            let budget = budget_for(*purpose);
            let needed = budget.attempt_timeout + backoff;
            assert!(
                needed <= budget.deadline,
                "{:?} needs {:?} for one attempt plus {:?} of backoff, but its deadline is {:?}",
                purpose,
                needed,
                backoff,
                budget.deadline
            );
        }
    }

    #[test]
    fn an_attempt_can_never_consume_the_whole_deadline() {
        for purpose in ALL_PURPOSES {
            let budget = budget_for(*purpose);
            assert!(
                budget.attempt_timeout < budget.deadline,
                "{:?} caps one attempt at {:?}, its whole deadline",
                purpose,
                budget.attempt_timeout
            );
        }
    }

    #[test]
    fn the_summary_gets_the_longer_budget() {
        let summary = budget_for(ContextPurpose::ConversationSummary);
        for purpose in ALL_PURPOSES {
            if *purpose == ContextPurpose::ConversationSummary {
                continue;
            }
            assert!(budget_for(*purpose).deadline < summary.deadline);
        }
    }
}
