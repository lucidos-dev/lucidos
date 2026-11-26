use std::time::Duration;

use serde_json::{json, Value};

use super::super::LucidosEngine;
use super::ToolOutcome;
use crate::engine::aux_purpose::budget_for;
use crate::engine::{AuxCapture, ContextPurpose};
use crate::llm::judgment::{jev_for_agent, JudgmentProvider, Question, JEV_DEFAULT_MODEL};

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

pub(crate) async fn ask_and_render<J: JudgmentProvider + ?Sized>(
    provider: &J,
    state: Value,
    questions: Vec<(String, Question)>,
    deadline: Duration,
    capture: &AuxCapture,
) -> ToolOutcome {
    let asked = questions.len();
    let bounded = capture.until(tokio::time::Instant::now() + deadline);
    let judgment = match bounded.judge(provider, state, questions).await {
        Ok(judgment) => judgment,
        Err(e) if e.is::<tokio::time::error::Elapsed>() => {
            return Err(format!(
                "Error: the judgment call did not answer within {:?}",
                deadline
            ))
        }
        Err(e) => return Err(format!("Error: the judgment call failed: {}", e)),
    };
    let model = judgment.model.as_deref().unwrap_or(JEV_DEFAULT_MODEL);

    log_legacy!(
        "[Judgment] judge answered {}/{} questions on {} ({} in, {} out)",
        judgment.answers.len(),
        asked,
        model,
        judgment.usage.unwrap_or_default().input_tokens,
        judgment.usage.unwrap_or_default().output_tokens
    );

    let body = json!({
        "answers": judgment.answers,
        "asked": asked,
        "answered": judgment.answers.len(),
    });
    Ok(body.to_string())
}

#[easy_ext::ext(JudgmentExt)]
pub(crate) impl LucidosEngine {
    async fn execute_judgment_tool(&self, args: &Value, thread_id: uuid::Uuid) -> ToolOutcome {
        let Some(state) = args.get("state").filter(|v| v.is_object()) else {
            return Err("Error: 'state' must be an object".to_string());
        };
        let questions = parse_questions(args.get("questions"))?;

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
            &capture,
        )
        .await
    }
}

#[cfg(test)]
#[path = "judgment_tests.rs"]
mod tests;
