//! A System One endpoint, which answers the typed questions in [`super`].
//!
//! One `POST` carries the state, the model and every question. TypeSafe's Jev,
//! Cloudflare's Clef and a self-hosted model all speak this body, so they
//! differ only in the URL, the model name and the key. [`super::endpoint`]
//! holds those three per row.
//!
//! The body building and the response parse are separate pure functions, so
//! the wire contract is tested without a server.
//!
//! **No retry.** A site on a System One endpoint falls back to its chat model
//! when this call fails. A retry inside a blocking path costs the user more
//! than that fallback does, so a `429` or `529` surfaces as `Err`.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Answer, Answers, Judgment, JudgmentProvider, JudgmentUsage, Question};

/// TypeSafe's API root. The builtin `typesafe` proxy pins it too.
pub const TYPESAFE_API_BASE_URL: &str = "https://api.typesafe.ai/v1";

/// The alias that always names TypeSafe's current flagship model.
pub const JEV_DEFAULT_MODEL: &str = "jev-latest";

/// A judgment provider backed by one System One endpoint.
pub struct SystemOneProvider {
    /// The full request URL, path included.
    url: String,
    /// The model the body asks for.
    model: String,
    /// Never logged and never formatted into an error. It reaches exactly one
    /// place, the `Authorization` header. `None` for a self-hosted endpoint
    /// that takes no key.
    api_key: Option<String>,
    client: reqwest::Client,
}

impl SystemOneProvider {
    pub fn new(
        url: String,
        model: String,
        api_key: Option<String>,
        timeout: Duration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            url,
            model,
            api_key,
            client,
        })
    }

    /// Whether a key rides in the `Authorization` header, without exposing it.
    #[cfg(test)]
    pub(crate) fn sends_a_key(&self) -> bool {
        self.api_key.is_some()
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
/// Cloudflare's REST API may wrap the body in a `result` envelope, so an
/// `answers` object found there is read the same way.
///
/// `request_chars` is left at zero here, because a response body cannot say how
/// big the request was. [`SystemOneProvider::ask`] fills it from what it sent.
pub(crate) fn parse_response(
    body: &str,
) -> Result<Judgment, Box<dyn std::error::Error + Send + Sync>> {
    let outer: Value = serde_json::from_str(body)?;
    let value = match outer.get("result") {
        Some(inner) if inner.get("answers").is_some() => inner,
        _ => &outer,
    };
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
            ..JudgmentUsage::default()
        },
        model: value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        request_chars: 0,
    })
}

#[async_trait]
impl JudgmentProvider for SystemOneProvider {
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
        let mut request = self.client.post(&self.url).json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await?;

        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            // The body describes the offending field on a 422 and is safe to
            // surface. The key is in the header, never here.
            return Err(format!(
                "System One endpoint returned {}: {}",
                status.as_u16(),
                text.trim()
            )
            .into());
        }
        let mut judgment = parse_response(&text)?;
        judgment.request_chars = request_chars;
        // A response naming no model was answered by the one we asked for.
        judgment.model.get_or_insert_with(|| self.model.clone());
        Ok(judgment)
    }
}

#[cfg(test)]
#[path = "system_one_tests.rs"]
mod tests;
