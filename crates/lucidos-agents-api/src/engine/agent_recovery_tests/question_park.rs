use std::collections::HashSet;

use crate::engine::agent_question::aq_test_helpers::{emit_user_question, seed_cc_thread};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::open_question::open_question;
use crate::engine::question_park_sql::{
    preserve_question_park_at_shutdown, thread_has_unanswered_question,
    unanswered_question_exists_sql,
};
use crate::engine::thread_events::{AnswerKind, EventChannel, EventMeta, ThreadEvent};
use crate::runtime::CodingAgent;
use crate::test_support::{setup_test_db, teardown_test_db};
use uuid::Uuid;

async fn parked_after(after: Option<ThreadEvent>) -> bool {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, "toolu_park").await;

    if let Some(event) = after {
        let label = event.event_type();
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap_or_else(|e| panic!("{label} emit: {e}"))
        .unwrap_or_else(|| panic!("{label} must persist to be visible to the predicate"));
    }

    let parked = thread_has_unanswered_question(&pool, thread_id).await;

    pool.close().await;
    teardown_test_db(&db_name).await;
    parked
}

#[tokio::test]
async fn bare_unanswered_question_is_parked() {
    assert!(
        parked_after(None).await,
        "a question with nothing after it is the canonical parked state"
    );
}

#[tokio::test]
async fn answer_ends_the_park() {
    assert!(
        !parked_after(Some(ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: "toolu_park".into(),
            quiet: false,
            answer: AnswerKind::Selected {
                option_id: "opt-0".into(),
            },
        }))
        .await,
        "an answered question is not parked"
    );
}

#[tokio::test]
async fn rejected_tool_result_ends_the_park() {
    assert!(
        !parked_after(Some(ThreadEvent::CodingAgentToolResult {
            name: String::new(),
            result: "The user doesn't want to proceed with this tool use. The tool use was \
                     rejected."
                .into(),
            coding_agent: CodingAgent::ClaudeCode,
            tool_use_id: "toolu_park".into(),
            parent_tool_use_id: None,
        }))
        .await,
        "the agent raced past the question, so the thread is an interrupted turn, \
         not a resumable checkpoint"
    );
}

#[tokio::test]
async fn a_late_result_of_an_earlier_call_keeps_the_park() {
    assert!(
        parked_after(Some(ThreadEvent::CodingAgentToolResult {
            name: "Bash".into(),
            result: "done".into(),
            coding_agent: CodingAgent::ClaudeCode,
            tool_use_id: "toolu_issued_before_the_question".into(),
            parent_tool_use_id: None,
        }))
        .await,
        "the agent is still blocked on the question"
    );
}

#[tokio::test]
async fn agent_text_after_the_question_ends_the_park() {
    assert!(
        !parked_after(Some(ThreadEvent::CodingAgentTextStreamed {
            text: "\n\n".into(),
            coding_agent: CodingAgent::ClaudeCode,
            parent_tool_use_id: None,
            progress_note: false,
        }))
        .await,
        "the agent kept talking past the question, so the card is dead"
    );
}

#[tokio::test]
async fn terminal_ends_the_park() {
    assert!(
        !parked_after(Some(ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        }))
        .await,
        "a terminal ends the turn that owned the question"
    );
}

#[tokio::test]
async fn pending_change_reemit_keeps_the_park() {
    assert!(
        parked_after(Some(ThreadEvent::ChangeProposed {
            content: Default::default(),
            change_id: Uuid::new_v4().to_string(),
            description: Some("wip".into()),
            files: vec!["a.txt".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: "claude-code/test".into(),
            repo_root: "/tmp/repo".into(),
            hardened: false,
            incomplete: true,
            set_aside: false,
            path: String::new(),
            diff: String::new(),
        }))
        .await,
        "the incomplete re-emit must leave the card answerable"
    );
}

#[test]
fn park_ends_on_a_moved_on_turn_or_the_cards_own_answer() {
    let sql = unanswered_question_exists_sql("$1");
    let listed: HashSet<&str> = sql
        .split("later.event_type IN (")
        .nth(1)
        .expect("fragment contains the park-ending IN list")
        .split(')')
        .next()
        .expect("IN list is closed")
        .split(',')
        .map(|t| t.trim().trim_matches('\''))
        .collect();

    let mut expected: HashSet<&str> = ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES
        .iter()
        .copied()
        .collect();
    expected.insert("ResponseGenerated");
    expected.insert("SessionEnded");
    assert_eq!(listed, expected);
    assert!(
        sql.contains("later.payload->>'tool_use_id' = uqa.payload->>'tool_use_id'"),
        "an answer closes only its own card"
    );

    for terminal in [
        "ResponseAborted",
        "CodingAgentIdled",
        "ResponseGenerated",
        "SessionEnded",
    ] {
        assert!(
            listed.contains(terminal),
            "{terminal} must still end the park"
        );
    }

    assert!(!listed.contains("ChangeProposed"));
}

mod preserve_at_shutdown {
    use super::*;

    async fn with_parked_thread<F, Fut, T>(body: F) -> T
    where
        F: FnOnce(sqlx::PgPool, Uuid) -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        emit_user_question(&bus, thread_id, "toolu_park").await;

        let out = body(pool.clone(), thread_id).await;

        pool.close().await;
        teardown_test_db(&db_name).await;
        out
    }

    #[tokio::test]
    async fn preserves_a_parked_session_at_teardown() {
        let preserved = with_parked_thread(|pool, tid| async move {
            preserve_question_park_at_shutdown(&pool, "test", tid, true).await
        })
        .await;
        assert!(
            preserved,
            "the teardown must leave a question-parked session untouched: no Esc, \
             no terminal, no text flush"
        );
    }

    #[tokio::test]
    async fn does_not_preserve_a_user_stop() {
        let preserved = with_parked_thread(|pool, tid| async move {
            preserve_question_park_at_shutdown(&pool, "test", tid, false).await
        })
        .await;
        assert!(
            !preserved,
            "a user-initiated stop still ends the turn, question or not"
        );
    }

    #[tokio::test]
    async fn does_not_preserve_a_working_session() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;

        assert!(
            !preserve_question_park_at_shutdown(&pool, "test", thread_id, true).await,
            "a session with no pending question is a normal interrupted turn"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn does_not_preserve_an_overtaken_question() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        emit_user_question(&bus, thread_id, "toolu_park").await;
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentTextStreamed {
                text: "moving on".into(),
                coding_agent: CodingAgent::ClaudeCode,
                parent_tool_use_id: None,
                progress_note: false,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .expect("CodingAgentTextStreamed emit")
        .expect("CodingAgentTextStreamed persisted");

        assert!(
            !preserve_question_park_at_shutdown(&pool, "test", thread_id, true).await,
            "an overtaken question is a dead card on an interrupted turn; preserving \
             it would strand the thread with no terminator"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

#[test]
fn both_teardown_sites_consult_the_preserve_guard() {
    for (label, source, needles) in [
        (
            "shutdown_agent_sessions (skips Claude Code's Esc)",
            include_str!("../../../../lucidos-engine/src/engine/engine_impl/shutdown.rs"),
            &["question_park_sql::preserve_question_park_at_shutdown("][..],
        ),
        (
            "emit_stop_terminal (skips the terminal and the text flush)",
            include_str!("../agent_session/runtime_helpers.rs"),
            &[
                "question_park_sql::preserve_question_park_at_shutdown(",
                "let is_shutdown = self.session_is_shutting_down(is_shutdown);",
            ][..],
        ),
    ] {
        for needle in needles {
            assert!(
                source.contains(needle),
                "{label} must CALL the shared preserve guard, not just mention it \
                 (missing: {needle})"
            );
        }
    }

    let source = include_str!("../agent_session/runtime_helpers.rs");
    let widen_at = source
        .find("let is_shutdown = self.session_is_shutting_down(is_shutdown);")
        .expect("asserted above");
    let guard_at = source
        .find("question_park_sql::preserve_question_park_at_shutdown(")
        .expect("asserted above");
    assert!(
        widen_at < guard_at,
        "emit_stop_terminal must widen `is_shutdown` BEFORE the preserve guard \
         reads it, or the guard runs on the per-session flag alone and a session \
         registered after the teardown snapshot loses its question park"
    );
    let guard_call = &source[guard_at..(guard_at + 240).min(source.len())];
    assert!(
        guard_call.contains("is_shutdown,"),
        "the preserve guard must be passed the widened `is_shutdown`; its \
         argument list reads:\n{guard_call}"
    );
}

#[tokio::test]
async fn the_open_question_reads_back_with_its_choices() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, "toolu_park").await;

    let open = open_question(&pool, thread_id)
        .await
        .expect("the thread is parked, so it has an open question");
    assert_eq!(open.question, "Pick one");
    assert_eq!(open.options.len(), 1);
    assert_eq!(open.options[0].label, "A");
    assert!(!open.multi_select);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_answered_question_reads_back_as_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, "toolu_park").await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: "toolu_park".into(),
            quiet: false,
            answer: AnswerKind::Canceled,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("UserQuestionAnswered emit")
    .expect("UserQuestionAnswered persisted");

    assert_eq!(open_question(&pool, thread_id).await, None);
    assert!(!thread_has_unanswered_question(&pool, thread_id).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_thread_with_no_question_reads_back_as_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;

    assert_eq!(open_question(&pool, thread_id).await, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
