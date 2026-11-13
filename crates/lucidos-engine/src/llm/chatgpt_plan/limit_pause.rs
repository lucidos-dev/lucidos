//! The pause after the plan's usage limit is spent. OpenAI's docs say to
//! pause new plan requests rather than retry, so each one fails fast, with no
//! request sent, until the pause ends.

use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};

use super::{ChatGptPlanError, ChatGptPlanFailure};
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};

/// How long plan requests fail fast after a limit answer.
pub const LIMIT_PAUSE: Duration = Duration::from_secs(10 * 60);

/// One pause per engine, shared by every provider build so a rebuild keeps it.
pub struct LimitPause {
    paused: Mutex<Option<Paused>>,
    /// Announces each pause. `None` in tests that build no event bus.
    event_bus: Option<EventBus>,
}

struct Paused {
    until: DateTime<Utc>,
    /// The answer that began the pause, returned to every request it stops.
    error: ChatGptPlanError,
}

impl LimitPause {
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            paused: Mutex::new(None),
            event_bus: Some(event_bus),
        }
    }

    #[cfg(test)]
    pub(crate) fn unannounced() -> Self {
        Self {
            paused: Mutex::new(None),
            event_bus: None,
        }
    }

    /// The limit answer that paused plan requests, while the pause lasts.
    pub fn refusal(&self) -> Option<ChatGptPlanError> {
        let paused = self.paused.lock().unwrap_or_else(|e| e.into_inner());
        paused
            .as_ref()
            .filter(|paused| Utc::now() < paused.until)
            .map(|paused| paused.error.clone())
    }

    /// When the current pause ends, or `None` with no pause.
    pub fn paused_until(&self) -> Option<DateTime<Utc>> {
        let paused = self.paused.lock().unwrap_or_else(|e| e.into_inner());
        paused
            .as_ref()
            .map(|paused| paused.until)
            .filter(|until| Utc::now() < *until)
    }

    /// Pause on a limit answer, and announce it unless a pause already runs.
    /// Any other failure leaves the pause alone.
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
        crate::log!(
            "[ChatGptPlan] Usage limit reached; pausing plan requests until {paused_until}"
        );
        if let Some(event_bus) = &self.event_bus {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::ChatGptPlanLimitReached { paused_until }),
                    "[ChatGptPlan] ChatGptPlanLimitReached",
                )
                .await;
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

    /// A second limit answer inside the pause neither extends nor announces.
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

    /// A pause that ran out lets requests through again.
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
