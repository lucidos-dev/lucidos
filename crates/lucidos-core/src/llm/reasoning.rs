use crate::llm::claude_model::{is_vertex_claude_model, thinking_mode, ThinkingMode};
use crate::llm::model_registry::ProviderKind;

/// INVARIANT: The unified reasoning vocabulary, in ascending order of effort. The order is
/// load-bearing: [`clamp_effort`] measures distance along it, and every set
/// below is a subset of it in the same order.
///
/// Also the allow-list the preference catalog validates each
/// `chat_reasoning_efforts` tier against
/// (`core::preference_catalog::REASONING_EFFORTS`), and the vocabulary a
/// trigger's `reasoning_effort` is checked against. So the accepted values and
/// the offered tiers cannot drift apart.
pub const EFFORT_LADDER: &[&str] = &["none", "low", "medium", "high", "xhigh", "max"];

const ALL_TIERS: &[&str] = EFFORT_LADDER;
const LOW_THROUGH_MAX: &[&str] = &["low", "medium", "high", "xhigh", "max"];
/// INVARIANT: Claude on the `budget_tokens` path: `xhigh` is deliberately not offered.
const NO_XHIGH: &[&str] = &["none", "low", "medium", "high", "max"];
const THROUGH_XHIGH: &[&str] = &["none", "low", "medium", "high", "xhigh"];
const THROUGH_HIGH: &[&str] = &["none", "low", "medium", "high"];
const LOW_THROUGH_HIGH: &[&str] = &["low", "medium", "high"];
/// INVARIANT: Ox Alpha on the keyless free tier: the only three levels it accepts. It
/// always reasons, so `none` is a 400 rather than a way to switch thinking off.
/// `medium` and `xhigh` are not in its vocabulary at all.
const LOW_HIGH_MAX: &[&str] = &["low", "high", "max"];

const OX_ALPHA_FREE: &str = "x-preview-f-free";

/// INVARIANT: GPT-6 Astra, which accepts the same full tier set as the GPT-5.6 family.
///
/// Named here rather than folded into a `gpt-6` prefix rule. A family that tops
/// out at `xhigh` answers `max` with a 400. So which ids reach [`ALL_TIERS`]
/// stays a per-family fact, never a guess from the id's shape.
const GPT_6_ASTRA: &str = "gpt-6-astra";

const ALWAYS_REASONING: &[&str] = &[
    "gpt-6.2-astra",
    "gpt-6.2-sol",
    "gpt-6.1-sol",
    "gemini-3.8-flash",
    "gemini-4-argon",
    "gemini-4-carbon",
    "gemini-4-neon",
];

pub(crate) fn always_reasons(model: &str) -> bool {
    let bare = model.rsplit('/').next().unwrap_or(model);
    ALWAYS_REASONING.contains(&bare)
}

pub fn supported_efforts(provider: ProviderKind, model: &str) -> &'static [&'static str] {
    match provider {
        ProviderKind::Anthropic | ProviderKind::ClaudeGateway => claude_tiers(model),
        ProviderKind::Vertex => {
            if is_vertex_claude_model(model) {
                claude_tiers(model)
            } else if model == "gemini-4-neon" {
                &["low", "medium", "high", "xhigh"]
            } else if always_reasons(model) {
                LOW_THROUGH_HIGH
            } else {
                THROUGH_HIGH
            }
        }
        ProviderKind::OpenAi | ProviderKind::ChatGptPlan => {
            if always_reasons(model) {
                LOW_THROUGH_MAX
            } else if model.starts_with("gpt-5.6") || model == GPT_6_ASTRA {
                ALL_TIERS
            } else {
                THROUGH_XHIGH
            }
        }
        ProviderKind::OpenCodeFree => {
            if model == OX_ALPHA_FREE {
                LOW_HIGH_MAX
            } else {
                THROUGH_HIGH
            }
        }
        // INVARIANT: OpenRouter can front Claude under its own id (`anthropic/claude-opus-5-5`),
        // which the substring match in `thinking_mode` still recognises.
        ProviderKind::OpenRouter
            if thinking_mode(model) == Some(ThinkingMode::AlwaysOn) || always_reasons(model) =>
        {
            LOW_THROUGH_HIGH
        }
        ProviderKind::OpenRouter | ProviderKind::XAi | ProviderKind::Local => THROUGH_HIGH,
    }
}

fn claude_tiers(model: &str) -> &'static [&'static str] {
    match thinking_mode(model) {
        Some(ThinkingMode::AlwaysOn) => LOW_THROUGH_MAX,
        Some(ThinkingMode::OffByDefault | ThinkingMode::OnByDefault) => ALL_TIERS,
        None => NO_XHIGH,
    }
}

/// INVARIANT: Snap `effort` onto the closest tier `model` supports on `provider`, breaking
/// ties toward the HIGHER tier. `None` means `effort` is not one of our tiers at
/// all, so there is nothing to snap and the caller should send no effort.
///
/// Ties break upward because switching models must not quietly spend less
/// thought than the user asked for. The tie that actually arises is `xhigh` on
/// the Claude budget path, one rung from both `high` and `max`: `max` wins,
/// since the user was reaching past `high` and `max` is the offer that honours
/// that. Where there is no tie the genuinely nearest tier wins even going down,
/// so `max` on Gemini becomes `xhigh` on Neon and `high` on other models.
///
/// **A value off [`EFFORT_LADDER`] entirely is dropped, never guessed at.** An
/// empty string or an unrecognised word carries no intent to preserve, so the
/// nearest-tier rule has nothing to measure from, and answering "the top tier"
/// would make a typo silently buy the most expensive reasoning setting the
/// model has. Only the `preferences` LLM tool validates against the ladder:
/// `PUT /api/v1/preferences` and `POST /api/v1/chat/stream`'s `reasoning_effort`
/// do not, so an arbitrary string really does reach here. Dropping it lets the
/// provider apply its own default, matching what `validate_codex_effort`
/// (`runtime/codex.rs`) does with the same input on the coding-agent side.
pub fn clamp_effort<'a>(effort: &'a str, provider: ProviderKind, model: &str) -> Option<&'a str> {
    let target = rung(effort)?;
    let supported = supported_efforts(provider, model);
    if supported.contains(&effort) {
        return Some(effort);
    }
    // INVARIANT: `supported` is ascending and the comparison is `<=`, so an equidistant
    // later (higher) tier replaces an earlier one.
    supported
        .iter()
        .filter_map(|tier| rung(tier).map(|r| (tier, target.abs_diff(r))))
        .reduce(|best, cur| if cur.1 <= best.1 { cur } else { best })
        .map(|(tier, _)| *tier)
}

pub fn tier(effort: &str) -> Option<&'static str> {
    EFFORT_LADDER.iter().find(|t| **t == effort).copied()
}

fn rung(effort: &str) -> Option<usize> {
    EFFORT_LADDER.iter().position(|tier| *tier == effort)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_set_is_an_ordered_subset_of_the_ladder() {
        for set in [
            ALL_TIERS,
            LOW_THROUGH_MAX,
            NO_XHIGH,
            THROUGH_XHIGH,
            THROUGH_HIGH,
            LOW_THROUGH_HIGH,
            LOW_HIGH_MAX,
        ] {
            assert!(!set.is_empty());
            let rungs: Vec<usize> = set
                .iter()
                .map(|t| rung(t).expect("tier on ladder"))
                .collect();
            assert!(
                rungs.windows(2).all(|w| w[0] < w[1]),
                "{set:?} is not in ladder order"
            );
        }
    }

    #[test]
    fn claude_tiers_split_on_the_adaptive_thinking_path() {
        for always_on in [
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
            "claude-sonnet-5-5",
            "claude-sonnet-5-5[1m]",
        ] {
            assert_eq!(
                supported_efforts(ProviderKind::Vertex, always_on),
                LOW_THROUGH_MAX,
                "{always_on} always thinks, so `none` is not a tier it has"
            );
        }
        for adaptive in [
            "claude-opus-5",
            "claude-opus-5[1m]",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-sonnet-5",
            "claude-haiku-5-5",
        ] {
            assert_eq!(
                supported_efforts(ProviderKind::Vertex, adaptive),
                ALL_TIERS,
                "{adaptive} is adaptive and should offer every tier"
            );
        }
        for budget in [
            "claude-sonnet-4-6",
            "claude-opus-4-6",
            "claude-opus-4-5",
            "claude-haiku-4-5",
        ] {
            for provider in [ProviderKind::Vertex, ProviderKind::Anthropic] {
                assert_eq!(
                    supported_efforts(provider, budget),
                    NO_XHIGH,
                    "{budget} is on the budget path"
                );
            }
        }
        for fable in ["claude-fable-5", "claude-fable-5-1", "claude-fable-5-1[1m]"] {
            assert_eq!(
                supported_efforts(ProviderKind::Anthropic, fable),
                LOW_THROUGH_MAX,
                "{fable} accepts every tier but `none` through output_config.effort"
            );
        }
    }

    #[test]
    fn a_claude_gateway_offers_the_direct_anthropic_tiers() {
        for model in [
            "claude-opus-5-5",
            "claude-sonnet-5-5[1m]",
            "claude-haiku-4-5",
            "claude-fable-5-1",
        ] {
            assert_eq!(
                supported_efforts(ProviderKind::ClaudeGateway, model),
                supported_efforts(ProviderKind::Anthropic, model),
                "{model}"
            );
        }
    }

    #[test]
    fn none_snaps_to_low_on_a_model_that_cannot_stop_thinking() {
        for (provider, model) in [
            (ProviderKind::Vertex, "claude-opus-5-5"),
            (ProviderKind::Anthropic, "claude-sonnet-5-5"),
            (ProviderKind::Anthropic, "claude-fable-5-1[1m]"),
        ] {
            assert_eq!(
                clamp_effort("none", provider, model),
                Some("low"),
                "{model}"
            );
        }
        assert_eq!(
            clamp_effort("none", ProviderKind::Vertex, "claude-opus-5"),
            Some("none")
        );
    }

    #[test]
    fn gemini_stops_at_high_because_nothing_above_it_is_distinct() {
        for gemini in [
            "gemini-3.1-pro-preview",
            "gemini-3.5-flash",
            "gemini-3-flash-preview",
        ] {
            assert_eq!(
                supported_efforts(ProviderKind::Vertex, gemini),
                THROUGH_HIGH
            );
        }
    }

    #[test]
    fn a_model_that_always_reasons_is_never_offered_none() {
        let routes = [
            (
                ProviderKind::Vertex,
                "gemini-4-neon",
                &["low", "medium", "high", "xhigh"][..],
            ),
            (ProviderKind::Vertex, "gemini-3.8-flash", LOW_THROUGH_HIGH),
            (
                ProviderKind::OpenRouter,
                "google/gemini-3.8-flash",
                LOW_THROUGH_HIGH,
            ),
            (ProviderKind::OpenAi, "gpt-6.2-astra", LOW_THROUGH_MAX),
            (ProviderKind::ChatGptPlan, "gpt-6.2-astra", LOW_THROUGH_MAX),
            (ProviderKind::OpenAi, "gpt-6.2-sol", LOW_THROUGH_MAX),
            (ProviderKind::ChatGptPlan, "gpt-6.2-sol", LOW_THROUGH_MAX),
            (
                ProviderKind::OpenRouter,
                "openai/gpt-6.2-sol",
                LOW_THROUGH_HIGH,
            ),
            (ProviderKind::OpenAi, "gpt-6.1-sol", LOW_THROUGH_MAX),
            (
                ProviderKind::OpenRouter,
                "openai/gpt-6.1-sol",
                LOW_THROUGH_HIGH,
            ),
        ];
        for (provider, model, tiers) in routes {
            assert_eq!(
                supported_efforts(provider, model),
                tiers,
                "{model} on {provider:?}"
            );
            assert_eq!(
                clamp_effort("none", provider, model),
                Some("low"),
                "{model} on {provider:?}"
            );
        }
    }

    #[test]
    fn openai_offers_max_on_gpt_5_6_and_astra() {
        for top in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna", GPT_6_ASTRA] {
            assert_eq!(supported_efforts(ProviderKind::OpenAi, top), ALL_TIERS);
            assert_eq!(
                clamp_effort("max", ProviderKind::OpenAi, top),
                Some("max"),
                "{top} must be sent max unchanged"
            );
        }
        for earlier in ["gpt-5.5-pro", "gpt-5.5", "gpt-5.4", "gpt-5.3-codex"] {
            assert_eq!(
                supported_efforts(ProviderKind::OpenAi, earlier),
                THROUGH_XHIGH
            );
        }
    }

    #[test]
    fn third_party_openai_compatible_servers_never_see_xhigh() {
        for provider in [
            ProviderKind::OpenRouter,
            ProviderKind::XAi,
            ProviderKind::Local,
        ] {
            for model in [
                "muse-glimmer:30b-mlx",
                "z-ai/glm-5.2",
                "moonshotai/kimi-k3",
                "llama3.1",
                "grok-4.6",
                "gpt-5.6-sol",
                GPT_6_ASTRA,
            ] {
                let tiers = supported_efforts(provider, model);
                assert_eq!(tiers, THROUGH_HIGH, "{provider:?}/{model}");
                for effort in EFFORT_LADDER {
                    assert_ne!(
                        clamp_effort(effort, provider, model),
                        Some("xhigh"),
                        "{provider:?}/{model} clamped {effort} to xhigh"
                    );
                }
            }
        }
    }

    #[test]
    fn openrouter_never_offers_none_to_a_model_that_always_thinks() {
        for model in ["anthropic/claude-opus-5-5", "anthropic/claude-fable-5-1"] {
            assert_eq!(
                supported_efforts(ProviderKind::OpenRouter, model),
                LOW_THROUGH_HIGH,
                "{model}"
            );
            assert_eq!(
                clamp_effort("none", ProviderKind::OpenRouter, model),
                Some("low")
            );
        }
        assert_eq!(
            supported_efforts(ProviderKind::OpenRouter, "anthropic/claude-opus-5"),
            THROUGH_HIGH
        );
    }

    #[test]
    fn the_free_tier_offers_only_levels_its_models_accept() {
        assert_eq!(
            supported_efforts(ProviderKind::OpenCodeFree, OX_ALPHA_FREE),
            LOW_HIGH_MAX
        );
        for model in [
            "laguna-s-2.1-free",
            "nemotron-3.5-lightning-free",
            "nemotron-3-ultra-free",
            "muse-spark-1.2-contributor-free",
            "hy3-free",
        ] {
            assert_eq!(
                supported_efforts(ProviderKind::OpenCodeFree, model),
                THROUGH_HIGH,
                "{model}"
            );
        }
        for effort in EFFORT_LADDER {
            let sent = clamp_effort(effort, ProviderKind::OpenCodeFree, OX_ALPHA_FREE);
            assert!(
                matches!(sent, Some("low") | Some("high") | Some("max")),
                "Ox Alpha asked for {effort} and would be sent {sent:?}"
            );
        }
        assert_eq!(
            clamp_effort("max", ProviderKind::OpenCodeFree, "hy3-free"),
            Some("high")
        );
    }

    #[test]
    fn the_local_model_that_400d_now_clamps_to_high() {
        let model = "muse-glimmer:30b-mlx";
        assert_eq!(
            clamp_effort("xhigh", ProviderKind::Local, model),
            Some("high")
        );
        assert_eq!(
            clamp_effort("max", ProviderKind::Local, model),
            Some("high")
        );
    }

    #[test]
    fn a_supported_tier_passes_through() {
        assert_eq!(
            clamp_effort("xhigh", ProviderKind::Vertex, "claude-opus-5"),
            Some("xhigh")
        );
        assert_eq!(
            clamp_effort("max", ProviderKind::OpenAi, "gpt-5.6-sol"),
            Some("max")
        );
        assert_eq!(
            clamp_effort("xhigh", ProviderKind::OpenAi, "gpt-5.4"),
            Some("xhigh")
        );
        assert_eq!(
            clamp_effort("none", ProviderKind::Local, "muse-glimmer:30b-mlx"),
            Some("none")
        );
    }

    #[test]
    fn ties_break_toward_the_higher_tier() {
        assert_eq!(
            clamp_effort("xhigh", ProviderKind::Vertex, "claude-opus-4-6"),
            Some("max")
        );
    }

    #[test]
    fn a_clear_nearest_tier_wins_even_when_it_is_lower() {
        assert_eq!(
            clamp_effort("max", ProviderKind::Vertex, "gemini-3.5-flash"),
            Some("high")
        );
        assert_eq!(
            clamp_effort("max", ProviderKind::OpenAi, "gpt-5.4"),
            Some("xhigh")
        );
    }

    #[test]
    fn a_value_off_the_ladder_is_dropped_rather_than_guessed() {
        for (provider, model) in [
            (ProviderKind::Local, "muse-glimmer:30b-mlx"),
            (ProviderKind::OpenAi, "gpt-5.4"),
            (ProviderKind::Vertex, "claude-opus-5"),
        ] {
            for junk in ["", "ultra", "MAX", "High", "  high  "] {
                assert_eq!(
                    clamp_effort(junk, provider, model),
                    None,
                    "{provider:?}/{model} invented a tier for {junk:?}"
                );
            }
        }
    }

    #[test]
    fn clamping_never_lands_outside_the_supported_set() {
        let cases = [
            (ProviderKind::Vertex, "claude-opus-5"),
            (ProviderKind::Vertex, "claude-sonnet-4-6"),
            (ProviderKind::Vertex, "gemini-3.1-pro-preview"),
            (ProviderKind::Anthropic, "claude-fable-5"),
            (ProviderKind::Anthropic, "claude-fable-5-1"),
            (ProviderKind::OpenAi, "gpt-5.6-sol"),
            (ProviderKind::OpenAi, GPT_6_ASTRA),
            (ProviderKind::OpenAi, "gpt-5.4"),
            (ProviderKind::OpenRouter, "z-ai/glm-5.2"),
            (ProviderKind::XAi, "grok-4.6"),
            (ProviderKind::Local, "muse-glimmer:30b-mlx"),
        ];
        for (provider, model) in cases {
            let supported = supported_efforts(provider, model);
            for effort in EFFORT_LADDER {
                let clamped = clamp_effort(effort, provider, model).unwrap_or_else(|| {
                    panic!("{provider:?}/{model} dropped the real tier {effort}")
                });
                assert!(
                    supported.contains(&clamped),
                    "{provider:?}/{model}: {effort} clamped to {clamped}, outside {supported:?}"
                );
            }
            for junk in ["", "ultra", "MAX"] {
                assert_eq!(clamp_effort(junk, provider, model), None);
            }
        }
    }
}
