//! A tool result of a call issued before a question does not unpark the
//! thread: the agent is still blocked on the question (ADR 0583). Only the
//! question's own result may move the status off `waiting_for_user_answer`.

use super::super::*;
use super::*;
use crate::runtime::CodingAgent;

const QUESTION_ID: &str = "codex-q-1#q0";

fn codex_meta() -> EventMeta {
    EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    }
}

async fn emit_tool_called(bus: &EventBus, thread_id: Uuid, tool_use_id: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentToolCalled {
            name: "command_execution".into(),
            args: serde_json::json!({"command": "./scripts/check.sh"}),
            description: String::new(),
            coding_agent: CodingAgent::Codex,
            tool_use_id: tool_use_id.into(),
            parent_tool_use_id: None,
            api_call_id: None,
        },
        meta: codex_meta(),
    })
    .await
    .unwrap();
}

async fn emit_tool_result(bus: &EventBus, thread_id: Uuid, tool_use_id: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentToolResult {
            name: "command_execution".into(),
            result: "ok".into(),
            coding_agent: CodingAgent::Codex,
            tool_use_id: tool_use_id.into(),
            parent_tool_use_id: None,
        },
        meta: codex_meta(),
    })
    .await
    .unwrap();
}

async fn emit_question(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: QUESTION_ID.into(),
            cc_session_id: String::new(),
            question: "Keep open in place, or revise the plan?".into(),
            options: vec![],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
        },
        meta: codex_meta(),
    })
    .await
    .unwrap();
}

async fn emit_permission_request(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "perm-1".into(),
            tool_use_id: "exec-gated".into(),
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": "rm -rf build"}),
            summary: "rm -rf build".into(),
            grant_offer: None,
        },
        meta: codex_meta(),
    })
    .await
    .unwrap();
}

async fn read_status(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The observed stuck state: a command started, the agent asked, then the
/// command finished. Both hang watchdogs spare a park only when the status
/// reads it, so `running` here also let them kill the waiting session.
#[tokio::test]
async fn a_late_result_of_an_earlier_call_keeps_the_thread_waiting() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit_cc_message_received(&bus, thread_id, None, "open new tabs in place").await;
    emit_tool_called(&bus, thread_id, "exec-earlier").await;
    emit_question(&bus, thread_id).await;
    emit_tool_result(&bus, thread_id, "exec-earlier").await;

    assert_eq!(
        read_status(&pool, thread_id).await,
        "waiting_for_user_answer",
        "the agent is still blocked on the question, so the late result of a \
         call it issued earlier must not show a Stop button over the card"
    );
    assert!(
        !crate::engine::claude_code::thread_is_running(&pool, thread_id)
            .await
            .unwrap(),
        "the hang watchdogs' past-ceiling probe must read the park as not running"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_late_result_keeps_a_permission_park() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit_cc_message_received(&bus, thread_id, None, "clean the build").await;
    emit_tool_called(&bus, thread_id, "exec-earlier").await;
    emit_permission_request(&bus, thread_id).await;
    emit_tool_result(&bus, thread_id, "exec-earlier").await;

    assert_eq!(
        read_status(&pool, thread_id).await,
        "waiting_for_user_answer"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An earlier question's own result cannot end a later permission park: only
/// the event that set the park decides what ends it.
#[tokio::test]
async fn an_earlier_questions_result_keeps_a_later_permission_park() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit_cc_message_received(&bus, thread_id, None, "clean the build").await;
    emit_question(&bus, thread_id).await;
    emit_permission_request(&bus, thread_id).await;
    emit_tool_result(&bus, thread_id, QUESTION_ID).await;

    assert_eq!(
        read_status(&pool, thread_id).await,
        "waiting_for_user_answer",
        "the permission card is still unresolved"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Claude Code records the rejection of an Esc'd `AskUserQuestion` as the
/// question's own result, which ends the park (ADR 0583).
#[tokio::test]
async fn the_questions_own_result_ends_the_park() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit_cc_message_received(&bus, thread_id, None, "open new tabs in place").await;
    emit_question(&bus, thread_id).await;
    emit_tool_result(&bus, thread_id, QUESTION_ID).await;

    assert_eq!(read_status(&pool, thread_id).await, "running");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_answer_then_a_result_runs_again() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit_cc_message_received(&bus, thread_id, None, "open new tabs in place").await;
    emit_question(&bus, thread_id).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: QUESTION_ID.into(),
            answer: crate::engine::thread_events::AnswerKind::Selected {
                option_id: "opt-0".into(),
            },
            quiet: false,
        },
        meta: codex_meta(),
    })
    .await
    .unwrap();
    emit_tool_result(&bus, thread_id, "exec-later").await;

    assert_eq!(read_status(&pool, thread_id).await, "running");

    pool.close().await;
    teardown_test_db(&db_name).await;
}
