//! Typed judgments: a question with a closed answer set, and the answer plus
//! its probability distribution.
//!
//! This is a sibling of [`crate::llm::provider::LlmProvider`], not a kind of
//! one. `chat` takes messages and tools and returns text; a judgment takes
//! state and named questions and returns typed answers. Encoding the questions
//! into a chat message would throw the distribution away, and the distribution
//! is the reason this layer exists: it lets a caller put its tie-break in Rust
//! instead of in prompt text.
//!
//! The question types come from TypeSafe's System One API, which [`jev`]
//! speaks. Nothing here is Jev-specific, so a second backend needs no change
//! to these types. Its third primitive, **Score**, is deliberately absent: no
//! call site asks one, and ADR 0220 records it as the next candidate.
//!
//! **Adding a judgment call site does not mean adding a judgment provider.**
//! Both current callers keep their existing prompt-and-parse path as the
//! default, and reach for this layer only when the user opts in. See
//! `docs/plans/2026-09-19-jev-judgment-provider-for-classification.md`.

use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;

use async_trait::async_trait;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod jev;
pub mod select;

pub use jev::{JevProvider, JEV_DEFAULT_MODEL, TYPESAFE_API_BASE_URL};
pub use select::{
    jev_for, jev_for_agent, judgment_available, JudgmentSite, TYPESAFE_API_KEY_ENV,
    TYPESAFE_CREDENTIAL_SERVICE,
};

/// One question, in the shape the wire wants.
///
/// `instructions` carries the judgment itself and `criteria` defines the
/// possible answers. The question's id is chosen by the caller and is never
/// sent to the model, so the meaning has to be complete inside these fields.
///
/// It reads as well as it writes, because the `judge` tool takes its questions
/// from the agent as JSON. One type therefore serves the tool's argument schema
/// and the wire body, and the two cannot drift.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Question {
    /// Yes or no, answered as the probability of yes.
    Noul {
        instructions: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    /// One option from a closed set, answered with a probability per option.
    Choice {
        instructions: String,
        /// Option name to rubric text. A `None` description means the name
        /// speaks for itself. Serialized as a JSON object in this order.
        #[serde(serialize_with = "pairs_as_map", deserialize_with = "pairs_from_map")]
        criteria: Vec<(String, Option<String>)>,
    },
}

/// What a yes and a no mean for one [`Question::Noul`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoulCriteria {
    /// Both field names are wire keywords, so they carry a `rename`.
    #[serde(rename = "true")]
    pub yes: String,
    #[serde(rename = "false")]
    pub no: String,
}

/// Serialize ordered pairs as a JSON object, keeping the declared order.
///
/// A `BTreeMap` would sort the options and a `HashMap` would shuffle them. The
/// order is not semantic to the model, but a stable one keeps a captured
/// request payload readable and keeps the tests deterministic.
fn pairs_as_map<S, V>(pairs: &[(String, V)], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    V: Serialize,
{
    let mut map = serializer.serialize_map(Some(pairs.len()))?;
    for (key, value) in pairs {
        map.serialize_entry(key, value)?;
    }
    map.end()
}

/// Read a JSON object back into ordered pairs, the inverse of [`pairs_as_map`].
///
/// A `MapAccess` visits entries in document order, so an agent's option order
/// survives a decode straight from the wire. Decoding from an already-parsed
/// `serde_json::Value` cannot keep it, that map being sorted. Neither matters
/// to the model, which never sees the order.
fn pairs_from_map<'de, D, V>(deserializer: D) -> Result<Vec<(String, V)>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct Pairs<V>(PhantomData<V>);

    impl<'de, V: Deserialize<'de>> Visitor<'de> for Pairs<V> {
        type Value = Vec<(String, V)>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an object mapping each option name to its rubric text")
        }

        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut pairs = Vec::with_capacity(map.size_hint().unwrap_or(0));
            while let Some(entry) = map.next_entry()? {
                pairs.push(entry);
            }
            Ok(pairs)
        }
    }

    deserializer.deserialize_map(Pairs(PhantomData))
}

/// One answer, matched to its question's type.
///
/// It serializes as well as it reads, because the `judge` tool hands the
/// answers back to the agent verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    Noul(NoulAnswer),
    Choice(ChoiceAnswer),
}

/// The probability that a [`Question::Noul`] is yes, from 0 to 1.
///
/// There is no separate confidence. A value near 0.5 means yes and no are
/// equally likely, never that the condition half holds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoulAnswer {
    pub noul: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChoiceAnswer {
    /// The highest-probability option.
    pub choice: String,
    /// Every option mapped to its probability. The values sum to 1.
    pub probabilities: HashMap<String, f64>,
    /// How concentrated the distribution is, from 0 to 1. It says nothing
    /// about whether the answer is correct.
    pub confidence: f64,
}

impl ChoiceAnswer {
    /// The probability of one option, or zero for an option the answer does
    /// not mention.
    ///
    /// **Zero is the safe reading, and callers depend on it.** A caller asking
    /// "how likely is the harmless option" must not read an absent key as
    /// harmless. Pick the option name so that a missing one errs the way you
    /// want.
    pub fn probability(&self, option: &str) -> f64 {
        self.probabilities.get(option).copied().unwrap_or(0.0)
    }
}

/// The answers to one request, keyed by the ids the caller chose.
///
/// The accessors return `None` for an id that is absent or came back as the
/// wrong type. A caller treats that as a judgment it did not get, which is the
/// same case as a transport failure.
///
/// It serializes as the bare object, so the `judge` tool's result is keyed by
/// the ids the agent chose.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Answers(HashMap<String, Answer>);

impl Answers {
    pub fn new(answers: HashMap<String, Answer>) -> Self {
        Self(answers)
    }

    /// The probability of yes for a Noul question.
    pub fn noul(&self, id: &str) -> Option<f64> {
        match self.0.get(id) {
            Some(Answer::Noul(a)) => Some(a.noul),
            _ => None,
        }
    }

    pub fn choice(&self, id: &str) -> Option<&ChoiceAnswer> {
        match self.0.get(id) {
            Some(Answer::Choice(a)) => Some(a),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What one judgment call cost, for the caller that records it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JudgmentUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// The answers to one request, with everything a caller needs to record it.
///
/// The provider fills `model` and `request_chars` rather than the caller. The
/// request id we ask for is an alias, `jev-latest`, and the response names the
/// version that answered. A caller stamping the alias would open a second model
/// line for one model. The size is the serialized body, which only the provider
/// has: a caller measuring its own state misses the questions riding with it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Judgment {
    pub answers: Answers,
    pub usage: JudgmentUsage,
    /// What the response says answered, or `None` when it named nothing. The
    /// caller then falls back to the alias it asked for.
    pub model: Option<String>,
    /// Chars in the request body the provider sent.
    pub request_chars: usize,
}

/// A backend that answers typed questions about one state.
///
/// One call carries every question, because independent questions over the
/// same state run in parallel on the backend. A caller that splits them pays
/// for the state twice.
#[async_trait]
pub trait JudgmentProvider: Send + Sync {
    async fn ask(
        &self,
        state: serde_json::Value,
        questions: Vec<(String, Question)>,
    ) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>>;
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
