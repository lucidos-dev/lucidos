#[derive(Debug, Clone, Copy)]
pub struct ProviderSelectionInputs {
    pub model_is_mock: bool,
    pub has_vertex: bool,
    pub has_openai: bool,
    pub has_anthropic: bool,
    pub has_openrouter: bool,
    pub has_xai: bool,
    pub has_opencode_free: bool,
    pub has_local: bool,
    pub has_claude_gateway: bool,
    pub has_chatgpt_plan: bool,
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
            || self.has_chatgpt_plan
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSelection {
    Mock,
    Real,
    Unconfigured,
}

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
            has_chatgpt_plan: false,
        }
    }

    #[test]
    fn mock_wins_outright() {
        let i = ProviderSelectionInputs {
            model_is_mock: true,
            has_vertex: true,
            ..none()
        };
        assert_eq!(select_provider(i), ProviderSelection::Mock);

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
            |i: &mut ProviderSelectionInputs| i.has_chatgpt_plan = true,
        ] {
            let mut i = none();
            set(&mut i);
            assert_eq!(select_provider(i), ProviderSelection::Real);
        }
    }

    #[test]
    fn no_provider_boots_unconfigured_not_mock() {
        let i = ProviderSelectionInputs { ..none() };
        let sel = select_provider(i);
        assert_eq!(sel, ProviderSelection::Unconfigured);
        assert_ne!(sel, ProviderSelection::Mock);
    }

    #[test]
    fn no_provider_without_gate_boots_unconfigured() {
        assert_eq!(select_provider(none()), ProviderSelection::Unconfigured);
    }
}
