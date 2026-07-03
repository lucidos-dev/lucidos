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
    /// A direct Anthropic provider is configured.
    pub has_anthropic: bool,
    /// An OpenRouter provider is configured.
    pub has_openrouter: bool,
    /// A local OpenAI-compatible provider (base URL) is configured.
    pub has_local: bool,
    /// `LUCIDOS_BOOT_WITHOUT_PROVIDER` is truthy — a packaged build lets the
    /// engine boot before any provider is configured (instead of crashing).
    pub boot_without_provider: bool,
}

impl ProviderSelectionInputs {
    fn has_real_provider(&self) -> bool {
        self.has_vertex
            || self.has_openai
            || self.has_anthropic
            || self.has_openrouter
            || self.has_local
    }
}

/// The provider the engine should install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSelection {
    /// Deterministic `MockProvider` — explicit `LUCIDOS_MODEL=mock` (E2E).
    Mock,
    /// At least one real provider is configured → `RoutingProvider`.
    Real,
    /// No real provider, but the build is allowed to boot without one →
    /// `UnconfiguredProvider` (boots, clear error on chat, drives onboarding).
    Unconfigured,
    /// No real provider and not allowed to boot without one → fail-fast panic
    /// (dev/docker default).
    FailFast,
}

/// Decide which provider to install. `mock` wins outright (it is the explicit
/// test opt-in); otherwise a real provider is preferred; otherwise the
/// boot-without-provider gate chooses between booting unconfigured and the
/// fail-fast panic.
pub fn select_provider(inputs: ProviderSelectionInputs) -> ProviderSelection {
    if inputs.model_is_mock {
        return ProviderSelection::Mock;
    }
    if inputs.has_real_provider() {
        return ProviderSelection::Real;
    }
    if inputs.boot_without_provider {
        ProviderSelection::Unconfigured
    } else {
        ProviderSelection::FailFast
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Base case: nothing configured, gate off.
    fn none() -> ProviderSelectionInputs {
        ProviderSelectionInputs {
            model_is_mock: false,
            has_vertex: false,
            has_openai: false,
            has_anthropic: false,
            has_openrouter: false,
            has_local: false,
            boot_without_provider: false,
        }
    }

    #[test]
    fn mock_wins_outright() {
        // Explicit LUCIDOS_MODEL=mock selects Mock regardless of everything else.
        let i = ProviderSelectionInputs {
            model_is_mock: true,
            has_vertex: true,
            boot_without_provider: true,
            ..none()
        };
        assert_eq!(select_provider(i), ProviderSelection::Mock);

        // Even with no real provider and the gate off (the E2E shape).
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
            |i: &mut ProviderSelectionInputs| i.has_local = true,
        ] {
            let mut i = none();
            set(&mut i);
            assert_eq!(select_provider(i), ProviderSelection::Real);
        }
    }

    #[test]
    fn no_provider_with_gate_boots_unconfigured_not_mock() {
        // The packaged-style config: no real provider, gate on, not mock.
        let i = ProviderSelectionInputs {
            boot_without_provider: true,
            ..none()
        };
        let sel = select_provider(i);
        assert_eq!(sel, ProviderSelection::Unconfigured);
        // The whole point of the fix: this must NOT pick Mock.
        assert_ne!(sel, ProviderSelection::Mock);
    }

    #[test]
    fn no_provider_without_gate_fails_fast() {
        // Dev/docker default: no real provider, gate off → fail-fast panic.
        assert_eq!(select_provider(none()), ProviderSelection::FailFast);
    }
}
