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

async fn requested_chat_thread(bus: &EventBus) -> Uuid {
    let thread_id = Uuid::new_v4();
    emit_thread_message(bus, thread_id, None, "write me the report").await;
    emit(
        bus,
        thread_id,
        ThreadEvent::ThreadReadRequested {
            parent_report: false,
        },
    )
    .await;
    emit_reply(bus, thread_id, "Here is the report.").await;
    thread_id
}

#[tokio::test]
async fn integrated_read_request_leaves_review_to_the_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = Uuid::new_v4();
    emit_thread_message(&bus, parent, None, "Coordinate the work").await;

    for (integrated, parent_report) in [(true, true), (false, true), (true, false)] {
        let child = Uuid::new_v4();
        emit_thread_message(&bus, child, Some(parent), "Write the report").await;
        sqlx::query(
            "UPDATE thread_summaries SET coding_agent_integrated = $2 WHERE thread_id = $1",
        )
        .bind(child)
        .bind(integrated)
        .execute(&pool)
        .await
        .unwrap();

        emit(
            &bus,
            child,
            ThreadEvent::ThreadReadRequested { parent_report },
        )
        .await;
        assert_eq!(
            read_requested(&pool, child).await,
            !(integrated && parent_report)
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
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
        ThreadEvent::ThreadReplySeen {
            seen_version,
            personal: false,
        },
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

async fn emit_answer(bus: &EventBus, thread_id: Uuid, actor: Option<MessageOrigin>, quiet: bool) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: Uuid::new_v4().to_string(),
            answer: AnswerKind::Selected {
                option_id: "opt-0".into(),
            },
            quiet,
        },
        meta: EventMeta {
            actor,
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_human_answer_clears_it_and_an_agent_or_engine_answer_does_not() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = requested_chat_thread(&bus).await;
    emit_answer(&bus, thread_id, None, false).await;
    let parent = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Agent,
        source_thread_id: Some(Uuid::new_v4()),
        emitting_trigger_id: None,
    };
    emit_answer(&bus, thread_id, Some(parent), false).await;
    assert!(
        read_requested(&pool, thread_id).await,
        "an engine padding or a parent's answer is not the user reading the reply"
    );

    let device = MessageOrigin::Device {
        device_id: "test-device".into(),
    };
    emit_answer(&bus, thread_id, Some(device.clone()), false).await;
    assert!(!read_requested(&pool, thread_id).await);

    emit(
        &bus,
        thread_id,
        ThreadEvent::ThreadReadRequested {
            parent_report: false,
        },
    )
    .await;
    emit_answer(&bus, thread_id, Some(device), true).await;
    assert!(
        !read_requested(&pool, thread_id).await,
        "a quiet pick reads the reply too"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

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

fn no(supersedes_request: bool) -> ThreadEvent {
    ThreadEvent::ThreadReadNotRequested { supersedes_request }
}

#[tokio::test]
async fn a_later_no_leaves_it_and_needs_no_attention() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (parent, child) = spawn_parent_child(&bus, EventChannel::Chat).await;
    emit(
        &bus,
        child,
        ThreadEvent::ThreadReadRequested {
            parent_report: false,
        },
    )
    .await;
    emit_reply(&bus, child, "Findings attached.").await;
    emit(&bus, child, no(false)).await;
    emit_reply(&bus, child, "A second reply, decided no.").await;

    assert!(read_requested(&pool, child).await);
    assert_eq!(read_attention_descendant_count(&pool, parent).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_superseding_no_clears_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (_parent, child) = spawn_parent_child(&bus, EventChannel::Chat).await;
    emit(
        &bus,
        child,
        ThreadEvent::ThreadReadRequested {
            parent_report: false,
        },
    )
    .await;
    emit_reply(&bus, child, "Blocked: the shell refuses every command.").await;
    emit(&bus, child, no(true)).await;
    emit_reply(&bus, child, "The shell works again.").await;

    assert!(!read_requested(&pool, child).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn empty_read_decision_payloads_read_as_unflagged() {
    let event: ThreadEvent =
        serde_json::from_value(serde_json::json!({"type": "ThreadReadNotRequested"})).unwrap();
    assert!(matches!(
        event,
        ThreadEvent::ThreadReadNotRequested {
            supersedes_request: false
        }
    ));
    assert_eq!(
        serde_json::to_value(no(false)).unwrap(),
        serde_json::json!({"type": "ThreadReadNotRequested"})
    );
    let yes: ThreadEvent =
        serde_json::from_value(serde_json::json!({"type": "ThreadReadRequested"})).unwrap();
    assert!(matches!(
        yes,
        ThreadEvent::ThreadReadRequested {
            parent_report: false
        }
    ));
}

#[tokio::test]
async fn a_coding_agent_user_message_or_human_prompt_clears_it_and_an_engine_prompt_does_not() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (_parent, child) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child).await;

    emit(
        &bus,
        child,
        ThreadEvent::ThreadReadRequested {
            parent_report: false,
        },
    )
    .await;
    emit(&bus, child, prompt(ActorMode::Engine)).await;
    assert!(
        read_requested(&pool, child).await,
        "an engine note is not the user"
    );

    emit(&bus, child, prompt(ActorMode::Human)).await;
    assert!(!read_requested(&pool, child).await);

    emit(
        &bus,
        child,
        ThreadEvent::ThreadReadRequested {
            parent_report: false,
        },
    )
    .await;
    emit(
        &bus,
        child,
        ThreadEvent::CodingAgentUserMessageSent {
            text: "looks good, carry on".into(),
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
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

#[tokio::test]
async fn a_child_request_never_counts_as_attention() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (parent, child) = spawn_parent_child(&bus, EventChannel::Chat).await;
    emit(
        &bus,
        child,
        ThreadEvent::ThreadReadRequested {
            parent_report: false,
        },
    )
    .await;
    emit_reply(&bus, child, "Findings attached.").await;

    assert!(read_requested(&pool, child).await);
    assert_eq!(read_attention_descendant_count(&pool, parent).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn requested_coding_agent_thread(bus: &EventBus, pool: &PgPool) -> Uuid {
    let (_parent, child) = spawn_parent_child(bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(bus, child).await;
    emit(
        bus,
        child,
        ThreadEvent::ThreadReadRequested {
            parent_report: false,
        },
    )
    .await;
    assert!(read_requested(pool, child).await);
    child
}

#[tokio::test]
async fn a_proposed_change_drops_the_request_and_blocks_a_new_one() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let child = requested_coding_agent_thread(&bus, &pool).await;
    emit_change_proposed(&bus, child, "claude-code/test", false).await;
    assert!(
        !read_requested(&pool, child).await,
        "the change asks for the look, so Apply must not leave an unread dot"
    );

    emit(
        &bus,
        child,
        ThreadEvent::ThreadReadRequested {
            parent_report: false,
        },
    )
    .await;
    assert!(!read_requested(&pool, child).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn withheld(reason: crate::engine::thread_events::UnproposedReason) -> ThreadEvent {
    ThreadEvent::ProposalWithheld {
        agent_returns: false,
        branch_name: "claude-code/test".into(),
        files: vec!["a.rs".into()],
        reason,
    }
}

#[tokio::test]
async fn work_held_for_a_harden_drops_the_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let child = requested_coding_agent_thread(&bus, &pool).await;
    emit(
        &bus,
        child,
        withheld(crate::engine::thread_events::UnproposedReason::HardeningMissing),
    )
    .await;
    assert!(!read_requested(&pool, child).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn integrated_hardening_hold_preserves_the_users_read_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let child = requested_coding_agent_thread(&bus, &pool).await;
    sqlx::query("UPDATE thread_summaries SET coding_agent_integrated = TRUE WHERE thread_id = $1")
        .bind(child)
        .execute(&pool)
        .await
        .unwrap();
    emit(
        &bus,
        child,
        withheld(crate::engine::thread_events::UnproposedReason::HardeningMissing),
    )
    .await;
    assert!(read_requested(&pool, child).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn work_held_for_any_other_reason_keeps_the_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let child = requested_coding_agent_thread(&bus, &pool).await;
    emit(
        &bus,
        child,
        withheld(crate::engine::thread_events::UnproposedReason::PlanMissing),
    )
    .await;
    assert!(read_requested(&pool, child).await);

    let committed = requested_coding_agent_thread(&bus, &pool).await;
    bus.refresh_branch_work_and_broadcast(committed, true)
        .await
        .unwrap();
    assert!(
        read_requested(&pool, committed).await,
        "a commit's unproposed work has no reason and lists nothing"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn start_trigger_run(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            retry_of: None,
            provider: None,
            trigger_id: "t-1".into(),
            trigger_name: Some("daily".into()),
            prompt: None,
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
            go_to_review: false,
            model: None,
            reasoning_effort: None,
            continue_key: None,
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

#[tokio::test]
async fn a_trigger_run_that_asks_lands_in_the_inbox() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let quiet = Uuid::new_v4();
    start_trigger_run(&bus, quiet).await;
    emit(&bus, quiet, no(false)).await;
    emit_reply(&bus, quiet, "Nothing new today.").await;
    assert_eq!(archive_state(&pool, quiet).await, "archived");

    let found = Uuid::new_v4();
    start_trigger_run(&bus, found).await;
    emit(
        &bus,
        found,
        ThreadEvent::ThreadReadRequested {
            parent_report: false,
        },
    )
    .await;
    emit_reply(&bus, found, "Three new invoices arrived.").await;
    assert_eq!(archive_state(&pool, found).await, "inbox");
    assert!(read_requested(&pool, found).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
