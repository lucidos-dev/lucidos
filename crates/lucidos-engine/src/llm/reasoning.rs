//! Which unified reasoning-effort tiers a model actually supports, and how to
//! snap an unsupported one onto the closest tier it does.
//!
//! **Single source of truth.** The Lucidos Agent picker, the trigger effort
//! pin, and every provider request resolve "what does this model support?"
//! here. The set is served to the frontend on `GET /api/v1/models`
//! (`reasoning_efforts` per row), so the picker filters against what the engine
//! will actually send rather than deriving its own answer. That is the whole
//! point of this module: the two used to derive it separately and disagreed.
//!
//! **Keyed on the provider, not just the id.** The id's shape says nothing
//! about which server receives the request or what vocabulary that server
//! validates against. `ProviderKind::{OpenAi, OpenRouter, Local}` all speak the
//! OpenAI wire format through the same [`crate::llm::OpenAiProvider`] struct
//! with only the base URL swapped, so a rule keyed on "the id starts with
//! `gpt-5.6`" reaches models OpenAI has never served. That is exactly how a
//! local `muse-glimmer:30b-mlx` turn died on 2026-08-12: the picker offered
//! `max` (its id matched no branch, so it fell through to a Gemini-shaped
//! rule), the wire builder rewrote `max` into `xhigh` because the id was not
//! `gpt-5.6`, and the local server rejected `xhigh` with a 400. See
//! `docs/plans/2026-08-12-reasoning-effort-follows-model.md`.
//!
//! **What each set is derived from**, so a new model lands in the right arm:
//!
//! - **Claude, adaptive** (`anthropic_wire::requires_adaptive_thinking`): the
//!   effort string is sent verbatim in `output_config.effort`, so every tier is
//!   distinct.
//! - **Claude, budget path**: `llm::thinking_budget_for_effort` maps each tier
//!   to a distinct `budget_tokens`, except that `xhigh` is not offered (a
//!   deliberate, pre-existing product choice, kept so no Claude model's offered
//!   set changes here).
//! - **Gemini**: `vertex::gemini::gemini_thinking_level` collapses `high`,
//!   `xhigh` and `max` onto the same `"high"` level, so offering the top two
//!   would show tiers that send an identical request.
//! - **OpenAI**: GPT-5.6 (Sol / Terra / Luna) accepts a distinct `max`; earlier
//!   families top out at `xhigh`.
//! - **OpenRouter / xAI / Local**: a server other than OpenAI's sits behind
//!   these, so only the tiers universal to the OpenAI-compatible wire format
//!   are offered. `xhigh` is OpenAI-proprietary and is never sent. xAI accepts
//!   a `reasoning_effort` on the same wire format, but which level names it
//!   validates is unverified here, so it takes the conservative set: the clamp
//!   only ever narrows, and an unsupported level is what earns a 400.
//! - **OpenCode Free**: measured per model rather than assumed, because the
//!   relay passes the level through to whoever serves it. Ox Alpha rejects
//!   `none`, `medium` and `xhigh` with a 400; the other seeded ids accept the
//!   conservative set. The matrix is in
//!   `docs/plans/2026-08-22-keyless-opencode-free-provider.md`.

use crate::llm::anthropic_wire::requires_adaptive_thinking;
use crate::llm::model_registry::ProviderKind;
use crate::llm::vertex::VertexProvider;

/// The unified reasoning vocabulary, in ascending order of effort. The order is
/// load-bearing: [`clamp_effort`] measures distance along it, and every set
/// below is a subset of it in the same order.
///
/// Also the allow-list the preference catalog validates `chat_reasoning_effort`
/// against (`core::preference_catalog::REASONING_EFFORTS`) and the vocabulary a
/// trigger's `reasoning_effort` is checked against, so the accepted values and
/// the offered tiers cannot drift apart.
pub const EFFORT_LADDER: &[&str] = &["none", "low", "medium", "high", "xhigh", "max"];

/// Claude on the adaptive-thinking path, and GPT-5.6: every tier is distinct.
const ALL_TIERS: &[&str] = EFFORT_LADDER;
/// Claude on the `budget_tokens` path: `xhigh` is deliberately not offered.
const NO_XHIGH: &[&str] = &["none", "low", "medium", "high", "max"];
/// OpenAI before GPT-5.6: tops out at `xhigh`.
const THROUGH_XHIGH: &[&str] = &["none", "low", "medium", "high", "xhigh"];
/// Gemini, OpenRouter, xAI and local servers: nothing above `high` is distinct
/// (Gemini) or universally accepted (the OpenAI-compatible third parties).
const THROUGH_HIGH: &[&str] = &["none", "low", "medium", "high"];
/// Ox Alpha on the keyless free tier: the only three levels it accepts. It
/// always reasons, so `none` is a 400 rather than a way to switch thinking off.
/// `medium` and `xhigh` are not in its vocabulary at all.
const LOW_HIGH_MAX: &[&str] = &["low", "high", "max"];

/// The one free-tier id whose accepted levels are [`LOW_HIGH_MAX`]. Every other
/// seeded free model takes [`THROUGH_HIGH`].
const OX_ALPHA_FREE: &str = "x-preview-f-free";

/// The tiers `model` supports when served by `provider`.
///
/// The Vertex arm asks [`VertexProvider::is_claude_model`] rather than
/// re-spelling its rule, so the tiers a Vertex model is offered cannot drift
/// from the request path it will actually take.
pub fn supported_efforts(provider: ProviderKind, model: &str) -> &'static [&'static str] {
    match provider {
        ProviderKind::Anthropic => claude_tiers(model),
        ProviderKind::Vertex => {
            if VertexProvider::is_claude_model(model) {
                claude_tiers(model)
            } else {
                THROUGH_HIGH
            }
        }
        ProviderKind::OpenAi => {
            if model.starts_with("gpt-5.6") {
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
        ProviderKind::OpenRouter | ProviderKind::XAi | ProviderKind::Local => THROUGH_HIGH,
    }
}

fn claude_tiers(model: &str) -> &'static [&'static str] {
    if requires_adaptive_thinking(model) {
        ALL_TIERS
    } else {
        NO_XHIGH
    }
}

/// Snap `effort` onto the closest tier `model` supports on `provider`, breaking
/// ties toward the HIGHER tier. `None` means `effort` is not one of our tiers at
/// all, so there is nothing to snap and the caller should send no effort.
///
/// Ties break upward because switching models must not quietly spend less
/// thought than the user asked for. The tie that actually arises is `xhigh` on
/// the Claude budget path, one rung from both `high` and `max`: `max` wins,
/// since the user was reaching past `high` and `max` is the offer that honours
/// that. Where there is no tie the genuinely nearest tier wins even going down,
/// so `max` on Gemini becomes `high` rather than staying above what the model
/// can distinguish.
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
    // `supported` is ascending and the comparison is `<=`, so an equidistant
    // later (higher) tier replaces an earlier one.
    supported
        .iter()
        .filter_map(|tier| rung(tier).map(|r| (tier, target.abs_diff(r))))
        .reduce(|best, cur| if cur.1 <= best.1 { cur } else { best })
        .map(|(tier, _)| *tier)
}

/// Position of `effort` on [`EFFORT_LADDER`], or `None` if it is not a tier.
fn rung(effort: &str) -> Option<usize> {
    EFFORT_LADDER.iter().position(|tier| *tier == effort)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every set is a subset of the ladder, in the ladder's own order. The
    /// clamp measures distance along that order, so a set that reordered or
    /// invented a tier would make "closest" meaningless.
    #[test]
    fn every_set_is_an_ordered_subset_of_the_ladder() {
        for set in [
            ALL_TIERS,
            NO_XHIGH,
            THROUGH_XHIGH,
            THROUGH_HIGH,
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

    /// The Claude arms. Adaptive models send the effort string verbatim, so
    /// they get every tier; the budget path keeps its pre-existing set.
    #[test]
    fn claude_tiers_split_on_the_adaptive_thinking_path() {
        for adaptive in [
            "claude-opus-5@default",
            "claude-opus-5@default[1m]",
            "claude-opus-4-8@default",
            "claude-opus-4-7",
            "claude-sonnet-5",
            "claude-fable-5[1m]",
        ] {
            assert_eq!(
                supported_efforts(ProviderKind::Vertex, adaptive),
                ALL_TIERS,
                "{adaptive} is adaptive and should offer every tier"
            );
        }
        for budget in ["claude-sonnet-4-6", "claude-opus-4-6", "claude-opus-4-5"] {
            assert_eq!(
                supported_efforts(ProviderKind::Vertex, budget),
                NO_XHIGH,
                "{budget} is on the budget path"
            );
        }
        // Fable is served by the direct Anthropic provider, and is adaptive.
        assert_eq!(
            supported_efforts(ProviderKind::Anthropic, "claude-fable-5"),
            ALL_TIERS
        );
    }

    /// A non-`claude` Vertex id is a Gemini id as far as `VertexProvider` is
    /// concerned, and Gemini collapses everything above `high` onto `high`.
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
    fn openai_offers_max_only_on_gpt_5_6() {
        for sixer in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            assert_eq!(supported_efforts(ProviderKind::OpenAi, sixer), ALL_TIERS);
        }
        for earlier in ["gpt-5.5-pro", "gpt-5.5", "gpt-5.4", "gpt-5.3-codex"] {
            assert_eq!(
                supported_efforts(ProviderKind::OpenAi, earlier),
                THROUGH_XHIGH
            );
        }
    }

    /// The regression. `xhigh` is OpenAI-proprietary; a third-party server
    /// behind OpenRouter or a local base URL must never be offered or sent it,
    /// whatever its id looks like. The `gpt-5.6`-shaped local id is the sharp
    /// case: the rule that made this fail keyed on the id alone.
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
                // A local server is free to serve an OpenAI-shaped id; the
                // server, not the id, decides the vocabulary.
                "gpt-5.6-sol",
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

    /// The free tier is the one provider whose accepted levels differ per
    /// model, so every seeded id is pinned against what the relay answered.
    /// Ox Alpha rejects `none`, `medium` and `xhigh`; the rest take the
    /// conservative set.
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
        // Nothing the caller can ask for reaches a level the model 400s on.
        for effort in EFFORT_LADDER {
            let sent = clamp_effort(effort, ProviderKind::OpenCodeFree, OX_ALPHA_FREE);
            assert!(
                matches!(sent, Some("low") | Some("high") | Some("max")),
                "Ox Alpha asked for {effort} and would be sent {sent:?}"
            );
        }
        // `max` is the one level hy3 rejects, and the conservative set has
        // already excluded it.
        assert_eq!(
            clamp_effort("max", ProviderKind::OpenCodeFree, "hy3-free"),
            Some("high")
        );
    }

    /// The exact turn that failed on 2026-08-12: the account effort was
    /// `xhigh`, the picker snapped it to `max`, and the wire layer sent
    /// `xhigh`. Both inputs must now land on `high`, which the local server
    /// accepts.
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

    /// A supported tier is returned untouched, on every provider.
    #[test]
    fn a_supported_tier_passes_through() {
        assert_eq!(
            clamp_effort("xhigh", ProviderKind::Vertex, "claude-opus-5@default"),
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

    /// Ties break toward the higher tier, matching `clampReasoningEffort` in
    /// `crates/lucidos-app/src/store/models.ts`. `xhigh` sits one rung from
    /// both `high` and `max` on the Claude budget path, and `max` wins.
    #[test]
    fn ties_break_toward_the_higher_tier() {
        assert_eq!(
            clamp_effort("xhigh", ProviderKind::Vertex, "claude-opus-4-6"),
            Some("max")
        );
    }

    /// A non-tie snaps to the genuinely nearest rung, which can be downward.
    #[test]
    fn a_clear_nearest_tier_wins_even_when_it_is_lower() {
        // `max` is two rungs above `high` and three above `medium`.
        assert_eq!(
            clamp_effort("max", ProviderKind::Vertex, "gemini-3.5-flash"),
            Some("high")
        );
        assert_eq!(
            clamp_effort("max", ProviderKind::OpenAi, "gpt-5.4"),
            Some("xhigh")
        );
    }

    /// A value that is not one of our tiers is DROPPED, never guessed at.
    ///
    /// Guessing the top tier would make a typo silently buy the most expensive
    /// reasoning the model has, and an arbitrary string genuinely reaches here:
    /// only the `preferences` LLM tool validates against the ladder, while
    /// `PUT /api/v1/preferences` and `POST /api/v1/chat/stream` do not.
    /// Dropping hands the decision to the provider's own default, as
    /// `validate_codex_effort` does.
    #[test]
    fn a_value_off_the_ladder_is_dropped_rather_than_guessed() {
        for (provider, model) in [
            (ProviderKind::Local, "muse-glimmer:30b-mlx"),
            (ProviderKind::OpenAi, "gpt-5.4"),
            (ProviderKind::Vertex, "claude-opus-5@default"),
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

    /// Whatever goes in, what comes out is either a tier the model supports or
    /// nothing at all. This is the property the picker and the wire both depend
    /// on: the request can never carry a value outside the offered set.
    #[test]
    fn clamping_never_lands_outside_the_supported_set() {
        let cases = [
            (ProviderKind::Vertex, "claude-opus-5@default"),
            (ProviderKind::Vertex, "claude-sonnet-4-6"),
            (ProviderKind::Vertex, "gemini-3.1-pro-preview"),
            (ProviderKind::Anthropic, "claude-fable-5"),
            (ProviderKind::OpenAi, "gpt-5.6-sol"),
            (ProviderKind::OpenAi, "gpt-5.4"),
            (ProviderKind::OpenRouter, "z-ai/glm-5.2"),
            (ProviderKind::XAi, "grok-4.6"),
            (ProviderKind::Local, "muse-glimmer:30b-mlx"),
        ];
        for (provider, model) in cases {
            let supported = supported_efforts(provider, model);
            // Every real tier resolves to something offered.
            for effort in EFFORT_LADDER {
                let clamped = clamp_effort(effort, provider, model).unwrap_or_else(|| {
                    panic!("{provider:?}/{model} dropped the real tier {effort}")
                });
                assert!(
                    supported.contains(&clamped),
                    "{provider:?}/{model}: {effort} clamped to {clamped}, outside {supported:?}"
                );
            }
            // Everything else resolves to nothing.
            for junk in ["", "ultra", "MAX"] {
                assert_eq!(clamp_effort(junk, provider, model), None);
            }
        }
    }
}
