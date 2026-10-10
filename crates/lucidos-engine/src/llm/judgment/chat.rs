//! A chat model answering the typed questions in [`super`] (ADR 0363).
//!
//! The questions travel as JSON, and the answer comes back through one tool
//! whose schema the questions define. That is the structured output every
//! provider here supports, and it needs no per-site prompt.
//!
//! **Rust reads the answer, not the model.** A Choice comes back as one number
//! per option, and [`read_choice`] normalises them into a distribution. An
//! answer that is missing, mistyped or out of range is dropped, never guessed.
//! Each site then applies its own threshold to what survived, and reads a
//! missing answer as its safe default. For the command guard that is
//! `IrreversibleDanger` (I13).
//!
//! **An unreadable reply is not an error.** Only a failed call is. A reply was
//! paid for, so the caller records its cost and reads the empty answer set.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use super::{
    Answer, Answers, ChoiceAnswer, Judgment, JudgmentProvider, JudgmentUsage, NoulAnswer, Question,
};
use crate::llm::provider::{LlmProvider, LlmResponse, Message, MessageContent, ToolDefinition};

/// The one tool the model answers through.
pub(crate) const ANSWER_TOOL: &str = "submit_answers";

/// The whole instruction. Every site's meaning rides in its questions, so this
/// text never changes per site.
const SYSTEM_PROMPT: &str = "You answer typed questions about a state, for software that \
reads your answers as numbers. You never write prose.

The user message holds a JSON `state` and a JSON object of `questions`, keyed by id. Each \
question carries `instructions` and `criteria`:
- A `noul` question asks whether a condition holds. Answer with the probability, from 0 to 1, \
that it does. `criteria.true` and `criteria.false` say what each side means.
- A `choice` question picks one option. `criteria` maps each option name to what it means. \
Answer with a probability from 0 to 1 for every option, summing to 1.

Report honest uncertainty. Spread probability across options you cannot tell apart rather \
than forcing one.

Call the `submit_answers` tool exactly once, with an answer for every question id. Do not \
answer in text.";

/// A judgment provider backed by a chat model.
pub struct ChatJudgmentProvider {
    provider: Arc<dyn LlmProvider>,
    /// The effort half of the site's *model selection*.
    reasoning: Option<String>,
}

impl ChatJudgmentProvider {
    pub fn new(provider: Arc<dyn LlmProvider>, reasoning: Option<String>) -> Self {
        Self {
            provider,
            reasoning,
        }
    }
}

/// The user message: the state, then the questions in the System One shape.
pub(crate) fn user_message(state: &Value, questions: &[(String, Question)]) -> String {
    let questions: Map<String, Value> = questions
        .iter()
        .map(|(id, q)| (id.clone(), serde_json::to_value(q).unwrap_or(Value::Null)))
        .collect();
    format!(
        "State:\n{}\n\nQuestions:\n{}",
        serde_json::to_string_pretty(state).unwrap_or_default(),
        serde_json::to_string_pretty(&Value::Object(questions)).unwrap_or_default(),
    )
}

/// The answer tool, its schema built from the questions so the model sees
/// every id and every option name it must answer.
pub(crate) fn answer_tool(questions: &[(String, Question)]) -> ToolDefinition {
    let mut properties = Map::with_capacity(questions.len());
    for (id, question) in questions {
        let schema = match question {
            Question::Noul { .. } => json!({
                "type": "number",
                "description": "Probability from 0 to 1 that the condition holds.",
            }),
            Question::Choice { criteria, .. } => {
                let options: Map<String, Value> = criteria
                    .iter()
                    .map(|(name, _)| (name.clone(), json!({ "type": "number" })))
                    .collect();
                let names: Vec<&String> = criteria.iter().map(|(name, _)| name).collect();
                json!({
                    "type": "object",
                    "description": "Probability from 0 to 1 for every option, summing to 1.",
                    "properties": options,
                    "required": names,
                })
            }
        };
        properties.insert(id.clone(), schema);
    }
    let ids: Vec<&String> = questions.iter().map(|(id, _)| id).collect();
    ToolDefinition {
        name: ANSWER_TOOL.to_string(),
        description: "Submit an answer for every question.".to_string(),
        parameters: json!({ "type": "object", "properties": properties, "required": ids }),
    }
}

/// The answers in one reply, keeping only those that read cleanly.
pub(crate) fn read_answers(response: &LlmResponse, questions: &[(String, Question)]) -> Answers {
    let Some(object) = reply_object(response) else {
        return Answers::default();
    };
    let mut answers = HashMap::with_capacity(questions.len());
    for (id, question) in questions {
        let read = object.get(id).and_then(|value| match question {
            Question::Noul { .. } => read_noul(value).map(Answer::Noul),
            Question::Choice { criteria, .. } => read_choice(criteria, value).map(Answer::Choice),
        });
        match read {
            Some(answer) => {
                answers.insert(id.clone(), answer);
            }
            None => log!(
                "[Judgment] The chat model gave no readable answer to {}",
                id
            ),
        }
    }
    Answers::new(answers)
}

/// The JSON object the model answered with: the tool call's arguments, or a
/// bare object in the text when the model ignored the tool.
fn reply_object(response: &LlmResponse) -> Option<Map<String, Value>> {
    if let Some(call) = response.tool_calls.iter().find(|c| c.name == ANSWER_TOOL) {
        return match &call.arguments {
            Value::Object(map) => Some(map.clone()),
            Value::String(text) => object_in_text(text),
            _ => None,
        };
    }
    object_in_text(response.content.as_deref().unwrap_or(""))
}

/// The outermost `{...}` in a text reply, fences and prose around it ignored.
fn object_in_text(text: &str) -> Option<Map<String, Value>> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    match serde_json::from_str(&text[start..=end]) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// A probability of yes, or `None` for anything outside 0 to 1.
fn read_noul(value: &Value) -> Option<NoulAnswer> {
    let p = value.as_f64()?;
    (p.is_finite() && (0.0..=1.0).contains(&p)).then_some(NoulAnswer { noul: p })
}

/// One number per declared option, normalised to sum to 1.
///
/// An option the reply left out reads as zero, and a name it invented is
/// ignored. A negative or non-numeric value, or no weight at all, drops the
/// whole answer: nothing is inferred from a reply that broke the schema.
pub(crate) fn read_choice(
    criteria: &[(String, Option<String>)],
    value: &Value,
) -> Option<ChoiceAnswer> {
    let given = value.as_object()?;
    let mut weights = Vec::with_capacity(criteria.len());
    for (name, _) in criteria {
        let weight = match given.get(name) {
            None => 0.0,
            Some(v) => v.as_f64().filter(|w| w.is_finite() && *w >= 0.0)?,
        };
        weights.push((name.clone(), weight));
    }
    let total: f64 = weights.iter().map(|(_, w)| w).sum();
    if total <= 0.0 {
        return None;
    }
    let probabilities: HashMap<String, f64> = weights
        .iter()
        .map(|(name, w)| (name.clone(), w / total))
        .collect();
    // Ties go to the option declared first, so the pick is deterministic.
    let mut choice = &weights[0];
    for candidate in &weights[1..] {
        if candidate.1 > choice.1 {
            choice = candidate;
        }
    }
    Some(ChoiceAnswer {
        choice: choice.0.clone(),
        confidence: concentration(probabilities.values().copied(), criteria.len()),
        probabilities,
    })
}

/// How concentrated a distribution is, from 0 (uniform) to 1 (certain): one
/// minus its entropy over the entropy of a uniform one.
fn concentration(probabilities: impl Iterator<Item = f64>, options: usize) -> f64 {
    if options < 2 {
        return 1.0;
    }
    let entropy: f64 = probabilities
        .filter(|p| *p > 0.0)
        .map(|p| -p * p.ln())
        .sum();
    (1.0 - entropy / (options as f64).ln()).clamp(0.0, 1.0)
}

#[async_trait]
impl JudgmentProvider for ChatJudgmentProvider {
    async fn ask(
        &self,
        state: Value,
        questions: Vec<(String, Question)>,
        call: crate::llm::metered::CallToken,
    ) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
        if questions.is_empty() {
            return Ok(Judgment::default());
        }
        let message = user_message(&state, &questions);
        let tool = answer_tool(&questions);
        let request_chars = SYSTEM_PROMPT.chars().count()
            + message.chars().count()
            + tool.parameters.to_string().chars().count();
        let response = self
            .provider
            .chat(
                vec![Message {
                    role: "user".to_string(),
                    content: MessageContent::Text(message),
                }],
                vec![tool],
                crate::llm::ModelSelection::default().with_effort(self.reasoning.as_deref()),
                Some(SYSTEM_PROMPT),
                None,
                call,
            )
            .await?;
        let model = self.provider.default_model();
        Ok(Judgment {
            answers: read_answers(&response, &questions),
            usage: JudgmentUsage {
                input_tokens: response.input_tokens.unwrap_or(0),
                output_tokens: response.output_tokens.unwrap_or(0),
                cache_read_tokens: response.cache_read_tokens.unwrap_or(0),
                cache_creation_tokens: response.cache_creation_tokens.unwrap_or(0),
            },
            model: (!model.is_empty()).then(|| model.to_string()),
            request_chars,
        })
    }
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
