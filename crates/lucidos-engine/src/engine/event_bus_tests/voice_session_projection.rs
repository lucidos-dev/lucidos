//! A call makes the thread real, and moves nothing a turn owns (ADR 0167).

use super::super::*;
use super::*;

/// A draft carrying a stored compose text, so the clear has something to clear.
async fn a_draft(bus: &EventBus, pool: &sqlx::PgPool) -> Uuid {
    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    sqlx::query("UPDATE thread_summaries SET compose_text = 'half a thought' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(pool)
        .await
        .unwrap();
    thread_id
}

async fn a_call_opens(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::VoiceSessionStarted {
            session_id: Uuid::new_v4(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn a_call_ends(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::VoiceSessionEnded {
            session_id: Uuid::new_v4(),
            reason: crate::engine::thread_events::VoiceSessionEndReason::Hangup,
            duration_secs: 12,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn the_caller_says(bus: &EventBus, thread_id: Uuid, text: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SpokenMessageReceived {
            session_id: Uuid::new_v4(),
            text: text.into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn the_talker_says(bus: &EventBus, thread_id: Uuid, text: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SpokenReplyGenerated {
            session_id: Uuid::new_v4(),
            text: text.into(),
            interrupted: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// The reported bug. The talker answered every utterance itself, so nothing
/// emitted a `MessageReceived` and the whole call stayed inside a draft.
#[tokio::test]
async fn a_spoken_word_promotes_the_draft_the_call_was_placed_from() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_draft(&bus, &pool).await;

    a_call_opens(&bus, thread_id).await;
    the_caller_says(&bus, thread_id, "what happened").await;

    let (state, text, mode): (String, String, Option<String>) = sqlx::query_as(
        "SELECT state, compose_text, compose_mode FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "active");
    assert_eq!(text, "", "the draft the call consumed was left behind");
    assert_eq!(mode, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The talker greets before the caller says anything, so its own row is
/// usually the first. Either can be, and either promotes.
#[tokio::test]
async fn a_greeting_promotes_it_too_when_it_lands_first() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_draft(&bus, &pool).await;

    a_call_opens(&bus, thread_id).await;
    the_talker_says(&bus, thread_id, "Hi there. How can I help?").await;

    let state: String =
        sqlx::query_scalar("SELECT state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "active");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Connecting is not a conversation. Hang up before a word is said and the
/// draft stays a draft, which the reader can still see and discard. Promoting
/// there would leave a row nothing lists and nobody can reach.
#[tokio::test]
async fn a_call_nobody_spoke_on_leaves_the_draft_a_draft() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_draft(&bus, &pool).await;

    a_call_opens(&bus, thread_id).await;
    a_call_ends(&bus, thread_id).await;

    let (state, text): (String, String) =
        sqlx::query_as("SELECT state, compose_text FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "composing");
    assert_eq!(text, "half a thought", "a wordless call ate the draft");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A call is not a turn (ADR 0149), so the promotion touches no column a turn
/// owns. A thread reading `running` with nothing running is the failure.
#[tokio::test]
async fn the_promotion_moves_no_column_a_turn_owns() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_draft(&bus, &pool).await;

    a_call_opens(&bus, thread_id).await;
    the_caller_says(&bus, thread_id, "what happened").await;

    let (status, source, count): (String, String, i32) = sqlx::query_as(
        "SELECT status, source, message_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "idle");
    assert_eq!(source, "chat");
    assert_eq!(count, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The epoch is how a device learns its draft is gone. Broadcast rather than
/// left for the next reload, because the draft is on screen right now.
#[tokio::test]
async fn the_promotion_tells_every_device_the_draft_is_gone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_draft(&bus, &pool).await;
    let mut sse = bus.subscribe();

    a_call_opens(&bus, thread_id).await;
    the_caller_says(&bus, thread_id, "what happened").await;

    let epoch: i64 =
        sqlx::query_scalar("SELECT compose_epoch FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(epoch, 1, "the compose slot was consumed without saying so");

    let mut cleared = false;
    while let Ok(emitted) = sse.try_recv() {
        if let BusEvent::System(SystemEvent::ThreadComposeChanged {
            id,
            text,
            compose_epoch,
            ..
        }) = &emitted.typed
        {
            if *id == thread_id {
                assert_eq!(text, "");
                assert_eq!(*compose_epoch, epoch);
                cleared = true;
            }
        }
    }
    assert!(cleared, "no device was told the draft had gone");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A second call on a thread that is already real changes nothing about it
/// being real. The gate is what keeps the epoch still, so a reply draft
/// typed between the two calls survives the second one.
#[tokio::test]
async fn a_call_on_a_live_thread_leaves_its_reply_draft_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_draft(&bus, &pool).await;
    a_call_opens(&bus, thread_id).await;
    the_caller_says(&bus, thread_id, "what happened").await;
    a_call_ends(&bus, thread_id).await;
    sqlx::query("UPDATE thread_summaries SET compose_text = 'a reply' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();

    a_call_opens(&bus, thread_id).await;
    the_caller_says(&bus, thread_id, "anything for me").await;

    let (text, epoch): (String, i64) = sqlx::query_as(
        "SELECT compose_text, compose_epoch FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(text, "a reply");
    assert_eq!(epoch, 1, "the second call consumed a slot it never held");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// What the caller said first is what the drawer row reads, because
/// `format_display_title` falls back to it. A later utterance must not rewrite
/// the title out from under the reader.
#[tokio::test]
async fn the_callers_first_words_title_a_call_nobody_delegated_from() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_draft(&bus, &pool).await;

    a_call_opens(&bus, thread_id).await;
    the_caller_says(&bus, thread_id, "what happened overnight").await;
    the_caller_says(&bus, thread_id, "anything for me").await;

    let first: Option<String> =
        sqlx::query_scalar("SELECT first_message FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(first.as_deref(), Some("what happened overnight"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The point of the whole change, asked of the query the drawer actually runs.
///
/// Promotion alone is not enough and looked like it was: `get_recent_threads`
/// filters on `has_response`, so a promoted call with that column false is
/// neither a draft nor a listed thread. Every column assertion above passed
/// while the thread was unreachable, which is why this test asks the query.
#[tokio::test]
async fn a_finished_call_reaches_the_drawer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_draft(&bus, &pool).await;

    a_call_opens(&bus, thread_id).await;
    the_talker_says(&bus, thread_id, "Hi there. How can I help?").await;
    the_caller_says(&bus, thread_id, "what happened overnight").await;
    a_call_ends(&bus, thread_id).await;

    let store = crate::core::EventStore::new(pool.clone());
    let recent = store.get_recent_threads(50).await.expect("read the drawer");
    let row = recent
        .iter()
        .find(|t| t.thread_id == thread_id.to_string())
        .expect("a call nobody delegated from never reached the drawer");
    assert_eq!(row.title, "what happened overnight");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The one-shot rescue for drafts the old projection already stranded.
///
/// The migration is applied before any test writes a row, so it cannot see one.
/// Run its own text against the shape it exists for, twice, since a backfill
/// that is not idempotent is one that cannot be replayed.
#[tokio::test]
async fn the_migration_rescues_a_call_the_old_projection_stranded() {
    const MIGRATION: &str =
        include_str!("../../../migrations/20260830185302_voice_call_promotes_its_draft.sql");

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let stranded = a_draft(&bus, &pool).await;
    let untouched = a_draft(&bus, &pool).await;

    // The pre-fix shape: the events landed, and no column moved. Put every
    // column the new arms write back where the old one left it, or the test
    // measures the arms rather than the migration.
    a_call_opens(&bus, stranded).await;
    the_caller_says(&bus, stranded, "what happened overnight").await;
    sqlx::query(
        "UPDATE thread_summaries \
         SET state = 'composing', first_message = NULL, compose_epoch = 0, \
             has_response = FALSE \
         WHERE thread_id = $1",
    )
    .bind(stranded)
    .execute(&pool)
    .await
    .unwrap();

    for _ in 0..2 {
        sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();
    }

    let (state, first, epoch, listed): (String, Option<String>, i64, bool) = sqlx::query_as(
        "SELECT state, first_message, compose_epoch, has_response \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(stranded)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "active");
    assert_eq!(first.as_deref(), Some("what happened overnight"));
    assert_eq!(epoch, 1, "a replay consumed a second compose slot");
    assert!(listed, "the rescued call still cannot reach the drawer");

    let still_a_draft: String =
        sqlx::query_scalar("SELECT state FROM thread_summaries WHERE thread_id = $1")
            .bind(untouched)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        still_a_draft, "composing",
        "a draft nobody called from was promoted"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A whole call has to move the drawer. Otherwise a talker-only conversation
/// leaves the thread sorted where it was before anybody spoke.
#[tokio::test]
async fn a_spoken_turn_keeps_the_threads_recency_current() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = a_draft(&bus, &pool).await;
    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT last_activity FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SpokenReplyGenerated {
            session_id: Uuid::new_v4(),
            text: "Nothing urgent.".into(),
            interrupted: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (after, agent): (
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "SELECT last_activity, last_agent_action FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(after >= before, "the call left the thread reading stale");
    assert!(agent.is_some(), "the talker spoke and nothing recorded it");

    pool.close().await;
    teardown_test_db(&db_name).await;
}
