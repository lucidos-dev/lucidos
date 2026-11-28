use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;

use crate::llm::model_registry::GenerationRule;
use crate::llm::provider::{
    forward, LlmProvider, LlmResponse, Message, ModelSelection, SharedCallback, TokenCallback,
    ToolDefinition,
};
use crate::llm::ModelNotServed;

#[async_trait]
pub trait NotServedReport: Send + Sync {
    async fn not_served(&self, model: &str, refusal: &ModelNotServed, moved_to: Option<&str>);
}

pub struct AuxProvider {
    inner: Arc<dyn LlmProvider>,
    model: String,
    fallback: Option<String>,
    moved: OnceLock<String>,
    report: Option<Arc<dyn NotServedReport>>,
    attempt_timeout: Duration,
    generation_rule: GenerationRule,
}

impl AuxProvider {
    pub fn new(inner: Arc<dyn LlmProvider>, model: String, attempt_timeout: Duration) -> Self {
        Self {
            inner,
            model,
            fallback: None,
            moved: OnceLock::new(),
            report: None,
            attempt_timeout,
            generation_rule: GenerationRule::default(),
        }
    }

    pub fn with_generation_rule(mut self, rule: GenerationRule) -> Self {
        self.generation_rule = rule;
        self
    }

    pub fn reporting(mut self, report: Arc<dyn NotServedReport>, fallback: Option<String>) -> Self {
        self.report = Some(report);
        self.fallback = fallback;
        self
    }

    fn current_model(&self) -> &str {
        self.moved.get().unwrap_or(&self.model)
    }

    fn pinned<'a>(&self, model: &'a str, selection: ModelSelection<'a>) -> ModelSelection<'a> {
        ModelSelection {
            model: Some(model),
            attempt_timeout: Some(self.attempt_timeout),
            ..selection
        }
        .with_generation_rule(self.generation_rule)
    }
}

#[async_trait]
impl LlmProvider for AuxProvider {
    fn resolved_provider_kind(
        &self,
        selection: &ModelSelection<'_>,
    ) -> Result<Option<crate::llm::ProviderKind>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner
            .resolved_provider_kind(&self.pinned(self.current_model(), *selection))
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let model = self.current_model().to_string();
        let shared: Option<SharedCallback> = on_token.map(Arc::from);
        let retry = (self.moved.get().is_none())
            .then(|| self.fallback.clone())
            .flatten()
            .map(|fallback| (fallback, messages.clone(), tools.clone()));
        let outcome = self
            .inner
            .chat(
                messages,
                tools,
                self.pinned(&model, selection),
                system_prompt,
                forward(&shared),
                call,
            )
            .await;
        let err = match outcome {
            Err(err) if err.is::<ModelNotServed>() => err,
            other => return other,
        };
        let refusal = err
            .downcast_ref::<ModelNotServed>()
            .expect("the guard above matched it");
        if let Some(report) = &self.report {
            let moved_to = retry.as_ref().map(|(fallback, _, _)| fallback.as_str());
            report.not_served(&model, refusal, moved_to).await;
        }
        let Some((fallback, messages, tools)) = retry else {
            return Err(err);
        };
        let fallback = self.moved.get_or_init(|| fallback).clone();
        let outcome = self
            .inner
            .chat(
                messages,
                tools,
                self.pinned(&fallback, selection),
                system_prompt,
                forward(&shared),
                call,
            )
            .await;
        if let (Err(err), Some(report)) = (&outcome, &self.report) {
            if let Some(refusal) = err.downcast_ref::<ModelNotServed>() {
                report.not_served(&fallback, refusal, None).await;
            }
        }
        outcome
    }

    fn default_model(&self) -> &str {
        self.current_model()
    }

    fn effort_sent(&self, selection: &ModelSelection<'_>) -> Option<String> {
        self.inner
            .effort_sent(&self.pinned(self.current_model(), *selection))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type Seen = (Option<String>, Option<String>, Option<Duration>);

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
