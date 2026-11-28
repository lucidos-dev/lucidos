mod anthropic;
mod chatgpt_plan;
pub(crate) mod openai;
mod vertex;

pub use anthropic::AnthropicServerToolSearch;
pub use chatgpt_plan::ChatGptPlanSearch;
pub use openai::OpenAiResponsesSearch;
pub use vertex::VertexGroundingSearch;

use async_trait::async_trait;
use std::sync::Arc;

use crate::llm::api_usage::ApiUsage;
use crate::llm::metered::CallToken;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub struct WebSearchResult {
    pub text: String,
    pub usage: Option<ApiUsage>,
    pub model: String,
    pub provider: Option<crate::engine::thread_events::billing_provider::BillingProvider>,
}

#[async_trait]
pub trait WebSearchProvider: Send + Sync {
    async fn search(
        &self,
        query: &str,
        max_results: usize,
        call: CallToken,
    ) -> Result<WebSearchResult, BoxError>;

    /// INVARIANT: Stable identifier for log lines and the exhausted-chain error. Never
    /// contains any part of a credential.
    fn id(&self) -> &'static str;

    fn model(&self) -> Option<&str> {
        None
    }
}

pub(crate) const SEARCH_SYSTEM_PROMPT: &str =
    "You are a web search assistant. Search the web thoroughly and return comprehensive, \
     detailed results with sources. Focus on finding and presenting relevant information from \
     search results rather than answering from your own knowledge.";

/// INVARIANT: Render a backend's answer into the one shape every backend returns: the
/// answer text, then a numbered `Sources:` list of `(title, url)` pairs.
///
/// Shared so the backends can't drift into several slightly different renderings
/// of the same result. Callers do their own selection (capping to
/// `max_results`, de-duplicating) before handing the pairs over.
///
/// An answer with no text and no sources renders as the "no results" line —
/// returned by callers as `Ok`, because a genuinely empty search is a success
/// and must stop the chain rather than falling through (see
/// [`WebSearchChain::search`]).
pub(super) fn format_search_result(answer: &str, sources: &[(String, String)]) -> String {
    let answer = answer.trim();
    if answer.is_empty() && sources.is_empty() {
        return "No search results found.".to_string();
    }
    let mut out = String::from(answer);
    if !sources.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str("Sources:\n\n");
        let rendered: Vec<String> = sources
            .iter()
            .enumerate()
            .map(|(i, (title, url))| format!("{}. {}\n   {}", i + 1, title, url))
            .collect();
        out.push_str(&rendered.join("\n\n"));
    }
    out
}

pub const NO_SEARCH_BACKEND: &str = concat!(
    "web_search has no configured backend. It runs on whichever \
     LLM provider you have set up — Vertex AI (Gemini Google Search grounding), Anthropic (the \
     web_search server tool), OpenAI (Responses web search), or a signed-in ChatGPT plan. Add one under ",
    providers_settings!(),
    ". OpenRouter and local OpenAI-compatible endpoints expose no web search tool, so \
     configuring one of the providers above is required even if you chat on another provider."
);

/// INVARIANT: The configured backends in preference order, tried until one answers.
///
/// Order is fixed rather than user-selectable (Vertex → Anthropic → OpenAI key → ChatGPT plan):
/// Vertex first so existing workspaces see no change in behavior, then
/// Anthropic before OpenAI because Anthropic's server tool carries no per-call
/// fee while OpenAI's Responses `web_search` bills per call on top of tokens.
/// The plan is last, preserving existing choices when another backend is configured.
pub struct WebSearchChain {
    backends: Vec<Arc<dyn WebSearchProvider>>,
}

impl WebSearchChain {
    pub fn new(backends: Vec<Arc<dyn WebSearchProvider>>) -> Self {
        Self { backends }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    pub fn backend_ids(&self) -> Vec<&'static str> {
        self.backends.iter().map(|b| b.id()).collect()
    }

    pub fn backend_models(&self) -> Vec<(&'static str, Option<&str>)> {
        self.backends.iter().map(|b| (b.id(), b.model())).collect()
    }
}

#[async_trait]
impl WebSearchProvider for WebSearchChain {
    /// INVARIANT: Walk the backends in order, returning the first `Ok`.
    ///
    /// Only an `Err` — an availability failure — advances to the next backend.
    /// A successful "no results found" answer stops here; falling through on it
    /// would re-run and re-bill every empty query against every configured
    /// provider.
    async fn search(
        &self,
        query: &str,
        max_results: usize,
        call: CallToken,
    ) -> Result<WebSearchResult, BoxError> {
        let mut failures: Vec<String> = Vec::new();

        for backend in &self.backends {
            crate::llm::call_trace::backend_began();
            match backend.search(query, max_results, call).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    crate::log_legacy!(
                        "[Search] backend '{}' unavailable, trying the next: {}",
                        backend.id(),
                        e
                    );
                    failures.push(format!("{}: {}", backend.id(), e));
                }
            }
        }

        if failures.is_empty() {
            return Err(NO_SEARCH_BACKEND.into());
        }
        Err(format!(
            "web_search failed on every configured backend — {}",
            failures.join("; ")
        )
        .into())
    }

    fn id(&self) -> &'static str {
        "chain"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct StubBackend {
        id: &'static str,
        outcome: Result<String, String>,
        calls: AtomicUsize,
    }

    impl StubBackend {
        fn ok(id: &'static str, body: &str) -> Arc<Self> {
            Arc::new(Self {
                id,
                outcome: Ok(body.to_string()),
                calls: AtomicUsize::new(0),
            })
        }
        fn err(id: &'static str, msg: &str) -> Arc<Self> {
            Arc::new(Self {
                id,
                outcome: Err(msg.to_string()),
                calls: AtomicUsize::new(0),
            })
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl WebSearchProvider for StubBackend {
        async fn search(
            &self,
            _query: &str,
            _max_results: usize,
            _call: CallToken,
        ) -> Result<WebSearchResult, BoxError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &self.outcome {
                Ok(body) => Ok(WebSearchResult {
                    text: body.clone(),
                    usage: None,
                    model: self.id.to_string(),
                    provider: None,
                }),
                Err(msg) => Err(msg.clone().into()),
            }
        }
        fn id(&self) -> &'static str {
            self.id
        }
    }

    #[test]
    fn formatter_numbers_sources_from_one() {
        let out = format_search_result(
            "  Answer text.  ",
            &[
                ("First".to_string(), "https://a".to_string()),
                ("Second".to_string(), "https://b".to_string()),
            ],
        );
        assert!(out.starts_with("Answer text."), "answer is trimmed: {out}");
        assert!(out.contains("1. First\n   https://a"), "{out}");
        assert!(out.contains("2. Second\n   https://b"), "{out}");
    }

    #[test]
    fn formatter_renders_the_no_results_line_when_wholly_empty() {
        assert_eq!(format_search_result("   ", &[]), "No search results found.");
    }

    #[test]
    fn formatter_handles_sources_without_answer_text() {
        let out = format_search_result("", &[("Only".to_string(), "https://a".to_string())]);
        assert!(out.starts_with("Sources:"), "{out}");
        assert!(out.contains("1. Only"), "{out}");
    }

    #[tokio::test]
    async fn first_backend_wins() {
        let first = StubBackend::ok("vertex-grounding", "from vertex");
        let second = StubBackend::ok("anthropic-server-tool", "from anthropic");
        let chain = WebSearchChain::new(vec![first.clone(), second.clone()]);

        assert_eq!(
            chain
                .search("q", 5, CallToken::for_test())
                .await
                .unwrap()
                .text,
            "from vertex"
        );
        assert_eq!(
            second.calls(),
            0,
            "the second backend must not be consulted"
        );
    }

    #[tokio::test]
    async fn availability_failure_falls_through() {
        let first = StubBackend::err("vertex-grounding", "404 Not Found");
        let second = StubBackend::ok("anthropic-server-tool", "from anthropic");
        let chain = WebSearchChain::new(vec![first.clone(), second.clone()]);

        assert_eq!(
            chain
                .search("q", 5, CallToken::for_test())
                .await
                .unwrap()
                .text,
            "from anthropic"
        );
        assert_eq!(second.calls(), 1);
    }

    #[tokio::test]
    async fn no_results_is_terminal_not_a_fallthrough() {
        let first = StubBackend::ok("vertex-grounding", "No search results found for: xyzzy");
        let second = StubBackend::ok("anthropic-server-tool", "from anthropic");
        let chain = WebSearchChain::new(vec![first.clone(), second.clone()]);

        let out = chain
            .search("xyzzy", 5, CallToken::for_test())
            .await
            .unwrap()
            .text;
        assert!(out.contains("No search results found"), "{out}");
        assert_eq!(
            second.calls(),
            0,
            "an empty result must not re-run the query on the next backend"
        );
    }

    #[tokio::test]
    async fn empty_chain_explains_what_to_configure() {
        let err = WebSearchChain::empty()
            .search("q", 5, CallToken::for_test())
            .await
            .err()
            .expect("an empty chain must error");
        let msg = err.to_string();
        for needle in ["Vertex AI", "Anthropic", "OpenAI", providers_settings!()] {
            assert!(msg.contains(needle), "missing {needle:?} in: {msg}");
        }
    }

    #[tokio::test]
    async fn exhausted_chain_reports_every_backend() {
        let chain = WebSearchChain::new(vec![
            StubBackend::err("vertex-grounding", "404 Not Found"),
            StubBackend::err("anthropic-server-tool", "401 Unauthorized"),
        ]);
        let msg = chain
            .search("q", 5, CallToken::for_test())
            .await
            .err()
            .expect("every backend failed")
            .to_string();
        assert!(msg.contains("vertex-grounding: 404 Not Found"), "{msg}");
        assert!(
            msg.contains("anthropic-server-tool: 401 Unauthorized"),
            "{msg}"
        );
    }
}
