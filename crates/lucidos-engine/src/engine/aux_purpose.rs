//! What an *auxiliary model call* resolves from its [`ContextPurpose`]: the
//! *model selection* it runs under, and the wall-clock budget it runs inside.
//! Five purposes read no preference pair, and [`AuxModelSource`] says why.
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

use std::time::Duration;

use sqlx::PgPool;

use crate::core::{
    PreferenceStore, DEFAULT_COMMAND_JUDGE_REASONING, PREF_IMAGE_MODEL, PREF_MODEL_COMMAND_JUDGE,
    PREF_MODEL_CONVERSATION_SUMMARY, PREF_MODEL_IMAGE_DESCRIPTION, PREF_MODEL_MEMORY,
    PREF_MODEL_QUERY_CLASSIFICATION, PREF_MODEL_TITLE, PREF_MODEL_VOICE_TALKER,
    PREF_REASONING_COMMAND_JUDGE, PREF_REASONING_CONVERSATION_SUMMARY,
    PREF_REASONING_IMAGE_DESCRIPTION, PREF_REASONING_MEMORY, PREF_REASONING_QUERY_CLASSIFICATION,
    PREF_REASONING_TITLE,
};
use crate::engine::ContextPurpose;

/// The reasoning half of a purpose's *model selection*.
pub(crate) struct AuxReasoningPref {
    pub(crate) key: &'static str,
    /// Consulted when `key` is unset, before the default. Only query
    /// classification has one, and it is the other half of its model fallback:
    /// the split must not quietly drop a workspace that raised
    /// `reasoning_memory`.
    ///
    /// The conversation summary deliberately has none. Its default is `low`,
    /// which is what its call site ran at, and inheriting `reasoning_memory`
    /// would LOWER it.
    pub(crate) fallback_key: Option<&'static str>,
    /// Used when neither key is set. Each is the literal its call site
    /// hardcoded before the preference existed, so the split changed nothing.
    ///
    /// Image description is the one exception. It passed no effort at all,
    /// which `gemini_generation_config` reads as `high`, so a caption was
    /// paying for the model's deepest thinking. Its default is `none`, matching
    /// every sibling background call.
    pub(crate) default: &'static str,
}

/// The preference pair one auxiliary purpose reads.
pub(crate) struct AuxModelPrefs {
    pub(crate) model_key: &'static str,
    /// Consulted when `model_key` is unset. Both keys split out of
    /// `model_memory` have one, the conversation summary and query
    /// classification: a workspace that pinned a model there must keep running
    /// all three jobs on it.
    pub(crate) model_fallback_key: Option<&'static str>,
    /// `None` for a purpose whose models offer no reasoning tiers, which is
    /// image generation. The tier set decides, so there is no key to store.
    pub(crate) reasoning: Option<AuxReasoningPref>,
}

/// A resolved *model selection* for one auxiliary call.
pub(crate) struct AuxSelection {
    /// Empty means "the extractor's default model", which is what
    /// `MemoryExtractor::provider_for_model` reads an empty id as.
    pub(crate) model: String,
    /// `None` means send no effort and let the provider decide.
    pub(crate) reasoning: Option<String>,
}

/// Where one purpose's model comes from.
///
/// Three arms read no preference, for three different reasons, and the reason
/// is what a reader needs. Collapsing them into one `Option` is what let the
/// command guard's judge sit outside the invariant unnoticed.
pub(crate) enum AuxModelSource {
    /// An agent's own round trip, which is not an auxiliary call.
    Turn,
    /// Pinned to one backend, so there is nothing to choose. The `judge` tool
    /// is Jev or nothing: unlike the two classification sites it has no chat
    /// path to fall back to, so a model preference would name nothing.
    BackendPinned,
    /// Runs the agent's own chat model. It owns no preference, and no reachable
    /// budget either: the call goes out through the agent's provider, under the
    /// timeout that provider was built with.
    AgentModel,
    /// The pair the user sets.
    Preferences(AuxModelPrefs),
}

impl AuxModelSource {
    /// The pair, for a caller that only handles the settable case.
    pub(crate) fn prefs(self) -> Option<AuxModelPrefs> {
        match self {
            Self::Preferences(prefs) => Some(prefs),
            _ => None,
        }
    }
}

/// Where `purpose` gets its model.
pub(crate) fn model_source(purpose: ContextPurpose) -> AuxModelSource {
    let prefs = match purpose {
        ContextPurpose::Turn => return AuxModelSource::Turn,
        ContextPurpose::JudgeTool => return AuxModelSource::BackendPinned,
        ContextPurpose::IntentLoop
        | ContextPurpose::MemoryCorrection
        | ContextPurpose::ArtifactSummary => return AuxModelSource::AgentModel,
        ContextPurpose::Title => AuxModelPrefs {
            model_key: PREF_MODEL_TITLE,
            model_fallback_key: None,
            reasoning: Some(AuxReasoningPref {
                key: PREF_REASONING_TITLE,
                fallback_key: None,
                default: "none",
            }),
        },
        ContextPurpose::ImageDescribe => AuxModelPrefs {
            model_key: PREF_MODEL_IMAGE_DESCRIPTION,
            model_fallback_key: None,
            reasoning: Some(AuxReasoningPref {
                key: PREF_REASONING_IMAGE_DESCRIPTION,
                fallback_key: None,
                default: "none",
            }),
        },
        ContextPurpose::Memory => AuxModelPrefs {
            model_key: PREF_MODEL_MEMORY,
            model_fallback_key: None,
            reasoning: Some(AuxReasoningPref {
                key: PREF_REASONING_MEMORY,
                fallback_key: None,
                default: "none",
            }),
        },
        ContextPurpose::ConversationSummary => AuxModelPrefs {
            model_key: PREF_MODEL_CONVERSATION_SUMMARY,
            model_fallback_key: Some(PREF_MODEL_MEMORY),
            reasoning: Some(AuxReasoningPref {
                key: PREF_REASONING_CONVERSATION_SUMMARY,
                fallback_key: None,
                default: "low",
            }),
        },
        ContextPurpose::QueryClassification => AuxModelPrefs {
            model_key: PREF_MODEL_QUERY_CLASSIFICATION,
            model_fallback_key: Some(PREF_MODEL_MEMORY),
            reasoning: Some(AuxReasoningPref {
                key: PREF_REASONING_QUERY_CLASSIFICATION,
                fallback_key: Some(PREF_REASONING_MEMORY),
                default: "none",
            }),
        },
        ContextPurpose::ImageGen => AuxModelPrefs {
            model_key: PREF_IMAGE_MODEL,
            model_fallback_key: None,
            reasoning: None,
        },
        // The rented talker (ADR 0149). No reasoning half: a speech-to-speech
        // model offers no tiers, and a spoken reply cannot wait for one.
        //
        // It reads its model here and nothing else. Voice holds a socket rather
        // than making an HTTP call, so `AuxCall` never sees it and the short
        // budget `budget_for` hands it is unreachable.
        ContextPurpose::Voice => AuxModelPrefs {
            model_key: PREF_MODEL_VOICE_TALKER,
            model_fallback_key: None,
            reasoning: None,
        },
        // Declared here so the uniqueness invariant covers the pair, but
        // RESOLVED by `PreferenceStore::command_judge_model`, which the judge
        // keeps calling. Its default is a named model where `AuxSelection`'s is
        // the extractor's own, and moving it would change the safety gate's
        // default model. The effort default is the same constant either way.
        ContextPurpose::CommandJudge => AuxModelPrefs {
            model_key: PREF_MODEL_COMMAND_JUDGE,
            model_fallback_key: None,
            reasoning: Some(AuxReasoningPref {
                key: PREF_REASONING_COMMAND_JUDGE,
                fallback_key: None,
                default: DEFAULT_COMMAND_JUDGE_REASONING,
            }),
        },
    };
    AuxModelSource::Preferences(prefs)
}

/// Read `purpose`'s *model selection* out of the preference store.
///
/// Total by construction. A missing row and a database error both resolve to
/// the default. A background call that refuses to run over a preference read
/// is strictly worse than one running at its default.
///
/// A purpose reading no preference gets the empty selection, which no such
/// purpose asks for: each already knows its own provider.
pub(crate) async fn resolve_selection(pool: &PgPool, purpose: ContextPurpose) -> AuxSelection {
    let Some(prefs) = model_source(purpose).prefs() else {
        return AuxSelection {
            model: String::new(),
            reasoning: None,
        };
    };
    let mut model = read_set(pool, prefs.model_key).await;
    if model.is_none() {
        if let Some(fallback) = prefs.model_fallback_key {
            model = read_set(pool, fallback).await;
        }
    }
    let reasoning = match &prefs.reasoning {
        Some(pref) => {
            let mut effort = read_set(pool, pref.key).await;
            if effort.is_none() {
                if let Some(fallback) = pref.fallback_key {
                    effort = read_set(pool, fallback).await;
                }
            }
            Some(effort.unwrap_or_else(|| pref.default.to_string()))
        }
        None => None,
    };
    AuxSelection {
        model: model.unwrap_or_default(),
        reasoning,
    }
}

/// Whether a *model selection*'s model resolves to the extractor's own default
/// rather than a named model.
///
/// The two spellings are `MemoryExtractor::provider_for_model`'s own branch:
/// an empty id and the literal `"default"` both take it. A caller recording
/// which model ran has to ask the same question, and asking it here is what
/// stops the two from drifting.
pub(crate) fn is_extractor_default(model: &str) -> bool {
    model.is_empty() || model == "default"
}

/// A preference's value when it is set to something non-blank, else `None`. A
/// read error logs and reads as unset.
async fn read_set(pool: &PgPool, key: &str) -> Option<String> {
    match PreferenceStore::get(pool, key).await {
        Ok(Some(v)) if !v.trim().is_empty() => Some(v),
        Ok(_) => None,
        Err(e) => {
            log!(
                "[AuxPurpose] Failed to read {}: {}. Treating it as unset",
                key,
                e
            );
            None
        }
    }
}

/// Wall-clock budget for one auxiliary call.
///
/// Two numbers rather than one, because a deadline alone cannot keep its
/// promise. The provider retries `MAX_RETRIES` times behind exponential
/// backoff, over a client whose own per-request timeout was 900s. So a 30s
/// deadline could only ever cut the FIRST attempt off, and the three retries
/// it was paying for never happened. `attempt_timeout` is what the aux
/// provider's HTTP client is built with, and
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

/// The budget `purpose` runs inside.
///
/// Exhaustive on purpose, with no wildcard arm. A new variant then fails the
/// build here until somebody decides how long its call may take. ADR 0107
/// promised that, and a `_` arm quietly took it back.
pub(crate) fn budget_for(purpose: ContextPurpose) -> AuxBudget {
    match purpose {
        ContextPurpose::ConversationSummary => SUMMARY_BUDGET,
        ContextPurpose::JudgeTool => JUDGE_TOOL_BUDGET,
        ContextPurpose::Title
        | ContextPurpose::ImageDescribe
        | ContextPurpose::Memory
        | ContextPurpose::QueryClassification
        | ContextPurpose::ImageGen
        | ContextPurpose::CommandJudge => SHORT_CALL_BUDGET,
        // Five that never ask. None is an auxiliary HTTP call this module
        // times: a turn and the three `AgentModel` purposes run on the agent's
        // own provider and its timeout, and the talker holds a socket.
        ContextPurpose::Turn
        | ContextPurpose::Voice
        | ContextPurpose::IntentLoop
        | ContextPurpose::MemoryCorrection
        | ContextPurpose::ArtifactSummary => SHORT_CALL_BUDGET,
    }
}

/// Everything one auxiliary call needs: which model, at what effort, and
/// inside what budget. Resolved once from a [`ContextPurpose`], then handed to
/// [`crate::memory::MemoryExtractor`], so no call site restates a model id or
/// hardcodes an effort.
pub(crate) struct AuxCall {
    selection: AuxSelection,
    budget: AuxBudget,
}

impl AuxCall {
    /// Read the purpose's *model selection* and pair it with its budget.
    pub(crate) async fn resolve(pool: &PgPool, purpose: ContextPurpose) -> Self {
        Self {
            selection: resolve_selection(pool, purpose).await,
            budget: budget_for(purpose),
        }
    }

    /// The purpose's declared defaults, with no preference read. Tests only:
    /// every production caller has a pool and must read what the user set.
    #[cfg(test)]
    pub(crate) fn defaults(purpose: ContextPurpose) -> Self {
        let reasoning = model_source(purpose)
            .prefs()
            .and_then(|p| p.reasoning)
            .map(|r| r.default.to_string());
        Self {
            selection: AuxSelection {
                model: String::new(),
                reasoning,
            },
            budget: budget_for(purpose),
        }
    }

    /// Empty means the extractor's own default model.
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
const ALL_PURPOSES: &[ContextPurpose] = &[
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
];

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
                | ContextPurpose::ArtifactSummary => {}
            }
        }
        assert_eq!(ALL_PURPOSES.len(), 13);
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
                AuxModelSource::BackendPinned => assert_eq!(
                    *purpose,
                    ContextPurpose::JudgeTool,
                    "the judge tool is the only backend-pinned purpose"
                ),
                AuxModelSource::AgentModel => assert!(
                    matches!(
                        purpose,
                        ContextPurpose::IntentLoop
                            | ContextPurpose::MemoryCorrection
                            | ContextPurpose::ArtifactSummary
                    ),
                    "{:?} claims to run the agent's own model",
                    purpose
                ),
                AuxModelSource::Preferences(prefs) => {
                    assert!(
                        !seen.contains(&prefs.model_key),
                        "{:?} reuses the model preference {}",
                        purpose,
                        prefs.model_key
                    );
                    seen.push(prefs.model_key);
                }
            }
        }
    }

    /// The reasoning half is owned just as exclusively.
    #[test]
    fn every_reasoning_preference_belongs_to_one_purpose() {
        let mut seen: Vec<&'static str> = vec![];
        for purpose in ALL_PURPOSES {
            let Some(reasoning) = model_source(*purpose).prefs().and_then(|p| p.reasoning) else {
                continue;
            };
            assert!(
                !seen.contains(&reasoning.key),
                "{:?} reuses the reasoning preference {}",
                purpose,
                reasoning.key
            );
            seen.push(reasoning.key);
        }
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
                crate::llm::EFFORT_LADDER.contains(&reasoning.default),
                "{:?} defaults to {:?}, which is not a tier",
                purpose,
                reasoning.default
            );
        }
    }

    /// Image generation renders no effort control, so it stores no effort.
    #[test]
    fn image_generation_has_no_reasoning_half() {
        let prefs = model_source(ContextPurpose::ImageGen)
            .prefs()
            .expect("image generation reads a model");
        assert!(prefs.reasoning.is_none());
    }

    /// The split must not move an existing workspace's summariser. Every one
    /// of them has a `model_memory` value and no `model_conversation_summary`.
    #[test]
    fn the_summary_falls_back_to_the_memory_model() {
        let prefs = model_source(ContextPurpose::ConversationSummary)
            .prefs()
            .expect("prefs");
        assert_eq!(prefs.model_key, PREF_MODEL_CONVERSATION_SUMMARY);
        assert_eq!(prefs.model_fallback_key, Some(PREF_MODEL_MEMORY));
    }

    /// Same promise as the summariser's, for the same reason. Every existing
    /// workspace has a `model_memory` value and no
    /// `model_query_classification`, so the split must leave it classifying on
    /// the model it already used.
    #[test]
    fn query_classification_falls_back_to_the_memory_model() {
        let prefs = model_source(ContextPurpose::QueryClassification)
            .prefs()
            .expect("prefs");
        assert_eq!(prefs.model_key, PREF_MODEL_QUERY_CLASSIFICATION);
        assert_eq!(prefs.model_fallback_key, Some(PREF_MODEL_MEMORY));
    }

    /// Fact extraction keeps `model_memory` outright. The split moved
    /// classification only, and a fallback here would mean the key moved.
    #[test]
    fn fact_extraction_still_owns_the_memory_model() {
        let prefs = model_source(ContextPurpose::Memory).prefs().expect("prefs");
        assert_eq!(prefs.model_key, PREF_MODEL_MEMORY);
        assert_eq!(prefs.model_fallback_key, None);
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
    /// under the uniqueness invariant. `PreferenceStore` still resolves it, so
    /// the declared effort default must be the constant that store falls back
    /// to. Two spellings of one default is the drift this catches.
    #[test]
    fn the_command_judge_declares_the_pair_it_reads() {
        let prefs = model_source(ContextPurpose::CommandJudge)
            .prefs()
            .expect("the command judge reads a pair");
        assert_eq!(prefs.model_key, PREF_MODEL_COMMAND_JUDGE);
        assert_eq!(prefs.model_fallback_key, None);
        let reasoning = prefs.reasoning.expect("it runs at an effort");
        assert_eq!(reasoning.key, PREF_REASONING_COMMAND_JUDGE);
        assert_eq!(reasoning.default, DEFAULT_COMMAND_JUDGE_REASONING);
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

    /// Both halves inherit, or the split is only half invisible. A workspace
    /// that raised `reasoning_memory` was classifying at that effort, and
    /// falling straight to the default would quietly lower it.
    #[test]
    fn query_classification_inherits_the_memory_effort_too() {
        let reasoning = model_source(ContextPurpose::QueryClassification)
            .prefs()
            .and_then(|p| p.reasoning)
            .expect("query classification has a reasoning half");
        assert_eq!(reasoning.key, PREF_REASONING_QUERY_CLASSIFICATION);
        assert_eq!(reasoning.fallback_key, Some(PREF_REASONING_MEMORY));
    }

    /// The summariser's default is `low`, which is what its call site ran at.
    /// Inheriting `reasoning_memory` would LOWER it, so it has no fallback.
    #[test]
    fn the_summary_inherits_no_effort() {
        let reasoning = model_source(ContextPurpose::ConversationSummary)
            .prefs()
            .and_then(|p| p.reasoning)
            .expect("the summary has a reasoning half");
        assert_eq!(reasoning.fallback_key, None);
    }

    /// An effort fallback is only ever another purpose's effort key. Pointing
    /// one at a MODEL key would send a model id to the wire as a tier.
    #[test]
    fn every_effort_fallback_names_an_effort_key() {
        let efforts: Vec<&'static str> = ALL_PURPOSES
            .iter()
            .filter_map(|p| {
                model_source(*p)
                    .prefs()
                    .and_then(|m| m.reasoning)
                    .map(|r| r.key)
            })
            .collect();
        for purpose in ALL_PURPOSES {
            let Some(reasoning) = model_source(*purpose).prefs().and_then(|p| p.reasoning) else {
                continue;
            };
            let Some(fallback) = reasoning.fallback_key else {
                continue;
            };
            assert!(
                efforts.contains(&fallback),
                "{:?} falls back to {}, which is not an effort key",
                purpose,
                fallback
            );
        }
    }

    /// ADR 0102's measurements say `low` is not the problem, so it stays and
    /// the deadline fix can be judged on its own.
    #[test]
    fn the_summary_still_defaults_to_low() {
        let reasoning = model_source(ContextPurpose::ConversationSummary)
            .prefs()
            .and_then(|p| p.reasoning)
            .expect("the summary has a reasoning half");
        assert_eq!(reasoning.default, "low");
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
