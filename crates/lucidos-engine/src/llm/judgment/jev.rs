//! TypeSafe's System One endpoint, which answers the typed questions in
//! [`super`].
//!
//! One `POST /v1/systemone` carries the state, the model and every question.
//! The body building and the response parse are separate pure functions, so
//! the wire contract is tested without a server.
//!
//! **No retry.** Both callers already hold a fallback, and a retry inside a
//! blocking path costs the user more than falling back does. A `429` or `529`
//! surfaces as `Err`, and the caller takes its existing path.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Answer, Answers, Judgment, JudgmentProvider, JudgmentUsage, Question};

/// The System One API root.
pub const TYPESAFE_API_BASE_URL: &str = "https://api.typesafe.ai/v1";

/// The alias that always names TypeSafe's current flagship model.
pub const JEV_DEFAULT_MODEL: &str = "jev-latest";

/// A judgment provider backed by TypeSafe.
///
/// The endpoint and the model are constants rather than fields. Nothing sets
/// either, so a field would only be a constant with extra steps. Pinning a
/// model version is a preference to add when someone wants one.
pub struct JevProvider {
    /// Never logged and never formatted into an error. It reaches exactly one
    /// place, the `Authorization` header.
    api_key: String,
    client: reqwest::Client,
}

impl JevProvider {
    pub fn new(
        api_key: String,
        timeout: Duration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self { api_key, client })
    }
}

/// Build the request body for one call.
///
/// Pure, so a test can assert the wire shape and that a redacted state stayed
/// redacted, without standing up a server.
pub(crate) fn build_request_body(
    model: &str,
    state: &Value,
    questions: &[(String, Question)],
) -> Value {
    let mut map = serde_json::Map::with_capacity(questions.len());
    for (id, question) in questions {
        map.insert(
            id.clone(),
            serde_json::to_value(question).unwrap_or(Value::Null),
        );
    }
    json!({ "state": state, "model": model, "questions": Value::Object(map) })
}

/// Parse one response body into the answers, what they cost, and what answered.
///
/// An answer whose type the enum does not recognize is dropped rather than
/// failing the whole call. The caller reads a missing id the same way it reads
/// a transport failure, so one unreadable answer never decides anything.
///
/// `request_chars` is left at zero here, because a response body cannot say how
/// big the request was. [`JevProvider::ask`] fills it from what it sent.
pub(crate) fn parse_response(
    body: &str,
) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
    let value: Value = serde_json::from_str(body)?;
    let raw = value
        .get("answers")
        .and_then(Value::as_object)
        .ok_or("judgment response carried no answers object")?;

    let mut answers = HashMap::with_capacity(raw.len());
    for (id, answer) in raw {
        match serde_json::from_value::<Answer>(answer.clone()) {
            Ok(parsed) => {
                answers.insert(id.clone(), parsed);
            }
            Err(e) => log!("[Judgment] Dropping unreadable answer {}: {}", id, e),
        }
    }

    let usage = value.get("usage");
    let count = |key: &str| -> u32 {
        usage
            .and_then(|u| u.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32
    };
    Ok(Judgment {
        answers: Answers::new(answers),
        usage: JudgmentUsage {
            input_tokens: count("input_tokens"),
            output_tokens: count("output_tokens"),
        },
        model: value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        request_chars: 0,
    })
}

#[async_trait]
impl JudgmentProvider for JevProvider {
    async fn ask(
        &self,
        state: Value,
        questions: Vec<(String, Question)>,
    ) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
        if questions.is_empty() {
            return Ok(Judgment::default());
        }
        let body = build_request_body(JEV_DEFAULT_MODEL, &state, &questions);
        let request_chars = body.to_string().chars().count();
        let response = self
            .client
            .post(format!("{TYPESAFE_API_BASE_URL}/systemone"))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            // The body describes the offending field on a 422 and is safe to
            // surface. The key is in the header, never here.
            return Err(format!("TypeSafe returned {}: {}", status.as_u16(), text.trim()).into());
        }
        let mut judgment = parse_response(&text)?;
        judgment.request_chars = request_chars;
        Ok(judgment)
    }
}

#[cfg(test)]
#[path = "jev_tests.rs"]
mod tests;
