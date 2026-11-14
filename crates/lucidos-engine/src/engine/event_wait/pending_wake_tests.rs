//! What the expected wake decision reads, and where it reads nothing.

use chrono::{Duration, Utc};
use uuid::Uuid;

use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::expected_wake::{record_agent_expected_wake, ExpectedWake};
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};

fn live(watches: &[&str]) -> LiveWait {
    LiveWait {
        wait_id: Uuid::new_v4(),
        thread_id: Uuid::new_v4(),
        tool_use_id: "toolu_1".into(),
        on: watches
            .iter()
            .map(|kind| EventSubscription {
                event_type: (*kind).into(),
                condition: None,
            })
            .collect(),
        reason: "the release build".into(),
        armed_at: Utc::now() - Duration::minutes(3),
        expires_at: Utc::now() + Duration::minutes(57),
        watermark: 0,
    }
}

fn message(text: &str, parent: Option<Uuid>) -> ThreadEvent {
    ThreadEvent::MessageReceived {
        provider: None,
        voice_session_id: None,
        wake_mode: None,
        text: text.into(),
        user_image_hashes: vec![],
        device_id: None,
        image_description: None,
        parent_thread_id: parent,
        spawning_event_id: None,
        mode: if parent.is_some() {
            ActorMode::Agent
        } else {
            ActorMode::Human
        },
        model: None,
        reasoning_effort: None,
        origin: None,
    }
}

async fn emit(bus: &EventBus, thread_id: Uuid, channel: EventChannel, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta {
            channel: Some(channel),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// The keep-alive pings only for waits on keep-warm kinds, so only those
/// count as pending.
#[test]
fn only_keep_warm_waits_count() {
    let waits = keep_warm_waits(vec![
        live(&["BackgroundBashCompleted"]),
        live(&["EmailReceived"]),
        live(&["ChangeApplied", "TriggerFired"]),
    ]);
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].watches, "BackgroundBashCompleted");
    assert_eq!(waits[0].reason, "the release build");
}

/// A thread with nothing keep-warm pending, a Codex thread and an unattended
/// trigger thread owe no expected wake. Every other agent with a wait does.
#[tokio::test]
async fn only_a_kept_thread_with_something_pending_reads_its_work() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let waits = || keep_warm_waits(vec![live(&["BackgroundBashCompleted"])]);

    let chat = Uuid::new_v4();
    emit(&bus, chat, EventChannel::Chat, message("release it", None)).await;
    assert_eq!(
        read_pending_wake(&pool, chat, Vec::new()).await.unwrap(),
        None
    );
    let read = read_pending_wake(&pool, chat, waits())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.agent, WaitAgent::Lucidos);
    assert_eq!(read.waits.len(), 1);

    let cc = Uuid::new_v4();
    emit(
        &bus,
        cc,
        EventChannel::ClaudeCode,
        message("build it", None),
    )
    .await;
    start_cc_session(&bus, cc, "claude-code/test", None).await;
    let read = read_pending_wake(&pool, cc, waits())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.agent, WaitAgent::ClaudeCode);

    sqlx::query("UPDATE thread_summaries SET coding_agent = $2 WHERE thread_id = $1")
        .bind(cc)
        .bind(CodingAgent::Codex.as_str())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(read_pending_wake(&pool, cc, waits()).await.unwrap(), None);

    let trigger = Uuid::new_v4();
    emit(
        &bus,
        trigger,
        EventChannel::Trigger,
        message("nightly", Some(Uuid::new_v4())),
    )
    .await;
    sqlx::query("UPDATE thread_summaries SET trigger_id = 'nightly' WHERE thread_id = $1")
        .bind(trigger)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        read_pending_wake(&pool, trigger, waits()).await.unwrap(),
        None
    );

    assert_eq!(
        read_pending_wake(&pool, Uuid::new_v4(), waits())
            .await
            .unwrap(),
        None
    );
    teardown_test_db(&db).await;
}

/// A working child that reports is pending work. A `no-wake` child never
/// wakes the thread, so it is not.
#[tokio::test]
async fn a_reporting_child_is_pending_and_a_no_wake_child_is_not() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = Uuid::new_v4();
    emit(
        &bus,
        parent,
        EventChannel::Chat,
        message("release it", None),
    )
    .await;

    let quiet = Uuid::new_v4();
    emit(
        &bus,
        quiet,
        EventChannel::Chat,
        message("watch the logs", Some(parent)),
    )
    .await;
    sqlx::query("UPDATE thread_summaries SET wake_mode = $2, status = $3 WHERE thread_id = $1")
        .bind(quiet)
        .bind(WakeMode::NoWake.as_str())
        .bind(ThreadStatus::Running.as_str())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        read_pending_wake(&pool, parent, Vec::new()).await.unwrap(),
        None
    );

    let builder = Uuid::new_v4();
    emit(
        &bus,
        builder,
        EventChannel::Chat,
        message("build the arm64 tarball", Some(parent)),
    )
    .await;
    sqlx::query("UPDATE thread_summaries SET status = $2 WHERE thread_id = $1")
        .bind(builder)
        .bind(ThreadStatus::Running.as_str())
        .execute(&pool)
        .await
        .unwrap();
    let read = read_pending_wake(&pool, parent, Vec::new())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.children.len(), 1);
    assert_eq!(read.children[0].task, "build the arm64 tarball");
    assert!(!read.children[0].awaiting_user);

    teardown_test_db(&db).await;
}

/// Children that all wait on a person keep nobody warm, so nothing is
/// pending. Beside a working child, such a child still reads as context.
#[tokio::test]
async fn children_that_all_wait_on_a_person_are_not_pending() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = Uuid::new_v4();
    emit(
        &bus,
        parent,
        EventChannel::Chat,
        message("release it", None),
    )
    .await;
    let child = |task: &'static str, status: ThreadStatus| {
        let (bus, pool) = (bus.clone(), pool.clone());
        async move {
            let id = Uuid::new_v4();
            emit(&bus, id, EventChannel::Chat, message(task, Some(parent))).await;
            sqlx::query("UPDATE thread_summaries SET status = $2 WHERE thread_id = $1")
                .bind(id)
                .bind(status.as_str())
                .execute(&pool)
                .await
                .unwrap();
        }
    };

    child("ask about the version", ThreadStatus::WaitingForUserAnswer).await;
    assert_eq!(
        read_pending_wake(&pool, parent, Vec::new()).await.unwrap(),
        None
    );

    child("build the arm64 tarball", ThreadStatus::Running).await;
    let read = read_pending_wake(&pool, parent, Vec::new())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.children.len(), 2);
    assert_eq!(read.children.iter().filter(|c| c.awaiting_user).count(), 1);

    teardown_test_db(&db).await;
}

/// The decision sees its own misses: an estimate that passed while this wait
/// ran. A future estimate has not missed yet.
#[tokio::test]
async fn an_estimate_that_passed_reads_as_a_miss() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit(
        &bus,
        thread_id,
        EventChannel::Chat,
        message("release it", None),
    )
    .await;
    let stated = |secs_ago: i64, reason: &str| {
        let mut wake = ExpectedWake::stated(60.0, reason, Utc::now()).unwrap();
        wake.at = Utc::now() - Duration::seconds(secs_ago);
        wake
    };
    record_agent_expected_wake(&bus, thread_id, stated(30, "about 25 min per run"))
        .await
        .unwrap();
    record_agent_expected_wake(&bus, thread_id, stated(-600, "another 10 min"))
        .await
        .unwrap();

    let waits = keep_warm_waits(vec![live(&["BackgroundBashCompleted"])]);
    let read = read_pending_wake(&pool, thread_id, waits)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.misses.len(), 1);
    assert_eq!(read.misses[0].reason, "about 25 min per run");
    assert_eq!(read.misses[0].source.0, WakeSource::Agent);

    teardown_test_db(&db).await;
}
