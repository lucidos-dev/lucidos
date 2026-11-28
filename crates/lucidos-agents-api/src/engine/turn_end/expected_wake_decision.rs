use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::engine::event_wait::{PendingWake, MAX_TIMEOUT_SECS};
use crate::engine::expected_wake::{describe_span, record_expected_wake, ExpectedWake};
use crate::engine::model_call::AuxCapture;
use crate::engine::thread_events::{WakeBucket, WakeSource};
use crate::engine::ContextPurpose;
use crate::llm::judgment::{
    ChatJudgmentProvider, ChoiceAnswer, JudgmentProvider, JudgmentSite, Question,
};

pub(crate) const BUCKET_EDGES_MINUTES: [i64; 11] = [
    5,
    10,
    15,
    20,
    30,
    45,
    60,
    90,
    120,
    240,
    MAX_TIMEOUT_SECS / 60,
];

const WAKE_QUESTION_ID: &str = "wake_within";

const REPLY_BYTES: usize = 6_000;
const TASK_BYTES: usize = 1_500;
const REASON_BYTES: usize = 300;

const WAKE_INSTRUCTIONS: &str = "A thread is waiting, and its agent's turn just ended. When \
    will the thread next wake? It wakes at the first of three things: an event wait receives \
    its event, an event wait times out, or a sub-thread reports.\n\n\
    Read what each item waits for, how long it has run, the latest output of its background \
    tasks, and the agent's reply. Past waits like these here are a guide, not a rule: this wait \
    may differ. An earlier estimate that passed with no wake was too early, so look past it.\n\n\
    Give the probability that the thread wakes within each span of time, counted from now.";

const RE_ESTIMATE_REPLY: &str = "(No new turn. The earlier estimate passed with no wake, so the \
    keep-alive asks again.)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecisionBackend {
    SystemOne,
    Chat,
}

fn minutes_label(minutes: i64) -> String {
    if minutes >= 120 && minutes % 60 == 0 {
        format!("{} h", minutes / 60)
    } else {
        format!("{minutes} min")
    }
}

fn bucket_spans_secs() -> impl Iterator<Item = (f64, f64)> {
    let starts = std::iter::once(0).chain(BUCKET_EDGES_MINUTES);
    starts
        .zip(BUCKET_EDGES_MINUTES)
        .map(|(start, end)| (start as f64 * 60.0, end as f64 * 60.0))
}

pub(crate) fn bucket_labels() -> Vec<String> {
    std::iter::once(0)
        .chain(BUCKET_EDGES_MINUTES)
        .zip(BUCKET_EDGES_MINUTES)
        .map(|(start, end)| match start {
            0 => format!("within {}", minutes_label(end)),
            _ => format!("{} to {}", minutes_label(start), minutes_label(end)),
        })
        .collect()
}

pub(crate) fn wake_questions() -> Vec<(String, Question)> {
    vec![(
        WAKE_QUESTION_ID.to_string(),
        Question::Choice {
            instructions: WAKE_INSTRUCTIONS.to_string(),
            criteria: bucket_labels()
                .into_iter()
                .map(|label| (label, None))
                .collect(),
        },
    )]
}

pub(crate) fn median_secs(probabilities: &[f64]) -> Option<f64> {
    let total: f64 = probabilities.iter().sum();
    if !total.is_finite() || total <= 0.0 {
        return None;
    }
    let mut before = 0.0;
    for (p, (start, end)) in probabilities.iter().zip(bucket_spans_secs()) {
        let share = p / total;
        if share > 0.0 && before + share >= 0.5 {
            return Some(start + (0.5 - before) / share * (end - start));
        }
        before += share;
    }
    None
}

fn bucket_weights(answer: &ChoiceAnswer, backend: DecisionBackend) -> Vec<f64> {
    bucket_labels()
        .iter()
        .map(|label| match backend {
            DecisionBackend::SystemOne => answer.probability(label),
            DecisionBackend::Chat => f64::from(u8::from(*label == answer.choice)),
        })
        .collect()
}

fn decision_reason(pending: &PendingWake) -> String {
    let items: Vec<&str> = pending
        .waits
        .iter()
        .map(|w| w.reason.as_str())
        .chain(pending.children.iter().map(|c| c.title.as_str()))
        .filter(|item| !item.trim().is_empty())
        .collect();
    let reason = match items.as_slice() {
        [] => "Estimated at turn end from what the thread waits on.".to_string(),
        items => format!(
            "Estimated at turn end from what the thread waits on: {}.",
            items.join("; ")
        ),
    };
    crate::core::middle_truncate(&reason, REASON_BYTES)
}

pub(crate) fn wake_from_answer(
    answer: &ChoiceAnswer,
    backend: DecisionBackend,
    pending: &PendingWake,
    now: DateTime<Utc>,
) -> Option<ExpectedWake> {
    let weights = bucket_weights(answer, backend);
    let median = median_secs(&weights)?;
    let at_secs = |secs: f64| now + chrono::Duration::milliseconds((secs * 1000.0).round() as i64);
    let distribution = match backend {
        DecisionBackend::SystemOne => weights
            .iter()
            .zip(bucket_spans_secs())
            .map(|(p, (_, end))| WakeBucket {
                by: at_secs(end),
                probability: *p,
            })
            .collect(),
        DecisionBackend::Chat => Vec::new(),
    };
    let at = pending
        .waits
        .iter()
        .map(|w| w.expires_at)
        .fold(at_secs(median), DateTime::min);
    Some(ExpectedWake {
        at,
        distribution,
        source: WakeSource::Decision,
        reason: decision_reason(pending),
    })
}

fn ago(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    format!("{} ago", describe_span((now - at).num_seconds() as f64))
}

pub(crate) fn decision_state(
    pending: &PendingWake,
    reply: &str,
    now: DateTime<Utc>,
) -> serde_json::Value {
    let waits: Vec<_> = pending
        .waits
        .iter()
        .map(|w| {
            json!({
                "waits_for": w.reason,
                "watches": w.watches,
                "armed": ago(w.armed_at, now),
                "times_out_in": describe_span((w.expires_at - now).num_seconds() as f64),
            })
        })
        .collect();
    let children: Vec<_> = pending
        .children
        .iter()
        .map(|c| {
            json!({
                "title": c.title,
                "task": crate::core::middle_truncate(&c.task, TASK_BYTES),
                "status": if c.awaiting_user { "waiting for its user's answer" } else { "working" },
                "started": ago(c.started_at, now),
            })
        })
        .collect();
    let tasks: Vec<_> = pending
        .tasks
        .iter()
        .map(|t| {
            json!({
                "what": t.what,
                "running_for": describe_span(t.running_secs as f64),
                "latest_output": t.latest_output,
            })
        })
        .collect();
    let misses: Vec<_> = pending
        .misses
        .iter()
        .map(|m| {
            json!({
                "stated": ago(m.stated_at, now),
                "expected_wake_was": ago(m.expected_at, now),
                "stated_by": match m.source.0 {
                    WakeSource::Agent => "the agent",
                    WakeSource::Decision => "an earlier estimate like this one",
                },
                "reason": m.reason,
            })
        })
        .collect();
    json!({
        "agents_reply": crate::core::middle_truncate(reply, REPLY_BYTES),
        "event_waits": waits,
        "sub_threads": children,
        "background_tasks": tasks,
        "past_waits_here": pending.history,
        "earlier_estimates_that_passed": misses,
    })
}

pub(crate) async fn ask_for_buckets(
    system_one: Option<&dyn JudgmentProvider>,
    chat: &dyn JudgmentProvider,
    capture: &AuxCapture,
    deadline: Duration,
    state: serde_json::Value,
) -> Option<(DecisionBackend, ChoiceAnswer)> {
    let capture = capture.until(tokio::time::Instant::now() + deadline);
    let backends = system_one
        .map(|provider| (DecisionBackend::SystemOne, provider))
        .into_iter()
        .chain([(DecisionBackend::Chat, chat)]);
    for (backend, provider) in backends {
        match capture
            .judge(provider, state.clone(), wake_questions())
            .await
        {
            Ok(judgment) => match judgment.answers.choice(WAKE_QUESTION_ID) {
                Some(answer) => return Some((backend, answer.clone())),
                None => {
                    crate::log_legacy!(
                        "[TurnEnd] The {backend:?} expected wake decision gave no answer"
                    )
                }
            },
            Err(e) => {
                crate::log_legacy!("[TurnEnd] The {backend:?} expected wake decision failed: {e}")
            }
        }
    }
    None
}

pub(crate) async fn decide_and_record(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    pending: &PendingWake,
    ask: impl std::future::Future<Output = Option<(DecisionBackend, ChoiceAnswer)>>,
) -> Option<ExpectedWake> {
    let Some((backend, answer)) = ask.await else {
        crate::log_legacy!(
            "[TurnEnd] No expected wake for thread {thread_id}: the decision failed, so the \
             bare reread cap applies"
        );
        return None;
    };
    let wake = wake_from_answer(&answer, backend, pending, Utc::now())?;
    match record_expected_wake(bus, thread_id, wake.clone(), None).await {
        Ok(()) => Some(wake),
        Err(e) => {
            crate::log_legacy!(
                "[TurnEnd] Recording the expected wake for thread {thread_id} failed: {e}"
            );
            None
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TurnStart {
    Opened(Uuid),
    SinceLastIdle,
}

pub(crate) async fn stated_this_turn(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    start: TurnStart,
) -> bool {
    let stated = |after: &str| {
        format!(
            "SELECT EXISTS (SELECT 1 FROM events \
                             WHERE thread_id = $1 AND event_type = 'WakeExpected' \
                               AND sequence > {after})"
        )
    };
    let read = match start {
        TurnStart::Opened(opener) => {
            sqlx::query_scalar::<_, bool>(&stated("(SELECT sequence FROM events WHERE id = $2)"))
                .bind(thread_id)
                .bind(opener)
                .fetch_one(pool)
                .await
        }
        TurnStart::SinceLastIdle => {
            sqlx::query_scalar::<_, bool>(&stated(
                "COALESCE((SELECT MAX(sequence) FROM events \
                            WHERE thread_id = $1 AND event_type = 'CodingAgentIdled'), -1)",
            ))
            .bind(thread_id)
            .fetch_one(pool)
            .await
        }
    };
    read.unwrap_or_else(|e| {
        crate::log_legacy!("[TurnEnd] Expected wakes of {thread_id} unreadable: {e}");
        true
    })
}

pub(crate) fn expected_wake_decision_chat_judge(
    provider: std::sync::Arc<dyn crate::llm::LlmProvider>,
    reasoning: Option<String>,
) -> ChatJudgmentProvider {
    ChatJudgmentProvider::new(provider, reasoning).forcing_the_answer()
}

#[easy_ext::ext(ExpectedWakeDecisionExt)]
pub(crate) impl crate::engine::LucidosEngine {
    async fn force_expected_wake_decision(&self, thread_id: Uuid, start: TurnStart, reply: &str) {
        let Some(pending) = self.pending_wake(thread_id).await else {
            return;
        };
        if stated_this_turn(&self.pool, thread_id, start).await {
            return;
        }
        self.decide_expected_wake(thread_id, &pending, reply).await;
    }

    async fn re_estimate_expected_wake(&self, thread_id: Uuid) {
        if let Some(pending) = self.pending_wake(thread_id).await {
            self.decide_expected_wake(thread_id, &pending, RE_ESTIMATE_REPLY)
                .await;
        }
    }

    async fn decide_expected_wake(&self, thread_id: Uuid, pending: &PendingWake, reply: &str) {
        let call = self.aux_call(ContextPurpose::ExpectedWakeDecision).await;
        let system_one = self
            .judgment_system_one(JudgmentSite::ExpectedWakeDecision, call.attempt_timeout())
            .await;
        let chat = expected_wake_decision_chat_judge(
            call.provider(),
            call.reasoning().map(str::to_string),
        );
        let capture = AuxCapture::new(
            &self.event_bus,
            thread_id,
            ContextPurpose::ExpectedWakeDecision,
        );
        let state = decision_state(pending, reply, Utc::now());
        let ask = ask_for_buckets(
            system_one.as_deref(),
            &chat,
            &capture,
            call.deadline(),
            state,
        );
        decide_and_record(&self.event_bus, thread_id, pending, ask).await;
    }
}

#[cfg(test)]
#[path = "expected_wake_decision_tests.rs"]
mod tests;
