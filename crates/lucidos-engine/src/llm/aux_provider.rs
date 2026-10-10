//! The provider one *auxiliary model call* runs on: the router, with the
//! call's model and attempt cap pinned (`engine::aux_purpose`).

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;

use crate::llm::provider::{
    LlmProvider, LlmResponse, Message, ModelSelection, TokenCallback, ToolDefinition,
};
use crate::llm::ModelNotServed;

/// Told when a model answered not-found, and where the call went next.
#[async_trait]
pub(crate) trait NotServedReport: Send + Sync {
    /// `moved_to` names the model the call retried on, or `None` when it
    /// failed for good.
    async fn not_served(&self, model: &str, refusal: &ModelNotServed, moved_to: Option<&str>);
}

/// One auxiliary call's model, pinned over a provider that can serve it.
///
/// Every request goes out on the pinned model with the purpose's attempt cap.
/// The effort stays the caller's. `default_model` names the model that ran,
/// so a cost record names it too.
///
/// An unset default carries a `fallback`. When the pinned model answers
/// not-found, the request retries once on it, and every later request goes
/// there directly (ADR 0403). A stored pick carries none.
pub(crate) struct AuxProvider {
    inner: Arc<dyn LlmProvider>,
    model: String,
    fallback: Option<String>,
    /// The fallback, once the pinned model answered not-found.
    moved: OnceLock<String>,
    report: Option<Arc<dyn NotServedReport>>,
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
            fallback: None,
            moved: OnceLock::new(),
            report: None,
            attempt_timeout,
        }
    }

    /// Report each not-found answer to `report`, and retry once on `fallback`.
    pub(crate) fn reporting(
        mut self,
        report: Arc<dyn NotServedReport>,
        fallback: Option<String>,
    ) -> Self {
        self.report = Some(report);
        self.fallback = fallback;
        self
    }

    fn current_model(&self) -> &str {
        self.moved.get().unwrap_or(&self.model)
    }

    /// The caller's selection, sent on `model` with the purpose's attempt cap.
    fn pinned<'a>(&self, model: &'a str, selection: ModelSelection<'a>) -> ModelSelection<'a> {
        ModelSelection {
            model: Some(model),
            attempt_timeout: Some(self.attempt_timeout),
            ..selection
        }
    }
}

/// One callback shared by the first attempt and the retry.
type SharedCallback = Arc<dyn Fn(&str) + Send + Sync>;

fn forward(shared: &Option<SharedCallback>) -> Option<TokenCallback> {
    shared
        .clone()
        .map(|cb| Box::new(move |token: &str| cb(token)) as TokenCallback)
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
        self.inner.effort_sent(&ModelSelection {
            model: Some(self.current_model()),
            ..*selection
        })
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
