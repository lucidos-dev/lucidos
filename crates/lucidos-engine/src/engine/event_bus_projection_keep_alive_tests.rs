//! A cache keep-alive turn leaves the summary row as it found it (I7), and a
//! real turn opener after it projects as usual (I8).

use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::{ActorMode, KEEP_ALIVE_IDLE_REASON};
use crate::test_support::{seed_thread_event, setup_test_db, start_cc_session, teardown_test_db};

fn injected(origin: Option<MessageOrigin>) -> ThreadEvent {
    ThreadEvent::PromptInjected {
        text: "go on".into(),
        mode: ActorMode::Engine,
        origin,
        injected_message_id: None,
        delivered_event_id: None,
    }
}

fn keep_alive_anchor() -> ThreadEvent {
    injected(Some(MessageOrigin::engine(EngineReason::CacheKeepAlive)))
}

fn idle(reason: Option<&str>) -> ThreadEvent {
    ThreadEvent::CodingAgentIdled {
        has_changes: false,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        reason: reason.map(String::from),
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
    }
}

/// What a person sees of the thread, plus the turn flag.
type Row = (String, chrono::DateTime<chrono::Utc>, i32, bool, bool);

async fn row(pool: &sqlx::PgPool, thread_id: Uuid) -> Row {
    sqlx::query_as(
        "SELECT status, last_activity, message_count, read_requested, keep_alive_turn \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .expect("the summary row")
}

#[test]
fn only_a_cache_keep_alive_origin_marks_the_anchor() {
    assert!(is_keep_alive_anchor(&keep_alive_anchor()));
    assert!(!is_keep_alive_anchor(&injected(None)));
    assert!(!is_keep_alive_anchor(&injected(Some(
        MessageOrigin::engine(EngineReason::MissingPlan)
    ))));
}

#[tokio::test]
async fn a_keep_alive_turn_leaves_the_row_as_it_was_and_projects_again_once_cleared() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/keep-alive", None).await;
    seed_thread_event(&bus, thread_id, injected(None)).await;
    seed_thread_event(&bus, thread_id, idle(None)).await;
    let waiting = row(&pool, thread_id).await;
    assert!(!waiting.4);

    seed_thread_event(&bus, thread_id, keep_alive_anchor()).await;
    bus.mark_keep_alive_turn(thread_id, true).await;
    seed_thread_event(&bus, thread_id, ThreadEvent::ThreadReadRequested).await;
    seed_thread_event(&bus, thread_id, idle(Some(KEEP_ALIVE_IDLE_REASON))).await;
    let after_keep_alive = row(&pool, thread_id).await;
    assert!(after_keep_alive.4, "the turn flag is set");
    assert_eq!(
        (
            &after_keep_alive.0,
            after_keep_alive.1,
            after_keep_alive.2,
            after_keep_alive.3
        ),
        (&waiting.0, waiting.1, waiting.2, waiting.3),
        "status, recency, count and read decision stay put"
    );

    bus.mark_keep_alive_turn(thread_id, false).await;
    seed_thread_event(&bus, thread_id, injected(None)).await;
    let real = row(&pool, thread_id).await;
    assert!(!real.4);
    assert_eq!(
        real.2,
        waiting.2 + 1,
        "the next real turn projects as usual"
    );
    assert_eq!(real.0, "running");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An anchor whose beat was skipped marks nothing, so the real turn that won
/// the thread projects in full.
#[tokio::test]
async fn an_anchor_alone_marks_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/keep-alive", None).await;
    seed_thread_event(&bus, thread_id, keep_alive_anchor()).await;
    seed_thread_event(&bus, thread_id, injected(None)).await;
    seed_thread_event(&bus, thread_id, idle(None)).await;
    let after = row(&pool, thread_id).await;
    assert!(!after.4);
    assert_eq!(after.2, 1, "only the real opener counts");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A card raised during a keep-alive makes the turn real: the thread waits on
/// a person, and what follows the answer must show.
#[tokio::test]
async fn a_card_ends_the_keep_alive_turn() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/keep-alive", None).await;
    seed_thread_event(&bus, thread_id, injected(None)).await;
    seed_thread_event(&bus, thread_id, idle(None)).await;
    bus.mark_keep_alive_turn(thread_id, true).await;
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::UserQuestionAsked {
            tool_use_id: "toolu_q".into(),
            cc_session_id: "s".into(),
            question: "which one?".into(),
            options: vec![],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
        },
    )
    .await;
    let after = row(&pool, thread_id).await;
    assert!(!after.4, "the card ends the keep-alive turn");
    assert_eq!(after.0, "waiting_for_user_answer");

    pool.close().await;
    teardown_test_db(&db_name).await;
}
