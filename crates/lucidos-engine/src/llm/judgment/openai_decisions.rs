//! OpenAI's Decisions API, which answers the typed questions in [`super`] on
//! its own wire.
//!
//! One `POST /v1/decisions` carries the model, the state as `input` text, and
//! the questions as an array, each named by its id. The answers come back as
//! an array, matched to their questions by `name`:
//!
//! - a Noul is a `predicate`, answered with the `probability` of yes;
//! - a Choice is a `choice`, with one `{value, probability}` per option.
//!
//! A `predicate` takes no criteria, so a Noul's two sides ride inside its
//! instructions. Anything else that comes back, a refusal included, is a
//! missing answer, and the site's safe default applies.
//!
//! The body building and the response parse are separate pure functions, so
//! the wire contract is tested without a server. **No retry**, for the reason
//! [`super::system_one`] gives.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Answer, Answers, ChoiceAnswer, Judgment, JudgmentProvider, NoulAnswer, Question};
use crate::llm::OPENAI_DEFAULT_BASE_URL;

/// The model the Decisions API answers on.
pub const OPENAI_DECISIONS_MODEL: &str = "gpt-6-luna";

/// The full Decisions request URL.
pub(crate) fn decisions_url() -> String {
    format!("{OPENAI_DEFAULT_BASE_URL}/decisions")
}

/// A judgment provider backed by OpenAI's Decisions API.
pub struct OpenAiDecisionsProvider {
    url: String,
    model: String,
    /// Never logged and never formatted into an error. It reaches exactly one
    /// place, the `Authorization` header.
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiDecisionsProvider {
    pub fn new(
        api_key: String,
        timeout: Duration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            url: decisions_url(),
            model: OPENAI_DECISIONS_MODEL.to_string(),
            api_key,
            client,
        })
    }
}

/// A Noul's instructions with what yes and no mean appended, since a
/// `predicate` has no field for them.
fn predicate_instructions(question: &Question) -> String {
    match question {
        Question::Noul {
            instructions,
            criteria: Some(criteria),
        } => format!(
            "{instructions}\n\nTrue means: {}\nFalse means: {}",
            criteria.yes, criteria.no
        ),
        Question::Noul { instructions, .. } | Question::Choice { instructions, .. } => {
            instructions.clone()
        }
    }
}

/// Build the request body for one call.
///
/// The state goes as text, because `input` takes a string or a message array
/// and never an object.
pub(crate) fn build_request_body(
    model: &str,
    state: &Value,
    questions: &[(String, Question)],
) -> Value {
    let questions: Vec<Value> = questions
        .iter()
        .map(|(id, question)| match question {
            Question::Noul { .. } => json!({
                "type": "predicate",
                "name": id,
                "instructions": predicate_instructions(question),
            }),
            Question::Choice {
                instructions,
                criteria,
            } => {
                let choices: Vec<Value> = criteria
                    .iter()
                    .map(|(value, description)| match description {
                        Some(description) => json!({ "value": value, "description": description }),
                        None => json!({ "value": value }),
                    })
                    .collect();
                json!({
                    "type": "choice",
                    "name": id,
                    "instructions": instructions,
                    "choices": choices,
                })
            }
        })
        .collect();
    json!({
        "model": model,
        "input": super::chat::state_text(state),
        "questions": questions,
    })
}

/// A probability, or `None` for anything outside 0 to 1.
fn probability(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
}

/// One answer read from the array, or `None` when it does not read cleanly.
fn read_answer(answer: &Value) -> Option<Answer> {
    match answer.get("type").and_then(Value::as_str)? {
        "predicate" => {
            probability(answer.get("probability")).map(|noul| Answer::Noul(NoulAnswer { noul }))
        }
        "choice" => {
            let mut probabilities = HashMap::new();
            for option in answer.get("probabilities")?.as_array()? {
                let value = option.get("value")?.as_str()?;
                probabilities.insert(value.to_string(), probability(option.get("probability"))?);
            }
            Some(Answer::Choice(ChoiceAnswer {
                choice: answer.get("choice")?.as_str()?.to_string(),
                probabilities,
                confidence: probability(answer.get("confidence"))?,
            }))
        }
        _ => None,
    }
}

/// Parse one response body into the answers, what they cost, and what answered.
///
/// An answer is kept only when its name is one of the ids asked and it reads
/// cleanly. A refusal, an unknown type or a malformed number is dropped with a
/// log line, so it reads as a missing answer.
pub(crate) fn parse_response(
    body: &str,
    questions: &[(String, Question)],
) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
    let value: Value = serde_json::from_str(body)?;
    let raw = value
        .get("answers")
        .and_then(Value::as_array)
        .ok_or("decisions response carried no answers array")?;

    let mut answers = HashMap::with_capacity(raw.len());
    for answer in raw {
        let name = answer
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !questions.iter().any(|(id, _)| id == name) {
            log!("[Judgment] Dropping a decision for unasked {:?}", name);
            continue;
        }
        match read_answer(answer) {
            Some(parsed) => {
                answers.insert(name.to_string(), parsed);
            }
            None => log!(
                "[Judgment] Dropping unreadable decision {}: {}",
                name,
                answer
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("no type")
            ),
        }
    }

    Ok(Judgment {
        answers: Answers::new(answers),
        usage: crate::llm::usage_wire::from_frame(&value),
        model: value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        served_model: None,
        request_chars: 0,
    })
}

#[async_trait]
impl JudgmentProvider for OpenAiDecisionsProvider {
    async fn ask(
        &self,
        state: Value,
        questions: Vec<(String, Question)>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
        if questions.is_empty() {
            return Ok(Judgment::default());
        }
        let body = build_request_body(&self.model, &state, &questions);
        let request_chars = body.to_string().chars().count();
        let response = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            // The body names the offending parameter and never the key, which
            // rides in the header.
            return Err(format!(
                "OpenAI Decisions returned {}: {}",
                status.as_u16(),
                text.trim()
            )
            .into());
        }
        let mut judgment = parse_response(&text, &questions)?;
        judgment.request_chars = request_chars;
        judgment.model.get_or_insert_with(|| self.model.clone());
        Ok(judgment)
    }
}

#[cfg(test)]
#[path = "openai_decisions_tests.rs"]
mod tests;
