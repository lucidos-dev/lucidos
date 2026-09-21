//! The `judge` tool handler: the agent's own route to a typed judgment.
//!
//! **Nothing here decides anything.** It resolves the provider, forwards the
//! questions the agent wrote, and hands the answers back with their whole
//! probability distributions. The distribution is the reason the tool exists,
//! so returning only the chosen option would empty it out.
//!
//! **A failure is reported as a failure.** The two classification sites fall
//! through to their own prompt-and-parse path when Jev errors. This caller has
//! no other path, and inventing an answer would be a lie about a number, so the
//! error goes to the model verbatim.

use std::time::Duration;

use serde_json::{json, Value};

use super::super::LucidosEngine;
use super::ToolOutcome;
use crate::llm::judgment::{jev_for_agent, JudgmentProvider, Question, JEV_DEFAULT_MODEL};

/// How long one `judge` call may take.
///
/// A user is waiting on the turn, so it is bounded. It is generous because the
/// tool asks for batches: a call carrying a hundred questions is the shape the
/// schema pushes for, and it is the whole point of the tool.
const JUDGE_TIMEOUT: Duration = Duration::from_secs(60);

/// Read the `questions` argument into the wire type.
///
/// The ids are the agent's and the order is whatever the parsed object gives,
/// which is sorted: nothing downstream reads either as meaning anything.
fn parse_questions(value: Option<&Value>) -> Result<Vec<(String, Question)>, String> {
    let Some(map) = value.and_then(Value::as_object) else {
        return Err("Error: 'questions' must be an object of question id to question".to_string());
    };
    if map.is_empty() {
        return Err("Error: 'questions' is empty, so there is nothing to ask".to_string());
    }
    let mut questions = Vec::with_capacity(map.len());
    for (id, raw) in map {
        match serde_json::from_value::<Question>(raw.clone()) {
            Ok(question) => questions.push((id.clone(), question)),
            Err(e) => return Err(format!("Error: question '{}' is malformed: {}", id, e)),
        }
    }
    Ok(questions)
}

impl LucidosEngine {
    pub(crate) async fn execute_judgment_tool(&self, args: &Value) -> ToolOutcome {
        let Some(state) = args.get("state").filter(|v| v.is_object()) else {
            return Err("Error: 'state' must be an object".to_string());
        };
        let questions = parse_questions(args.get("questions"))?;

        let provider = match jev_for_agent(&self.pool, JUDGE_TIMEOUT).await {
            Ok(provider) => provider,
            Err(reason) => return Err(format!("Error: {}", reason)),
        };

        let asked = questions.len();
        let judgment = match provider.ask(state.clone(), questions).await {
            Ok(judgment) => judgment,
            Err(e) => return Err(format!("Error: the judgment call failed: {}", e)),
        };

        log!(
            "[Judgment] judge answered {}/{} questions on {} ({} in, {} out)",
            judgment.answers.len(),
            asked,
            JEV_DEFAULT_MODEL,
            judgment.usage.input_tokens,
            judgment.usage.output_tokens
        );

        // The count travels beside the answers so the model can see that one
        // was dropped. An unreadable answer is parsed out upstream, and a
        // silently short object reads as a judgment nobody made.
        let body = json!({
            "answers": judgment.answers,
            "asked": asked,
            "answered": judgment.answers.len(),
        });
        Ok(body.to_string())
    }
}

#[cfg(test)]
#[path = "judgment_tests.rs"]
mod tests;
