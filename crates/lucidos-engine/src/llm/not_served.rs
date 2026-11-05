//! A *not-served model*: the provider answered that it has no such model. It
//! was retired, never enabled for the project, or misspelled.
//!
//! The error is typed because callers branch on it: an auxiliary call moves an
//! unset default to its next recommended model, and the command judge names
//! the refusal on its card. Decision: ADR 0403.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use crate::llm::model_registry::ProviderKind;

/// How long a not-found answer keeps a model out of reach. After it, the next
/// call tries the model again, so enabling it in the provider's console takes
/// effect with no restart.
pub const NOT_SERVED_WINDOW: Duration = Duration::from_secs(6 * 60 * 60);

/// A provider's answer that it does not serve the requested model. Its text is
/// the provider's own failure message, rewritten into advice where we have one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelNotServed {
    message: String,
    refused_by: Option<(ProviderKind, String)>,
}

impl ModelNotServed {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            refused_by: None,
        }
    }

    /// Name the provider and the registry model that refused. The router
    /// stamps this, because only it knows which model it actually sent.
    pub(crate) fn refused_by(mut self, provider: ProviderKind, model: impl Into<String>) -> Self {
        self.refused_by = Some((provider, model.into()));
        self
    }

    /// The provider and model that refused, when the router stamped them.
    pub fn refusal(&self) -> Option<(ProviderKind, &str)> {
        self.refused_by
            .as_ref()
            .map(|(provider, model)| (*provider, model.as_str()))
    }
}

impl fmt::Display for ModelNotServed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ModelNotServed {}

/// The `(provider, model)` pairs that answered not-found within
/// [`NOT_SERVED_WINDOW`]. One shared ledger is what the router, default
/// selection and Settings read, so they resolve the same model.
///
/// It lives in memory. The engine seeds it at boot from the
/// `ModelNotServedObserved` events, so the window survives a restart.
#[derive(Clone, Debug, Default)]
pub struct NotServedLedger(Arc<RwLock<HashMap<(ProviderKind, String), SystemTime>>>);

impl NotServedLedger {
    /// Record a not-found answer from `provider` for `model`, now.
    pub fn mark(&self, provider: ProviderKind, model: &str) {
        self.seed(provider, model, Duration::ZERO);
    }

    /// Record a not-found answer observed `age` ago. An answer older than the
    /// window is dropped.
    pub fn seed(&self, provider: ProviderKind, model: &str, age: Duration) {
        let Some(at) = SystemTime::now()
            .checked_sub(age)
            .filter(|_| age < NOT_SERVED_WINDOW)
        else {
            return;
        };
        if let Ok(mut map) = self.0.write() {
            let entry = map.entry((provider, model.to_string())).or_insert(at);
            *entry = (*entry).max(at);
        }
    }

    /// Whether `provider` answered not-found for `model` within the window.
    pub fn refused(&self, provider: ProviderKind, model: &str) -> bool {
        self.0
            .read()
            .ok()
            .and_then(|map| map.get(&(provider, model.to_string())).copied())
            .is_some_and(within_window)
    }

    /// The pairs inside the window, as one consistent view.
    pub fn snapshot(&self) -> Vec<(ProviderKind, String)> {
        self.0
            .read()
            .map(|map| {
                map.iter()
                    .filter(|(_, at)| within_window(**at))
                    .map(|(pair, _)| pair.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Whether an answer observed `at` is still inside [`NOT_SERVED_WINDOW`].
/// Wall-clock time, so the window keeps running while the host sleeps. A
/// clock stepped back reads as just observed.
fn within_window(at: SystemTime) -> bool {
    at.elapsed().unwrap_or_default() < NOT_SERVED_WINDOW
}

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

    /// The stamp names the refusing route, and the message stays the
    /// provider's own.
    #[test]
    fn the_stamp_names_who_refused() {
        let refusal = ModelNotServed::new("Vertex has no `claude-haiku-5-5`")
            .refused_by(ProviderKind::Vertex, "claude-haiku-5-5");
        assert_eq!(
            refusal.refusal(),
            Some((ProviderKind::Vertex, "claude-haiku-5-5"))
        );
        assert_eq!(refusal.to_string(), "Vertex has no `claude-haiku-5-5`");
        assert_eq!(ModelNotServed::new("x").refusal(), None);
    }

    /// The pair is the provider AND the model, and an answer older than the
    /// window does not count.
    #[test]
    fn the_ledger_holds_pairs_inside_the_window() {
        let ledger = NotServedLedger::default();
        ledger.mark(ProviderKind::Vertex, "claude-haiku-5-5");
        ledger.seed(
            ProviderKind::Anthropic,
            "claude-haiku-4-5",
            NOT_SERVED_WINDOW + Duration::from_secs(60),
        );
        ledger.seed(
            ProviderKind::Vertex,
            "gemini-3.8-flash",
            Duration::from_secs(60),
        );
        assert!(ledger.refused(ProviderKind::Vertex, "claude-haiku-5-5"));
        assert!(!ledger.refused(ProviderKind::Anthropic, "claude-haiku-5-5"));
        assert!(!ledger.refused(ProviderKind::Anthropic, "claude-haiku-4-5"));
        assert!(ledger.refused(ProviderKind::Vertex, "gemini-3.8-flash"));
        let mut pairs = ledger.snapshot();
        pairs.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(
            pairs,
            vec![
                (ProviderKind::Vertex, "claude-haiku-5-5".to_string()),
                (ProviderKind::Vertex, "gemini-3.8-flash".to_string()),
            ]
        );
    }
}
