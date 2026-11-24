//! API selection and wire constants shared by OpenAI callers.

use super::model_registry::gpt_major_version;

/// Base URL for the direct-OpenAI API (`{base}/chat/completions`,
/// `{base}/responses`). The OpenRouter and local OpenAI-compatible backends
/// reuse `OpenAiProvider` with a different base via `OpenAiProvider::new_with_base_url`.
pub const OPENAI_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// GPT-5+ and Codex models use the Responses API, all others use Chat Completions.
///
/// The GPT half reads the id's major version through
/// [`gpt_major_version`] rather than matching `gpt-5`. The literal spelling
/// sent `gpt-6-astra` to Chat Completions, which is the wrong endpoint for it,
/// and the same would happen to every family after it.
pub fn uses_responses_api(model: &str) -> bool {
    model.contains("codex") || gpt_major_version(model).is_some_and(|major| major >= 5)
}

/// Breakpoints a request in explicit mode may carry: all four writes, since
/// nothing writes at the end.
pub const MAX_EXPLICIT_BREAKPOINTS: usize = 4;
