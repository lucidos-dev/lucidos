use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    EventChannel, EventMeta, FreshSessionReason, HandoffOpening, ThreadEvent,
};
use crate::runtime::CodingAgent;
use crate::test_support::{seed_coding_thread, setup_test_db, teardown_test_db};

fn kind_of(continuity: &SessionContinuity) -> String {
    serde_json::to_value(continuity).unwrap()["kind"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The SQL and [`SessionContinuity::opens_session`] agree on every kind.
#[test]
fn the_sql_names_every_kind_that_opens_a_session() {
    let every_kind = [
        SessionContinuity::First,
        SessionContinuity::Resumed,
        SessionContinuity::Fresh {
            reason: FreshSessionReason::SessionEnded,
        },
        SessionContinuity::Handoff {
            checkpoint: None,
            progress: None,
            opening: HandoffOpening::Brief,
            message: String::new(),
        },
    ];
    for continuity in every_kind {
        assert_eq!(
            OPENING_KINDS.contains(&kind_of(&continuity).as_str()),
            continuity.opens_session(),
            "{continuity:?}"
        );
    }
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("emit");
}

fn session_started(continuity: SessionContinuity) -> ThreadEvent {
    ThreadEvent::SessionStarted {
        coding_agent: CodingAgent::ClaudeCode,
        session_id: "test-session".into(),
        branch: "claude-code/scope".into(),
        repo_id: None,
        coding_agent_kind: Default::default(),
        coding_agent_folder: String::new(),
        app_id: None,
        integrated: false,
        auto_compact_window: None,
        auto_compact_arm: None,
        continuity: Some(continuity),
    }
}

fn handoff() -> SessionContinuity {
    SessionContinuity::Handoff {
        checkpoint: None,
        progress: None,
        opening: HandoffOpening::Narrative,
        message: String::new(),
    }
}

async fn capture(bus: &EventBus, thread_id: Uuid) {
    emit(
        bus,
        thread_id,
        crate::engine::model_call::auxiliary_capture(
            crate::engine::ContextPurpose::Title,
            "claude-opus-5-5",
            10,
            None,
            false,
        ),
    )
    .await;
}

/// Every capture names its session: a resume stays in it, a handoff and a
/// fresh session each open the next.
#[tokio::test]
async fn the_bus_stamps_each_capture_with_its_session() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    capture(&bus, thread).await;
    emit(&bus, thread, session_started(SessionContinuity::Resumed)).await;
    capture(&bus, thread).await;
    emit(&bus, thread, session_started(handoff())).await;
    capture(&bus, thread).await;
    emit(
        &bus,
        thread,
        session_started(SessionContinuity::Fresh {
            reason: FreshSessionReason::ChangeApplied,
        }),
    )
    .await;
    capture(&bus, thread).await;

    let sessions: Vec<Option<i64>> = sqlx::query_scalar(
        "SELECT (payload->>'session')::bigint FROM events \
         WHERE thread_id = $1 AND event_type = 'ContextCaptured' ORDER BY sequence",
    )
    .bind(thread)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(sessions, vec![Some(1), Some(1), Some(2), Some(3)]);

    teardown_test_db(&db).await;
}

/// Current-session reads: right after a handoff the context size is unknown
/// and the cache is cold, so the Successor card hides and the counter drops.
/// The successor session's first call is the size from then on.
#[tokio::test]
async fn size_and_warmth_read_only_the_current_session() {
    use crate::engine::handoff_estimate::{cache_warmth, CacheWarmth};
    use crate::engine::successor::{thread_context, HANDOFF_OFFER_TOKENS};
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let turn = |tokens: u64| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
                 VALUES (gen_random_uuid(), 'ContextCaptured', $1, $2, 'thread', $2::text)",
            )
            .bind(serde_json::json!({
                "model": "claude-opus-5-5",
                "usage": {"input_tokens": tokens, "cache_creation_tokens": 1000, "cache_creation_1h_tokens": 0},
            }))
            .bind(thread)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    let large = HANDOFF_OFFER_TOKENS + 1;
    turn(large).await;
    let before = thread_context(&pool, thread).await.unwrap();
    assert_eq!(before.input_tokens, Some(large));
    assert!(before.offer);
    assert_eq!(
        cache_warmth(&pool, thread).await.unwrap(),
        CacheWarmth::Warm
    );

    emit(&bus, thread, session_started(SessionContinuity::Resumed)).await;
    assert_eq!(
        thread_context(&pool, thread).await.unwrap().input_tokens,
        Some(large),
        "a resume keeps its session's size"
    );

    emit(&bus, thread, session_started(handoff())).await;
    let after = thread_context(&pool, thread).await.unwrap();
    assert_eq!(after.input_tokens, None);
    assert!(!after.offer);
    assert_eq!(
        cache_warmth(&pool, thread).await.unwrap(),
        CacheWarmth::Cold
    );

    turn(20_000).await;
    assert_eq!(
        thread_context(&pool, thread).await.unwrap().input_tokens,
        Some(20_000)
    );

    teardown_test_db(&db).await;
}
