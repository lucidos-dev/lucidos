//! What an *auxiliary model call* resolves from its [`ContextPurpose`]: the
//! *model selection* it runs under, and the wall-clock budget it runs inside.
//! Seven auxiliary purposes read no preference pair, and [`AuxModelSource`]
//! says why.
//!
//! **One purpose per auxiliary model preference.** The standing invariant this
//! module exists to hold, enforced by
//! [`every_purpose_owns_exactly_one_model_preference`]. Before the split,
//! `Memory` stamped fact extraction, query classification and history
//! summarisation alike. So the wire could not tell a 94,903-char summariser
//! call from a 6,800-char extraction, and Settings named no summariser.
//!
//! **A caller that resamples bounds its whole loop, not each call.** An
//! [`AuxBudget::deadline`] covers one call, so N attempts cost N deadlines
//! unless the loop is wrapped. Title generation resamples twice and fact
//! extraction three times, and both escaped a per-call bound until they were.
//!
//! **A purpose that reads no preference says WHICH kind it is.**
//! [`AuxModelSource`] carries an arm per reason, and the invariant test asserts
//! the membership of each. An `Option` said only "no pair". It could not tell a
//! turn from a purpose with nothing to choose, so the second such purpose would
//! have joined the first in silence.

mod default;
mod not_served_report;
mod reach;

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use crate::core::prefs::{self, Optional, Pref, Text};
use crate::engine::ContextPurpose;
use crate::llm::model_registry::{default_effort, ModelRegistry, ProviderKind};
use crate::llm::LlmProvider;

use crate::engine::event_bus::EventBus;
use crate::llm::AuxProvider;
pub use default::ModelSource;
pub(crate) use default::{recommended, recommended_effort, select as select_model};
use not_served_report::NotServedRecorder;
pub(crate) use reach::{NotServed, Reach};

/// The preference pair one auxiliary purpose reads.
///
/// Which key an unset half inherits, and its default, live in the catalog
/// (`PrefDefault::Inherits`). Only the change summary inherits, from the
/// title pair. The memory tasks inherit nothing: each was measured alone.
pub(crate) struct AuxModelPrefs {
    pub(crate) model: &'static Pref<Text>,
    /// `None` for a purpose whose models offer no reasoning tiers, which is
    /// image generation. The tier set decides, so there is no key to store.
    pub(crate) reasoning: Option<&'static Pref<Text>>,
    /// Whether the call sends images, so its model must read them. Only image
    /// description does.
    pub(crate) needs_vision: bool,
}

/// A pair whose unset default depends on which providers are configured, so
/// no catalog value can say it. Its owner resolves it.
pub(crate) struct ProviderResolvedPrefs {
    pub(crate) model: &'static Pref<Optional>,
    pub(crate) reasoning: &'static Pref<Optional>,
}

/// The compactor's pair, which `summary_tree::compactor_selection` resolves.
pub(crate) const SUMMARY_COMPACTION_PREFS: ProviderResolvedPrefs = ProviderResolvedPrefs {
    model: &prefs::MODEL_SUMMARY_COMPACTION,
    reasoning: &prefs::REASONING_SUMMARY_COMPACTION,
};

/// A resolved *model selection* for one auxiliary call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuxSelection {
    /// The model every request names. Never empty.
    pub(crate) model: String,
    /// `None` leaves the tier to the router: the model's *default effort*,
    /// else no effort at all.
    pub(crate) reasoning: Option<String>,
    pub(crate) source: ModelSource,
    /// Whether a configured provider serves `model` and has not answered
    /// not-found for it. Only a stored pick or the chat model can be
    /// unreachable, and their calls then fail naming why.
    pub(crate) reachable: bool,
    /// Where a default call moves when `model` answers not-found. `None` for a
    /// stored pick or the chat model, which move nowhere (ADR 0403).
    pub(crate) fallback: Option<String>,
    /// The models this purpose would run on ahead of `model`, plus `model`,
    /// whose provider answered not-found within the window.
    pub(crate) not_served: Vec<String>,
    /// Whether the purpose sends images, so `model` must read them.
    pub(crate) needs_vision: bool,
    /// Whether `model` reads images, by its row's *vision flag*.
    pub(crate) vision: bool,
}

impl AuxSelection {
    /// Why this call must not go out, or `None` when it may.
    ///
    /// A purpose that sends images refuses a model that cannot read them,
    /// before anything is spent. A stored pick is named as the user's choice;
    /// an unset one means no recommended model that can is reachable.
    pub(crate) fn refusal(&self) -> Option<String> {
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

/// Where one purpose's model comes from.
///
/// Four arms read no preference, for four different reasons, and the reason
/// is what a reader needs. Collapsing them into one `Option` is what let the
/// command guard's judge sit outside the invariant unnoticed.
pub(crate) enum AuxModelSource {
    /// An agent's own round trip, which is not an auxiliary call.
    Turn,
    /// The backend fixes the model, so there is nothing to choose. The `judge`
    /// tool is Jev or nothing: unlike the judgment sites it has no chat model
    /// to fall back to. Each web search backend sends its own search model.
    BackendPinned,
    /// Runs the agent's own chat model. It owns no preference, and no reachable
    /// budget either: the call goes out through the agent's provider, under the
    /// timeout that provider was built with.
    AgentModel,
    /// The pair the user sets.
    Preferences(AuxModelPrefs),
    /// A pair the user sets, whose default is resolved against the configured
    /// providers. Only the compactor's: [`SUMMARY_COMPACTION_PREFS`].
    ProviderResolved,
    /// The caller names the model in its own request, as an app does through
    /// the credentialed proxy. The engine chooses nothing.
    CallerChosen,
}

#[cfg(test)]
impl AuxModelSource {
    /// The pair, for a test that only handles the settable case.
    pub(crate) fn prefs(self) -> Option<AuxModelPrefs> {
        match self {
            Self::Preferences(pair) => Some(pair),
            _ => None,
        }
    }
}

/// Whether `purpose` sends images, so its model must read them.
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

/// Where `purpose` gets its model.
pub(crate) fn model_source(purpose: ContextPurpose) -> AuxModelSource {
    let pair = match purpose {
        ContextPurpose::Turn => return AuxModelSource::Turn,
        ContextPurpose::JudgeTool | ContextPurpose::WebSearch => {
            return AuxModelSource::BackendPinned
        }
        ContextPurpose::Proxy => return AuxModelSource::CallerChosen,
        ContextPurpose::IntentLoop
        | ContextPurpose::MemoryCorrection
        | ContextPurpose::ArtifactSummary
        | ContextPurpose::SideQuestion => return AuxModelSource::AgentModel,
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
        // The rented talker (ADR 0149). No reasoning half: a speech-to-speech
        // model offers no tiers, and a spoken reply cannot wait for one.
        //
        // It reads its model here and nothing else. Voice holds a socket rather
        // than making an HTTP call, so `AuxCall` never sees it and the short
        // budget `budget_for` hands it is unreachable.
        ContextPurpose::Voice => AuxModelPrefs {
            model: &prefs::MODEL_VOICE_TALKER,
            reasoning: None,
            needs_vision: false,
        },
        // The safety gate. Its catalog default is the measured one, Gemini 3
        // Flash (ADR 0406), which also leads the shared fallbacks.
        ContextPurpose::CommandJudge => AuxModelPrefs {
            model: &prefs::MODEL_COMMAND_JUDGE,
            reasoning: Some(&prefs::REASONING_COMMAND_JUDGE),
            needs_vision: false,
        },
    };
    AuxModelSource::Preferences(pair)
}

/// Read `purpose`'s *model selection* and resolve it against the providers
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
    // An unfiltered router (`None`) answers for every provider.
    let configured = router
        .configured_providers()
        .unwrap_or_else(|| ProviderKind::ALL.to_vec());
    let reach = Reach::new(configured, NotServed::recent(pool).await);
    let recommended = recommended(purpose, registry);
    // The model turns run on: the stored one, else the router's own default,
    // which already carries `LUCIDOS_MODEL`.
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
            // Settings once wrote `default` for "the extractor's own model",
            // which is what unset means now.
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
    // An engine on `LUCIDOS_MODEL=mock`, the e2e opt-in, runs these calls on
    // its chat model, the mock. A test then spends nothing, and the record
    // says so.
    if router.default_model() == crate::llm::MOCK_MODEL {
        return AuxSelection {
            model: crate::llm::MOCK_MODEL.to_string(),
            reasoning,
            source: ModelSource::ChatModel,
            reachable: true,
            fallback: None,
            not_served: Vec::new(),
            needs_vision: needs_vision(purpose),
            vision: true,
        };
    }
    // Only a purpose reading its own pair moves. The compactor re-resolves per
    // node instead, each model at its own measured tier.
    let fallback = match (source, model_source(purpose)) {
        (ModelSource::Default, AuxModelSource::Preferences(_)) => {
            default::next_reachable(&model, &recommended, registry, &reach)
        }
        _ => None,
    };
    AuxSelection {
        reachable: reach.serves(registry, &model),
        not_served: default::passed_over(&model, source, &recommended, registry, &reach),
        fallback,
        needs_vision: needs_vision(purpose),
        vision: crate::llm::model_registry::reads_images(registry, &model),
        model,
        reasoning,
        source,
    }
}

/// The tier a task runs `model` at when no tier is stored. The task's own tier
/// was chosen for its recommended models, so it binds only those. Any other
/// model runs at its *default effort*.
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

/// The purposes Settings shows a background model row for, in its order.
pub(crate) const BACKGROUND_ROWS: &[ContextPurpose] = &[
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
];

/// The model preference key `purpose` reads, which names its Settings row.
pub(crate) fn model_key(purpose: ContextPurpose) -> Option<&'static str> {
    match model_source(purpose) {
        AuxModelSource::Preferences(pair) => Some(pair.model.key()),
        AuxModelSource::ProviderResolved => Some(SUMMARY_COMPACTION_PREFS.model.key()),
        AuxModelSource::Turn
        | AuxModelSource::BackendPinned
        | AuxModelSource::AgentModel
        | AuxModelSource::CallerChosen => None,
    }
}

/// Wall-clock budget for one auxiliary call.
///
/// Two numbers rather than one, because a deadline alone cannot keep its
/// promise. The provider retries `MAX_RETRIES` times behind exponential
/// backoff, over a client whose own per-request timeout was 900s. So a 30s
/// deadline could only ever cut the FIRST attempt off, and the three retries
/// it was paying for never happened. `attempt_timeout` rides on every request
/// as `ModelSelection::attempt_timeout`, and
/// [`a_deadline_holds_one_full_attempt_and_the_whole_backoff`] pins the
/// arithmetic.
///
/// **The bound is ONE full attempt plus the whole backoff, not four.** Four
/// would force `attempt_timeout` down to a quarter of the deadline, which is
/// the trap: an attempt shorter than the call's real latency turns one success
/// into four guaranteed failures. The observed failure mode is a transport
/// error that returns in milliseconds. So what the deadline must hold is one
/// attempt that can actually finish, plus the backoff between the cheap
/// retries. A server that hangs four times over consumes the deadline, which
/// is precisely what a deadline is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuxBudget {
    /// Cap on the whole call, retries included.
    pub(crate) deadline: Duration,
    /// Cap on one HTTP attempt.
    pub(crate) attempt_timeout: Duration,
}

/// The summariser's budget. It is the only auxiliary call that ships tens of
/// thousands of tokens, and the only one whose failure is invisible: since ADR
/// 0102 a miss keeps the cached paragraph, so nothing surfaces except a staler
/// summary. It ran at 30s and landed 3 times in 19 eligible turns.
///
/// The attempt cap is generous because the input is: 83k tokens of assistant
/// turns. The deadline leaves room for a retry after a fast failure.
///
/// **It is the only thing that stops the call.** The refresh runs in a detached
/// task (ADR 0102), so no turn is waiting on it and nothing else cancels it.
const SUMMARY_BUDGET: AuxBudget = AuxBudget {
    deadline: Duration::from_secs(90),
    attempt_timeout: Duration::from_secs(45),
};

/// Every other auxiliary call: a short prompt, a short answer, and a user
/// waiting on the turn behind it.
const SHORT_CALL_BUDGET: AuxBudget = AuxBudget {
    deadline: Duration::from_secs(30),
    attempt_timeout: Duration::from_secs(20),
};

/// The `judge` tool's budget. It is the only auxiliary call an agent asks for
/// by name, and the schema pushes it toward batches: a hundred questions in one
/// request is the shape the tool exists for.
///
/// The attempt cap is generous for that reason. Jev retries nothing, so the cap
/// is the whole call in practice, and the deadline is the outer bound the
/// caller applies on top.
const JUDGE_TOOL_BUDGET: AuxBudget = AuxBudget {
    deadline: Duration::from_secs(75),
    attempt_timeout: Duration::from_secs(60),
};

/// One compactor node. Its input is up to 16 KB of context plus a whole
/// message, and a line over the size limit costs a follow-up round. The
/// deadline covers every round of one node; a node that runs out is retried.
const COMPACTION_BUDGET: AuxBudget = AuxBudget {
    deadline: Duration::from_secs(85),
    attempt_timeout: Duration::from_secs(45),
};

/// One `find` batch: about 40 questions over 40 lines, in one request.
const MEMORY_FIND_BUDGET: AuxBudget = AuxBudget {
    deadline: Duration::from_secs(60),
    attempt_timeout: Duration::from_secs(45),
};

/// The budget `purpose` runs inside.
///
/// Exhaustive on purpose, with no wildcard arm. A new variant then fails the
/// build here until somebody decides how long its call may take. ADR 0107
/// promised that, and a `_` arm quietly took it back.
pub(crate) fn budget_for(purpose: ContextPurpose) -> AuxBudget {
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
        | ContextPurpose::ReadDecision => SHORT_CALL_BUDGET,
        // Eight that never ask. None is an auxiliary HTTP call this module
        // times: a turn and the four `AgentModel` purposes run on the agent's
        // own provider and its timeout, and the talker holds a socket. Web
        // search times its own backends, and the proxy its own forwards.
        ContextPurpose::Turn
        | ContextPurpose::Voice
        | ContextPurpose::IntentLoop
        | ContextPurpose::MemoryCorrection
        | ContextPurpose::ArtifactSummary
        | ContextPurpose::SideQuestion
        | ContextPurpose::WebSearch
        | ContextPurpose::Proxy => SHORT_CALL_BUDGET,
    }
}

/// Everything one auxiliary call needs: which model, at what effort, inside
/// what budget, and the router that serves it. Resolved once from a
/// [`ContextPurpose`], so no call site restates a model id or hardcodes an
/// effort.
pub(crate) struct AuxCall {
    selection: AuxSelection,
    budget: AuxBudget,
    router: Arc<dyn LlmProvider>,
    /// Who hears a not-found answer. Unset in tests that build no engine.
    report: Option<Arc<NotServedRecorder>>,
}

impl AuxCall {
    /// Resolve the purpose's *model selection* against `router` and pair it
    /// with its budget.
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

    /// Report a not-found answer on `bus`: the event that moves later
    /// defaults, and a notification when a stored pick failed (ADR 0403).
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
            reach: Reach::new(configured, NotServed::default()),
            purpose,
            source: self.selection.source,
        }));
        self
    }

    /// A call over `provider` at the purpose's declared defaults, with no
    /// preference read. Tests only: production resolves against the router.
    #[cfg(test)]
    pub(crate) fn over(provider: Arc<dyn LlmProvider>, purpose: ContextPurpose) -> Self {
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

    /// The provider every request of this call goes out on: the router, with
    /// the selected model and the purpose's attempt cap pinned.
    pub(crate) fn provider(&self) -> Arc<dyn LlmProvider> {
        let provider = AuxProvider::new(
            self.router.clone(),
            self.selection.model.clone(),
            self.budget.attempt_timeout,
        );
        Arc::new(match &self.report {
            Some(report) => provider.reporting(report.clone(), self.selection.fallback.clone()),
            None => provider,
        })
    }

    pub(crate) fn selection(&self) -> &AuxSelection {
        &self.selection
    }

    pub(crate) fn model(&self) -> &str {
        &self.selection.model
    }

    pub(crate) fn reasoning(&self) -> Option<&str> {
        self.selection.reasoning.as_deref()
    }

    pub(crate) fn attempt_timeout(&self) -> Duration {
        self.budget.attempt_timeout
    }

    pub(crate) fn deadline(&self) -> Duration {
        self.budget.deadline
    }
}

/// Every purpose the enum has, so a new variant fails the tests below until it
/// declares what it reads and how long it may take.
#[cfg(test)]
pub(crate) const ALL_PURPOSES: &[ContextPurpose] = &[
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
];

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod resolve_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// The list above must stay exhaustive. Only a wildcard-free `match` can
    /// tell us. So adding a variant breaks the build here, rather than
    /// silently skipping it in every test below.
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
                | ContextPurpose::ReadDecision => {}
            }
        }
        assert_eq!(ALL_PURPOSES.len(), 20);
    }

    /// The key an unset handle inherits, if any.
    fn inherited_key(handle: &Pref<Text>) -> Option<&'static str> {
        match handle.spec.default {
            prefs::PrefDefault::Inherits(other) => Some(other.key),
            _ => None,
        }
    }

    /// The invariant this module exists for. Two purposes sharing one model
    /// preference is exactly what made the summariser unnameable.
    ///
    /// Every arm is asserted, not just the settable one. A purpose reading
    /// nothing must name which kind of nothing. So a new variant cannot join a
    /// no-preference arm without a reviewer seeing the membership change.
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
                AuxModelSource::CallerChosen => assert_eq!(
                    *purpose,
                    ContextPurpose::Proxy,
                    "the proxy is the only purpose whose caller names the model"
                ),
                AuxModelSource::AgentModel => assert!(
                    matches!(
                        purpose,
                        ContextPurpose::IntentLoop
                            | ContextPurpose::MemoryCorrection
                            | ContextPurpose::ArtifactSummary
                            | ContextPurpose::SideQuestion
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

    /// Every reasoning key a purpose reads, the provider-resolved one included.
    fn reasoning_key(purpose: ContextPurpose) -> Option<&'static str> {
        match model_source(purpose) {
            AuxModelSource::Preferences(pair) => pair.reasoning.map(|r| r.key()),
            AuxModelSource::ProviderResolved => Some(SUMMARY_COMPACTION_PREFS.reasoning.key()),
            _ => None,
        }
    }

    /// Only image description sends images. A second purpose needing vision
    /// would change what its picker offers and what it refuses.
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

    /// The reasoning half is owned just as exclusively.
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

    /// The compactor's default depends on the configured providers, so no
    /// catalog value can state it: both of its keys are unset by default.
    #[test]
    fn the_compactor_resolves_its_default_against_the_providers() {
        assert!(matches!(
            model_source(ContextPurpose::SummaryCompaction),
            AuxModelSource::ProviderResolved
        ));
    }

    /// Every default must be a real tier. A default off the ladder would be
    /// dropped at the wire, so the Settings row would show a value the request
    /// never carries.
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

    /// Image generation renders no effort control, so it stores no effort.
    #[test]
    fn image_generation_has_no_reasoning_half() {
        let pair = model_source(ContextPurpose::ImageGen)
            .prefs()
            .expect("image generation reads a model");
        assert!(pair.reasoning.is_none());
    }

    /// A stored `model_memory` was chosen for extraction, and the model one
    /// workspace stored there wrote the worst search queries. So no memory
    /// task follows another's key, model or effort.
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

    /// Both halves follow the title pair while unset, so a workspace that
    /// tuned its title model gets change summaries from the same model.
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

    /// Fact extraction keeps `model_memory` outright.
    #[test]
    fn fact_extraction_still_owns_the_memory_model() {
        let pair = model_source(ContextPurpose::Memory).prefs().expect("prefs");
        assert_eq!(pair.model.key(), prefs::MODEL_MEMORY.key());
    }

    /// The `judge` tool is Jev or nothing, so a model preference would name a
    /// model nothing reads. It says that in the type rather than by absence.
    #[test]
    fn the_judge_tool_owns_no_model_preference() {
        assert!(matches!(
            model_source(ContextPurpose::JudgeTool),
            AuxModelSource::BackendPinned
        ));
    }

    /// The command guard declares the pair it reads, which is what brings it
    /// under the uniqueness invariant. The judge reads the same two handles.
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

    /// The `judge` tool asks for batches, so its attempt cap is the generous
    /// one its call site used before the budget moved here. Lowering it turns a
    /// hundred-question request into a timeout.
    #[test]
    fn the_judge_tool_keeps_its_generous_attempt_cap() {
        assert_eq!(
            budget_for(ContextPurpose::JudgeTool).attempt_timeout,
            Duration::from_secs(60)
        );
    }

    /// An effort only ever inherits another purpose's effort key. Pointing
    /// one at a MODEL key would send a model id to the wire as a tier.
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

    /// The whole point of pairing a deadline with an attempt timeout. A
    /// deadline too short for one full attempt plus the backoff cuts a retry
    /// off mid-flight, and the retry never happens.
    ///
    /// One attempt, not `MAX_RETRIES + 1`: see the note on [`AuxBudget`] for
    /// why bounding all four is worse than bounding none.
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

    /// An attempt cap below the deadline is what makes the retries reachable
    /// at all. A cap at or above it means the first attempt can eat the whole
    /// budget, which is the 900s-client behaviour this replaced.
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

    /// The summariser gets a longer rope than the calls a user is waiting on.
    /// It carries 80k-token payloads, and it runs on a refresh rather than
    /// every turn.
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
