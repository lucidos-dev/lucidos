use crate::llm::provider::{
    LlmProvider, LlmResponse, Message, ModelSelection, TokenCallback, ToolDefinition,
};
use async_trait::async_trait;

pub const NO_PROVIDER_MESSAGE: &str = concat!(
    "No LLM provider configured. Add one in ",
    providers_settings!(),
    "."
);

/// INVARIANT: Sentinel LLM provider for the "booted before any provider is configured"
/// state.
///
/// Every launch mode boots before the user has entered any provider credential.
/// This provider keeps Settings reachable and returns an actionable chat error.
/// It never serves deterministic mock output to a real user.
///
/// It reports `is_configured() == false`, which `/health` exposes as
/// `llm_configured: false` so the frontend shows first-run provider onboarding.
///
/// This is distinct from `MockProvider`, which stays reachable ONLY via the
/// explicit `LUCIDOS_MODEL=mock` opt-in (E2E) and reports `is_configured()`.
pub struct UnconfiguredProvider;

impl UnconfiguredProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UnconfiguredProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmProvider for UnconfiguredProvider {
    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _selection: ModelSelection<'_>,
        _system_prompt: Option<&str>,
        _on_token: Option<TokenCallback>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        Err(NO_PROVIDER_MESSAGE.into())
    }

    fn default_model(&self) -> &str {
        "unconfigured"
    }

    fn is_configured(&self) -> bool {
        false
    }

    /// INVARIANT: No provider is configured — report an empty set (not `None`) so the model
    /// picker filters to nothing rather than skipping the filter.
    fn configured_providers(&self) -> Option<Vec<crate::llm::model_registry::ProviderKind>> {
        Some(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_unconfigured() {
        assert!(!UnconfiguredProvider::new().is_configured());
    }

    #[tokio::test]
    async fn chat_returns_clear_actionable_error_never_mock() {
        let provider = UnconfiguredProvider::new();
        let result = provider
            .chat(
                vec![],
                vec![],
                ModelSelection::default(),
                None,
                None,
                crate::llm::metered::CallToken::for_test(),
            )
            .await;
        let err = result.expect_err("unconfigured provider must error, never return content");
        let msg = err.to_string();
        assert!(
            msg.contains("No LLM provider configured"),
            "error must name the problem: {msg}"
        );
        assert!(
            msg.contains(providers_settings!()),
            "error must point to the fix: {msg}"
        );
        assert!(
            !msg.contains(crate::llm::mock::MOCK_RESPONSE),
            "unconfigured error must never carry mock output"
        );
        assert!(
            !msg.to_lowercase().contains("quick brown fox"),
            "unconfigured error must never carry the pangram"
        );
    }
}
