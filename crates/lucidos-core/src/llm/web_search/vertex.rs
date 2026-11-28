use async_trait::async_trait;

use super::{WebSearchProvider, WebSearchResult};
use crate::llm::metered::CallToken;
use crate::llm::vertex::{LocationHandle, TokenCache, VertexProvider, GROUNDING_MODEL};

pub struct VertexGroundingSearch {
    provider: VertexProvider,
}

impl VertexGroundingSearch {
    /// INVARIANT: Build from the engine's resolved Vertex config. Shares the caller's
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
        _call: CallToken,
    ) -> Result<WebSearchResult, Box<dyn std::error::Error + Send + Sync>> {
        let (text, usage) = self
            .provider
            .search_with_grounding(query, max_results)
            .await?;
        Ok(WebSearchResult {
            text,
            usage,
            model: GROUNDING_MODEL.to_string(),
            provider: Some(crate::engine::thread_events::billing_provider::BillingProvider::Vertex),
        })
    }

    fn id(&self) -> &'static str {
        "vertex-grounding"
    }

    fn model(&self) -> Option<&str> {
        Some(GROUNDING_MODEL)
    }
}
