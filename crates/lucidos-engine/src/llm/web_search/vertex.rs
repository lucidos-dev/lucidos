//! Vertex AI backend for `web_search` — Gemini with Google Search grounding.
//!
//! The incumbent backend, and first in the chain so existing workspaces see no
//! change in behavior. Owns its own [`VertexProvider`] built from the engine's
//! Vertex config rather than borrowing the `MemoryExtractor`'s: web search and
//! memory extraction are unrelated capabilities that merely happened to share a
//! provider, and that coupling is what left every non-Vertex user with no
//! search at all.

use async_trait::async_trait;

use super::WebSearchProvider;
use crate::llm::vertex::{LocationHandle, TokenCache, VertexProvider};

/// Model the grounded search runs on. Only used to construct the provider —
/// [`VertexProvider::search_with_grounding`] names its own model per request.
const GROUNDING_MODEL: &str = "gemini-2.5-flash-lite";

pub struct VertexGroundingSearch {
    provider: VertexProvider,
}

impl VertexGroundingSearch {
    /// Build from the engine's resolved Vertex config. Shares the caller's
    /// [`TokenCache`] so this backend doesn't mint its own access tokens, and
    /// the live [`LocationHandle`] so it stays consistent with the rest of the
    /// Vertex surface — note the *grounding request itself* ignores that
    /// location and always targets the global endpoint (see
    /// `VertexProvider::global_gemini_endpoint`).
    pub fn new(
        project_id: String,
        location: LocationHandle,
        token_cache: TokenCache,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let provider = VertexProvider::with_location_handle(
            project_id,
            location,
            GROUNDING_MODEL.to_string(),
            token_cache,
        )?;
        Ok(Self { provider })
    }
}

#[async_trait]
impl WebSearchProvider for VertexGroundingSearch {
    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        self.provider.search_with_grounding(query, max_results).await
    }

    fn id(&self) -> &'static str {
        "vertex-grounding"
    }

    fn model(&self) -> Option<&str> {
        Some(GROUNDING_MODEL)
    }
}
