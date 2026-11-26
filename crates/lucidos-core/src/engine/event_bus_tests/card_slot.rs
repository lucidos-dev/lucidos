use super::super::*;
use super::*;

fn question(tool_use_id: &str) -> ThreadEvent {
    ThreadEvent::UserQuestionAsked {
        tool_use_id: tool_use_id.into(),
        cc_session_id: String::new(),
        question: format!("Question {tool_use_id}?"),
        options: vec![],
        worktree_path: None,
        multi_select: false,
        owner_approval: None,
        permission_request: None,
    }
}

async fn ask(bus: &EventBus, thread_id: Uuid, tool_use_id: &str) -> bool {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: question(tool_use_id),
        meta: EventMeta::NONE,
    })
    .await
    .unwrap()
    .is_some()
}

async fn answer(
    bus: &EventBus,
    thread_id: Uuid,
    tool_use_id: &str,
    answer: AnswerKind,
) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>> {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: tool_use_id.into(),
            answer,
            quiet: false,
        },
        meta: EventMeta::NONE,
    })
    .await
}

async fn card_rows(pool: &PgPool, thread_id: Uuid) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT event_type, payload->>'tool_use_id' FROM events \
         WHERE aggregate_id = $1 AND event_type IN ('UserQuestionAsked', 'UserQuestionQueued') \
         ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .fetch_all(pool)
    .await
    .unwrap()
}

fn row(event_type: &str, tool_use_id: &str) -> (String, String) {
    (event_type.to_string(), tool_use_id.to_string())
}

async fn read_status(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_second_card_queues_behind_the_open_one() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;

    ask(&bus, thread_id, "a").await;
    ask(&bus, thread_id, "b").await;

    assert_eq!(
        card_rows(&pool, thread_id).await,
        vec![
            row("UserQuestionAsked", "a"),
            row("UserQuestionQueued", "b")
        ]
    );
    assert_eq!(
        read_status(&pool, thread_id).await,
        "waiting_for_user_answer"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_atomic_batch_sees_its_own_earlier_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;

    bus.emit_atomically(
        thread_id,
        vec![
            (question("a"), EventMeta::NONE),
            (question("b"), EventMeta::NONE),
            (question("c"), EventMeta::NONE),
        ],
    )
    .await
    .unwrap()
    .expect("the batch lands");

    assert_eq!(
        card_rows(&pool, thread_id).await,
        vec![
            row("UserQuestionAsked", "a"),
            row("UserQuestionQueued", "b"),
            row("UserQuestionQueued", "c"),
        ]
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn concurrent_producers_open_one_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;

    let ids: Vec<String> = (0..6).map(|i| format!("card-{i}")).collect();
    futures::future::join_all(ids.iter().map(|id| ask(&bus, thread_id, id))).await;

    let rows = card_rows(&pool, thread_id).await;
    let asked = rows
        .iter()
        .filter(|(t, _)| t == "UserQuestionAsked")
        .count();
    let queued = rows
        .iter()
        .filter(|(t, _)| t == "UserQuestionQueued")
        .count();
    assert_eq!((asked, queued), (1, 5), "{rows:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_open_permission_prompt_holds_the_slot() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::McpPermissionRequested {
            request_id: "req-1".into(),
            tool_use_id: "tu-mcp".into(),
            server_id: "files".into(),
            server_name: "Files".into(),
            tool_name: "read".into(),
            arguments_summary: "{}".into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    ask(&bus, thread_id, "a").await;
    assert_eq!(
        card_rows(&pool, thread_id).await,
        vec![row("UserQuestionQueued", "a")]
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::McpPermissionResolved {
            request_id: "req-1".into(),
            allowed: true,
            reason: None,
            persist_scope: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    ask(&bus, thread_id, "a").await;
    assert_eq!(
        card_rows(&pool, thread_id).await,
        vec![
            row("UserQuestionQueued", "a"),
            row("UserQuestionAsked", "a")
        ],
        "a release posts the queued card once the prompt resolves"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn mcp_prompt(request_id: &str) -> ThreadEvent {
    ThreadEvent::McpPermissionRequested {
        request_id: request_id.into(),
        tool_use_id: format!("tu-{request_id}"),
        server_id: "files".into(),
        server_name: "Files".into(),
        tool_name: "read".into(),
        arguments_summary: "{}".into(),
    }
}

fn mcp_resolution(request_id: &str) -> ThreadEvent {
    ThreadEvent::McpPermissionResolved {
        request_id: request_id.into(),
        allowed: false,
        reason: Some("auto-denied".into()),
        persist_scope: None,
    }
}

#[tokio::test]
async fn a_permission_prompt_is_refused_while_a_card_is_open() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;
    ask(&bus, thread_id, "a").await;

    let refused = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: mcp_prompt("req-1"),
            meta: EventMeta::NONE,
        })
        .await
        .err()
        .expect("the slot is taken");
    assert!(
        refused.is::<crate::engine::event_bus::CardSlotTaken>(),
        "{refused}"
    );

    bus.emit_atomically(
        thread_id,
        vec![
            (mcp_prompt("req-2"), EventMeta::NONE),
            (mcp_resolution("req-2"), EventMeta::NONE),
        ],
    )
    .await
    .unwrap()
    .expect("a prompt resolved in its own batch is a record, not a card");

    answer(&bus, thread_id, "a", AnswerKind::Canceled)
        .await
        .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: mcp_prompt("req-1"),
        meta: EventMeta::NONE,
    })
    .await
    .expect("the slot is free");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_release_posts_only_into_a_free_slot_and_only_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;
    ask(&bus, thread_id, "a").await;
    ask(&bus, thread_id, "b").await;

    assert!(
        !ask(&bus, thread_id, "b").await,
        "the slot is taken, so the release records nothing"
    );

    answer(
        &bus,
        thread_id,
        "a",
        AnswerKind::Selected {
            option_id: "yes".into(),
        },
    )
    .await
    .unwrap();
    assert!(ask(&bus, thread_id, "b").await);
    assert!(!ask(&bus, thread_id, "b").await, "posted already");

    assert_eq!(
        card_rows(&pool, thread_id).await,
        vec![
            row("UserQuestionAsked", "a"),
            row("UserQuestionQueued", "b"),
            row("UserQuestionAsked", "b"),
        ]
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_owner_cannot_answer_a_queued_card_but_the_engine_can_retire_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;
    ask(&bus, thread_id, "a").await;
    ask(&bus, thread_id, "b").await;

    let refused = answer(
        &bus,
        thread_id,
        "b",
        AnswerKind::FreeText {
            text: "hi".into(),
            image_hashes: vec![],
        },
    )
    .await;
    assert_eq!(
        refused.err().expect("refused").to_string(),
        "question_queued"
    );

    let client_cancel = bus
        .emit_question_answer(
            thread_id,
            ThreadEvent::UserQuestionAnswered {
                compose: None,
                tool_use_id: "b".into(),
                answer: AnswerKind::Canceled,
                quiet: false,
            },
            EventMeta::NONE,
            None,
            None,
        )
        .await;
    assert_eq!(
        client_cancel.err().expect("refused").to_string(),
        "question_queued"
    );

    answer(&bus, thread_id, "b", AnswerKind::Canceled)
        .await
        .expect("the engine retires a queued card");
    assert_eq!(
        read_status(&pool, thread_id).await,
        "waiting_for_user_answer",
        "the open card still holds the thread"
    );
    assert!(
        !ask(&bus, thread_id, "b").await,
        "a retired card is never posted"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn retiring_a_queued_card_leaves_the_open_one_holding_the_slot() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;
    ask(&bus, thread_id, "a").await;
    ask(&bus, thread_id, "b").await;
    answer(&bus, thread_id, "b", AnswerKind::Superseded)
        .await
        .expect("the engine retires a queued card");

    ask(&bus, thread_id, "c").await;

    assert_eq!(
        card_rows(&pool, thread_id).await,
        vec![
            row("UserQuestionAsked", "a"),
            row("UserQuestionQueued", "b"),
            row("UserQuestionQueued", "c")
        ],
        "an answer to another card never frees the open card's slot"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
