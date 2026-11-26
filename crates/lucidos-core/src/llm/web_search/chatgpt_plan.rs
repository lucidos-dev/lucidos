use async_trait::async_trait;

use super::{WebSearchProvider, WebSearchResult};
use crate::llm::{metered::CallToken, OpenAiProvider};

pub struct ChatGptPlanSearch {
    provider: OpenAiProvider,
    model: String,
}

impl ChatGptPlanSearch {
    pub fn new(
        provider: OpenAiProvider,
        model: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if !provider.is_chatgpt_plan() {
            return Err("ChatGPT plan search requires a plan endpoint".into());
        }
        Ok(Self { provider, model })
    }
}

#[async_trait]
impl WebSearchProvider for ChatGptPlanSearch {
    async fn search(
        &self,
        query: &str,
        max_results: usize,
        call: CallToken,
    ) -> Result<WebSearchResult, Box<dyn std::error::Error + Send + Sync>> {
        self.provider
            .search_plan(&self.model, query, max_results, call)
            .await
    }

    fn id(&self) -> &'static str {
        "chatgpt-plan-responses"
    }

    fn model(&self) -> Option<&str> {
        Some(&self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_platform_provider_cannot_be_used_for_plan_search() {
        let platform = OpenAiProvider::new_with_base_url(
            crate::llm::ProviderKind::OpenAi,
            "platform-key".into(),
            "gpt-6.1-sol".into(),
            crate::llm::OPENAI_DEFAULT_BASE_URL,
            Vec::new(),
            false,
        )
        .unwrap();
        assert!(ChatGptPlanSearch::new(platform, "gpt-6.1-sol".into()).is_err());
    }
}
