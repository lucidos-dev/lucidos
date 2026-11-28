use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;

use async_trait::async_trait;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod chat;
pub mod endpoint;
mod fallback;
pub mod openai_decisions;
pub mod select;
pub mod system_one;

pub use chat::ChatJudgmentProvider;
pub use endpoint::{SystemOneEndpoint, TYPESAFE_API_KEY_ENV, TYPESAFE_CREDENTIAL_SERVICE};
pub use fallback::for_site;
pub use openai_decisions::{OpenAiDecisionsProvider, OPENAI_DECISIONS_MODEL};
pub use select::{
    jev_for_agent, judgment_available, system_one_for, system_one_or_default_for, JudgmentSite,
    WAKE_DECISION_DEFAULT,
};
pub use system_one::{SystemOneProvider, JEV_DEFAULT_MODEL, TYPESAFE_API_BASE_URL};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Question {
    Noul {
        instructions: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    Choice {
        instructions: String,
        #[serde(serialize_with = "pairs_as_map", deserialize_with = "pairs_from_map")]
        criteria: Vec<(String, Option<String>)>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoulCriteria {
    #[serde(rename = "true")]
    pub yes: String,
    #[serde(rename = "false")]
    pub no: String,
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    Noul(NoulAnswer),
    Choice(ChoiceAnswer),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoulAnswer {
    pub noul: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChoiceAnswer {
    pub choice: String,
    pub probabilities: HashMap<String, f64>,
    pub confidence: f64,
}

impl ChoiceAnswer {
    /// INVARIANT: The probability of one option, or zero for an option the answer does
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

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Answers(HashMap<String, Answer>);

impl Answers {
    pub fn new(answers: HashMap<String, Answer>) -> Self {
        Self(answers)
    }

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

/// INVARIANT: The answers to one request, with everything a caller needs to record it.
///
/// The provider fills `model` and `request_chars` rather than the caller. The
/// request id we ask for is an alias, `jev-latest`, and the response names the
/// version that answered. A caller stamping the alias would open a second model
/// line for one model. The size is the serialized body, which only the provider
/// has: a caller measuring its own state misses the questions riding with it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Judgment {
    pub answers: Answers,
    pub usage: Option<crate::llm::api_usage::ApiUsage>,
    pub model: Option<String>,
    pub served_model: Option<String>,
    pub request_chars: usize,
}

#[async_trait]
pub trait JudgmentProvider: Send + Sync {
    async fn ask(
        &self,
        state: serde_json::Value,
        questions: Vec<(String, Question)>,
        call: crate::llm::metered::CallToken,
    ) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>>;
}

#[async_trait]
impl<P: JudgmentProvider + ?Sized> JudgmentProvider for Box<P> {
    async fn ask(
        &self,
        state: serde_json::Value,
        questions: Vec<(String, Question)>,
        call: crate::llm::metered::CallToken,
    ) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
        (**self).ask(state, questions, call).await
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
