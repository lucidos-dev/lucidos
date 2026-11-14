//! The expected wake decision: each test pins one invariant of
//! `docs/plans/2026-10-30-keep-alive-follows-the-expected-wake.md`.

use std::collections::HashMap;

use chrono::{DateTime, Duration as Span, Utc};
use uuid::Uuid;

use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::event_wait::{MissedEstimate, PendingChild, PendingWait, WaitAgent};
use crate::engine::expected_wake::record_agent_expected_wake;
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::engine::tools::bash_background::TaskProgress;
use crate::llm::judgment::{Answer, Answers, Judgment};
use crate::test_support::{setup_test_db, teardown_test_db};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A deadline no offline test reaches.
const TEST_DEADLINE: Duration = Duration::from_secs(30);

fn now() -> DateTime<Utc> {
    "2026-10-30T19:20:00Z".parse().unwrap()
}

fn wait(expires_in_min: i64) -> PendingWait {
    PendingWait {
        reason: "the release tarballs".into(),
        watches: "BackgroundBashCompleted".into(),
        armed_at: now() - Span::minutes(2),
        expires_at: now() + Span::minutes(expires_in_min),
    }
}

fn pending(waits: Vec<PendingWait>) -> PendingWake {
    PendingWake {
        agent: WaitAgent::Lucidos,
        waits,
        children: Vec::new(),
        tasks: Vec::new(),
        history: Vec::new(),
        misses: Vec::new(),
    }
}

/// A choice answer naming `choice`, with `weights` per bucket label.
fn answer(choice: &str, weights: &[(&str, f64)]) -> ChoiceAnswer {
    ChoiceAnswer {
        choice: choice.into(),
        probabilities: weights.iter().map(|(k, p)| (k.to_string(), *p)).collect(),
        confidence: 0.5,
    }
}

fn minutes_after_now(at: DateTime<Utc>) -> f64 {
    (at - now()).num_milliseconds() as f64 / 60_000.0
}

#[test]
fn the_buckets_run_from_now_to_a_waits_longest_timeout() {
    let labels = bucket_labels();
    assert_eq!(labels.len(), BUCKET_EDGES_MINUTES.len());
    assert_eq!(labels[0], "within 5 min");
    assert_eq!(labels[6], "45 min to 60 min");
    assert_eq!(labels.last().unwrap(), "4 h to 24 h");
    assert_eq!(
        *BUCKET_EDGES_MINUTES.last().unwrap() * 60,
        MAX_TIMEOUT_SECS,
        "no estimate outlives every wait"
    );
    assert!(BUCKET_EDGES_MINUTES.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn the_median_falls_inside_its_bucket() {
    let mut one = vec![0.0; BUCKET_EDGES_MINUTES.len()];
    one[6] = 1.0;
    assert_eq!(
        median_secs(&one),
        Some(52.5 * 60.0),
        "the middle of 45 to 60 min"
    );

    let mut split = vec![0.0; BUCKET_EDGES_MINUTES.len()];
    split[0] = 0.25;
    split[1] = 0.75;
    let median = median_secs(&split).unwrap() / 60.0;
    assert!((median - 6.666).abs() < 0.01, "{median}");

    assert_eq!(median_secs(&vec![0.0; BUCKET_EDGES_MINUTES.len()]), None);
}

/// A System One model answers with a probability per bucket, recorded whole.
#[test]
fn a_system_one_answer_records_its_distribution_and_its_median() {
    let labels = bucket_labels();
    let given = answer(&labels[6], &[(&labels[5], 0.4), (&labels[6], 0.6)]);
    let wake =
        wake_from_answer(&given, DecisionBackend::SystemOne, &pending(vec![]), now()).unwrap();
    assert_eq!(wake.source, WakeSource::Decision);
    assert_eq!(wake.distribution.len(), labels.len());
    assert_eq!(wake.distribution[6].by, now() + Span::minutes(60));
    assert!((wake.distribution[6].probability - 0.6).abs() < 1e-9);
    let median = minutes_after_now(wake.at);
    assert!((45.0..=60.0).contains(&median), "{median}");
}

/// The chat model answers with one bucket: a point at its middle, and no
/// distribution, whatever weights its reply spread.
#[test]
fn a_chat_answer_is_one_bucket_at_its_middle() {
    let labels = bucket_labels();
    let given = answer(&labels[3], &[(&labels[3], 0.5), (&labels[9], 0.5)]);
    let wake = wake_from_answer(&given, DecisionBackend::Chat, &pending(vec![]), now()).unwrap();
    assert!(wake.distribution.is_empty());
    assert_eq!(
        minutes_after_now(wake.at),
        17.5,
        "the middle of 15 to 20 min"
    );

    let unknown = answer("never", &[]);
    assert!(wake_from_answer(&unknown, DecisionBackend::Chat, &pending(vec![]), now()).is_none());
}

/// A wait's timeout wakes the thread, so no estimate lands past it.
#[test]
fn no_estimate_lands_past_the_earliest_waits_timeout() {
    let labels = bucket_labels();
    let late = answer(&labels[9], &[]);
    let wake = wake_from_answer(
        &late,
        DecisionBackend::Chat,
        &pending(vec![wait(90), wait(40)]),
        now(),
    )
    .unwrap();
    assert_eq!(wake.at, now() + Span::minutes(40));
    assert!(
        wake.reason.contains("the release tarballs"),
        "{}",
        wake.reason
    );
}

/// The call reads what each item waits on, its progress, the reply, the
/// history and its own earlier misses.
#[test]
fn the_state_carries_every_input() {
    let mut waiting = pending(vec![wait(60)]);
    waiting.children.push(PendingChild {
        title: "Build the arm64 tarball".into(),
        task: "Build and upload it".into(),
        awaiting_user: false,
        started_at: now() - Span::minutes(10),
    });
    waiting.tasks.push(TaskProgress {
        what: "the release build".into(),
        running_secs: 600,
        latest_output: "step 3/5: linking".into(),
    });
    waiting
        .history
        .push("Waits on BackgroundBashCompleted: Past waits like this here".into());
    waiting.misses.push(MissedEstimate {
        stated_at: now() - Span::minutes(30),
        expected_at: now() - Span::minutes(5),
        source: sqlx::types::Json(WakeSource::Agent),
        reason: "about 25 min per run".into(),
    });
    let state = decision_state(&waiting, "The tarballs are still building.", now());
    let text = state.to_string();
    for needle in [
        "the release tarballs",
        "BackgroundBashCompleted",
        "about 1 h",
        "Build the arm64 tarball",
        "working",
        "step 3/5: linking",
        "Past waits like this here",
        "about 25 min per run",
        "the agent",
        "The tarballs are still building.",
    ] {
        assert!(text.contains(needle), "{needle} missing from {text}");
    }
}

/// A backend that answers with a fixed result.
struct Fixed(Result<Option<String>, &'static str>);

#[async_trait::async_trait]
impl JudgmentProvider for Fixed {
    async fn ask(
        &self,
        _: serde_json::Value,
        questions: Vec<(String, Question)>,
        _: crate::llm::metered::CallToken,
    ) -> Result<Judgment, BoxError> {
        assert_eq!(questions, wake_questions());
        let choice = self.0.clone().map_err(BoxError::from)?;
        let answers = choice
            .map(|label| {
                let answer = Answer::Choice(answer(&label, &[(&label, 1.0)]));
                HashMap::from([(WAKE_QUESTION_ID.to_string(), answer)])
            })
            .unwrap_or_default();
        Ok(Judgment {
            answers: Answers::new(answers),
            ..Judgment::default()
        })
    }
}

/// System One answers when it can, and the chat model fills in for a failed
/// or empty System One call. With neither, there is no answer (I9).
#[tokio::test]
async fn the_chat_model_answers_without_system_one() {
    let capture = AuxCapture::discarding(ContextPurpose::ExpectedWakeDecision);
    let label = bucket_labels()[4].clone();
    let ask = |system_one: Option<Fixed>, chat: Fixed| {
        let capture = capture.clone();
        async move {
            ask_for_buckets(
                system_one.as_ref().map(|p| p as &dyn JudgmentProvider),
                &chat,
                &capture,
                TEST_DEADLINE,
                serde_json::json!({}),
            )
            .await
            .map(|(backend, answer)| (backend, answer.choice))
        }
    };
    let ok = || Fixed(Ok(Some(label.clone())));
    assert_eq!(
        ask(Some(ok()), Fixed(Err("unused"))).await,
        Some((DecisionBackend::SystemOne, label.clone()))
    );
    assert_eq!(
        ask(None, ok()).await,
        Some((DecisionBackend::Chat, label.clone()))
    );
    assert_eq!(
        ask(Some(Fixed(Err("529"))), ok()).await,
        Some((DecisionBackend::Chat, label.clone()))
    );
    assert_eq!(
        ask(Some(Fixed(Ok(None))), ok()).await,
        Some((DecisionBackend::Chat, label.clone()))
    );
    assert_eq!(ask(None, Fixed(Err("down"))).await, None);
}

async fn emit_on(
    bus: &EventBus,
    thread_id: Uuid,
    channel: EventChannel,
    event: ThreadEvent,
) -> Uuid {
    let id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta {
            channel: Some(channel),
            event_id: Some(id),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    id
}

fn message(text: &str) -> ThreadEvent {
    ThreadEvent::MessageReceived {
        provider: None,
        voice_session_id: None,
        wake_mode: None,
        text: text.into(),
        user_image_hashes: vec![],
        device_id: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    }
}

fn idled_event() -> ThreadEvent {
    serde_json::from_value(serde_json::json!({ "type": "CodingAgentIdled", "data": {} })).unwrap()
}

async fn state_wake(bus: &EventBus, thread_id: Uuid) {
    let wake = ExpectedWake::stated(1800.0, "the release build", Utc::now()).unwrap();
    record_agent_expected_wake(bus, thread_id, wake)
        .await
        .unwrap();
}

/// Only the turn's own statement spares it the decision, for a Lucidos Agent
/// turn and a coding-agent turn alike (I8).
#[tokio::test]
async fn only_a_statement_in_this_turn_spares_the_decision() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let chat = Uuid::new_v4();
    let first = emit_on(&bus, chat, EventChannel::Chat, message("release it")).await;
    assert!(!stated_this_turn(&pool, chat, TurnStart::Opened(first)).await);
    state_wake(&bus, chat).await;
    assert!(stated_this_turn(&pool, chat, TurnStart::Opened(first)).await);
    let second = emit_on(&bus, chat, EventChannel::Chat, message("are we releasing?")).await;
    assert!(
        !stated_this_turn(&pool, chat, TurnStart::Opened(second)).await,
        "a user's turn restates the expected wake"
    );

    let cc = Uuid::new_v4();
    let channel = EventChannel::ClaudeCode;
    emit_on(&bus, cc, channel, message("build the release")).await;
    assert!(!stated_this_turn(&pool, cc, TurnStart::SinceLastIdle).await);
    state_wake(&bus, cc).await;
    assert!(stated_this_turn(&pool, cc, TurnStart::SinceLastIdle).await);
    emit_on(&bus, cc, channel, idled_event()).await;
    assert!(
        !stated_this_turn(&pool, cc, TurnStart::SinceLastIdle).await,
        "an earlier turn's statement does not cover the next"
    );

    teardown_test_db(&db).await;
}

/// The decision records in the engine's name, sourced as a decision. A failed
/// call records nothing, so the bare reread cap applies (I9).
#[tokio::test]
async fn the_decision_records_in_the_engines_name_and_a_failure_records_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_on(&bus, thread_id, EventChannel::Chat, message("release it")).await;
    let waiting = pending(vec![wait(120)]);

    assert!(decide_and_record(&bus, thread_id, &waiting, async { None })
        .await
        .is_none());
    let label = bucket_labels()[5].clone();
    let picked = answer(&label, &[(&label, 1.0)]);
    let wake = decide_and_record(&bus, thread_id, &waiting, async {
        Some((DecisionBackend::Chat, picked))
    })
    .await
    .unwrap();

    let rows: Vec<(serde_json::Value, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT payload, payload->'actor' FROM events \
          WHERE thread_id = $1 AND event_type = 'WakeExpected'",
    )
    .bind(thread_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "the failed call recorded nothing");
    let (payload, actor) = &rows[0];
    assert_eq!(payload["source"], "decision");
    assert_eq!(*actor, None, "the engine decided, so no agent is named");
    let at: DateTime<Utc> = serde_json::from_value(payload["at"].clone()).unwrap();
    assert_eq!(at.timestamp_millis(), wake.at.timestamp_millis());

    teardown_test_db(&db).await;
}
