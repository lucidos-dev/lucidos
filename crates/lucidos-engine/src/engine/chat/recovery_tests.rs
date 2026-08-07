//! Tests for the orphan-thread / orphan-toolcall recovery sweeps.
//!
//! Covers the user-spotted regression where a thread the user archived got
//! reanimated to inbox days later by an engine restart: the sweep emitted
//! `ResponseAborted { cause: RecoveryAfterRestart }` against the archived
//! row, and `thread_lifecycle::resolve_transition` routes every
//! `ResponseAborted` `to_inbox` regardless of cause. The fix is at the
//! source: the sweep's enumeration query filters out archived threads, so
//! the recovery event never gets emitted in the first place.
//!
//! The tests run the same `ORPHAN_THREADS_SQL` / `ORPHAN_TOOL_CALLS_SQL`
//! `const` strings the production code uses, so a future change to either
//! query that drops the `archive_state != 'archived'` filter fails this
//! suite immediately.

use super::{orphan_threads_sql, orphan_tool_calls_sql, ORPHAN_THREADS_SQL, ORPHAN_TOOL_CALLS_SQL};
use crate::core::EventRow;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, EventMeta, QuestionOption, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

/// Emit an unanswered `UserQuestionAsked` — the parked-on-a-question state the
/// shared preserve guard (`unanswered_question_exists_sql`) must exclude from
/// every restart-abort sweep. Chat channel (`EventMeta::NONE`).
async fn emit_unanswered_question(bus: &EventBus, thread_id: Uuid, tool_use_id: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.into(),
            cc_session_id: String::new(),
            question: "Pick one".into(),
            options: vec![QuestionOption {
                id: "opt-0".into(),
                label: "A".into(),
                description: None,
            }],
            worktree_path: None,
            multi_select: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// `ORPHAN_THREADS_SQL` uses a strict `last_activity > last_start` predicate
/// keyed off the `created` timestamp (PG `now()`, microsecond resolution).
/// Two `bus.emit()` calls without any gap can land in the same microsecond
/// on fast hardware / under CI load, collapsing the predicate to `T > T =
/// FALSE` and silently dropping the orphan from the result set. Production
/// streaming has tens of ms gaps; tests have zero gap unless we add one
/// here. A single millisecond is plenty — the per-event persist work
/// already takes much longer than that, this just guarantees the timestamps
/// are distinct.
async fn tick() {
    tokio::time::sleep(Duration::from_millis(1)).await;
}

/// Emit a "user typed + assistant streamed partial text" pair, leaving the
/// turn intentionally orphaned (no `ResponseGenerated`). Mirrors the shape
/// the recovery sweep targets: activity events after the last start with
/// no terminal in between.
async fn emit_orphan_turn(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "stuck request".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TextStreamed {
            text: "partial reply".into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// Emit a `MessageReceived` followed by an unmatched `ToolCalled`. The
/// orphan-tool-call sweep walks the (`ToolCalled`, `ToolResult`) pairs and
/// emits a synthetic `ToolResult` for any `ToolCalled` without a partner.
async fn emit_orphan_tool_call(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "trigger a tool".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolCalled {
            name: "Read".into(),
            args: json!({"path": "/tmp/x"}),
            description: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn archive(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// SQL row shape `ORPHAN_THREADS_SQL` returns. Mirrors the tuple production
/// uses (kept private to the test module because it's only meaningful here).
type OrphanThreadRow = (
    Uuid,
    Option<String>,
    Option<Uuid>,
    Option<String>,
    Option<String>,
);

#[tokio::test]
async fn orphan_threads_query_includes_unarchived_orphan() {
    // Precondition assertion that anchors the negative test below: the
    // exact same query DOES return a row for an orphan turn on a regular
    // inbox thread. Without this, the negative case ("archived → no row")
    // could be passing for the wrong reason (e.g. a typo in the orphan
    // fixture making no thread ever match).
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    emit_orphan_turn(&bus, thread_id).await;

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(ORPHAN_THREADS_SQL)
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    let hit = rows.iter().find(|(tid, _, _, _, _)| *tid == thread_id);
    assert!(
        hit.is_some(),
        "unarchived orphan must be visible to the recovery sweep — got {:?}",
        rows.iter().map(|r| r.0).collect::<Vec<_>>()
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_threads_query_skips_archived_thread() {
    // The user's scenario: a thread had an in-flight turn that never
    // settled, the user archived the row, and a later engine restart's
    // recovery sweep flipped it back to inbox via the contract layer's
    // `ResponseAborted → to_inbox` rule. Fix: filter the sweep's
    // candidate set on `archive_state != 'archived'` so the recovery
    // event never gets emitted.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let archived_id = Uuid::new_v4();
    emit_orphan_turn(&bus, archived_id).await;
    archive(&bus, archived_id).await;

    // Sanity: the projection records the archive on `archive_state` (sole
    // archive flag post-collapse).
    let archive_state: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(archived_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(archive_state, "archived");

    // Add a control row: an unarchived thread with the same orphan shape.
    // The query MUST return it but NOT the archived one.
    let control_id = Uuid::new_v4();
    emit_orphan_turn(&bus, control_id).await;

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(ORPHAN_THREADS_SQL)
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    let returned: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    assert!(
        returned.contains(&control_id),
        "control (unarchived) orphan must be returned — got {:?}",
        returned
    );
    assert!(
        !returned.contains(&archived_id),
        "archived orphan must NOT be returned — got {:?}",
        returned
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_threads_query_excludes_threads_owned_by_cc_recovery() {
    // `recover_orphaned_worktrees` runs first and returns the set of
    // coding-agent threads it is resuming/settling; `main.rs` passes that set as
    // `exclude_thread_ids` so the chat orphan sweep does NOT emit a duplicate
    // `ResponseAborted` on a thread CC recovery already owns (which — combined
    // with the startup-lease serialization — is what stops the chat sweep from
    // aborting an in-flight CC thread out from under its auto-resume). This pins
    // the `exclude_thread_ids` bind: an excluded orphan is filtered out, an
    // otherwise-identical control orphan is still returned.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let cc_owned_id = Uuid::new_v4();
    emit_orphan_turn(&bus, cc_owned_id).await;

    let control_id = Uuid::new_v4();
    emit_orphan_turn(&bus, control_id).await;

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(ORPHAN_THREADS_SQL)
        .bind(vec![cc_owned_id])
        .fetch_all(&pool)
        .await
        .unwrap();
    let returned: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    assert!(
        returned.contains(&control_id),
        "a non-excluded orphan must still be returned — got {:?}",
        returned
    );
    assert!(
        !returned.contains(&cc_owned_id),
        "an orphan in exclude_thread_ids (owned by CC recovery) must NOT be returned — got {:?}",
        returned
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_threads_query_skips_question_parked_thread() {
    // The reproduced bug: a thread parked on an unanswered AskUserQuestion was
    // swept into a `ResponseAborted` ("System — Response interrupted") on
    // restart. For a coding-agent thread this fired even after
    // `recover_orphaned_worktrees` deliberately preserved it (it is NOT added to
    // `exclude_thread_ids`); for a chat thread this sweep is the only abort path
    // it hits. The shared preserve guard (`unanswered_question_exists_sql`) baked
    // into `orphan_threads_sql()` must drop it — while a control orphan with no
    // pending question is still returned.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    // Parked-on-a-question orphan: MessageReceived + TextStreamed (activity, so
    // it qualifies as an orphan candidate) + an unanswered UserQuestionAsked.
    let parked_id = Uuid::new_v4();
    emit_orphan_turn(&bus, parked_id).await;
    tick().await;
    emit_unanswered_question(&bus, parked_id, "toolu_parked").await;

    // Control: an ordinary orphan (no question) — must still be recovered.
    let control_id = Uuid::new_v4();
    emit_orphan_turn(&bus, control_id).await;

    // Precondition: the UNGUARDED base query DOES return the parked thread, so
    // this test can only pass because the guard in `orphan_threads_sql()` drops
    // it (not because the fixture failed to match).
    let base_rows: Vec<OrphanThreadRow> = sqlx::query_as(ORPHAN_THREADS_SQL)
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(
        base_rows.iter().any(|(tid, _, _, _, _)| *tid == parked_id),
        "precondition: unguarded base query must see the parked orphan"
    );

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(&orphan_threads_sql())
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    let returned: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    assert!(
        returned.contains(&control_id),
        "a non-question orphan must still be recovered — got {:?}",
        returned
    );
    assert!(
        !returned.contains(&parked_id),
        "a thread parked on an unanswered question must NOT be swept — got {:?}",
        returned
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_threads_query_recovers_answered_then_interrupted_thread() {
    // The guard must be narrow: a thread whose question was ANSWERED and then
    // interrupted mid-continuation (no terminal) is a genuine orphan and must
    // still be recovered — otherwise answering-then-crashing would strand it.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let answered_id = Uuid::new_v4();
    emit_orphan_turn(&bus, answered_id).await;
    tick().await;
    emit_unanswered_question(&bus, answered_id, "toolu_answered").await;
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id: answered_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: "toolu_answered".into(),
            answer: crate::engine::thread_events::AnswerKind::Selected {
                option_id: "opt-0".into(),
            },
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    tick().await;
    // More activity after the answer, still no terminal → a real orphan.
    bus.emit(BusEvent::Thread {
        thread_id: answered_id,
        event: ThreadEvent::TextStreamed {
            text: "resumed work".into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(&orphan_threads_sql())
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(
        rows.iter().any(|(tid, _, _, _, _)| *tid == answered_id),
        "an answered-then-interrupted thread is a genuine orphan and must still be recovered"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_tool_calls_query_skips_question_parked_thread() {
    // A question-parked chat thread has a dangling `ToolCalled{ask_user_question}`
    // (the loop emits it before blocking in `walk_question_batch`). The guard in
    // `orphan_tool_calls_sql()` must skip the thread so the sweep never
    // synthesizes a "[Tool execution interrupted…]" ToolResult that would poison
    // the pending question's tool-use pair.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parked_id = Uuid::new_v4();
    emit_orphan_tool_call(&bus, parked_id).await; // MessageReceived + unmatched ToolCalled
    tick().await;
    emit_unanswered_question(&bus, parked_id, "toolu_parked_tc").await;

    let control_id = Uuid::new_v4();
    emit_orphan_tool_call(&bus, control_id).await;

    let rows: Vec<EventRow> = sqlx::query_as(&orphan_tool_calls_sql())
        .fetch_all(&pool)
        .await
        .unwrap();
    let returned: std::collections::HashSet<Uuid> =
        rows.iter().filter_map(|r| r.thread_id).collect();
    assert!(
        returned.contains(&control_id),
        "a non-question thread's orphan ToolCalled must still be paired — got {:?}",
        returned
    );
    assert!(
        !returned.contains(&parked_id),
        "a question-parked thread's ToolCalled must NOT be swept — got {:?}",
        returned
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_tool_calls_query_skips_archived_thread() {
    // Mirror the orphan-thread test for the inner-tool-layer sweep: a
    // synthetic `ToolResult` for an archived thread would bump
    // `last_activity` (via the projection's ToolResult branch) and surface
    // the row in any activity-sorted list. Filter at the SQL JOIN.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let archived_id = Uuid::new_v4();
    emit_orphan_tool_call(&bus, archived_id).await;
    archive(&bus, archived_id).await;

    let control_id = Uuid::new_v4();
    emit_orphan_tool_call(&bus, control_id).await;

    let rows: Vec<EventRow> = sqlx::query_as(ORPHAN_TOOL_CALLS_SQL)
        .fetch_all(&pool)
        .await
        .unwrap();
    let returned_threads: std::collections::HashSet<Uuid> =
        rows.iter().filter_map(|r| r.thread_id).collect();
    assert!(
        returned_threads.contains(&control_id),
        "control thread's ToolCalled must be returned — got {:?}",
        returned_threads
    );
    assert!(
        !returned_threads.contains(&archived_id),
        "archived thread's ToolCalled must NOT be returned — got {:?}",
        returned_threads
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// -- Chat auto-resume after a user-initiated switch ---------------------------
//
// `switch_resume_candidates` is the chat half of the switch auto-resume (the
// coding-agent half is `recover_orphaned_worktrees` → `enqueue_switch_resume`).
// These tests pin the selection contract directly against the production SQL:
// what counts as a switch, what must stay on the manual Continue affordance,
// and the loop-breaker that stops a resume re-resuming itself forever.
mod switch_resume {
    use super::tick;
    use crate::engine::chat::recovery::switch_resume_candidates;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{
        AbortCause, ActorMode, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
    };
    use crate::test_support::{setup_test_db, teardown_test_db};
    use uuid::Uuid;

    fn device_actor() -> MessageOrigin {
        MessageOrigin::Device {
            device_id: "dev-1".into(),
            label: "My MacBook".into(),
        }
    }

    /// A user turn on `channel`, which is also what creates the
    /// `thread_summaries` row (and its `source`) via the projection.
    async fn seed_turn(bus: &EventBus, thread_id: Uuid, channel: Option<EventChannel>) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                text: "do the thing".into(),
                user_image_hashes: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel,
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    /// The teardown boundary `abort_in_flight_for_restart` emits for a chat
    /// thread on a user switch: `EngineShutdown` + the clicking device's actor.
    async fn emit_abort(
        bus: &EventBus,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
        cause: AbortCause,
    ) {
        crate::engine::thread_events::emit_response_aborted(
            bus,
            thread_id,
            cause,
            "interrupted".into(),
            vec![],
            None,
            None,
            EventMeta {
                actor,
                ..EventMeta::NONE
            },
            "[test] teardown abort",
        )
        .await;
    }

    async fn candidates_contain(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
        switch_resume_candidates(pool).await.contains(&thread_id)
    }

    #[tokio::test]
    async fn device_shutdown_abort_makes_a_chat_thread_a_resume_candidate() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_turn(&bus, thread_id, None).await;
        tick().await;
        emit_abort(
            &bus,
            thread_id,
            Some(device_actor()),
            AbortCause::EngineShutdown,
        )
        .await;

        assert!(
            candidates_contain(&pool, thread_id).await,
            "a chat thread interrupted by a user Switch to new version must auto-resume, \
             not wait for a manual Continue click"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Crash-safety: only the device-attributed `EngineShutdown` teardown counts.
    /// A crash emits nothing (or a `System` abort from the recovery sweep), and a
    /// `StaleSettle` abort carries a device actor from a user Stop/Archive button
    /// — none of those may re-run work the user did not ask to resume.
    #[tokio::test]
    async fn crash_and_user_stop_shaped_aborts_are_not_candidates() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        // System actor (crash recovery sweep).
        let sys = Uuid::new_v4();
        seed_turn(&bus, sys, None).await;
        tick().await;
        emit_abort(
            &bus,
            sys,
            Some(MessageOrigin::system()),
            AbortCause::EngineShutdown,
        )
        .await;

        // Device actor but a user-button cause, not a teardown.
        let settle = Uuid::new_v4();
        seed_turn(&bus, settle, None).await;
        tick().await;
        emit_abort(&bus, settle, Some(device_actor()), AbortCause::StaleSettle).await;

        // No abort at all — the shape of a question-parked thread, which the
        // preserve guard keeps abort-free. It must never be auto-resumed;
        // answering is what resumes it.
        let parked = Uuid::new_v4();
        seed_turn(&bus, parked, None).await;

        let found = switch_resume_candidates(&pool).await;
        assert!(
            !found.contains(&sys),
            "a system-attributed abort is the crash path → manual Continue"
        );
        assert!(
            !found.contains(&settle),
            "a StaleSettle abort carries the actor of a user Stop/Archive button, \
             not of a switch — resuming would re-run work the user abandoned"
        );
        assert!(
            !found.contains(&parked),
            "a thread with no abort (question-parked) is not an interrupted turn"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Loop-breaker: `continue_chat` emits `ContinuationStarted`, which is in the
    /// shared start set — so a resume that itself dies before producing anything
    /// else falls back to the manual Continue instead of resuming forever.
    #[tokio::test]
    async fn an_already_resumed_thread_is_not_a_candidate_again() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_turn(&bus, thread_id, None).await;
        tick().await;
        emit_abort(
            &bus,
            thread_id,
            Some(device_actor()),
            AbortCause::EngineShutdown,
        )
        .await;
        assert!(candidates_contain(&pool, thread_id).await);

        tick().await;
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinuationStarted {
                branch: String::new(),
                origin: None,
                reason: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();

        assert!(
            !candidates_contain(&pool, thread_id).await,
            "the switch abort has been consumed by a resume — a second boot must not re-resume it"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Coding-agent threads are owned by `recover_orphaned_worktrees`; picking one
    /// up here would resume it twice, and with the wrong (chat) machinery.
    #[tokio::test]
    async fn coding_agent_threads_are_left_to_the_worktree_recovery_pass() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_turn(&bus, thread_id, Some(EventChannel::ClaudeCode)).await;
        tick().await;
        emit_abort(
            &bus,
            thread_id,
            Some(device_actor()),
            AbortCause::EngineShutdown,
        )
        .await;

        let source: Option<String> =
            sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert_eq!(
            source.as_deref(),
            Some("claude_code"),
            "fixture precondition: the thread must project as a coding-agent thread"
        );

        assert!(
            !candidates_contain(&pool, thread_id).await,
            "the chat pass must not claim a coding-agent thread"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Same rationale as the orphan sweep's archived filter: the user closed the
    /// thread, and resuming it would revive the row they deliberately dismissed.
    #[tokio::test]
    async fn archived_threads_are_not_candidates() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_turn(&bus, thread_id, None).await;
        tick().await;
        emit_abort(
            &bus,
            thread_id,
            Some(device_actor()),
            AbortCause::EngineShutdown,
        )
        .await;
        assert!(candidates_contain(&pool, thread_id).await);

        super::archive(&bus, thread_id).await;

        assert!(
            !candidates_contain(&pool, thread_id).await,
            "an archived thread must not be revived by the auto-resume"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

// ── Event-wait park (Phase 4 guard, I4 + I5) ────────────────────────
//
// The same hazard as the question park, reached by a different route. A thread
// parked on `await_event` has NO terminator by design and a deliberately
// dangling `ToolCalled{await_event}`, so both sweeps would treat it as a
// crashed turn: one would emit "Response interrupted", the other would fill the
// rendezvous slot with "[Tool execution interrupted…]" and the woken model
// would read that instead of its event.

/// Park a chat thread on an event wait: the `await_event` call, then the
/// The real registration shape: an `await_event` `ToolCalled`, its
/// `EventWaitStarted`, and the `ToolResult` that pairs the call. All three, in
/// that order, because the pairing is the point: since 2026-08-06 `await_event`
/// closes its own call, so a subscribed thread leaves no orphan behind and
/// needs no guard in either sweep.
async fn emit_event_wait_subscription(bus: &EventBus, thread_id: Uuid, wait_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolCalled {
            name: "await_event".into(),
            args: json!({ "reason": "waiting for a change" }),
            description: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitStarted {
            wait_id,
            tool_use_id: format!("toolu_{}", wait_id.simple()),
            on: vec![crate::core::event_subscription::EventSubscription {
                event_type: "ChangeProposed".into(),
                condition: None,
            }],
            reason: "waiting for a change".into(),
            armed_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            watermark: 0,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap()
    .expect("EventWaitStarted must persist");
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolResult {
            name: "await_event".into(),
            result: "Subscribed to ChangeProposed. Nothing is blocking.".into(),
            images: vec![],
            success: true,
            tool_called_event_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// **A subscription is not a park, so it gets no exemption.** A thread whose
/// turn genuinely died mid-flight is an orphan whatever subscriptions it
/// happens to hold, and the sweep must settle it: withholding the abort would
/// leave it reading "Working" forever with no Continue button.
///
/// Both preserve guards this sweep carries for the event wait were deleted with
/// the attached shape (2026-08-06). This is their replacement, asserting the
/// opposite.
#[tokio::test]
async fn orphan_threads_query_recovers_a_thread_that_holds_a_subscription() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let subscribed_id = Uuid::new_v4();
    emit_orphan_turn(&bus, subscribed_id).await;
    tick().await;
    emit_event_wait_subscription(&bus, subscribed_id, Uuid::new_v4()).await;

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(&orphan_threads_sql())
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(
        rows.iter().any(|r| r.0 == subscribed_id),
        "a crashed turn is an orphan whether or not the thread is watching for \
         something; only an unanswered question is preserved"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The tool-call sweep needs no exemption either, and for a stronger reason
/// than "the guard was dropped": `await_event` pairs its own call, so it never
/// leaves an orphan for the sweep to find in the first place.
#[tokio::test]
async fn an_await_event_call_is_never_an_orphan_tool_call() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let subscribed_id = Uuid::new_v4();
    emit_orphan_turn(&bus, subscribed_id).await;
    tick().await;
    emit_event_wait_subscription(&bus, subscribed_id, Uuid::new_v4()).await;

    // The query fetches every `ToolCalled` / `ToolResult` on the thread; the
    // pairing that decides what is an orphan happens in Rust afterwards, so
    // assert on that, through the same helper the sweep uses.
    let rows: Vec<EventRow> = sqlx::query_as(&orphan_tool_calls_sql())
        .fetch_all(&pool)
        .await
        .unwrap();
    let mine: Vec<EventRow> = rows
        .into_iter()
        .filter(|r| r.thread_id == Some(subscribed_id))
        .collect();
    assert!(
        mine.iter().any(|r| r.event_type == "ToolCalled"),
        "precondition: the await_event call is in the sweep's candidate set"
    );
    let orphans = crate::core::store::find_orphan_tool_called_ids(&mine);
    assert!(
        orphans.is_empty(),
        "await_event pairs its own call, so no synthetic result is owed: {orphans:?}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
