//! Measured token estimate for user-visible context reporting.

/// Estimates user-visible tokens at 2.5 characters per token.
/// Feeds `ContextCaptured.estimated_total_tokens` and the thought stream.
///
/// The prompt packer uses a separate conservative ratio of 1.5.
/// Applying that ratio to reporting overstated a measured 205k prompt as 361k.
///
/// Calibration used 12,069 main LLM captures after tool schemas joined the count.
/// The observed ratios were 2.28 at p01, 2.60 at p50 and 2.74 at p99.
/// Measured Gemini and GPT tokenizers use more characters per token.
/// The calibration log in `agentic_loop/run.rs` tracks estimate versus usage.
///
/// The previous prose ratio of four understated a trigger's 1.54M-token input.
/// The separate estimate preserves the packer's conservative budget.
pub fn estimate_tokens_from_chars(chars: usize) -> usize {
    // 2/5 = 2.5 chars/token (integer math).
    chars.saturating_mul(2) / 5
}

/// Default input-packing margin for models with no explicit shared-window
/// output cap. Gemini 4 instead reserves its whole wire cap through
/// `agent_context_char_budget_for`.
pub const RESPONSE_TOKEN_RESERVE: usize = 8_000;

/// Per-request char budget for `(system_prompt + tools + messages)`, derived
/// from the model's resolved context window. JSON-heavy tool calls average
/// ~1.5 chars/token (prose is ~3.5); the 3/2 multiplier picks the
/// conservative end so the char total never under-counts the token total.
/// A 1M-token model gets ~1.49M chars; the default 200k Claude gets ~288k
/// chars (close to the previous hardcoded 300k); GPT-5 at 400k gets ~588k
/// chars. Callers subtract their own prompt overhead (system prompt + tool
/// definitions) before handing the remainder to `trim_context_if_needed`.
///
/// Takes the already-resolved window rather than a model string: the window
/// now comes from the `models` registry when the row declares one, and only
/// falls back to `context_window_from_prefix` when it doesn't. Resolution
/// lives in `llm::model_registry::context_window_for`.
///
/// The old 300k-char constant wasted larger model windows.
/// It could trim the original user message even when the whole thread fit.
pub fn agent_context_char_budget(context_window: usize) -> usize {
    let usable_tokens = context_window.saturating_sub(RESPONSE_TOKEN_RESERVE);
    // 3/2 = 1.5 chars/token (integer math).
    usable_tokens.saturating_mul(3) / 2
}

/// Model-aware request budget: reserve the full output cap for models with
/// a unified input/output window. Existing models retain the 8k margin.
pub fn agent_context_char_budget_for(model: &str, context_window: usize) -> usize {
    match crate::llm::model_registry::unified_output_limit(model) {
        Some(reserve) => context_window.saturating_sub(reserve).saturating_mul(3) / 2,
        None => agent_context_char_budget(context_window),
    }
}
