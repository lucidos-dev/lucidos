//! The provider one *auxiliary model call* runs on: the router, with the
//! call's model and attempt cap pinned (`engine::aux_purpose`).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::llm::provider::{
    LlmProvider, LlmResponse, Message, ModelSelection, TokenCallback, ToolDefinition,
};

/// One auxiliary call's model, pinned over a provider that can serve it.
///
/// Every request goes out on the pinned model with the purpose's attempt cap.
/// The effort stays the caller's. `default_model` names the pinned model, so
/// a cost record names the model that ran.
pub(crate) struct AuxProvider {
    inner: Arc<dyn LlmProvider>,
    model: String,
    attempt_timeout: Duration,
}

impl AuxProvider {
    pub(crate) fn new(
        inner: Arc<dyn LlmProvider>,
        model: String,
        attempt_timeout: Duration,
    ) -> Self {
        Self {
            inner,
            model,
            attempt_timeout,
        }
    }
}

#[async_trait]
impl LlmProvider for AuxProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let pinned = ModelSelection {
            model: Some(&self.model),
            attempt_timeout: Some(self.attempt_timeout),
            ..selection
        };
        self.inner
            .chat(messages, tools, pinned, system_prompt, on_token, call)
            .await
    }

    fn default_model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The model, effort and attempt cap one request reached the provider with.
    type Seen = (Option<String>, Option<String>, Option<Duration>);

    /// Records the selection each request reached it with.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<Seen>>);

    #[async_trait]
    impl LlmProvider for Recorder {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            selection: ModelSelection<'_>,
            _system_prompt: Option<&str>,
            _on_token: Option<TokenCallback>,
            _call: crate::llm::metered::CallToken,
        ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
            self.0.lock().unwrap().push((
                selection.model.map(str::to_string),
                selection.reasoning_effort.map(str::to_string),
                selection.attempt_timeout,
            ));
            Ok(LlmResponse::default())
        }
    }

    /// The pinned model and cap reach the provider below, whatever the caller
    /// passed, and the caller's effort survives.
    #[tokio::test]
    async fn every_request_carries_the_pinned_model_and_cap() {
        let recorder = Arc::new(Recorder::default());
        let cap = Duration::from_secs(20);
        let provider = AuxProvider::new(recorder.clone(), "claude-haiku-4-5".to_string(), cap);
        provider
            .chat(
                vec![],
                vec![],
                ModelSelection::model("something-else").with_effort(Some("low")),
                None,
                None,
                crate::llm::metered::CallToken::for_test(),
            )
            .await
            .expect("the recorder answers");
        assert_eq!(
            recorder.0.lock().unwrap().as_slice(),
            [(
                Some("claude-haiku-4-5".to_string()),
                Some("low".to_string()),
                Some(cap)
            )]
        );
        assert_eq!(provider.default_model(), "claude-haiku-4-5");
    }
}
