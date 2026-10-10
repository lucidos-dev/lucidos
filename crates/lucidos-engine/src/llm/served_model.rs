//! The *served model*: the model id a provider's reply says answered a call.
//!
//! A provider can answer a request with a model other than the one asked for,
//! for example by routing a retired id to its replacement. The reply names the
//! model that really ran, so the capture records it and the router compares it
//! with what it sent.

use serde_json::Value;

/// The model id a reply names: `model` (OpenAI, Anthropic, OpenRouter) or
/// `modelVersion` (Gemini). `None` when it names neither, or an empty id.
pub fn served_model_of(reply: &Value) -> Option<String> {
    ["model", "modelVersion"]
        .iter()
        .find_map(|key| reply.get(*key).and_then(Value::as_str))
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

/// Whether a reply naming `served` answered a request that sent `sent`.
///
/// Both ids drop a `vendor/` prefix and a `[1m]` suffix first. A Vertex `@`
/// snapshot reads as `-`, so `x@20251101` and `x-20251101` are equal.
/// Providers answer with a dated snapshot (`claude-haiku-4-5-20251001`,
/// `-001`) or a local tag (`qwen3:latest`), so the served id may carry one. A
/// short number is a version, not a snapshot: `claude-opus-5-5` is not Opus 5.
pub fn served_as_sent(sent: &str, served: &str) -> bool {
    let bare = |id: &str| {
        let id = id.rsplit('/').next().unwrap_or(id);
        id.strip_suffix("[1m]")
            .unwrap_or(id)
            .replace('@', "-")
            .to_ascii_lowercase()
    };
    let (sent, served) = (bare(sent), bare(served));
    let Some(rest) = served.strip_prefix(sent.as_str()) else {
        return false;
    };
    let snapshot = |stamp: &str| stamp.chars().take_while(char::is_ascii_digit).count() >= 3;
    rest.is_empty() || rest.starts_with(':') || rest.strip_prefix('-').is_some_and(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_reply_names_its_model_under_either_key() {
        assert_eq!(
            served_model_of(&json!({"modelVersion": "gemini-3.6-flash"})).as_deref(),
            Some("gemini-3.6-flash")
        );
        assert_eq!(
            served_model_of(&json!({"model": "gpt-5.6-luna-2026-04-01"})).as_deref(),
            Some("gpt-5.6-luna-2026-04-01")
        );
        assert_eq!(served_model_of(&json!({"model": ""})), None);
        assert_eq!(served_model_of(&json!({"id": "msg_1"})), None);
    }

    /// Google routes a retired Gemini id to another model. That is the
    /// silent reroute this check exists to catch.
    #[test]
    fn a_different_version_is_a_reroute() {
        assert!(!served_as_sent("gemini-3.5-flash", "gemini-3.6-flash"));
        assert!(!served_as_sent("gemini-3.5-flash", "gemini-3.5-pro"));
        assert!(!served_as_sent("claude-opus-5", "claude-opus-5-5"));
        assert!(!served_as_sent("gemini-3.8-flash", "gemini-3.8-flash-lite"));
    }

    #[test]
    fn a_snapshot_or_tag_of_the_sent_id_is_the_same_model() {
        for (sent, served) in [
            ("gemini-3.8-flash", "gemini-3.8-flash"),
            ("claude-haiku-4-5", "claude-haiku-4-5-20251001"),
            ("gpt-5.6-luna", "gpt-5.6-luna-2026-04-01"),
            ("gemini-3.8-flash", "gemini-3.8-flash-001"),
            ("google/gemini-3.8-flash", "gemini-3.8-flash"),
            ("gemini-3.8-flash", "google/gemini-3.8-flash"),
            ("claude-opus-5[1m]", "claude-opus-5"),
            ("claude-opus-4-5", "claude-opus-4-5@20251101"),
            ("claude-opus-4-5@20251101", "claude-opus-4-5-20251101"),
            ("qwen3", "qwen3:latest"),
            ("GPT-5.6-Luna", "gpt-5.6-luna"),
        ] {
            assert!(served_as_sent(sent, served), "{sent} served as {served}");
        }
    }
}
