use crate::llm::provider::{LlmProvider, LlmResponse, Message, TokenCallback, ToolDefinition};
use async_trait::async_trait;

/// The clear, actionable error a no-provider chat returns. Surfaced to the user
/// as a `ThreadEvent::ResponseFailed` (see `engine/agentic_loop/run.rs`), so the
/// wording is user-facing. Must never be mistaken for real or mock output.
///
/// No "restart" instruction: configuring a provider in Settings → Models →
/// Providers now takes effect at runtime (the config subscriber hot-swaps the
/// active provider, see `spawn_provider_config_subscriber`).
pub const NO_PROVIDER_MESSAGE: &str =
    "No LLM provider configured. Add one in Settings → Models → Providers.";

/// Sentinel LLM provider for the "booted before any provider is configured"
/// state.
///
/// A packaged desktop build sets `LUCIDOS_BOOT_WITHOUT_PROVIDER=1` so the engine
/// still boots on first run — before the user has entered any provider
/// credential — instead of crashing the `.app` on launch. When no real provider
/// (Vertex / OpenAI / Anthropic / OpenRouter / xAI / local) is configured, this
/// provider is installed instead of the deterministic `MockProvider`: it lets
/// the engine boot but returns a clear, actionable error on every chat attempt,
/// so a shipped release never serves mock output to a real user.
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
        _model_override: Option<&str>,
        _system_prompt: Option<&str>,
        _on_token: Option<TokenCallback>,
        _reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        Err(NO_PROVIDER_MESSAGE.into())
    }

    fn default_model(&self) -> &str {
        "unconfigured"
    }

    fn is_configured(&self) -> bool {
        false
    }

    /// No provider is configured — report an empty set (not `None`) so the model
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
        let result = provider.chat(vec![], vec![], None, None, None, None).await;
        let err = result.expect_err("unconfigured provider must error, never return content");
        let msg = err.to_string();
        // Clear + actionable: names the problem AND where to fix it.
        assert!(
            msg.contains("No LLM provider configured"),
            "error must name the problem: {msg}"
        );
        assert!(
            msg.contains("Settings → Models → Providers"),
            "error must point to the fix: {msg}"
        );
        // Must NEVER be the mock pangram — that is the entire bug this fixes.
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
