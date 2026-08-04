//! Direct-Anthropic backend for `web_search` — the `web_search` **server tool**
//! on `/v1/messages`.
//!
//! Anthropic runs the search itself and returns the results inline, so this is a
//! single round trip with no client-side execution loop and no extra credential:
//! it reuses the Anthropic key the user already configured for chat.
//!
//! Deliberately a small one-shot call rather than a reuse of
//! [`crate::llm::anthropic::AnthropicProvider`] — that path is built for
//! streaming chat with a tool loop, none of which applies here. It does share
//! that module's auth-header and API-version constants so the two can't drift.

use async_trait::async_trait;
use std::time::Duration;

use super::{format_search_result, WebSearchProvider, SEARCH_SYSTEM_PROMPT};
use crate::llm::anthropic::chat::{auth_header, ANTHROPIC_VERSION};
use crate::llm::anthropic::{AnthropicAuth, ANTHROPIC_OAUTH_BETA};
use crate::llm::ANTHROPIC_API_BASE_URL;

/// Output ceiling for the summary Claude writes over the search results. Small
/// on purpose: it bounds the cost of a search whatever model tier the caller
/// picks, so reusing the user's own Anthropic chat model (see
/// `provider_build::search_model_for`) can't turn one search into a large bill.
const MAX_TOKENS: u32 = 2048;

/// Whole-request timeout. A grounded search that hasn't answered by now is not
/// going to; failing lets the chain try the next backend.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// How many searches Claude may run to answer one `web_search` tool call.
///
/// Deliberately NOT tied to `max_results`, which is a *display* cap on how many
/// sources to list. They measure different things, and conflating them turns a
/// caller asking for more sources into a caller silently paying for more
/// searches — `max_results: 20` would authorise 10 billed searches for one tool
/// call. Three is enough for the model to cross-check a claim without turning a
/// single tool call into a research session.
const MAX_SEARCHES: u32 = 3;

/// Pick the server-tool version the model will accept.
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

/// Build the `/v1/messages` body. Pure, so the shape is unit-testable without
/// a network call.
fn build_request(model: &str, query: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": SEARCH_SYSTEM_PROMPT,
        "messages": [{ "role": "user", "content": query }],
        "tools": [{
            "type": search_tool_type_for(model),
            "name": "web_search",
            "max_uses": MAX_SEARCHES,
        }],
    })
}

/// Flatten the response into the same shape every other backend returns: answer
/// text, then a numbered `Sources:` list.
///
/// Two response shapes need care:
/// - A `web_search_tool_result` block's `content` is a **list** on success but
///   an **object** carrying `error_code` on failure. Indexing it blindly would
///   silently yield zero sources for a failed search that still returned 200.
/// - `stop_reason: "refusal"` is a 200 with no usable content, so it must be
///   reported as an error rather than as an empty answer.
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
                // Error shape: an object with `error_code`, not a result list.
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

    // Truncation that produced nothing usable is a FAILURE, not an empty
    // search. Returning the "no results" line here would be `Ok`, which the
    // chain treats as terminal — so a search that was merely cut short would
    // stop the chain and report "nothing found" to the user. Same class of bug
    // as `acc19104f` (fetch_news reporting failures as "no news"). A truncated
    // response that still carried sources is kept: partial is useful.
    if body["stop_reason"].as_str() == Some("max_tokens")
        && answer.trim().is_empty()
        && sources.is_empty()
    {
        return Err(format!(
            "Anthropic search response hit the {MAX_TOKENS}-token output cap before returning \
             any result"
        )
        .into());
    }

    // A genuinely empty result renders as the "no results" line and stays `Ok`,
    // so the chain treats it as terminal instead of re-billing the query.
    Ok(format_search_result(&answer, &sources))
}

pub struct AnthropicServerToolSearch {
    auth: AnthropicAuth,
    model: String,
    client: reqwest::Client,
}

impl AnthropicServerToolSearch {
    /// `model` MUST be a model **Anthropic** serves — the caller resolves it via
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
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let (auth_name, auth_value) = auth_header(&self.auth);
        let mut request = self
            .client
            .post(format!("{ANTHROPIC_API_BASE_URL}/messages"))
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Content-Type", "application/json")
            .header(auth_name, auth_value);
        // A subscription OAuth token is only accepted on the Messages API with
        // this beta flag; an API key must not send it.
        if matches!(self.auth, AnthropicAuth::OAuthBearer(_)) {
            request = request.header("anthropic-beta", ANTHROPIC_OAUTH_BETA);
        }

        let response = request
            .json(&build_request(&self.model, query))
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(format!("Anthropic search API error ({status}): {body}").into());
        }

        let parsed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse Anthropic search response: {e}"))?;
        parse_response(&parsed, max_results)
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

    /// The tool version must match the model family or the API 400s.
    #[test]
    fn tool_version_pairs_with_the_model_family() {
        for model in [
            "claude-opus-5",
            "claude-opus-5[1m]",
            "claude-opus-4-8",
            "claude-sonnet-4-6",
        ] {
            assert_eq!(
                search_tool_type_for(model),
                "web_search_20260209",
                "{model} accepts dynamic filtering"
            );
        }
        for model in ["claude-haiku-4-5", "claude-fable-5", "claude-opus-4-5"] {
            assert_eq!(
                search_tool_type_for(model),
                "web_search_20250305",
                "{model} predates dynamic filtering"
            );
        }
    }

    /// An unrecognized model must fall to the basic variant — accepted
    /// everywhere — rather than the newer one, which would 400.
    #[test]
    fn unknown_model_falls_back_to_the_basic_tool() {
        assert_eq!(
            search_tool_type_for("claude-something-unreleased"),
            "web_search_20250305"
        );
    }

    /// `max_uses` is clamped so one `web_search` call can't fan out into an
    /// unbounded number of billed searches.
    /// `max_uses` bounds SEARCHES and must not track `max_results`, which is a
    /// display cap on sources. Tying them together made a caller asking for more
    /// sources silently pay for more searches.
    #[test]
    fn max_uses_is_independent_of_max_results() {
        for query in ["a", "b"] {
            assert_eq!(
                build_request("claude-opus-5", query)["tools"][0]["max_uses"],
                MAX_SEARCHES
            );
        }
        // A const block, so a MAX_SEARCHES of 0 fails to COMPILE rather than
        // failing this one test — the bound is a property of the constant, not
        // of any particular run.
        const { assert!(MAX_SEARCHES >= 1, "at least one search must be permitted") };
    }

    #[test]
    fn request_carries_model_query_and_tool() {
        let req = build_request("claude-opus-5", "rust release date");
        assert_eq!(req["model"], "claude-opus-5");
        assert_eq!(req["messages"][0]["content"], "rust release date");
        assert_eq!(req["tools"][0]["name"], "web_search");
        assert_eq!(req["max_tokens"], MAX_TOKENS);
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

    /// A failed search returns HTTP 200 with an `error_code` OBJECT where the
    /// result list would be. Treating that as "zero sources" would report a
    /// broken search as an empty one and stop the chain from falling through.
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

    /// A refusal is a 200 with no usable content — must surface as an error, so
    /// the chain can try another backend rather than returning nothing.
    #[test]
    fn refusal_is_an_error() {
        let body = serde_json::json!({"stop_reason": "refusal", "content": []});
        let err = parse_response(&body, 5).expect_err("a refusal must error");
        assert!(err.to_string().contains("refusal"), "{err}");
    }

    /// Truncated-to-nothing must be an ERROR so the chain falls through. As
    /// `Ok`, it would render "no results found" and stop the chain — reporting
    /// a cut-short search as a genuinely empty one.
    #[test]
    fn truncated_to_nothing_is_an_error_not_an_empty_result() {
        let body = serde_json::json!({"stop_reason": "max_tokens", "content": []});
        let err = parse_response(&body, 5).expect_err("truncation with no result must error");
        assert!(err.to_string().contains("output cap"), "{err}");
    }

    /// …but a truncated response that still carried sources keeps them: partial
    /// is useful, and erroring would discard a real answer.
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

    /// A genuinely empty search is `Ok` — the chain must treat it as terminal
    /// rather than re-running the query against every other backend.
    #[test]
    fn genuinely_empty_result_is_ok_not_an_error() {
        let body = serde_json::json!({"stop_reason": "end_turn", "content": []});
        let out = parse_response(&body, 5).expect("an empty result is a success");
        assert!(out.contains("No search results found"), "{out}");
    }
}
