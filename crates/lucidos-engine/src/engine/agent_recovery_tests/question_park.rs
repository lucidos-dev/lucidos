//! Tests for the preserve predicate: `thread_has_unanswered_question` and the
//! `unanswered_question_exists_sql` fragment behind it.
//!
//! "Parked on a question" is what every restart-abort path keys on to decide
//! whether a thread is a resumable checkpoint (leave it alone, the card stays
//! answerable) or an interrupted turn (abort it, surface Continue). Getting the
//! boundary wrong is expensive in both directions, so each event class that can
//! land after a `UserQuestionAsked` gets a case here.
//!
//! Regression driving the progression half (2026-08-01): the teardown Esc'd a
//! session parked on an unanswered `AskUserQuestion`, so Claude Code recorded a
//! rejection the user never made as a `CodingAgentToolResult` and raced past the
//! question. The predicate was terminals-only, so it still reported the thread as
//! preserved. Recovery skipped it, no terminator ever landed, and the thread read
//! "Working" forever with a struck-through card the user could not answer. See
//! `docs/plans/2026-08-01-preserve-question-parked-session-through-teardown.md`.

use std::collections::HashSet;

use crate::engine::agent_question::aq_test_helpers::{emit_user_question, seed_cc_thread};
use crate::engine::agent_recovery::{
    preserve_question_park_at_shutdown, thread_has_unanswered_question,
    unanswered_question_exists_sql,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{AnswerKind, EventChannel, EventMeta, ThreadEvent};
use crate::runtime::CodingAgent;
use crate::test_support::{setup_test_db, teardown_test_db};
use uuid::Uuid;

/// Seed a question-parked coding-agent thread, emit `after` (if any), and return
/// whether the predicate still reports the thread as parked.
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
            tool_use_id: "toolu_park".into(),
            answer: AnswerKind::Selected {
                option_id: "opt-0".into(),
            },
        }))
        .await,
        "an answered question is not parked"
    );
}

/// The reproduction. Claude Code's canned message for a refused tool: the Esc at
/// teardown cancelled the pending `AskUserQuestion`, and by the time this lands
/// the card on screen is already struck through.
#[tokio::test]
async fn rejected_tool_result_ends_the_park() {
    assert!(
        !parked_after(Some(ThreadEvent::CodingAgentToolResult {
            name: String::new(),
            result: "The user doesn't want to proceed with this tool use. The tool use was \
                     rejected."
                .into(),
            coding_agent: CodingAgent::ClaudeCode,
            tool_use_id: String::new(),
        }))
        .await,
        "the agent raced past the question, so the thread is an interrupted turn, \
         not a resumable checkpoint"
    );
}

#[tokio::test]
async fn agent_text_after_the_question_ends_the_park() {
    assert!(
        !parked_after(Some(ThreadEvent::CodingAgentTextStreamed {
            text: "\n\n".into(),
            coding_agent: CodingAgent::ClaudeCode,
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
        }))
        .await,
        "a terminal ends the turn that owned the question"
    );
}

/// `docs/code-review-priors.md`: recovery re-emits the pending change as
/// `incomplete` BEFORE consulting the preserve guard, deliberately. That re-emit
/// must NOT read as the agent racing past the question, or the guard would
/// defeat itself on every parked thread that has a pending change.
#[tokio::test]
async fn pending_change_reemit_keeps_the_park() {
    assert!(
        parked_after(Some(ThreadEvent::ChangeProposed {
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
            path: String::new(),
            diff: String::new(),
        }))
        .await,
        "the incomplete re-emit must leave the card answerable"
    );
}

/// The park-ending list is the shared overtaken set plus exactly three extras.
/// Asserting the whole set (rather than spot-checking) is what catches a
/// hand-edit of the SQL: a name dropped from the `IN (...)` arm silently widens
/// the preserve guard, and a typo'd one silently never matches.
#[test]
fn park_ending_list_is_the_overtaken_set_plus_three_extras() {
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
    expected.insert("UserQuestionAnswered");
    expected.insert("ResponseGenerated");
    expected.insert("SessionEnded");
    assert_eq!(listed, expected);

    // The pre-2026-08-01 terminals-only list must remain a subset: widening the
    // guard must never DROP a terminal, or an aborted thread would read as
    // preserved and never be settled.
    for terminal in [
        "UserQuestionAnswered",
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

    // `ChangeProposed` is deliberately absent (see
    // `pending_change_reemit_keeps_the_park` and `docs/code-review-priors.md`).
    assert!(!listed.contains("ChangeProposed"));
}

/// `preserve_question_park_at_shutdown` is the one decision both teardown sites
/// make: `shutdown_agent_sessions` (skip the Esc) and `run_session`'s stop /
/// chat-cancel arms (skip the terminal and the text flush). Its two halves are
/// the cause gate and the shared predicate, and both are load-bearing.
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

    /// The cause gate. A user Stop / Apply / Discard / Archive with a question on
    /// screen is a deliberate end-of-turn, and those paths cancel-stamp the card
    /// themselves. Dropping the gate would silently swallow their terminal.
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

    /// A session mid-work at teardown is an interrupted turn: it keeps today's
    /// graceful Esc and its boundary terminal, so the user gets Continue.
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

    /// The reproduction, from the teardown's point of view. Once the agent has
    /// raced past the question there is nothing left to preserve, so the teardown
    /// must fall through to the normal abort path rather than leave the turn with
    /// no terminator at all.
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

/// Wiring tripwire. There is no test harness that can construct a
/// `LucidosEngine`, so the two teardown call sites cannot be driven end to end
/// from a unit test. Pin them by source instead: dropping either call silently
/// restores the 2026-08-01 bug (an Esc'd question, or a terminal / text flush
/// landing after it), and every behavioural test above would still pass because
/// they exercise the guard directly.
#[test]
fn both_teardown_sites_consult_the_preserve_guard() {
    // Each needle matches the CALL, not the bare name: both files also mention
    // the guard in prose (a doc link, and the comment block explaining why the
    // Esc is skipped). A substring check on the name alone would stay green if
    // someone deleted the call and left the comment that explains it, which is
    // the likeliest accidental regression of the two.
    for (label, source, needles) in [
        (
            "shutdown_agent_sessions (skips Claude Code's Esc)",
            include_str!("../engine_impl/shutdown.rs"),
            &["agent_recovery::preserve_question_park_at_shutdown("][..],
        ),
        (
            "emit_stop_terminal (skips the terminal and the text flush)",
            include_str!("../agent_session/runtime_helpers.rs"),
            // The second needle pins the cause gate's widening. `is_shutdown`
            // alone is the PER-SESSION flag, so dropping the OR silently
            // un-guards the very race the guard's doc names: a session inserted
            // after `shutdown_agent_sessions` took its flag pass. Every
            // behavioural test would still pass, because they drive the guard
            // directly rather than through this call site.
            &[
                "agent_recovery::preserve_question_park_at_shutdown(",
                "is_shutdown || self.is_shutting_down()",
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
}
