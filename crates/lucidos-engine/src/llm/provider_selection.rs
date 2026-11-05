//! Pure decision for which LLM provider the engine installs at startup.
//!
//! Extracted from `main.rs` so the branch logic is unit-testable without
//! standing up real providers or a database. `main.rs` resolves the booleans
//! (which credentials/projects exist, the env flags) and maps the returned
//! [`ProviderSelection`] onto an actual `Arc<dyn LlmProvider>`.

/// Inputs to the provider-selection decision. All real-provider availability is
/// collapsed to booleans the caller resolves from env + DB.
#[derive(Debug, Clone, Copy)]
pub struct ProviderSelectionInputs {
    /// `LUCIDOS_MODEL == "mock"` — the explicit E2E opt-in.
    pub model_is_mock: bool,
    /// A Vertex project id is configured.
    pub has_vertex: bool,
    /// A direct OpenAI provider is configured (stored credential or env key).
    pub has_openai: bool,
    /// A direct Anthropic provider is configured (stored credential or env key).
    pub has_anthropic: bool,
    /// An OpenRouter provider is configured.
    pub has_openrouter: bool,
    /// A direct xAI provider is configured (stored credential or env key).
    pub has_xai: bool,
    /// The keyless OpenCode Free tier is switched on. The one input carrying no
    /// credential: it comes from the `opencode_free_enabled` preference or its
    /// env fallback.
    pub has_opencode_free: bool,
    /// A local OpenAI-compatible provider (base URL) is configured.
    pub has_local: bool,
    /// Lucidos is signed in to the configured Claude gateway.
    pub has_claude_gateway: bool,
}

impl ProviderSelectionInputs {
    fn has_real_provider(&self) -> bool {
        self.has_vertex
            || self.has_openai
            || self.has_anthropic
            || self.has_openrouter
            || self.has_xai
            || self.has_opencode_free
            || self.has_local
            || self.has_claude_gateway
    }
}

/// The provider the engine should install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSelection {
    /// Deterministic `MockProvider` — explicit `LUCIDOS_MODEL=mock` (E2E).
    Mock,
    /// At least one real provider is configured → `RoutingProvider`.
    Real,
    /// No real provider →
    /// `UnconfiguredProvider` (boots, clear error on chat, drives onboarding).
    Unconfigured,
}

/// Decide which provider to install. `mock` wins outright (it is the explicit
/// test opt-in); otherwise a real provider is preferred; otherwise the
/// engine boots unconfigured so Settings remains reachable.
pub fn select_provider(inputs: ProviderSelectionInputs) -> ProviderSelection {
    if inputs.model_is_mock {
        return ProviderSelection::Mock;
    }
    if inputs.has_real_provider() {
        return ProviderSelection::Real;
    }
    ProviderSelection::Unconfigured
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Base case: nothing configured.
    fn none() -> ProviderSelectionInputs {
        ProviderSelectionInputs {
            model_is_mock: false,
            has_vertex: false,
            has_openai: false,
            has_anthropic: false,
            has_openrouter: false,
            has_xai: false,
            has_opencode_free: false,
            has_local: false,
            has_claude_gateway: false,
        }
    }

    #[test]
    fn mock_wins_outright() {
        // Explicit LUCIDOS_MODEL=mock selects Mock regardless of everything else.
        let i = ProviderSelectionInputs {
            model_is_mock: true,
            has_vertex: true,
            ..none()
        };
        assert_eq!(select_provider(i), ProviderSelection::Mock);

        // Even with no real provider (the E2E shape).
        assert_eq!(
            select_provider(ProviderSelectionInputs {
                model_is_mock: true,
                ..none()
            }),
            ProviderSelection::Mock
        );
    }

    #[test]
    fn any_real_provider_selects_real() {
        for set in [
            |i: &mut ProviderSelectionInputs| i.has_vertex = true,
            |i: &mut ProviderSelectionInputs| i.has_openai = true,
            |i: &mut ProviderSelectionInputs| i.has_anthropic = true,
            |i: &mut ProviderSelectionInputs| i.has_openrouter = true,
            |i: &mut ProviderSelectionInputs| i.has_xai = true,
            |i: &mut ProviderSelectionInputs| i.has_opencode_free = true,
            |i: &mut ProviderSelectionInputs| i.has_local = true,
            |i: &mut ProviderSelectionInputs| i.has_claude_gateway = true,
        ] {
            let mut i = none();
            set(&mut i);
            assert_eq!(select_provider(i), ProviderSelection::Real);
        }
    }

    #[test]
    fn no_provider_boots_unconfigured_not_mock() {
        // No real provider, no mock opt-in.
        let i = ProviderSelectionInputs { ..none() };
        let sel = select_provider(i);
        assert_eq!(sel, ProviderSelection::Unconfigured);
        // The whole point of the fix: this must NOT pick Mock.
        assert_ne!(sel, ProviderSelection::Mock);
    }

    #[test]
    fn no_provider_without_gate_boots_unconfigured() {
        // Source launches must reach Settings without a provider opt-in.
        assert_eq!(select_provider(none()), ProviderSelection::Unconfigured);
    }
}
