//! A *not-served model*: the provider answered that it has no such model. It
//! was retired, never enabled for the project, or misspelled.
//!
//! The error is typed because callers branch on it: an auxiliary call moves an
//! unset default to its next recommended model, and the command judge names
//! the refusal on its card. Decision: ADR 0403.

use std::fmt;

/// A provider's answer that it does not serve the requested model. Its text is
/// the provider's own failure message, rewritten into advice where we have one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelNotServed(String);

impl ModelNotServed {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ModelNotServed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ModelNotServed {}

/// The phrases a 404 body carries when the missing thing is the model. Each
/// is paired with the word "model", so a 404 for a wrong path never matches.
const UNKNOWN_MODEL_PHRASES: &[&str] = &[
    // Anthropic `not_found_error`, OpenAI `model_not_found`.
    "not_found",
    // Ollama, LM Studio.
    "not found",
    // OpenAI, vLLM.
    "does not exist",
];

/// OpenRouter's 404 once no upstream serves the model. It names the model by
/// id alone, so it carries no "model" to pair with.
const NO_ENDPOINTS: &str = "no endpoints found for";

/// Whether a failed answer from the direct Anthropic API or an
/// OpenAI-compatible backend says the model does not exist there.
///
/// A rate limit, an auth failure or a 404 for anything else is not one. Reading
/// those as "not served" would hide a model the user can still reach.
fn names_unknown_model(status: u16, body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    match status {
        404 => {
            lower.contains(NO_ENDPOINTS)
                || (lower.contains("model")
                    && UNKNOWN_MODEL_PHRASES.iter().any(|p| lower.contains(p)))
        }
        // OpenRouter answers an id it has never listed with a 400.
        400 => lower.contains("is not a valid model id"),
        _ => false,
    }
}

/// The error for a failed answer from the direct Anthropic API or an
/// OpenAI-compatible backend: [`ModelNotServed`] when it names an unknown
/// model, else `message` as is.
pub(crate) fn failure(
    status: u16,
    body: &str,
    message: String,
) -> Box<dyn std::error::Error + Send + Sync> {
    match names_unknown_model(status, body) {
        true => Box::new(ModelNotServed::new(message)),
        false => message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bodies recorded from each backend's answer for a model it lacks. These
    /// pin wire shapes, so the literals are the contract.
    #[test]
    fn each_backends_unknown_model_answer_classifies() {
        let cases = [
            (
                404,
                r#"{"type":"error","error":{"type":"not_found_error","message":"model: claude-haiku-4-5"}}"#,
            ),
            (
                404,
                r#"{"error":{"message":"The model `gpt-9` does not exist or you do not have access to it.","type":"invalid_request_error","param":null,"code":"model_not_found"}}"#,
            ),
            (
                404,
                r#"{"error":{"message":"No endpoints found for anthropic/claude-haiku-4.5.","code":404}}"#,
            ),
            (
                400,
                r#"{"error":{"message":"anthropic/claude-nope is not a valid model ID","code":400}}"#,
            ),
            (
                404,
                r#"{"error":{"message":"model \"llama9\" not found, try pulling it first"}}"#,
            ),
        ];
        for (status, body) in cases {
            assert!(names_unknown_model(status, body), "{status}: {body}");
        }
    }

    /// A credential, quota or path problem keeps the model in play.
    #[test]
    fn other_failures_do_not_classify() {
        let cases = [
            (404, r#"{"error":"Not Found"}"#),
            (404, "404 page not found"),
            (
                401,
                r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
            ),
            (
                403,
                r#"{"error":{"message":"model access denied for this key","code":403}}"#,
            ),
            (
                429,
                r#"{"type":"error","error":{"type":"rate_limit_error","message":"model rate limit not found"}}"#,
            ),
            (500, r#"{"error":{"message":"model server error"}}"#),
            (
                400,
                r#"{"error":{"message":"max_tokens: too large for model"}}"#,
            ),
        ];
        for (status, body) in cases {
            assert!(!names_unknown_model(status, body), "{status}: {body}");
        }
    }

    /// The marker survives the boxing every provider returns through.
    #[test]
    fn a_boxed_refusal_still_reads_as_not_served() {
        let boxed: Box<dyn std::error::Error + Send + Sync> =
            Box::new(ModelNotServed::new("Vertex has no `x`"));
        assert!(boxed.is::<ModelNotServed>());
        assert_eq!(boxed.to_string(), "Vertex has no `x`");
        let other: Box<dyn std::error::Error + Send + Sync> = "Claude API error (404)".into();
        assert!(!other.is::<ModelNotServed>());
    }
}
