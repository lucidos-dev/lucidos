use crate::core::event_hooks::HookVerdict;
use crate::engine::chat::process::ChatProcessExt;
use crate::engine::event_bus::{BusEvent, HookRefusal, SystemEvent};
use crate::engine::thread_events::{
    ActorMode, EventMeta, McpEnablementSource, ThreadEvent, TriggerInvocation,
};
use crate::engine::LucidosEngine;
use crate::test_support::{
    arm_hook, boot_mock_engine, hook_refusals, hook_refuses, install_refusing_hook,
    run_on_engine_runtime, seed_chat_thread, setup_test_db, teardown_test_db, FakeHookAsker,
};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

async fn rows(engine: &LucidosEngine, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = $2")
        .bind(thread_id)
        .bind(event_type)
        .fetch_one(engine.pool())
        .await
        .unwrap()
}

async fn send(engine: Arc<LucidosEngine>, thread_id: Uuid, text: &'static str) {
    let event_id = Uuid::new_v4().to_string();
    engine
        .process_message_with_steps(
            text,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(false),
            Some(&event_id),
            Some(thread_id),
            None,
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            None,
            None,
            None,
            None,
            crate::engine::FollowUpUrgency::Normal,
            None,
        )
        .await
        .expect("a refused start ends its run without an error, so the queue keeps nothing");
}

#[test]
fn a_refused_first_message_records_nothing_and_tells_its_sender() {
    run_on_engine_runtime(
        "engine::chat::process::hook_refused_tests::a_refused_first_message_records_nothing_and_tells_its_sender",
        refused_first_message_case,
    );
}

async fn refused_first_message_case() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    install_refusing_hook(
        &engine.event_bus,
        "office-hours",
        "MessageReceived",
        "New threads start in office hours.",
    )
    .await;
    let mut deliveries = engine.event_bus.subscribe();
    let thread_id = Uuid::new_v4();

    send(engine.clone(), thread_id, "start something").await;

    assert_eq!(rows(&engine, thread_id, "MessageReceived").await, 0);
    assert_eq!(rows(&engine, thread_id, "ResponseFailed").await, 0);
    assert_eq!(hook_refusals(&pool, "office-hours").await, 1);
    assert!(!engine.processing_thread_ids().contains(&thread_id));
    let mut toasts = Vec::new();
    while let Ok(delivery) = deliveries.try_recv() {
        if let BusEvent::System(SystemEvent::Toast { message, level }) = delivery.typed {
            toasts.push((message, level));
        }
    }
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert_eq!(toasts[0].1, "error");
    assert!(
        toasts[0].0.contains("New threads start in office hours."),
        "{toasts:?}"
    );
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

async fn wait_for_asks(asker: &FakeHookAsker, count: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while asker.asked().len() < count {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("ask {count} never arrived"));
}

async fn install_held_refusal(
    engine: &LucidosEngine,
    slug: &str,
) -> (Arc<FakeHookAsker>, Arc<tokio::sync::Notify>) {
    let judging = Arc::new(tokio::sync::Notify::new());
    let asker = FakeHookAsker::held(
        hook_refuses("New threads start in office hours."),
        Some(judging.clone()),
    );
    engine
        .event_bus
        .install_event_hook_asker(asker.clone())
        .await
        .unwrap();
    arm_hook(&engine.event_bus, slug, "MessageReceived").await;
    (asker, judging)
}

#[test]
fn a_message_sent_while_the_first_is_judged_waits_and_is_judged_itself() {
    run_on_engine_runtime(
        "engine::chat::process::hook_refused_tests::a_message_sent_while_the_first_is_judged_waits_and_is_judged_itself",
        second_message_case,
    );
}

async fn second_message_case() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let (asker, judging) = install_held_refusal(&engine, "office-hours").await;
    let thread_id = Uuid::new_v4();

    let first = tokio::spawn(send(engine.clone(), thread_id, "start something"));
    wait_for_asks(&asker, 1).await;
    let second = tokio::spawn(send(engine.clone(), thread_id, "and one more thing"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        asker.asked().len(),
        1,
        "the second message waits for the first"
    );
    assert!(!second.is_finished());
    judging.notify_waiters();
    first.await.unwrap();
    wait_for_asks(&asker, 2).await;
    judging.notify_waiters();
    second.await.unwrap();

    assert_eq!(rows(&engine, thread_id, "MessageReceived").await, 0);
    assert_eq!(hook_refusals(&pool, "office-hours").await, 2);
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

#[test]
fn a_coding_agent_message_sent_while_the_run_starts_is_judged_as_the_first() {
    run_on_engine_runtime(
        "engine::chat::process::hook_refused_tests::a_coding_agent_message_sent_while_the_run_starts_is_judged_as_the_first",
        starting_run_case,
    );
}

async fn starting_run_case() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let thread_id = Uuid::new_v4();
    let mut starting = engine
        .claim_thread_for_spawn(thread_id, None)
        .await
        .unwrap()
        .expect("the thread is free");
    install_refusing_hook(
        &engine.event_bus,
        "office-hours",
        "MessageReceived",
        "New threads start in office hours.",
    )
    .await;
    let event_id = Uuid::new_v4().to_string();

    engine
        .process_message_with_steps(
            "and one more thing",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(true),
            Some(&event_id),
            Some(thread_id),
            None,
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            None,
            None,
            None,
            None,
            crate::engine::FollowUpUrgency::Normal,
            None,
        )
        .await
        .expect("a refused message ends its run without an error");

    assert_eq!(rows(&engine, thread_id, "MessageReceived").await, 0);
    assert_eq!(hook_refusals(&pool, "office-hours").await, 1);
    assert!(starting.entry.msg_rx.try_recv().is_err(), "nothing queued");
    drop(starting);
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

#[test]
fn a_child_whose_first_message_a_hook_allows_inherits_its_parents_servers() {
    run_on_engine_runtime(
        "engine::chat::process::hook_refused_tests::a_child_whose_first_message_a_hook_allows_inherits_its_parents_servers",
        allowed_child_case,
    );
}

async fn allowed_child_case() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let parent = seed_chat_thread(&engine.event_bus, "fan out").await;
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id: parent,
            event: ThreadEvent::ThreadMcpServerEnabled {
                server_id: "stub".into(),
                by: McpEnablementSource::Agent,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    let asker = FakeHookAsker::new(HookVerdict::Allowed(Vec::new()));
    engine
        .event_bus
        .install_event_hook_asker(asker.clone())
        .await
        .unwrap();
    arm_hook(&engine.event_bus, "office-hours", "MessageReceived").await;
    let child = Uuid::new_v4();

    let turn = tokio::spawn({
        let engine = engine.clone();
        async move {
            let _ = engine
                .process_message_with_steps(
                    "check the stub",
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(false),
                    None,
                    Some(child),
                    None,
                    None,
                    None,
                    Some(parent),
                    None,
                    None,
                    ActorMode::Agent,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    crate::engine::FollowUpUrgency::Normal,
                    None,
                )
                .await;
        }
    });
    let inherited = tokio::time::timeout(Duration::from_secs(10), async {
        while !engine
            .mcp_manager
            .enabled_servers(child)
            .await
            .contains("stub")
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    turn.abort();

    assert!(
        inherited.is_ok(),
        "the child never inherited its parent's server"
    );
    assert_eq!(asker.asked().len(), 1, "the hook judged the first message");
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

#[test]
fn a_refused_trigger_start_fails_the_fire() {
    run_on_engine_runtime(
        "engine::chat::process::hook_refused_tests::a_refused_trigger_start_fails_the_fire",
        refused_trigger_start_case,
    );
}

async fn refused_trigger_start_case() {
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    install_refusing_hook(
        &engine.event_bus,
        "quiet-nights",
        "TriggerStarted",
        "No trigger runs at night.",
    )
    .await;

    let error = engine
        .process_trigger(
            "nightly-report",
            "Nightly report",
            "nightly-report",
            "Write the report.",
            TriggerInvocation::Schedule,
            false,
            Vec::new(),
            None,
            None,
            None,
            Uuid::new_v4(),
            None,
            crate::triggers::continue_key::FireTarget::NewThread(None),
            None,
        )
        .await
        .expect_err("a refused trigger start fails the fire, so the scheduler reports it");

    let refusal = error
        .downcast_ref::<HookRefusal>()
        .expect("the fire fails with the hook's refusal");
    assert_eq!(refusal.slug.as_deref(), Some("quiet-nights"));
    let started: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'TriggerStarted'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(started, 0);
    assert_eq!(hook_refusals(&pool, "quiet-nights").await, 1);
    engine.pool().close().await;
    teardown_test_db(&db).await;
}
