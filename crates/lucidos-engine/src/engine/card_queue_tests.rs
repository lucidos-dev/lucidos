use std::time::Duration;

use super::*;
use crate::engine::agent_host::NoAgentHost;
use crate::engine::permission::{menu, raise_request, Action, RequestCard};
use crate::engine::thread_events::EventChannel;
use crate::test_support::{
    boot_mock_engine_with_host, seed_coding_thread_as, setup_test_db, teardown_test_db,
};

async fn boot_with_thread(db: &str) -> (Arc<LucidosEngine>, tempfile::TempDir, Uuid) {
    let thread_id = Uuid::new_v4();
    let host = NoAgentHost {
        held_message_release_thread: Some(thread_id),
    };
    let (engine, workspace) = boot_mock_engine_with_host(db, Arc::new(host)).await;
    seed_coding_thread_as(&engine.event_bus, thread_id, None).await;
    end_turn(&engine, thread_id).await;
    (engine, workspace, thread_id)
}

async fn start_turn(engine: &LucidosEngine, thread_id: Uuid) {
    emit(
        engine,
        thread_id,
        crate::test_support::message_received("carry on"),
    )
    .await;
}

fn agent_card(tool_use_id: &str) -> ThreadEvent {
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

async fn emit(engine: &LucidosEngine, thread_id: Uuid, event: ThreadEvent) {
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
}

async fn ask(engine: &LucidosEngine, thread_id: Uuid, tool_use_id: &str) {
    emit(engine, thread_id, agent_card(tool_use_id)).await;
}

async fn end_turn(engine: &LucidosEngine, thread_id: Uuid) {
    emit(
        engine,
        thread_id,
        ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        },
    )
    .await;
}

async fn raise_engine_card(engine: &LucidosEngine, thread_id: Uuid) -> String {
    raise_request(
        &engine.event_bus,
        thread_id,
        host_log_card(thread_id),
        EventChannel::ClaudeCode,
    )
    .await
    .unwrap()
}

async fn raise_turn_holding_card(engine: &LucidosEngine, thread_id: Uuid) -> String {
    crate::engine::permission::raise_turn_holding_request(
        &engine.event_bus,
        thread_id,
        host_log_card(thread_id),
        EventChannel::ClaudeCode,
    )
    .await
    .unwrap()
}

fn host_log_card(thread_id: Uuid) -> RequestCard {
    let action: Action = serde_json::from_value(serde_json::json!("transfer-host-logs")).unwrap();
    RequestCard {
        question: "Send the host logs?".into(),
        header: "Permission",
        action,
        breadth: Some("host-log-transfer:test".into()),
        options: menu::options(action, thread_id, "The host logs were not sent."),
        batch: None,
    }
}

async fn answer(engine: &Arc<LucidosEngine>, thread_id: Uuid, tool_use_id: &str, kind: AnswerKind) {
    let result = answer_pending_question(engine, thread_id, tool_use_id.into(), kind, None).await;
    assert!(matches!(result, AnswerResult::Resumed), "{result:?}");
}

fn typed(text: &str) -> AnswerKind {
    AnswerKind::FreeText {
        text: text.into(),
        image_hashes: vec![],
    }
}

async fn card_rows(engine: &LucidosEngine, thread_id: Uuid) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT event_type, payload->>'tool_use_id' FROM events WHERE aggregate_id = $1 \
         AND event_type IN ('UserQuestionAsked', 'UserQuestionQueued') ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .fetch_all(engine.pool())
    .await
    .unwrap()
}

fn row(event_type: &str, tool_use_id: &str) -> (String, String) {
    (event_type.into(), tool_use_id.into())
}

async fn answer_kind(engine: &LucidosEngine, thread_id: Uuid, tool_use_id: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT payload->'answer'->>'kind' FROM events WHERE aggregate_id = $1 \
         AND event_type = 'UserQuestionAnswered' AND payload->>'tool_use_id' = $2",
    )
    .bind(thread_id.to_string())
    .bind(tool_use_id)
    .fetch_optional(engine.pool())
    .await
    .unwrap()
}

async fn two_agent_cards(engine: &LucidosEngine, thread_id: Uuid) {
    ask(engine, thread_id, "a").await;
    ask(engine, thread_id, "b").await;
    assert_eq!(
        card_rows(engine, thread_id).await,
        vec![
            row("UserQuestionAsked", "a"),
            row("UserQuestionQueued", "b")
        ]
    );
}

fn posted_both() -> Vec<(String, String)> {
    vec![
        row("UserQuestionAsked", "a"),
        row("UserQuestionQueued", "b"),
        row("UserQuestionAsked", "b"),
    ]
}

#[tokio::test]
async fn an_answered_card_releases_the_next_one() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    let engine_card = raise_engine_card(&engine, thread_id).await;
    ask(&engine, thread_id, "b").await;

    release(&engine, thread_id).await.unwrap();
    assert_eq!(
        card_rows(&engine, thread_id).await.len(),
        2,
        "the request is open"
    );

    let option_id = first_option_id(&engine, thread_id, &engine_card).await;
    answer(
        &engine,
        thread_id,
        &engine_card,
        AnswerKind::Selected { option_id },
    )
    .await;
    release(&engine, thread_id).await.unwrap();
    assert_eq!(
        card_rows(&engine, thread_id).await,
        vec![
            row("UserQuestionAsked", &engine_card),
            row("UserQuestionQueued", "b"),
            row("UserQuestionAsked", "b"),
        ]
    );

    release(&engine, thread_id).await.unwrap();
    assert_eq!(card_rows(&engine, thread_id).await.len(), 3, "posted once");
    teardown_test_db(&db).await;
}

async fn first_option_id(engine: &LucidosEngine, thread_id: Uuid, tool_use_id: &str) -> String {
    sqlx::query_scalar(
        "SELECT payload->'options'->0->>'id' FROM events WHERE aggregate_id = $1 \
         AND event_type = 'UserQuestionAsked' AND payload->>'tool_use_id' = $2",
    )
    .bind(thread_id.to_string())
    .bind(tool_use_id)
    .fetch_one(engine.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn a_canceled_or_superseded_card_releases_the_next_one() {
    let (_pool, db) = setup_test_db().await;
    for kind in [AnswerKind::Canceled, AnswerKind::Superseded] {
        let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
        two_agent_cards(&engine, thread_id).await;
        answer(&engine, thread_id, "a", kind).await;
        release(&engine, thread_id).await.unwrap();
        assert_eq!(card_rows(&engine, thread_id).await, posted_both());
        assert_eq!(answer_kind(&engine, thread_id, "b").await, None);
    }
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_overtaken_card_releases_a_queued_engine_card() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    ask(&engine, thread_id, "a").await;
    let engine_card = raise_engine_card(&engine, thread_id).await;
    end_turn(&engine, thread_id).await;

    release(&engine, thread_id).await.unwrap();
    assert_eq!(
        card_rows(&engine, thread_id).await.last(),
        Some(&row("UserQuestionAsked", &engine_card)),
        "an engine card waits for its turn on screen, whatever the agent did"
    );
    assert_eq!(answer_kind(&engine, thread_id, &engine_card).await, None);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_queued_agent_card_whose_turn_moved_on_is_superseded_not_posted() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    let engine_card = raise_engine_card(&engine, thread_id).await;
    ask(&engine, thread_id, "b").await;
    ask(&engine, thread_id, "c").await;
    end_turn(&engine, thread_id).await;

    answer(&engine, thread_id, &engine_card, AnswerKind::Canceled).await;
    release(&engine, thread_id).await.unwrap();

    let posted: Vec<_> = card_rows(&engine, thread_id)
        .await
        .into_iter()
        .filter(|(event_type, _)| event_type == "UserQuestionAsked")
        .collect();
    assert_eq!(posted, vec![row("UserQuestionAsked", &engine_card)]);
    for card in ["b", "c"] {
        assert_eq!(
            answer_kind(&engine, thread_id, card).await.as_deref(),
            Some("Superseded")
        );
    }
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_owner_cannot_answer_a_queued_card_but_the_engine_can_retire_it() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    two_agent_cards(&engine, thread_id).await;

    let reply = answer_pending_question(&engine, thread_id, "b".into(), typed("no"), None).await;
    assert!(matches!(reply, AnswerResult::Conflict(_)), "{reply:?}");
    answer(&engine, thread_id, "b", AnswerKind::Canceled).await;

    answer(&engine, thread_id, "a", typed("yes")).await;
    release(&engine, thread_id).await.unwrap();
    assert_eq!(
        card_rows(&engine, thread_id).await.len(),
        2,
        "b was retired"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_consumer_releases_a_card_closed_while_it_was_down() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    two_agent_cards(&engine, thread_id).await;
    answer(&engine, thread_id, "a", AnswerKind::Canceled).await;
    assert_eq!(card_rows(&engine, thread_id).await.len(), 2);

    let consumer = spawn(engine.clone());
    wait_for_rows(&engine, thread_id, 3).await;
    assert_eq!(card_rows(&engine, thread_id).await, posted_both());

    ask(&engine, thread_id, "c").await;
    answer(&engine, thread_id, "b", AnswerKind::Canceled).await;
    wait_for_rows(&engine, thread_id, 5).await;
    assert_eq!(
        card_rows(&engine, thread_id).await.last(),
        Some(&row("UserQuestionAsked", "c")),
        "the live consumer releases on the close it sees"
    );
    consumer.abort();
    teardown_test_db(&db).await;
}

async fn wait_for_rows(engine: &LucidosEngine, thread_id: Uuid, count: usize) {
    for _ in 0..200 {
        if card_rows(engine, thread_id).await.len() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the card queue never reached {count} card events");
}

async fn mcp_request_ids(engine: &LucidosEngine, thread_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT payload->>'request_id' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'McpPermissionRequested' ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .fetch_all(engine.pool())
    .await
    .unwrap()
}

async fn wait_for_mcp_requests(engine: &LucidosEngine, thread_id: Uuid, count: usize) {
    for _ in 0..200 {
        if mcp_request_ids(engine, thread_id).await.len() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the thread never held {count} MCP permission requests");
}

fn ask_mcp(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    tool: &'static str,
    cancel: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<crate::engine::mcp_permission::McpAsk> {
    let engine = engine.clone();
    tokio::spawn(async move {
        engine
            .ask_mcp_permission(
                "files",
                "Files",
                tool,
                &format!("mcp__files__{tool}"),
                &serde_json::json!({}),
                "{}",
                thread_id,
                &format!("tu-{tool}"),
                &EventMeta::NONE,
                &cancel,
            )
            .await
    })
}

async fn allow_mcp(engine: &LucidosEngine, request_id: String) {
    assert!(
        crate::engine::mcp_permission::resolve_mcp_permission(
            engine,
            request_id,
            true,
            None,
            crate::engine::permission::tool_grants::AnsweredBy::tap(None),
            "test",
        )
        .await
    );
}

async fn boot_chat_with_open_question(db: &str) -> (Arc<LucidosEngine>, tempfile::TempDir, Uuid) {
    let (engine, workspace) = crate::test_support::boot_mock_engine(db).await;
    let thread_id = crate::test_support::seed_chat_thread(&engine.event_bus, "use files").await;
    ask(&engine, thread_id, "a").await;
    (engine, workspace, thread_id)
}

async fn cancel_question(engine: &LucidosEngine, thread_id: Uuid, tool_use_id: &str) {
    emit(
        engine,
        thread_id,
        ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: tool_use_id.into(),
            answer: AnswerKind::Canceled,
            quiet: false,
        },
    )
    .await;
}

#[tokio::test]
async fn mcp_prompts_wait_for_the_open_card_and_show_one_at_a_time() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_chat_with_open_question(&db).await;

    let cancel = tokio_util::sync::CancellationToken::new();
    let read = ask_mcp(&engine, thread_id, "read", cancel.clone());
    let write = ask_mcp(&engine, thread_id, "write", cancel.clone());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        mcp_request_ids(&engine, thread_id).await.is_empty(),
        "no prompt shows while the question is open"
    );

    cancel_question(&engine, thread_id, "a").await;
    wait_for_mcp_requests(&engine, thread_id, 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let shown = mcp_request_ids(&engine, thread_id).await;
    assert_eq!(shown.len(), 1, "one prompt at a time");

    allow_mcp(&engine, shown[0].clone()).await;
    wait_for_mcp_requests(&engine, thread_id, 2).await;
    allow_mcp(
        &engine,
        mcp_request_ids(&engine, thread_id).await[1].clone(),
    )
    .await;

    for waiter in [read, write] {
        let ask = tokio::time::timeout(Duration::from_secs(10), waiter)
            .await
            .expect("both calls get an answer")
            .unwrap();
        assert!(matches!(
            ask,
            crate::engine::mcp_permission::McpAsk::Proceed
        ));
    }
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_prompt_canceled_while_it_waits_leaves_no_card() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_chat_with_open_question(&db).await;

    let cancel = tokio_util::sync::CancellationToken::new();
    let read = ask_mcp(&engine, thread_id, "read", cancel.clone());
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();
    let ask = tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .expect("a cancel ends the wait")
        .unwrap();
    assert!(matches!(
        ask,
        crate::engine::mcp_permission::McpAsk::Refuse(_)
    ));

    cancel_question(&engine, thread_id, "a").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(mcp_request_ids(&engine, thread_id).await.is_empty());
    assert!(engine
        .pending_mcp_permission
        .lock()
        .unwrap()
        .by_request_id
        .is_empty());
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_stop_retires_queued_agent_cards_and_keeps_an_engine_card() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    two_agent_cards(&engine, thread_id).await;
    let engine_card = raise_engine_card(&engine, thread_id).await;

    assert!(
        crate::engine::agent_question::resolve_pending_question_as_canceled(
            &engine, thread_id, None,
        )
        .await
    );
    assert_eq!(
        answer_kind(&engine, thread_id, "a").await.as_deref(),
        Some("Canceled")
    );
    assert_eq!(
        answer_kind(&engine, thread_id, "b").await.as_deref(),
        Some("Canceled")
    );
    assert_eq!(answer_kind(&engine, thread_id, &engine_card).await, None);

    end_turn(&engine, thread_id).await;
    release(&engine, thread_id).await.unwrap();
    assert_eq!(
        card_rows(&engine, thread_id).await,
        vec![
            row("UserQuestionAsked", "a"),
            row("UserQuestionQueued", "b"),
            row("UserQuestionQueued", &engine_card),
            row("UserQuestionAsked", &engine_card),
        ]
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_follow_up_supersedes_queued_agent_cards() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    two_agent_cards(&engine, thread_id).await;

    assert!(
        crate::engine::agent_question::resolve_pending_question_as_superseded(
            &engine, thread_id, None,
        )
        .await
    );
    assert_eq!(
        answer_kind(&engine, thread_id, "a").await.as_deref(),
        Some("Superseded")
    );
    assert_eq!(
        answer_kind(&engine, thread_id, "b").await.as_deref(),
        Some("Superseded")
    );
    release(&engine, thread_id).await.unwrap();
    assert_eq!(
        card_rows(&engine, thread_id).await.len(),
        2,
        "nothing posts"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_engine_card_promoted_mid_turn_stays_open_after_the_turn_ends() {
    use crate::engine::question_park_sql::{
        open_card, open_card_exists, open_permission_card_exists, OpenCardKind,
    };
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    start_turn(&engine, thread_id).await;
    ask(&engine, thread_id, "a").await;
    let engine_card = raise_engine_card(&engine, thread_id).await;
    answer(&engine, thread_id, "a", typed("approve")).await;
    release(&engine, thread_id).await.unwrap();
    assert_eq!(
        card_rows(&engine, thread_id).await.last(),
        Some(&row("UserQuestionQueued", &engine_card)),
        "the engine card waits while the answered turn runs on"
    );
    end_turn(&engine, thread_id).await;
    release(&engine, thread_id).await.unwrap();

    let mut conn = engine.pool().acquire().await.unwrap();
    let open = open_card(&mut conn, thread_id).await.unwrap();
    assert_eq!(
        open.as_ref().map(|c| c.id.as_str()),
        Some(engine_card.as_str())
    );
    assert_eq!(open.map(|c| c.kind), Some(OpenCardKind::EngineRequest));
    assert!(open_card_exists(&mut conn, thread_id).await.unwrap());
    assert!(open_permission_card_exists(&mut conn, thread_id)
        .await
        .unwrap());
    drop(conn);
    assert_eq!(
        crate::engine::open_question::open_question(engine.pool(), thread_id)
            .await
            .map(|q| q.tool_use_id),
        Some(engine_card.clone())
    );

    ask(&engine, thread_id, "c").await;
    assert_eq!(
        card_rows(&engine, thread_id).await.last(),
        Some(&row("UserQuestionQueued", "c")),
        "a second card queues behind the open engine card"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_agent_card_closes_with_its_turn() {
    use crate::engine::question_park_sql::open_card;
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    ask(&engine, thread_id, "a").await;
    end_turn(&engine, thread_id).await;
    let mut conn = engine.pool().acquire().await.unwrap();
    assert_eq!(open_card(&mut conn, thread_id).await.unwrap(), None);
    teardown_test_db(&db).await;
}

async fn post_legacy_copy(engine: &LucidosEngine, thread_id: Uuid, tool_use_id: &str, copy: &str) {
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, thread_id, event_type, payload) \
         SELECT gen_random_uuid(), aggregate, aggregate_id, thread_id, event_type, \
                jsonb_set(payload, '{tool_use_id}', to_jsonb($3::text)) \
         FROM events WHERE aggregate_id = $1 AND event_type = 'UserQuestionAsked' \
           AND payload->>'tool_use_id' = $2",
    )
    .bind(thread_id.to_string())
    .bind(tool_use_id)
    .bind(copy)
    .execute(engine.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn the_owner_answers_legacy_open_cards_oldest_first() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    let first = raise_engine_card(&engine, thread_id).await;
    let second = format!("{first}-copy");
    post_legacy_copy(&engine, thread_id, &first, &second).await;
    end_turn(&engine, thread_id).await;

    let option_id = first_option_id(&engine, thread_id, &first).await;
    let early = answer_pending_question(
        &engine,
        thread_id,
        second.clone(),
        AnswerKind::Selected {
            option_id: option_id.clone(),
        },
        None,
    )
    .await;
    assert!(matches!(early, AnswerResult::Conflict(_)), "{early:?}");

    answer(
        &engine,
        thread_id,
        &first,
        AnswerKind::Selected {
            option_id: option_id.clone(),
        },
    )
    .await;
    answer(
        &engine,
        thread_id,
        &second,
        AnswerKind::Selected { option_id },
    )
    .await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_engine_card_raised_mid_turn_posts_when_the_turn_ends() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    start_turn(&engine, thread_id).await;
    let engine_card = raise_engine_card(&engine, thread_id).await;
    release(&engine, thread_id).await.unwrap();
    assert_eq!(
        card_rows(&engine, thread_id).await,
        vec![row("UserQuestionQueued", &engine_card)],
        "the card waits while the turn runs"
    );

    end_turn(&engine, thread_id).await;
    release(&engine, thread_id).await.unwrap();
    assert_eq!(
        card_rows(&engine, thread_id).await,
        vec![
            row("UserQuestionQueued", &engine_card),
            row("UserQuestionAsked", &engine_card),
        ]
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_card_a_turn_holds_for_posts_mid_turn() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    start_turn(&engine, thread_id).await;
    let card = raise_turn_holding_card(&engine, thread_id).await;
    assert_eq!(
        card_rows(&engine, thread_id).await,
        vec![row("UserQuestionAsked", &card)]
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_waiting_engine_card_does_not_hold_back_an_agent_card() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    start_turn(&engine, thread_id).await;
    ask(&engine, thread_id, "a").await;
    let engine_card = raise_engine_card(&engine, thread_id).await;
    ask(&engine, thread_id, "b").await;
    answer(&engine, thread_id, "a", AnswerKind::Canceled).await;
    release(&engine, thread_id).await.unwrap();
    assert_eq!(
        card_rows(&engine, thread_id).await.last(),
        Some(&row("UserQuestionAsked", "b"))
    );
    assert_eq!(answer_kind(&engine, thread_id, &engine_card).await, None);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_engine_card_answer_leaves_the_thread_idle() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace, thread_id) = boot_with_thread(&db).await;
    let first = raise_engine_card(&engine, thread_id).await;
    let second = raise_engine_card(&engine, thread_id).await;
    let option_id = first_option_id(&engine, thread_id, &first).await;
    answer(
        &engine,
        thread_id,
        &first,
        AnswerKind::Selected { option_id },
    )
    .await;
    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(engine.pool())
            .await
            .unwrap();
    assert_ne!(status, "running");
    release(&engine, thread_id).await.unwrap();
    assert_eq!(
        card_rows(&engine, thread_id).await.last(),
        Some(&row("UserQuestionAsked", &second))
    );
    teardown_test_db(&db).await;
}
