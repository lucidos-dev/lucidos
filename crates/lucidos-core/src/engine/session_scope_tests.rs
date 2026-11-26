use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::CodingAgent;
use crate::engine::thread_events::{
    EventChannel, EventMeta, FreshSessionReason, HandoffOpening, ThreadEvent,
};
use crate::test_support::{seed_coding_thread, setup_test_db, teardown_test_db};

fn kind_of(continuity: &SessionContinuity) -> String {
    serde_json::to_value(continuity).unwrap()["kind"]
        .as_str()
        .unwrap()
        .to_string()
}

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
            crossing: None,
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
        sandbox: None,
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
        crossing: None,
        opening: HandoffOpening::Narrative,
        message: String::new(),
    }
}

async fn capture(bus: &EventBus, thread_id: Uuid) {
    emit(
        bus,
        thread_id,
        crate::engine::thread_events::aux_capture::auxiliary_capture(
            crate::engine::thread_events::ContextPurpose::Title,
            "claude-opus-5-5",
            10,
            None,
            false,
        ),
    )
    .await;
}

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
