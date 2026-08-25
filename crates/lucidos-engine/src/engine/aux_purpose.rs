//! What an *auxiliary model call* resolves from its [`ContextPurpose`]: the
//! *model selection* it runs under, and the wall-clock budget it runs inside.
//! `ContextPurpose::Turn` has no entry, being an agent's own round trip.
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
//! **The command guard's judge has no purpose here**, and so no capture,
//! though it spends real tokens. `engine::command_judge` reads its pair.

use std::time::Duration;

use sqlx::PgPool;

use crate::core::{
    PreferenceStore, PREF_IMAGE_MODEL, PREF_MODEL_CONVERSATION_SUMMARY,
    PREF_MODEL_IMAGE_DESCRIPTION, PREF_MODEL_MEMORY, PREF_MODEL_TITLE,
    PREF_REASONING_CONVERSATION_SUMMARY, PREF_REASONING_IMAGE_DESCRIPTION, PREF_REASONING_MEMORY,
    PREF_REASONING_TITLE,
};
use crate::engine::ContextPurpose;

/// The reasoning half of a purpose's *model selection*.
pub(crate) struct AuxReasoningPref {
    pub(crate) key: &'static str,
    /// Used when the key is unset. Each is the literal its call site hardcoded
    /// before the preference existed, so the split changed nothing.
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
    /// Consulted when `model_key` is unset. Only the conversation summary has
    /// one: it was split out of `model_memory`, and a workspace that pinned a
    /// model there must keep summarising on it.
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

/// The preference pair `purpose` reads, or `None` for `Turn`.
pub(crate) fn model_prefs(purpose: ContextPurpose) -> Option<AuxModelPrefs> {
    let prefs = match purpose {
        ContextPurpose::Turn => return None,
        ContextPurpose::Title => AuxModelPrefs {
            model_key: PREF_MODEL_TITLE,
            model_fallback_key: None,
            reasoning: Some(AuxReasoningPref {
                key: PREF_REASONING_TITLE,
                default: "none",
            }),
        },
        ContextPurpose::ImageDescribe => AuxModelPrefs {
            model_key: PREF_MODEL_IMAGE_DESCRIPTION,
            model_fallback_key: None,
            reasoning: Some(AuxReasoningPref {
                key: PREF_REASONING_IMAGE_DESCRIPTION,
                default: "none",
            }),
        },
        ContextPurpose::Memory => AuxModelPrefs {
            model_key: PREF_MODEL_MEMORY,
            model_fallback_key: None,
            reasoning: Some(AuxReasoningPref {
                key: PREF_REASONING_MEMORY,
                default: "none",
            }),
        },
        ContextPurpose::ConversationSummary => AuxModelPrefs {
            model_key: PREF_MODEL_CONVERSATION_SUMMARY,
            model_fallback_key: Some(PREF_MODEL_MEMORY),
            reasoning: Some(AuxReasoningPref {
                key: PREF_REASONING_CONVERSATION_SUMMARY,
                default: "low",
            }),
        },
        ContextPurpose::ImageGen => AuxModelPrefs {
            model_key: PREF_IMAGE_MODEL,
            model_fallback_key: None,
            reasoning: None,
        },
    };
    Some(prefs)
}

/// Read `purpose`'s *model selection* out of the preference store.
///
/// Total by construction. A missing row and a database error both resolve to
/// the default. A background call that refuses to run over a preference read
/// is strictly worse than one running at its default.
pub(crate) async fn resolve_selection(pool: &PgPool, purpose: ContextPurpose) -> AuxSelection {
    let Some(prefs) = model_prefs(purpose) else {
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
        Some(pref) => Some(
            read_set(pool, pref.key)
                .await
                .unwrap_or_else(|| pref.default.to_string()),
        ),
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
/// turns. The deadline leaves room for a retry after a fast failure. It also
/// stops a hung call from stalling the turn past a minute and a half.
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

/// The budget for an auxiliary call that has no [`ContextPurpose`] of its own.
/// The command guard's judge is the only one, and the module doc says why.
pub(crate) const UNCAPTURED_CALL_BUDGET: AuxBudget = SHORT_CALL_BUDGET;

/// The budget `purpose` runs inside.
///
/// `Turn` takes the short budget as an unreachable default: an agent's own
/// round trip is not an auxiliary call and never asks.
pub(crate) fn budget_for(purpose: ContextPurpose) -> AuxBudget {
    match purpose {
        ContextPurpose::ConversationSummary => SUMMARY_BUDGET,
        _ => SHORT_CALL_BUDGET,
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
        let reasoning = model_prefs(purpose)
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
    ContextPurpose::ImageGen,
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
                | ContextPurpose::ImageGen => {}
            }
        }
        assert_eq!(ALL_PURPOSES.len(), 6);
    }

    /// The invariant this module exists for. Two purposes sharing one model
    /// preference is exactly what made the summariser unnameable.
    #[test]
    fn every_purpose_owns_exactly_one_model_preference() {
        let mut seen: Vec<&'static str> = vec![];
        for purpose in ALL_PURPOSES {
            let Some(prefs) = model_prefs(*purpose) else {
                assert_eq!(*purpose, ContextPurpose::Turn, "only Turn has no prefs");
                continue;
            };
            assert!(
                !seen.contains(&prefs.model_key),
                "{:?} reuses the model preference {}",
                purpose,
                prefs.model_key
            );
            seen.push(prefs.model_key);
        }
    }

    /// The reasoning half is owned just as exclusively.
    #[test]
    fn every_reasoning_preference_belongs_to_one_purpose() {
        let mut seen: Vec<&'static str> = vec![];
        for purpose in ALL_PURPOSES {
            let Some(reasoning) = model_prefs(*purpose).and_then(|p| p.reasoning) else {
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
            let Some(reasoning) = model_prefs(*purpose).and_then(|p| p.reasoning) else {
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
        let prefs = model_prefs(ContextPurpose::ImageGen).expect("image generation reads a model");
        assert!(prefs.reasoning.is_none());
    }

    /// The split must not move an existing workspace's summariser. Every one
    /// of them has a `model_memory` value and no `model_conversation_summary`.
    #[test]
    fn the_summary_falls_back_to_the_memory_model() {
        let prefs = model_prefs(ContextPurpose::ConversationSummary).expect("prefs");
        assert_eq!(prefs.model_key, PREF_MODEL_CONVERSATION_SUMMARY);
        assert_eq!(prefs.model_fallback_key, Some(PREF_MODEL_MEMORY));
    }

    /// ADR 0102's measurements say `low` is not the problem, so it stays and
    /// the deadline fix can be judged on its own.
    #[test]
    fn the_summary_still_defaults_to_low() {
        let reasoning = model_prefs(ContextPurpose::ConversationSummary)
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
