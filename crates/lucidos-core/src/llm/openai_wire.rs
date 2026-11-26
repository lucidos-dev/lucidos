use super::model_registry::gpt_major_version;

pub const OPENAI_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// INVARIANT: GPT-5+ and Codex models use the Responses API, all others use Chat Completions.
///
/// The GPT half reads the id's major version through
/// [`gpt_major_version`] rather than matching `gpt-5`. The literal spelling
/// sent `gpt-6-astra` to Chat Completions, which is the wrong endpoint for it,
/// and the same would happen to every family after it.
pub fn uses_responses_api(model: &str) -> bool {
    model.contains("codex") || gpt_major_version(model).is_some_and(|major| major >= 5)
}

pub const MAX_EXPLICIT_BREAKPOINTS: usize = 4;
