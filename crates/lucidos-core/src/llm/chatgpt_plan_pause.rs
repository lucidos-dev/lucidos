//! INVARIANT: The pause after the plan's usage limit is spent. OpenAI's docs say to
//! pause new plan requests rather than retry, so each one fails fast, with no
//! request sent, until the pause ends.

use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;

use super::chatgpt_plan_error::{ChatGptPlanError, ChatGptPlanFailure};

pub const LIMIT_PAUSE: Duration = Duration::from_secs(10 * 60);

type PauseAnnouncement = dyn Fn(DateTime<Utc>) -> BoxFuture<'static, ()> + Send + Sync;

/// INVARIANT: One pause per engine, shared by every provider build so a rebuild keeps it.
pub struct LimitPause {
    paused: Mutex<Option<Paused>>,
    announce: Option<Box<PauseAnnouncement>>,
}

struct Paused {
    until: DateTime<Utc>,
    error: ChatGptPlanError,
}

impl LimitPause {
    pub fn new(
        announce: impl Fn(DateTime<Utc>) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            paused: Mutex::new(None),
            announce: Some(Box::new(announce)),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn unannounced() -> Self {
        Self {
            paused: Mutex::new(None),
            announce: None,
        }
    }

    pub fn refusal(&self) -> Option<ChatGptPlanError> {
        let paused = self.paused.lock().unwrap_or_else(|e| e.into_inner());
        paused
            .as_ref()
            .filter(|paused| Utc::now() < paused.until)
            .map(|paused| paused.error.clone())
    }

    pub fn paused_until(&self) -> Option<DateTime<Utc>> {
        let paused = self.paused.lock().unwrap_or_else(|e| e.into_inner());
        paused
            .as_ref()
            .map(|paused| paused.until)
            .filter(|until| Utc::now() < *until)
    }

    pub async fn observe(&self, error: &ChatGptPlanError) {
        if error.failure != ChatGptPlanFailure::LimitReached {
            return;
        }
        let began = {
            let mut paused = self.paused.lock().unwrap_or_else(|e| e.into_inner());
            let now = Utc::now();
            if paused.as_ref().is_some_and(|paused| now < paused.until) {
                None
            } else {
                let until = now + LIMIT_PAUSE;
                *paused = Some(Paused {
                    until,
                    error: error.clone(),
                });
                Some(until)
            }
        };
        let Some(paused_until) = began else {
            return;
        };
        crate::log_legacy!(
            "[ChatGptPlan] Usage limit reached; pausing plan requests until {paused_until}"
        );
        if let Some(announce) = &self.announce {
            announce(paused_until).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(code: &str) -> ChatGptPlanError {
        ChatGptPlanError::from_error_object(&serde_json::json!({ "code": code, "message": "m" }))
            .unwrap()
    }

    #[tokio::test]
    async fn a_limit_answer_pauses_and_returns_itself_while_paused() {
        let pause = LimitPause::unannounced();
        assert!(pause.refusal().is_none());
        pause
            .observe(&failure("subscription_sharing_usage_limit_exceeded"))
            .await;
        let refusal = pause.refusal().expect("paused");
        assert_eq!(refusal.failure, ChatGptPlanFailure::LimitReached);
        let until = pause.paused_until().unwrap();
        let length = until - Utc::now();
        assert!(length <= chrono::Duration::from_std(LIMIT_PAUSE).unwrap());
        assert!(length > chrono::Duration::minutes(9));
    }

    #[tokio::test]
    async fn a_second_limit_answer_keeps_the_first_pause() {
        let pause = LimitPause::unannounced();
        let limit = failure("subscription_sharing_usage_limit_exceeded");
        pause.observe(&limit).await;
        let first = pause.paused_until().unwrap();
        pause.observe(&limit).await;
        assert_eq!(pause.paused_until(), Some(first));
    }

    #[tokio::test]
    async fn any_other_failure_never_pauses() {
        let pause = LimitPause::unannounced();
        for code in [
            "subscription_sharing_usage_unavailable",
            "subscription_sharing_user_not_eligible",
            "subscription_sharing_invalid_user",
        ] {
            pause.observe(&failure(code)).await;
        }
        assert!(pause.refusal().is_none());
    }

    #[tokio::test]
    async fn an_ended_pause_lets_requests_through() {
        let pause = LimitPause::unannounced();
        *pause.paused.lock().unwrap() = Some(Paused {
            until: Utc::now() - chrono::Duration::seconds(1),
            error: failure("subscription_sharing_usage_limit_exceeded"),
        });
        assert!(pause.refusal().is_none());
        assert!(pause.paused_until().is_none());
    }
}
