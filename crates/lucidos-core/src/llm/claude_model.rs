//! Claude model classification shared by the wire formats and reasoning policy.

/// Models that support extended thinking (reasoning goes to dedicated blocks
/// instead of polluting the text response).
///
/// The Sonnet families need two rules. `claude-sonnet-4` covers 4.x
/// only. Without its own arm, a new generation falls to the no-thinking
/// 8192-token path.
///
///
/// Haiku 4.5 rides the `budget_tokens` path. Its 64K output ceiling clears
/// the largest budget plus the reply allowance in `thinking_config`. The
/// Haiku 5 family is adaptive, and needs its own arm for the same reason as
/// Sonnet 5. Its [`ADAPTIVE_THINKING_MODELS`] entry matches the same fragment,
/// so no Haiku 5 id can reach the `budget_tokens` path.
pub(crate) fn supports_extended_thinking(model: &str) -> bool {
    model.contains("claude-3-7-sonnet")
        || model.contains("claude-haiku-4-5")
        || model.contains("claude-haiku-5")
        || model.contains("claude-sonnet-4")
        || model.contains("claude-sonnet-5")
        || model.contains("claude-opus-4")
        || model.contains("claude-opus-5")
        || model.contains("claude-fable-5")
}

/// What an adaptive-thinking model does when a request omits `thinking`, and
/// whether it can be turned off at all. Effort `none` means something
/// different in each case, so `thinking_config` branches on this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThinkingMode {
    /// Omitting `thinking` runs the model without it.
    OffByDefault,
    /// Omitting `thinking` still runs adaptive thinking, so turning it off
    /// takes an explicit `{type: "disabled"}`.
    OnByDefault,
    /// Thinking cannot be turned off: `{type: "disabled"}` is a 400. Effort is
    /// the only control, and the model returns its notes between tool calls
    /// as `thinking` blocks rather than `text`.
    AlwaysOn,
}

/// One adaptive-thinking Claude family. See [`ADAPTIVE_THINKING_MODELS`].
pub(super) struct AdaptiveModel {
    pub(super) fragment: &'static str,
    pub(super) max_output_tokens: u32,
    pub(super) mode: ThinkingMode,
}

/// Adaptive-thinking Claude models, each with the ceiling the API accepts for
/// its `max_tokens` and its [`ThinkingMode`].
///
/// The first matching fragment wins, so a more specific id sits above the
/// family it shares a prefix with: `claude-opus-5-5` before `claude-opus-5`,
/// `claude-sonnet-5-5` before `claude-sonnet-5`.
///
/// One list answers every question on purpose. `max_tokens` bounds thinking AND
/// response text together. Too low a value cuts a deep turn wherever it had
/// reached, including mid tool-call
/// (`docs/plans/2026-08-18-a-truncated-tool-argument-stream.md`). Too high is
/// worse: the API rejects the request, so every turn on that model fails rather
/// than one long one. Each entry names its ceiling and its mode, so a new arm
/// cannot be added without both.
///
/// 128_000 is the exact ceiling, not a decimal reading of "128k". Vertex
/// rejects anything above it with `max_tokens: N > 128000, which is the maximum
/// allowed number of output tokens`. Opus 4.7, Opus 4.8, Opus 5, Sonnet 5 and
/// Haiku 5.5 each report that same number.
pub(super) const ADAPTIVE_THINKING_MODELS: &[AdaptiveModel] = &[
    AdaptiveModel {
        fragment: "claude-opus-4-7",
        max_output_tokens: 128_000,
        mode: ThinkingMode::OffByDefault,
    },
    AdaptiveModel {
        fragment: "claude-opus-4-8",
        max_output_tokens: 128_000,
        mode: ThinkingMode::OffByDefault,
    },
    AdaptiveModel {
        fragment: "claude-opus-5-5",
        max_output_tokens: 128_000,
        mode: ThinkingMode::AlwaysOn,
    },
    AdaptiveModel {
        fragment: "claude-opus-5",
        max_output_tokens: 128_000,
        mode: ThinkingMode::OnByDefault,
    },
    AdaptiveModel {
        fragment: "claude-sonnet-5-5",
        max_output_tokens: 128_000,
        mode: ThinkingMode::AlwaysOn,
    },
    AdaptiveModel {
        fragment: "claude-sonnet-5",
        max_output_tokens: 128_000,
        mode: ThinkingMode::OnByDefault,
    },
    AdaptiveModel {
        fragment: "claude-haiku-5",
        max_output_tokens: 128_000,
        mode: ThinkingMode::OnByDefault,
    },
    AdaptiveModel {
        fragment: "claude-fable-5",
        max_output_tokens: 128_000,
        mode: ThinkingMode::AlwaysOn,
    },
];

pub(super) fn adaptive_model(model: &str) -> Option<&'static AdaptiveModel> {
    ADAPTIVE_THINKING_MODELS
        .iter()
        .find(|entry| model.contains(entry.fragment))
}

/// The [`ThinkingMode`] of an adaptive-thinking model, or `None` when the model
/// is not one. `Some` means adaptive thinking only: no `budget_tokens`, no
/// `temperature`/`top_p`/`top_k`, effort through `output_config.effort`.
/// Sonnet 4.6 and older stay on the `budget_tokens` path.
pub fn thinking_mode(model: &str) -> Option<ThinkingMode> {
    adaptive_model(model).map(|entry| entry.mode)
}

/// Whether Vertex uses the Claude request path for this model.
pub fn is_vertex_claude_model(model: &str) -> bool {
    model.starts_with("claude")
}
