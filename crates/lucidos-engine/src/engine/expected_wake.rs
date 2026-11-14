//! The *expected wake*: when a waiting thread will next wake, from the
//! earliest of its event waits and the children that report to it (ADR 0603).
//!
//! The agent states it first: the `expect_wake` tool and `lucidos
//! expect-wake` record the same `WakeExpected`, with the agent thread as its
//! actor. The newest one stands, and the keep-alive plans its pings from it.

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use crate::engine::chat::agent_archive::agent_thread_actor;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::event_wait::MAX_TIMEOUT_SECS;
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent, WakeBucket, WakeSource};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub(crate) use crate::llm::tools::{EXPECT_WAKE_MINUTES_ARG, EXPECT_WAKE_REASON_ARG};

/// One expected wake, ready to record.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExpectedWake {
    pub at: DateTime<Utc>,
    pub distribution: Vec<WakeBucket>,
    pub source: WakeSource,
    pub reason: String,
}

impl ExpectedWake {
    /// The agent's own statement: it expects to wake `in_secs` after `now`.
    /// The span runs from one second to a wait's longest timeout, and the
    /// reason is required.
    pub(crate) fn stated(in_secs: f64, reason: &str, now: DateTime<Utc>) -> Result<Self, String> {
        if !(in_secs >= 1.0 && in_secs <= MAX_TIMEOUT_SECS as f64) {
            return Err(format!(
                "Error: the expected wake must be 1 second to 24 hours away, got {in_secs} \
                 seconds. Past 24 hours no event wait is still armed."
            ));
        }
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(
                "Error: the expected wake needs a reason: what you wait for, and why it \
                 should come by then (\"the release build, about 20 minutes per run\")."
                    .to_string(),
            );
        }
        Ok(Self {
            at: now + Duration::milliseconds((in_secs * 1000.0).round() as i64),
            distribution: Vec::new(),
            source: WakeSource::Agent,
            reason: reason.to_string(),
        })
    }

    /// Read `expect_wake`'s arguments. A missing or non-numeric span is an
    /// error the model reads back, never a default.
    pub(crate) fn from_tool_args(
        args: &serde_json::Value,
        now: DateTime<Utc>,
    ) -> Result<Self, String> {
        let minutes = args[EXPECT_WAKE_MINUTES_ARG].as_f64().ok_or_else(|| {
            format!(
                "Error: expect_wake needs `{EXPECT_WAKE_MINUTES_ARG}`: how many minutes from \
                 now you expect this thread to wake."
            )
        })?;
        let reason = args[EXPECT_WAKE_REASON_ARG].as_str().unwrap_or_default();
        Self::stated(minutes * 60.0, reason, now)
    }

    fn event(self) -> ThreadEvent {
        ThreadEvent::WakeExpected {
            at: self.at,
            distribution: self.distribution,
            source: self.source,
            reason: self.reason,
        }
    }

    /// What the agent reads back, from the tool and the CLI.
    pub(crate) fn ack(&self, now: DateTime<Utc>) -> String {
        format!(
            "Recorded: you expect to wake in {}. While the keep-alive is on, it keeps the \
             prompt cache warm until then when that costs clearly less than one reread. A \
             later statement replaces this one.",
            describe_span((self.at - now).num_seconds() as f64),
        )
    }
}

/// A span as the agent reads it: "about 45 min", "about 2 h 10 min".
pub(crate) fn describe_span(secs: f64) -> String {
    let minutes = (secs / 60.0).round().max(1.0) as i64;
    if minutes < 60 {
        format!("about {minutes} min")
    } else if minutes % 60 == 0 {
        format!("about {} h", minutes / 60)
    } else {
        format!("about {} h {} min", minutes / 60, minutes % 60)
    }
}

/// Record `thread_id`'s own agent's expected wake. The `expect_wake` tool's
/// actor: the agent thread itself.
pub(crate) async fn record_agent_expected_wake(
    bus: &EventBus,
    thread_id: Uuid,
    wake: ExpectedWake,
) -> Result<(), BoxError> {
    record_expected_wake(bus, thread_id, wake, Some(agent_thread_actor(thread_id))).await
}

/// Record an expected wake on `thread_id`, by `actor`. `None` is the engine,
/// for the expected wake decision.
pub(crate) async fn record_expected_wake(
    bus: &EventBus,
    thread_id: Uuid,
    wake: ExpectedWake,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: wake.event(),
        meta: EventMeta::with_actor(actor),
    })
    .await?;
    Ok(())
}

#[cfg(test)]
#[path = "expected_wake_tests.rs"]
mod tests;
