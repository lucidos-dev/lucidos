//! A thread's agent asks the user to read its latest reply (ADR 0409).
//!
//! `ThreadReadRequested` sets `read_requested`. A sighting, a human message or
//! an archive clears it, and nothing else does: a `ThreadReadNotRequested`
//! projects nothing. Each test pins one invariant of
//! `docs/plans/2026-10-10-agent-read-request.md` or
//! `docs/plans/2026-10-10-turn-end-gate-enforces-valid-states.md`.

use super::*;

async fn read_requested(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT read_requested FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn emit_reply(bus: &EventBus, thread_id: Uuid, text: &str) {
    emit(
        bus,
        thread_id,
        ThreadEvent::ResponseGenerated {
            text: text.into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;
}

fn prompt(mode: ActorMode) -> ThreadEvent {
    ThreadEvent::PromptInjected {
        text: "and one more thing".into(),
        mode,
        origin: None,
        injected_message_id: None,
        delivered_event_id: None,
    }
}

/// A chat thread whose turn asked to be read and then ended.
async fn requested_chat_thread(bus: &EventBus) -> Uuid {
    let thread_id = Uuid::new_v4();
    emit_thread_message(bus, thread_id, None, "write me the report").await;
    emit(bus, thread_id, ThreadEvent::ThreadReadRequested).await;
    emit_reply(bus, thread_id, "Here is the report.").await;
    thread_id
}

#[tokio::test]
async fn a_request_sets_the_flag_and_a_sighting_clears_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = requested_chat_thread(&bus).await;
    assert!(read_requested(&pool, thread_id).await);

    let seen_version: i64 =
        sqlx::query_scalar("SELECT summary_version FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    emit(
        &bus,
        thread_id,
        ThreadEvent::ThreadReplySeen { seen_version },
    )
    .await;
    assert!(!read_requested(&pool, thread_id).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_human_message_clears_it_and_an_agent_message_does_not() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = requested_chat_thread(&bus).await;
    let mut agent_message = thread_message(thread_id, None, "a follow-up from the parent");
    if let BusEvent::Thread {
        event: ThreadEvent::MessageReceived { mode, .. },
        ..
    } = &mut agent_message
    {
        *mode = ActorMode::Agent;
    }
    bus.emit(agent_message).await.unwrap();
    assert!(
        read_requested(&pool, thread_id).await,
        "an agent's message is not the user reading the reply"
    );

    emit_thread_message(&bus, thread_id, None, "thanks, now do the next one").await;
    assert!(!read_requested(&pool, thread_id).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A later turn that does not ask leaves an unseen request in place: the user
/// still has not read the reply that asked.
#[tokio::test]
async fn a_later_turn_end_leaves_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = requested_chat_thread(&bus).await;
    emit_reply(&bus, thread_id, "A second reply, no request.").await;
    assert!(read_requested(&pool, thread_id).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A later turn's no leaves an unseen request in place too, and counts as no
/// attention for the parent.
#[tokio::test]
async fn a_later_no_leaves_it_and_needs_no_attention() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (parent, child) = spawn_parent_child(&bus, EventChannel::Chat).await;
    emit(&bus, child, ThreadEvent::ThreadReadRequested).await;
    emit_reply(&bus, child, "Findings attached.").await;
    emit(&bus, child, ThreadEvent::ThreadReadNotRequested).await;
    emit_reply(&bus, child, "A second reply, decided no.").await;

    assert!(read_requested(&pool, child).await);
    assert_eq!(read_attention_descendant_count(&pool, parent).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_coding_agent_user_message_or_human_prompt_clears_it_and_an_engine_prompt_does_not() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (_parent, child) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child).await;

    emit(&bus, child, ThreadEvent::ThreadReadRequested).await;
    emit(&bus, child, prompt(ActorMode::Engine)).await;
    assert!(
        read_requested(&pool, child).await,
        "an engine note is not the user"
    );

    emit(&bus, child, prompt(ActorMode::Human)).await;
    assert!(!read_requested(&pool, child).await);

    emit(&bus, child, ThreadEvent::ThreadReadRequested).await;
    emit(
        &bus,
        child,
        ThreadEvent::CodingAgentUserMessageSent {
            text: "looks good, carry on".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
    )
    .await;
    assert!(!read_requested(&pool, child).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_archive_dismisses_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = requested_chat_thread(&bus).await;
    emit(&bus, thread_id, ThreadEvent::ThreadArchived).await;
    assert!(!read_requested(&pool, thread_id).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A read request is never something a thread is blocked on, so a child that
/// asks adds nothing to its parent's attention roll-up.
#[tokio::test]
async fn a_child_request_never_counts_as_attention() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (parent, child) = spawn_parent_child(&bus, EventChannel::Chat).await;
    emit(&bus, child, ThreadEvent::ThreadReadRequested).await;
    emit_reply(&bus, child, "Findings attached.").await;

    assert!(read_requested(&pool, child).await);
    assert_eq!(read_attention_descendant_count(&pool, parent).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn start_trigger_run(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            provider: None,
            trigger_id: "t-1".into(),
            trigger_name: Some("daily".into()),
            prompt: None,
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
            go_to_review: false,
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn archive_state(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// An unattended trigger run stays archived, unless it asked to be read. Then
/// its turn end lands it in the inbox, where Review lists it.
#[tokio::test]
async fn a_trigger_run_that_asks_lands_in_the_inbox() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let quiet = Uuid::new_v4();
    start_trigger_run(&bus, quiet).await;
    emit(&bus, quiet, ThreadEvent::ThreadReadNotRequested).await;
    emit_reply(&bus, quiet, "Nothing new today.").await;
    assert_eq!(archive_state(&pool, quiet).await, "archived");

    let found = Uuid::new_v4();
    start_trigger_run(&bus, found).await;
    emit(&bus, found, ThreadEvent::ThreadReadRequested).await;
    emit_reply(&bus, found, "Three new invoices arrived.").await;
    assert_eq!(archive_state(&pool, found).await, "inbox");
    assert!(read_requested(&pool, found).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
