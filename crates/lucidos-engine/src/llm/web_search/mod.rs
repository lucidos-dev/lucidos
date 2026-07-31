//! Backends for the `web_search` LLM tool, and the chain that picks one.
//!
//! # Why this is not routed by the chat model
//!
//! Web search is a *background capability*, like memory extraction — not a
//! property of whichever model the user happens to be chatting with. So the
//! chain resolves over the **set of configured providers**, in a fixed
//! preference order, independent of `model_registry` routing.
//!
//! That is what makes the fallback work: a user chatting on OpenRouter (which
//! has no search tool of its own) still gets web search as long as *some*
//! search-capable provider is configured.
//!
//! # Why provider-native, and not a search vendor
//!
//! Two doors are deliberately closed (see
//! `docs/plans/2026-07-27-web-search-provider-routing.md` for the evidence):
//!
//! - **Keyless scraping of a general engine** — tried and lost. Commit
//!   `8b917c19b` replaced a DuckDuckGo backend with Gemini grounding because
//!   DuckDuckGo served CAPTCHAs. General engines block unauthenticated
//!   datacenter traffic by design; this would fail again.
//! - **A dedicated search vendor** (Brave / Tavily / Exa / Google CSE) — would
//!   force a credential *and a credit card and a per-search bill* on every user
//!   for something their LLM provider already bundles. Google's Custom Search
//!   JSON API is closed to new customers and shuts down 2027-01-01; Bing's was
//!   retired in 2025; Brave dropped its free tier in 2026-02.
//!
//! The accepted cost: results are **not** uniform across users — a user on
//! Anthropic gets Anthropic's index, one on Vertex gets Google's.

mod anthropic;
mod openai;
mod vertex;

pub use anthropic::AnthropicServerToolSearch;
pub use openai::OpenAiResponsesSearch;
pub use vertex::VertexGroundingSearch;

use async_trait::async_trait;
use std::sync::Arc;

/// One web-search backend. Implementations wrap whatever search facility a
/// configured LLM provider exposes.
#[async_trait]
pub trait WebSearchProvider: Send + Sync {
    /// Run a search and return a formatted result: the answer text, then a
    /// `Sources:` list of at most `max_results` numbered entries.
    ///
    /// **`Ok` means the backend answered**, including a legitimate "no results
    /// found" — [`WebSearchChain`] treats that as terminal. Reserve `Err` for
    /// availability failures (transport error, auth rejection, model/endpoint
    /// missing), which are what the chain falls through on.
    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    /// Stable identifier for log lines and the exhausted-chain error. Never
    /// contains any part of a credential.
    fn id(&self) -> &'static str;

    /// The model this backend sends, when it has one. Surfaced so the boot log
    /// says which model search runs on, and so the build path can be tested for
    /// the property that matters: each backend must get a model valid for *its
    /// own* provider, not whatever the user happens to chat with.
    fn model(&self) -> Option<&str> {
        None
    }
}

/// Instruction every backend gives its model: summarise what the search found,
/// don't answer from memory. One copy — three near-identical prompts would drift
/// into three subtly different search behaviors.
pub(crate) const SEARCH_SYSTEM_PROMPT: &str =
    "You are a web search assistant. Search the web thoroughly and return comprehensive, \
     detailed results with sources. Focus on finding and presenting relevant information from \
     search results rather than answering from your own knowledge.";

/// Render a backend's answer into the one shape every backend returns: the
/// answer text, then a numbered `Sources:` list of `(title, url)` pairs.
///
/// Shared so the backends can't drift into three slightly different renderings
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

/// Message returned when no search-capable provider is configured. Names the
/// three that work and where to add one, so the failure is actionable wherever
/// it surfaces — mirrors the `EMBEDDER_UNAVAILABLE` convention in
/// `memory::embedder_slot`.
pub const NO_SEARCH_BACKEND: &str = "web_search has no configured backend. It runs on whichever \
     LLM provider you have set up — Vertex AI (Gemini Google Search grounding), Anthropic (the \
     web_search server tool), or OpenAI (Responses web search). Add one under Settings → Models → \
     Providers. OpenRouter and local OpenAI-compatible endpoints expose no web search tool, so \
     configuring one of the three above is required even if you chat on another provider.";

/// The configured backends in preference order, tried until one answers.
///
/// Order is fixed rather than user-selectable (Vertex → Anthropic → OpenAI):
/// Vertex first so existing workspaces see no change in behavior, then
/// Anthropic before OpenAI because Anthropic's server tool carries no per-call
/// fee while OpenAI's Responses `web_search` bills per call on top of tokens.
pub struct WebSearchChain {
    backends: Vec<Arc<dyn WebSearchProvider>>,
}

impl WebSearchChain {
    pub fn new(backends: Vec<Arc<dyn WebSearchProvider>>) -> Self {
        Self { backends }
    }

    /// An empty chain — no search-capable provider configured. Every search
    /// returns [`NO_SEARCH_BACKEND`].
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Ids of the configured backends, in order. For boot logging.
    pub fn backend_ids(&self) -> Vec<&'static str> {
        self.backends.iter().map(|b| b.id()).collect()
    }

    /// `(id, model)` per backend, in order — the boot log's detail line, and
    /// what lets a test assert each backend got a model its own provider can
    /// actually serve.
    pub fn backend_models(&self) -> Vec<(&'static str, Option<&str>)> {
        self.backends.iter().map(|b| (b.id(), b.model())).collect()
    }
}

#[async_trait]
impl WebSearchProvider for WebSearchChain {
    /// Walk the backends in order, returning the first `Ok`.
    ///
    /// Only an `Err` — an availability failure — advances to the next backend.
    /// A successful "no results found" answer stops here; falling through on it
    /// would re-run and re-bill every empty query against every configured
    /// provider.
    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let mut failures: Vec<String> = Vec::new();

        for backend in &self.backends {
            match backend.search(query, max_results).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    // Not silent: a backend dropping out is worth a log line
                    // even when a later one rescues the search.
                    crate::log!(
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

    /// Records how many times it was asked, so a test can prove a later backend
    /// was never consulted.
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
        ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &self.outcome {
                Ok(body) => Ok(body.clone()),
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

    /// Nothing at all renders as the shared "no results" line — the string the
    /// chain treats as a terminal success rather than an availability failure.
    #[test]
    fn formatter_renders_the_no_results_line_when_wholly_empty() {
        assert_eq!(format_search_result("   ", &[]), "No search results found.");
    }

    /// Sources with no answer text still render, without a leading blank line.
    #[test]
    fn formatter_handles_sources_without_answer_text() {
        let out = format_search_result("", &[("Only".to_string(), "https://a".to_string())]);
        assert!(out.starts_with("Sources:"), "{out}");
        assert!(out.contains("1. Only"), "{out}");
    }

    /// The first configured backend wins — this is what keeps an existing
    /// Vertex workspace on Vertex.
    #[tokio::test]
    async fn first_backend_wins() {
        let first = StubBackend::ok("vertex-grounding", "from vertex");
        let second = StubBackend::ok("anthropic-server-tool", "from anthropic");
        let chain = WebSearchChain::new(vec![first.clone(), second.clone()]);

        assert_eq!(chain.search("q", 5).await.unwrap(), "from vertex");
        assert_eq!(
            second.calls(),
            0,
            "the second backend must not be consulted"
        );
    }

    /// An availability failure falls through to the next backend — the fallback
    /// that lets a search-incapable primary provider still serve search.
    #[tokio::test]
    async fn availability_failure_falls_through() {
        let first = StubBackend::err("vertex-grounding", "404 Not Found");
        let second = StubBackend::ok("anthropic-server-tool", "from anthropic");
        let chain = WebSearchChain::new(vec![first.clone(), second.clone()]);

        assert_eq!(chain.search("q", 5).await.unwrap(), "from anthropic");
        assert_eq!(second.calls(), 1);
    }

    /// A legitimate empty result is a SUCCESS and must stop the chain. Falling
    /// through here would bill every configured provider for every zero-result
    /// query.
    #[tokio::test]
    async fn no_results_is_terminal_not_a_fallthrough() {
        let first = StubBackend::ok("vertex-grounding", "No search results found for: xyzzy");
        let second = StubBackend::ok("anthropic-server-tool", "from anthropic");
        let chain = WebSearchChain::new(vec![first.clone(), second.clone()]);

        let out = chain.search("xyzzy", 5).await.unwrap();
        assert!(out.contains("No search results found"), "{out}");
        assert_eq!(
            second.calls(),
            0,
            "an empty result must not re-run the query on the next backend"
        );
    }

    /// Nothing configured → one actionable error naming the providers and where
    /// to add one. Never an empty string passed off as a result.
    #[tokio::test]
    async fn empty_chain_explains_what_to_configure() {
        let err = WebSearchChain::empty()
            .search("q", 5)
            .await
            .expect_err("an empty chain must error");
        let msg = err.to_string();
        for needle in [
            "Vertex AI",
            "Anthropic",
            "OpenAI",
            "Settings → Models → Providers",
        ] {
            assert!(msg.contains(needle), "missing {needle:?} in: {msg}");
        }
    }

    /// Every backend down → one error naming each one that was tried and why,
    /// rather than only the last failure.
    #[tokio::test]
    async fn exhausted_chain_reports_every_backend() {
        let chain = WebSearchChain::new(vec![
            StubBackend::err("vertex-grounding", "404 Not Found"),
            StubBackend::err("anthropic-server-tool", "401 Unauthorized"),
        ]);
        let msg = chain.search("q", 5).await.unwrap_err().to_string();
        assert!(msg.contains("vertex-grounding: 404 Not Found"), "{msg}");
        assert!(
            msg.contains("anthropic-server-tool: 401 Unauthorized"),
            "{msg}"
        );
    }
}
