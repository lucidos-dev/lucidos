use async_trait::async_trait;
use std::collections::HashSet;
use std::time::Duration;

use super::{format_search_result, WebSearchProvider, WebSearchResult, SEARCH_SYSTEM_PROMPT};
use crate::llm::metered::CallToken;

/// INVARIANT: Output ceiling for the summary written over the search results. Bounds the
/// token half of the bill; the per-call search fee is bounded by this backend
/// issuing exactly one request per `web_search` tool call.
const MAX_OUTPUT_TOKENS: u32 = 2048;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

fn build_request(model: &str, query: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "instructions": SEARCH_SYSTEM_PROMPT,
        "input": query,
        "tools": [{ "type": "web_search" }],
        "max_output_tokens": MAX_OUTPUT_TOKENS,
        // INVARIANT: One-shot call with nothing to chain from, so the `store: true`
        // default would only hand OpenAI a 30-day copy of the query and answer
        // that we never read back. Opt out.
        "store": false,
    })
}

pub(crate) fn parse_response(
    body: &serde_json::Value,
    max_results: usize,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(message) = body["error"]["message"].as_str() {
        return Err(format!("OpenAI search failed: {message}").into());
    }

    let mut answer = String::new();
    let mut sources: Vec<(String, String)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for item in body["output"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        if item["type"].as_str() != Some("message") {
            continue;
        }
        for part in item["content"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            if part["type"].as_str() != Some("output_text") {
                continue;
            }
            if let Some(text) = part["text"].as_str() {
                answer.push_str(text);
            }
            for note in part["annotations"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
            {
                if note["type"].as_str() != Some("url_citation") {
                    continue;
                }
                let url = note["url"].as_str().unwrap_or_default();
                if url.is_empty() || sources.len() >= max_results || !seen.insert(url.to_string()) {
                    continue;
                }
                let title = note["title"].as_str().unwrap_or("Untitled");
                sources.push((title.to_string(), url.to_string()));
            }
        }
    }

    // INVARIANT: An `incomplete` response that produced nothing usable is a FAILURE, not
    // an empty search. Rendering the "no results" line would be `Ok`, which the
    // chain treats as terminal — so a run merely cut short by the output cap
    // would stop the chain and tell the user nothing was found. Reasoning
    // models spend `max_output_tokens` on reasoning too, which makes this the
    // likely truncation path rather than a rare one. Partial results are kept.
    if body["status"].as_str() == Some("incomplete")
        && answer.trim().is_empty()
        && sources.is_empty()
    {
        let reason = body["incomplete_details"]["reason"]
            .as_str()
            .unwrap_or("unknown");
        return Err(format!(
            "OpenAI search response was incomplete ({reason}) before returning any result"
        )
        .into());
    }

    Ok(format_search_result(&answer, &sources))
}

pub struct OpenAiResponsesSearch {
    api_key: String,
    model: String,
    responses_url: String,
    billing_provider: Option<crate::engine::thread_events::billing_provider::BillingProvider>,
    client: reqwest::Client,
}

impl OpenAiResponsesSearch {
    /// INVARIANT: `base_url` is the OpenAI-compatible API root; `/responses` is appended.
    ///
    /// `model` MUST be a model **OpenAI** serves on the Responses API — the
    /// caller resolves it via `provider_build::search_model_for`, which uses the
    /// user's chat model only when it routes to OpenAI and a known OpenAI id
    /// otherwise. Handing it an Anthropic or OpenRouter id would reject on every
    /// cross-provider fallback, which is the case this backend exists to serve.
    pub fn new(
        api_key: String,
        model: String,
        base_url: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            api_key,
            model,
            billing_provider: (base_url.trim_end_matches('/')
                == crate::llm::OPENAI_DEFAULT_BASE_URL)
                .then_some(crate::engine::thread_events::billing_provider::BillingProvider::Openai),
            responses_url: format!("{}/responses", base_url.trim_end_matches('/')),
            client,
        })
    }
}

#[async_trait]
impl WebSearchProvider for OpenAiResponsesSearch {
    async fn search(
        &self,
        query: &str,
        max_results: usize,
        _call: CallToken,
    ) -> Result<WebSearchResult, Box<dyn std::error::Error + Send + Sync>> {
        crate::llm::metered::check_provider(crate::llm::ProviderKind::OpenAi).await?;
        let response = self
            .client
            .post(&self.responses_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&build_request(&self.model, query))
            .send()
            .await?;

        let error_headers = response.headers().clone();
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            if let Some(cap) = crate::llm::metered::http_spend_cap(
                crate::llm::ProviderKind::OpenAi,
                status.as_u16(),
                &error_headers,
                &body,
            )
            .await
            {
                return Err(cap.into());
            }
            return Err(
                crate::llm::ProviderError::from_http("OpenAI", status.as_u16(), &body).into(),
            );
        }

        let parsed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse OpenAI search response: {e}"))?;
        Ok(WebSearchResult {
            text: parse_response(&parsed, max_results)?,
            usage: crate::llm::usage_wire::from_json(&parsed),
            model: self.model.clone(),
            provider: self.billing_provider,
        })
    }

    fn id(&self) -> &'static str {
        "openai-responses"
    }

    fn model(&self) -> Option<&str> {
        Some(&self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_url_is_derived_from_the_base() {
        let s = OpenAiResponsesSearch::new(
            "sk-test".into(),
            "gpt-5.5".into(),
            "https://api.openai.com/v1/",
        )
        .unwrap();
        assert_eq!(s.responses_url, "https://api.openai.com/v1/responses");
    }

    #[test]
    fn search_billing_requires_the_canonical_openai_endpoint() {
        for (base, expected) in [
            (
                crate::llm::OPENAI_DEFAULT_BASE_URL,
                Some(crate::engine::thread_events::billing_provider::BillingProvider::Openai),
            ),
            (
                "https://api.openai.com/v1/",
                Some(crate::engine::thread_events::billing_provider::BillingProvider::Openai),
            ),
            ("https://example.com/v1", None),
            ("https://api.openai.com.example.com/v1", None),
            ("http://api.openai.com/v1", None),
        ] {
            let search =
                OpenAiResponsesSearch::new("fixture".into(), "model".into(), base).unwrap();
            assert_eq!(search.billing_provider, expected);
        }
    }

    #[test]
    fn request_declares_the_web_search_tool() {
        let req = build_request("gpt-5.5", "rust release date");
        assert_eq!(req["model"], "gpt-5.5");
        assert_eq!(req["input"], "rust release date");
        assert_eq!(req["tools"][0]["type"], "web_search");
        assert_eq!(req["max_output_tokens"], MAX_OUTPUT_TOKENS);
    }

    #[test]
    fn request_opts_out_of_server_side_storage() {
        let req = build_request("gpt-5.5", "rust release date");
        assert_eq!(req["store"], false);
    }

    #[test]
    fn parses_answer_and_url_citations() {
        let body = serde_json::json!({
            "output": [
                {"type": "web_search_call", "status": "completed"},
                {"type": "message", "content": [{
                    "type": "output_text",
                    "text": "Rust 1.99 shipped in July 2026.",
                    "annotations": [
                        {"type": "url_citation", "url": "https://blog.rust-lang.org/x", "title": "Announcing Rust"},
                        {"type": "url_citation", "url": "https://example.com/y", "title": "Coverage"}
                    ]
                }]}
            ]
        });
        let out = parse_response(&body, 5).unwrap();
        assert!(out.starts_with("Rust 1.99 shipped in July 2026."), "{out}");
        assert!(out.contains("1. Announcing Rust"), "{out}");
        assert!(out.contains("2. Coverage"), "{out}");
    }

    #[test]
    fn repeated_citations_are_deduplicated() {
        let body = serde_json::json!({
            "output": [{"type": "message", "content": [{
                "type": "output_text",
                "text": "Answer.",
                "annotations": [
                    {"type": "url_citation", "url": "https://a", "title": "A"},
                    {"type": "url_citation", "url": "https://a", "title": "A"},
                    {"type": "url_citation", "url": "https://b", "title": "B"}
                ]
            }]}]
        });
        let out = parse_response(&body, 5).unwrap();
        assert!(out.contains("1. A") && out.contains("2. B"), "{out}");
        assert_eq!(out.matches("https://a").count(), 1, "deduped: {out}");
    }

    #[test]
    fn honors_max_results() {
        let body = serde_json::json!({
            "output": [{"type": "message", "content": [{
                "type": "output_text", "text": "Answer.",
                "annotations": [
                    {"type": "url_citation", "url": "https://a", "title": "A"},
                    {"type": "url_citation", "url": "https://b", "title": "B"},
                    {"type": "url_citation", "url": "https://c", "title": "C"}
                ]
            }]}]
        });
        let out = parse_response(&body, 2).unwrap();
        assert!(!out.contains("3. C"), "must truncate to max_results: {out}");
    }

    #[test]
    fn top_level_error_is_reported() {
        let body = serde_json::json!({"error": {"message": "model not found"}});
        let err = parse_response(&body, 5).expect_err("an error body must not parse as success");
        assert!(err.to_string().contains("model not found"), "{err}");
    }

    #[test]
    fn incomplete_with_nothing_is_an_error() {
        let body = serde_json::json!({
            "status": "incomplete",
            "incomplete_details": {"reason": "max_output_tokens"},
            "output": []
        });
        let err = parse_response(&body, 5).expect_err("an incomplete empty run must error");
        assert!(err.to_string().contains("max_output_tokens"), "{err}");
    }

    #[test]
    fn incomplete_with_partial_output_is_kept() {
        let body = serde_json::json!({
            "status": "incomplete",
            "incomplete_details": {"reason": "max_output_tokens"},
            "output": [{"type": "message", "content": [
                {"type": "output_text", "text": "Partial answer.", "annotations": []}
            ]}]
        });
        let out = parse_response(&body, 5).expect("partial output is still a success");
        assert!(out.contains("Partial answer."), "{out}");
    }

    #[test]
    fn genuinely_empty_result_is_ok_not_an_error() {
        let body = serde_json::json!({"output": []});
        let out = parse_response(&body, 5).expect("an empty result is a success");
        assert!(out.contains("No search results found"), "{out}");
    }
}
