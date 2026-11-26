pub fn estimate_tokens_from_chars(chars: usize) -> usize {
    chars.saturating_mul(2) / 5
}

pub const RESPONSE_TOKEN_RESERVE: usize = 8_000;

pub fn agent_context_char_budget(context_window: usize) -> usize {
    let usable_tokens = context_window.saturating_sub(RESPONSE_TOKEN_RESERVE);
    usable_tokens.saturating_mul(3) / 2
}

pub fn agent_context_char_budget_for(model: &str, context_window: usize) -> usize {
    match crate::llm::model_registry::unified_output_limit(model) {
        Some(reserve) => context_window.saturating_sub(reserve).saturating_mul(3) / 2,
        None => agent_context_char_budget(context_window),
    }
}
