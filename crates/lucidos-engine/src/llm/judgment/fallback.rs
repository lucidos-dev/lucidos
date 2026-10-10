//! The provider one site asks: its System One pick with its chat model behind
//! it, or the chat model alone.
//!
//! The fallback is generic, one for every site (ADR 0363). It catches a failed
//! call only. An answer the System One model did give is final, missing ids
//! included.
//!
//! **The caller's deadline bounds both attempts together.** Each site wraps
//! [`JudgmentProvider::ask`] in its own deadline, so a slow primary leaves the
//! fallback only what remains of it, never a fresh one.

use async_trait::async_trait;
use serde_json::Value;

use super::{ChatJudgmentProvider, Judgment, JudgmentProvider, Question, SystemOneProvider};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Ask `primary`, and `fallback` when that call fails.
pub(crate) struct WithFallback<P, F> {
    primary: P,
    fallback: F,
}

#[async_trait]
impl<P: JudgmentProvider, F: JudgmentProvider> JudgmentProvider for WithFallback<P, F> {
    async fn ask(
        &self,
        state: Value,
        questions: Vec<(String, Question)>,
        call: crate::llm::metered::CallToken,
    ) -> Result<Judgment, BoxError> {
        match self
            .primary
            .ask(state.clone(), questions.clone(), call)
            .await
        {
            Ok(judgment) => Ok(judgment),
            Err(e) => {
                log!(
                    "[Judgment] The System One call failed: {}. Asking the chat model",
                    e
                );
                self.fallback.ask(state, questions, call).await
            }
        }
    }
}

/// The provider for one site: a System One pick with the chat model behind it,
/// or the chat model alone.
pub fn for_site(
    system_one: Option<SystemOneProvider>,
    chat: ChatJudgmentProvider,
) -> Box<dyn JudgmentProvider> {
    match system_one {
        Some(primary) => Box::new(WithFallback {
            primary,
            fallback: chat,
        }),
        None => Box::new(chat),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::judgment::{Answer, Answers, NoulAnswer};

    struct Failing;

    #[async_trait]
    impl JudgmentProvider for Failing {
        async fn ask(
            &self,
            _: Value,
            _: Vec<(String, Question)>,
            _: crate::llm::metered::CallToken,
        ) -> Result<Judgment, BoxError> {
            Err("System One endpoint returned 529".into())
        }
    }

    /// Answers with one noul, so a test can tell which provider spoke.
    struct Answering(f64);

    #[async_trait]
    impl JudgmentProvider for Answering {
        async fn ask(
            &self,
            _: Value,
            _: Vec<(String, Question)>,
            _: crate::llm::metered::CallToken,
        ) -> Result<Judgment, BoxError> {
            let answers = [("q".to_string(), Answer::Noul(NoulAnswer { noul: self.0 }))];
            Ok(Judgment {
                answers: Answers::new(answers.into_iter().collect()),
                ..Judgment::default()
            })
        }
    }

    #[tokio::test]
    async fn a_failed_primary_call_is_answered_by_the_fallback() {
        let provider = WithFallback {
            primary: Failing,
            fallback: Answering(0.2),
        };
        let judgment = provider
            .ask(
                Value::Null,
                vec![],
                crate::llm::metered::CallToken::for_test(),
            )
            .await
            .expect("answered");
        assert_eq!(judgment.answers.noul("q"), Some(0.2));
    }

    #[tokio::test]
    async fn an_answering_primary_is_final() {
        let provider = WithFallback {
            primary: Answering(0.9),
            fallback: Failing,
        };
        let judgment = provider
            .ask(
                Value::Null,
                vec![],
                crate::llm::metered::CallToken::for_test(),
            )
            .await
            .expect("answered");
        assert_eq!(judgment.answers.noul("q"), Some(0.9));
    }

    #[tokio::test]
    async fn both_failing_is_the_fallbacks_error() {
        let provider = WithFallback {
            primary: Failing,
            fallback: Failing,
        };
        assert!(provider
            .ask(
                Value::Null,
                vec![],
                crate::llm::metered::CallToken::for_test()
            )
            .await
            .is_err());
    }
}
