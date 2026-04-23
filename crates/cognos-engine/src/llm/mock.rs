use crate::llm::provider::{LlmProvider, LlmResponse, Message, TokenCallback, ToolDefinition};
use async_trait::async_trait;
use std::time::Duration;

// ~100 words — long enough for streaming/cancel tests to have content to work with.
const MOCK_RESPONSE: &str = "\
The quick brown fox jumps over the lazy dog. A pangram is a sentence that \
contains every letter of the alphabet at least once. The five boxing wizards \
jump quickly across the moonlit field. Pack my box with five dozen liquor jugs. \
How vexingly quick daft zebras jump. The jay, pig, fox, zebra and my wolves \
quack. Crazy Frederick bought many very exquisite opal jewels. We promptly \
judged antique ivory buckles for the next prize. Sixty zippers were quickly \
picked from the woven jute bag. A large fawn jumped quickly over white zinc \
boxes on the shelf.";

/// A deterministic LLM provider for E2E testing.
///
/// Returns a fixed text response, streamed word-by-word with a small delay
/// between tokens. Never returns tool calls. Activate with `COGNOS_MODEL=mock`.
pub struct MockProvider {
    default_model: String,
}

impl MockProvider {
    pub fn new(model: String) -> Self {
        Self {
            default_model: model,
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _model_override: Option<&str>,
        _system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        _reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        // Small initial delay to simulate network round-trip
        tokio::time::sleep(Duration::from_millis(50)).await;

        if let Some(cb) = &on_token {
            for (i, word) in MOCK_RESPONSE.split_whitespace().enumerate() {
                if i > 0 {
                    cb(" ");
                }
                cb(word);
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }

        Ok(LlmResponse {
            content: Some(MOCK_RESPONSE.to_string()),
            tool_calls: vec![],
        })
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_returns_fixed_response() {
        let provider = MockProvider::new("mock".to_string());
        let resp = provider
            .chat(vec![], vec![], None, None, None, None)
            .await
            .unwrap();
        assert!(resp.content.is_some());
        assert!(resp.content.as_ref().unwrap().len() > 10);
        assert!(resp.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn mock_streams_tokens() {
        let provider = MockProvider::new("mock".to_string());
        let tokens = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let tokens_clone = tokens.clone();
        let cb: TokenCallback = Box::new(move |token: &str| {
            tokens_clone.lock().unwrap().push(token.to_string());
        });
        let resp = provider
            .chat(vec![], vec![], None, None, Some(cb), None)
            .await
            .unwrap();
        assert!(resp.content.is_some());
        let collected = tokens.lock().unwrap();
        assert!(collected.len() > 10, "should stream many tokens");
    }
}
