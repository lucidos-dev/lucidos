use async_trait::async_trait;
use std::time::Duration;

use super::{format_search_result, WebSearchProvider, WebSearchResult, SEARCH_SYSTEM_PROMPT};
use crate::llm::anthropic::chat::{auth_header, ANTHROPIC_VERSION};
use crate::llm::anthropic::{AnthropicAuth, ANTHROPIC_OAUTH_BETA};
use crate::llm::anthropic_wire::{thinking_mode, ThinkingMode};
use crate::llm::metered::CallToken;
use crate::llm::ANTHROPIC_API_BASE_URL;

const MAX_TOKENS: u32 = 2048;

/// INVARIANT: The ceiling for a model that thinks unless told otherwise. Thinking counts
/// toward `max_tokens`, so 2048 could run out before the summary began. Such a
/// request also sends effort `low`, which keeps the thinking itself small.
const THINKING_MAX_TOKENS: u32 = 8192;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// INVARIANT: How many searches Claude may run to answer one `web_search` tool call.
///
/// Deliberately NOT tied to `max_results`, which is a *display* cap on how many
/// sources to list. They measure different things, and conflating them turns a
/// caller asking for more sources into a caller silently paying for more
/// searches — `max_results: 20` would authorise 10 billed searches for one tool
/// call. Three is enough for the model to cross-check a claim without turning a
/// single tool call into a research session.
const MAX_SEARCHES: u32 = 3;

/// INVARIANT: Pick the server-tool version the model will accept.
///
/// The `_20260209` variant (dynamic filtering) is rejected with a 400 on models
/// that predate it, so this pairing is load-bearing rather than cosmetic. The
/// families that accept it are Opus 4.6 and newer and Sonnet 4.6 and newer;
/// everything else — including Haiku and Fable — takes the basic `_20250305`.
///
/// Matching is by prefix so suffixed aliases (`claude-opus-5[1m]`) and dated
/// snapshots resolve to the same family. An unknown model falls to the basic
/// variant: the older tool is accepted everywhere the newer one is, so the
/// wrong guess costs a feature, never a 400.
fn search_tool_type_for(model: &str) -> &'static str {
    const DYNAMIC_FILTERING_FAMILIES: &[&str] = &[
        "claude-opus-5",
        "claude-opus-4-8",
        "claude-opus-4-7",
        "claude-opus-4-6",
        "claude-sonnet-5",
        "claude-sonnet-4-6",
    ];
    if DYNAMIC_FILTERING_FAMILIES
        .iter()
        .any(|family| model.starts_with(family))
    {
        "web_search_20260209"
    } else {
        "web_search_20250305"
    }
}

fn build_request(model: &str, query: &str) -> serde_json::Value {
    let thinks_by_default = matches!(
        thinking_mode(model),
        Some(ThinkingMode::OnByDefault | ThinkingMode::AlwaysOn)
    );
    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": if thinks_by_default { THINKING_MAX_TOKENS } else { MAX_TOKENS },
        "system": SEARCH_SYSTEM_PROMPT,
        "messages": [{ "role": "user", "content": query }],
        "tools": [{
            "type": search_tool_type_for(model),
            "name": "web_search",
            "max_uses": MAX_SEARCHES,
        }],
    });
    if thinks_by_default {
        body["output_config"] = serde_json::json!({ "effort": "low" });
    }
    body
}

fn parse_response(
    body: &serde_json::Value,
    max_results: usize,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if body["stop_reason"].as_str() == Some("refusal") {
        return Err("Anthropic declined the search request (stop_reason: refusal)".into());
    }

    let blocks = body["content"].as_array().map(Vec::as_slice).unwrap_or(&[]);

    let mut answer = String::new();
    let mut sources: Vec<(String, String)> = Vec::new();

    for block in blocks {
        match block["type"].as_str() {
            Some("text") => {
                if let Some(text) = block["text"].as_str() {
                    answer.push_str(text);
                }
            }
            Some("web_search_tool_result") => {
                let content = &block["content"];
                if let Some(code) = content["error_code"].as_str() {
                    return Err(format!("Anthropic web search failed: {code}").into());
                }
                for result in content.as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    if sources.len() >= max_results {
                        break;
                    }
                    let url = result["url"].as_str().unwrap_or_default();
                    if url.is_empty() {
                        continue;
                    }
                    let title = result["title"].as_str().unwrap_or("Untitled");
                    sources.push((title.to_string(), url.to_string()));
                }
            }
            _ => {}
        }
    }

    // INVARIANT: Truncation that produced nothing usable is a FAILURE, not an empty
    // search. Returning the "no results" line here would be `Ok`, which the
    // chain treats as terminal — so a search that was merely cut short would
    // stop the chain and report "nothing found" to the user. Same class of bug
    // as `acc19104f` (fetch_news reporting failures as "no news"). A truncated
    // response that still carried sources is kept: partial is useful.
    if body["stop_reason"].as_str() == Some("max_tokens")
        && answer.trim().is_empty()
        && sources.is_empty()
    {
        return Err(
            "Anthropic search response hit its output cap before returning any result".into(),
        );
    }

    // INVARIANT: A genuinely empty result renders as the "no results" line and stays `Ok`,
    // so the chain treats it as terminal instead of re-billing the query.
    Ok(format_search_result(&answer, &sources))
}

pub struct AnthropicServerToolSearch {
    auth: AnthropicAuth,
    model: String,
    client: reqwest::Client,
}

impl AnthropicServerToolSearch {
    fn billing_provider(&self) -> crate::engine::thread_events::billing_provider::BillingProvider {
        use crate::engine::thread_events::billing_provider::BillingProvider;
        match self.auth {
            AnthropicAuth::ApiKey(_) => BillingProvider::Anthropic,
            AnthropicAuth::OAuthBearer(_) => BillingProvider::AnthropicSubscription,
        }
    }

    /// INVARIANT: `model` MUST be a model **Anthropic** serves — the caller resolves it via
    /// `provider_build::search_model_for`, which uses the user's chat model only
    /// when that model routes to Anthropic and a known Anthropic id otherwise.
    /// Passing the raw chat model would break the headline case: a user on
    /// OpenRouter would send `z-ai/glm-5.2` here and be rejected, so the
    /// fallback would fail precisely when it was needed.
    pub fn new(
        auth: AnthropicAuth,
        model: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            auth,
            model,
            client,
        })
    }
}

#[async_trait]
impl WebSearchProvider for AnthropicServerToolSearch {
    async fn search(
        &self,
        query: &str,
        max_results: usize,
        _call: CallToken,
    ) -> Result<WebSearchResult, Box<dyn std::error::Error + Send + Sync>> {
        crate::llm::metered::check_provider(crate::llm::ProviderKind::Anthropic).await?;
        let (auth_name, auth_value) = auth_header(&self.auth);
        let mut request = self
            .client
            .post(format!("{ANTHROPIC_API_BASE_URL}/messages"))
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Content-Type", "application/json")
            .header(auth_name, auth_value);
        if matches!(self.auth, AnthropicAuth::OAuthBearer(_)) {
            request = request.header("anthropic-beta", ANTHROPIC_OAUTH_BETA);
        }

        let response = request
            .json(&build_request(&self.model, query))
            .send()
            .await?;

        let error_headers = response.headers().clone();
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            if let Some(cap) = crate::llm::metered::http_spend_cap(
                crate::llm::ProviderKind::Anthropic,
                status.as_u16(),
                &error_headers,
                &body,
            )
            .await
            {
                return Err(cap.into());
            }
            return Err(
                crate::llm::ProviderError::from_http("Anthropic", status.as_u16(), &body).into(),
            );
        }

        let parsed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse Anthropic search response: {e}"))?;
        Ok(WebSearchResult {
            text: parse_response(&parsed, max_results)?,
            usage: crate::llm::usage_wire::from_json(&parsed),
            model: self.model.clone(),
            provider: Some(self.billing_provider()),
        })
    }

    fn id(&self) -> &'static str {
        "anthropic-server-tool"
    }

    fn model(&self) -> Option<&str> {
        Some(&self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_billing_distinguishes_key_and_subscription_auth() {
        for (auth, expected) in [
            (AnthropicAuth::ApiKey("fixture".into()), "anthropic"),
            (
                AnthropicAuth::OAuthBearer("fixture".into()),
                "anthropic-subscription",
            ),
        ] {
            let search = AnthropicServerToolSearch::new(auth, "model".into()).unwrap();
            assert_eq!(search.billing_provider().id(), expected);
        }
    }

    #[test]
    fn tool_version_pairs_with_the_model_family() {
        for model in [
            "claude-opus-5",
            "claude-opus-5[1m]",
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
            "claude-opus-4-8",
            "claude-sonnet-5-5",
            "claude-sonnet-4-6",
        ] {
            assert_eq!(
                search_tool_type_for(model),
                "web_search_20260209",
                "{model} accepts dynamic filtering"
            );
        }
        for model in [
            "claude-haiku-4-5",
            "claude-fable-5",
            "claude-fable-5-1",
            "claude-opus-4-5",
        ] {
            assert_eq!(
                search_tool_type_for(model),
                "web_search_20250305",
                "{model} predates dynamic filtering"
            );
        }
    }

    #[test]
    fn unknown_model_falls_back_to_the_basic_tool() {
        assert_eq!(
            search_tool_type_for("claude-something-unreleased"),
            "web_search_20250305"
        );
    }

    #[test]
    fn max_uses_is_independent_of_max_results() {
        for query in ["a", "b"] {
            assert_eq!(
                build_request("claude-opus-5", query)["tools"][0]["max_uses"],
                MAX_SEARCHES
            );
        }
        const { assert!(MAX_SEARCHES >= 1, "at least one search must be permitted") };
    }

    #[test]
    fn request_carries_model_query_and_tool() {
        let req = build_request("claude-opus-5", "rust release date");
        assert_eq!(req["model"], "claude-opus-5");
        assert_eq!(req["messages"][0]["content"], "rust release date");
        assert_eq!(req["tools"][0]["name"], "web_search");
    }

    #[test]
    fn a_model_that_thinks_by_default_searches_at_low_effort_with_room() {
        for model in [
            "claude-opus-5",
            "claude-opus-5-5",
            "claude-fable-5-1",
            "claude-sonnet-5",
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
        ] {
            let req = build_request(model, "q");
            assert_eq!(req["max_tokens"], THINKING_MAX_TOKENS, "{model}");
            assert_eq!(req["output_config"]["effort"], "low", "{model}");
            assert!(req.get("thinking").is_none(), "{model}");
        }
    }

    #[test]
    fn every_other_model_keeps_the_small_search_request() {
        for model in ["claude-haiku-4-5", "claude-opus-4-8", "claude-sonnet-4-6"] {
            let req = build_request(model, "q");
            assert_eq!(req["max_tokens"], MAX_TOKENS, "{model}");
            assert!(req.get("output_config").is_none(), "{model}");
        }
    }

    #[test]
    fn parses_answer_and_sources() {
        let body = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [
                {"type": "text", "text": "Rust 1.99 shipped in July 2026."},
                {"type": "web_search_tool_result", "content": [
                    {"type": "web_search_result", "url": "https://blog.rust-lang.org/x", "title": "Announcing Rust"},
                    {"type": "web_search_result", "url": "https://example.com/y", "title": "Coverage"}
                ]}
            ]
        });
        let out = parse_response(&body, 5).unwrap();
        assert!(out.starts_with("Rust 1.99 shipped in July 2026."), "{out}");
        assert!(out.contains("Sources:"), "{out}");
        assert!(out.contains("1. Announcing Rust"), "{out}");
        assert!(out.contains("2. Coverage"), "{out}");
    }

    #[test]
    fn honors_max_results() {
        let body = serde_json::json!({
            "content": [{"type": "web_search_tool_result", "content": [
                {"url": "https://a", "title": "A"},
                {"url": "https://b", "title": "B"},
                {"url": "https://c", "title": "C"}
            ]}]
        });
        let out = parse_response(&body, 2).unwrap();
        assert!(out.contains("1. A") && out.contains("2. B"), "{out}");
        assert!(!out.contains("3. C"), "must truncate to max_results: {out}");
    }

    #[test]
    fn tool_result_error_object_is_an_error_not_an_empty_result() {
        let body = serde_json::json!({
            "content": [
                {"type": "web_search_tool_result",
                 "content": {"type": "web_search_tool_result_error", "error_code": "max_uses_exceeded"}}
            ]
        });
        let err = parse_response(&body, 5).expect_err("an error_code must not parse as success");
        assert!(err.to_string().contains("max_uses_exceeded"), "{err}");
    }

    #[test]
    fn refusal_is_an_error() {
        let body = serde_json::json!({"stop_reason": "refusal", "content": []});
        let err = parse_response(&body, 5).expect_err("a refusal must error");
        assert!(err.to_string().contains("refusal"), "{err}");
    }

    #[test]
    fn truncated_to_nothing_is_an_error_not_an_empty_result() {
        let body = serde_json::json!({"stop_reason": "max_tokens", "content": []});
        let err = parse_response(&body, 5).expect_err("truncation with no result must error");
        assert!(err.to_string().contains("output cap"), "{err}");
    }

    #[test]
    fn truncated_with_sources_still_returns_them() {
        let body = serde_json::json!({
            "stop_reason": "max_tokens",
            "content": [{"type": "web_search_tool_result", "content": [
                {"url": "https://a", "title": "A"}
            ]}]
        });
        let out = parse_response(&body, 5).expect("partial results are still a success");
        assert!(out.contains("1. A"), "{out}");
    }

    #[test]
    fn genuinely_empty_result_is_ok_not_an_error() {
        let body = serde_json::json!({"stop_reason": "end_turn", "content": []});
        let out = parse_response(&body, 5).expect("an empty result is a success");
        assert!(out.contains("No search results found"), "{out}");
    }
}
