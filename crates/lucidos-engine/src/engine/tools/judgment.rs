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
use crate::engine::aux_purpose::budget_for;
use crate::engine::{AuxCapture, ContextPurpose};
use crate::llm::judgment::{jev_for_agent, JudgmentProvider, Question, JEV_DEFAULT_MODEL};

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

/// Ask one judgment, record what it cost, and render the agent's result.
///
/// Split from the handler for the reason `command_judge` splits its own core:
/// resolving a provider needs a key and a server, so only a split lets a stub
/// exercise the ask, the capture and the rendering offline.
///
/// **The deadline covers the provider call alone.** Around the whole function
/// it would cover the capture too. A call answering just inside the deadline
/// could then have its row cancelled mid-write, losing the accounting for a
/// call the user paid for.
pub(crate) async fn ask_and_render<J: JudgmentProvider + ?Sized>(
    provider: &J,
    state: Value,
    questions: Vec<(String, Question)>,
    deadline: Duration,
    capture: Option<&AuxCapture>,
) -> ToolOutcome {
    let asked = questions.len();
    let judgment = match tokio::time::timeout(deadline, provider.ask(state, questions)).await {
        Ok(Ok(judgment)) => judgment,
        Ok(Err(e)) => return Err(format!("Error: the judgment call failed: {}", e)),
        Err(_) => {
            return Err(format!(
                "Error: the judgment call did not answer within {:?}",
                deadline
            ))
        }
    };
    let model = judgment.model.as_deref().unwrap_or(JEV_DEFAULT_MODEL);

    if let Some(capture) = capture {
        capture.record_judgment(&judgment).await;
    }

    log!(
        "[Judgment] judge answered {}/{} questions on {} ({} in, {} out)",
        judgment.answers.len(),
        asked,
        model,
        judgment.usage.input_tokens,
        judgment.usage.output_tokens
    );

    // The count travels beside the answers so the model can see that one was
    // dropped. An unreadable answer is parsed out upstream, and a silently
    // short object reads as a judgment nobody made.
    let body = json!({
        "answers": judgment.answers,
        "asked": asked,
        "answered": judgment.answers.len(),
    });
    Ok(body.to_string())
}

impl LucidosEngine {
    pub(crate) async fn execute_judgment_tool(
        &self,
        args: &Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let Some(state) = args.get("state").filter(|v| v.is_object()) else {
            return Err("Error: 'state' must be an object".to_string());
        };
        let questions = parse_questions(args.get("questions"))?;

        // The budget is the purpose's, so the timeout a reader finds in
        // `aux_purpose` is the timeout this call runs under.
        let budget = budget_for(ContextPurpose::JudgeTool);
        let provider = match jev_for_agent(&self.pool, budget.attempt_timeout).await {
            Ok(provider) => provider,
            Err(reason) => return Err(format!("Error: {}", reason)),
        };
        let capture = AuxCapture::new(&self.event_bus, thread_id, ContextPurpose::JudgeTool);

        ask_and_render(
            &provider,
            state.clone(),
            questions,
            budget.deadline,
            Some(&capture),
        )
        .await
    }
}

#[cfg(test)]
#[path = "judgment_tests.rs"]
mod tests;
