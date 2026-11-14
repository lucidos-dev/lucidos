use super::*;
use crate::engine::thread_events::{ActorMode, EventChannel};
use crate::test_support::{setup_test_db, teardown_test_db};

fn now() -> DateTime<Utc> {
    "2026-10-30T19:20:00Z".parse().unwrap()
}

#[test]
fn a_statement_dates_the_wake_from_now() {
    let wake = ExpectedWake::stated(2700.0, " the release build ", now()).unwrap();
    assert_eq!(wake.at, now() + Duration::minutes(45));
    assert_eq!(wake.source, WakeSource::Agent);
    assert_eq!(wake.reason, "the release build");
    assert!(wake.distribution.is_empty());
}

#[test]
fn a_statement_needs_a_span_inside_a_wait_and_a_reason() {
    for secs in [0.0, -60.0, f64::NAN, (MAX_TIMEOUT_SECS + 1) as f64] {
        assert!(
            ExpectedWake::stated(secs, "a build", now()).is_err(),
            "{secs}"
        );
    }
    assert!(ExpectedWake::stated(MAX_TIMEOUT_SECS as f64, "a build", now()).is_ok());
    assert!(ExpectedWake::stated(60.0, "  ", now()).is_err());
}

#[test]
fn the_tool_takes_minutes_and_refuses_a_missing_span() {
    let args = serde_json::json!({ "minutes": 1.5, "reason": "the suite" });
    let wake = ExpectedWake::from_tool_args(&args, now()).unwrap();
    assert_eq!(wake.at, now() + Duration::seconds(90));

    let missing = serde_json::json!({ "reason": "the suite" });
    let err = ExpectedWake::from_tool_args(&missing, now()).unwrap_err();
    assert!(err.contains(EXPECT_WAKE_MINUTES_ARG), "{err}");
}

#[test]
fn a_span_reads_in_minutes_then_hours() {
    assert_eq!(describe_span(20.0), "about 1 min");
    assert_eq!(describe_span(2700.0), "about 45 min");
    assert_eq!(describe_span(7200.0), "about 2 h");
    assert_eq!(describe_span(7800.0), "about 2 h 10 min");
}

#[test]
fn the_ack_names_the_span() {
    let wake = ExpectedWake::stated(2700.0, "the build", now()).unwrap();
    assert!(
        wake.ack(now()).contains("about 45 min"),
        "{}",
        wake.ack(now())
    );
}

/// The event names the agent thread that stated it, and carries its payload.
#[tokio::test]
async fn an_agent_statement_records_wake_expected() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "release it".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let wake = ExpectedWake::stated(2700.0, "the release build", now()).unwrap();
    record_agent_expected_wake(&bus, thread_id, wake)
        .await
        .unwrap();

    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE aggregate_id = $1 AND event_type = 'WakeExpected'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(payload["source"], "agent");
    assert_eq!(payload["reason"], "the release build");
    assert_eq!(payload["at"], "2026-10-30T20:05:00Z");
    assert!(payload.get("distribution").is_none(), "{payload}");
    assert_eq!(
        payload["actor"]["source_thread_id"],
        serde_json::json!(thread_id.to_string())
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
