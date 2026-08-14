use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

/// Regression: orphan recovery for a branch that was mid-CC-turn at engine
/// restart must flag its pending change as `incomplete: true`. The pending
/// row was populated by per-commit emits during the dying turn — its
/// description, files, requires_restart reflect mid-merge or mid-edit
/// work the user never confirmed. Auto-applying without confirmation would
/// land half-finished code.
///
/// Per the user's report on thread ef2685a9: a clean prior turn proposed a
/// change; the user clicked Apply; the merge-resolution session committed
/// (per-commit hook updated the row), then died; after engine restart the
/// row showed as "ready to apply" with `incomplete: false`. The recovery
/// must intervene to flip the flag so the Apply UI requires confirmation.
#[tokio::test]
async fn recovery_marks_pending_change_incomplete_for_mid_turn_branch() {
    use super::mark_pending_change_incomplete;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let branch = "claude-code/mid-turn-recovery-test";
    let change_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    // Seed a CC SessionStarted so the lifecycle classifier accepts CC events.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-midturn".into(),
            branch: branch.into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("session start emits")
    .expect("event persisted");

    // Establish the pending change row from a clean prior turn (incomplete: false).
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: change_id.to_string(),
            description: Some("clean prior turn".into()),
            files: vec!["a.rs".into(), "b.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: branch.into(),
            repo_root: "/tmp/repo".into(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("change proposed emits")
    .expect("event persisted");

    // Pre-load the Change row the same way recover_orphaned_worktrees does
    // (via the projection) — the helper assumes the caller already has it.
    let change = bus
        .changes_projection()
        .get_pending_by_branch(branch)
        .await
        .expect("lookup succeeds")
        .expect("row exists");
    assert!(
        !change.incomplete,
        "precondition: pending row from the clean prior turn must start as incomplete=false"
    );

    // Recovery detects branch was mid-turn at restart — flip the row to incomplete.
    mark_pending_change_incomplete(&bus, thread_id, &change).await;

    let after: bool = sqlx::query_scalar(
        "SELECT incomplete FROM changes WHERE branch_name = $1 AND status = 'pending'",
    )
    .bind(branch)
    .fetch_one(&pool)
    .await
    .expect("row still exists");
    assert!(
        after,
        "recovery must flag the pending change as incomplete: true so Apply requires confirmation"
    );

    // Sanity: a second call is idempotent — already incomplete, no further emit.
    let change_after = bus
        .changes_projection()
        .get_pending_by_branch(branch)
        .await
        .expect("lookup succeeds")
        .expect("row exists");
    mark_pending_change_incomplete(&bus, thread_id, &change_after).await;

    let proposed_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeProposed'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        proposed_count, 2,
        "expected 2 ChangeProposed events (1 seed + 1 mark-incomplete), got {}",
        proposed_count
    );

    teardown_test_db(&db_name).await;
}

/// Regression: when the user clicks Apply on a thread whose Claude Code
/// session has cleanly exited at idle (the Phase 5.3 happy path), `apply_now`
/// MUST apply the existing clean pending change directly instead of routing
/// through `end_stale_waiting_session`. Per dev-workspace thread
/// `00c98c90-841e-47fd-807d-ff16627d8588` (2026-05-22): the clean prior turn
/// proposed at 18:29:15 with `incomplete=false`, then the user clicked
/// Apply at 18:31:27 and the engine re-emitted ChangeProposed with origin
/// `stale_session` + `incomplete=true` (because `propose_branch_changes`
/// hard-coded `incomplete: true` for engine-driven recovery).
///
/// Since 1f7f945ed, `propose_branch_changes` derives `incomplete` from the
/// prior turn's actual terminal event via `last_turn_ended_cleanly`, so the
/// recovery path no longer flips clean rows to `incomplete=true` on its
/// own. The fast path is still worth keeping: it avoids the
/// worktree-scan + describe + re-emit round-trip that would otherwise
/// produce a duplicate `ChangeProposed` event in the timeline for the same
/// underlying state. A future refactor that drops the fast path would
/// reintroduce noise events (and, if `last_turn_ended_cleanly` ever shifts
/// semantics, could reintroduce the original incomplete-flip bug too).
///
/// Two assertions:
///   1. The projection contract apply_now's fast path relies on: a clean
///      ChangeProposed lands as a pending row with `incomplete=false`,
///      visible via `pending_for_thread`.
///   2. The apply_now source actually checks `pending_for_thread` BEFORE
///      calling `end_stale_waiting_session`, so the fast path is taken.
#[tokio::test]
async fn apply_now_no_live_session_fast_path_preserves_clean_pending_change() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let branch = "claude-code/apply-now-fast-path-test";
    let change_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-fastpath".into(),
            branch: branch.into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("session start emits")
    .expect("event persisted");

    // The clean prior turn proposes a change.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: change_id.to_string(),
            description: Some("clean prior turn".into()),
            files: vec!["src/lib.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: branch.into(),
            repo_root: "/tmp/repo".into(),
            hardened: true,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: cc_meta,
    })
    .await
    .expect("change proposed emits")
    .expect("event persisted");

    // Assertion 1: the projection surfaces the clean change to apply_now's
    // fast-path check.
    let pending = bus
        .changes_projection()
        .pending_for_thread(thread_id)
        .await
        .expect("pending_for_thread query");
    assert_eq!(
        pending.len(),
        1,
        "apply_now's fast path queries pending_for_thread — must return the clean change"
    );
    assert!(
        !pending[0].incomplete,
        "the seeded clean ChangeProposed must project as incomplete=false; got incomplete=true"
    );

    // Assertion 2: apply_now's no-live-session branch checks pending FIRST
    // (so the apply runs directly) before falling through to
    // end_stale_waiting_session (which would re-emit a duplicate
    // ChangeProposed for the same underlying state).
    let apply_now_src = include_str!("../agent_session/apply_now.rs");
    let fast_path_marker = "Fast path: apply existing pending change directly";
    assert!(
        apply_now_src.contains(fast_path_marker),
        "apply_now.rs no longer contains the fast-path marker '{}' — the no-live-session \
         branch may have reverted to unconditionally calling end_stale_waiting_session, \
         which would re-emit a duplicate ChangeProposed for the existing clean row \
         (and, if last_turn_ended_cleanly ever shifts semantics, could reintroduce \
         the original incomplete-flip bug)",
        fast_path_marker
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: stale-session recovery must derive the `incomplete` flag from
/// the prior turn's actual terminal event, not hardcode `true`. Per the user's
/// report on dev thread `183cf6f3-0cd1-406d-a1f9-40fad8010cb8` (2026-05-25):
/// the original CC turn emitted `ResponseGenerated` + `CodingAgentIdled`
/// cleanly with `has_changes=true`, but the per-idle `propose_change` never
/// landed (at the time, the since-removed bg-bash gate held it back), so no
/// `ChangeProposed` fired. Hours later the user clicked Archive; the
/// archive cascade routed through `end_stale_waiting_session` →
/// `propose_branch_changes`, which proposed the existing committed work but
/// flagged it `incomplete=true` (the hardcoded value), producing both a
/// "this came from a failed turn" confirm dialog AND a "Change ready to apply"
/// push from the `notify-on-idle-and-new-changes` trigger.
///
/// Three cases pinned:
///   1. Latest terminal is `ResponseGenerated` → `last_turn_ended_cleanly`
///      returns `true` → stale recovery would propose `incomplete: false`.
///   2. Latest terminal is `ResponseAborted` / `ResponseCanceled` /
///      `ResponseFailed` → returns `false` → stale recovery proposes
///      `incomplete: true` (the legitimate mid-turn-crash case the original
///      hardcoded value was written for).
///   3. No terminal event at all → returns `false` → `incomplete: true`.
#[tokio::test]
async fn last_turn_ended_cleanly_distinguishes_terminal_kinds() {
    use crate::engine::agent_recovery::last_turn_ended_cleanly;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    // Case 3 (no terminal yet): bare SessionStarted, nothing else.
    let no_terminal = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: no_terminal,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-noterm".into(),
            branch: "claude-code/noterm".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        !last_turn_ended_cleanly(&pool, no_terminal).await,
        "no terminal event yet — must NOT be classified as clean (fall back to incomplete: true)"
    );

    // Case 1 (clean): SessionStarted → ResponseGenerated → CodingAgentIdled.
    // Mirrors the 183cf6f3 thread shape: CC produced a clean Generated terminal,
    // CodingAgentIdled reported has_changes=true, no further turn ran. The
    // per-idle `propose_change` never landed (e.g. the engine died between
    // idle and proposal) but the work itself is complete.
    let clean = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: clean,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-clean".into(),
            branch: "claude-code/clean".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: clean,
        event: ThreadEvent::ResponseGenerated {
            text: "Done.".into(),
            images: Vec::new(),
            model: Some("test-model".into()),
            reasoning_effort: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: clean,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("sid-clean".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        last_turn_ended_cleanly(&pool, clean).await,
        "ResponseGenerated as the latest terminal MUST classify as clean — \
         the stale-session recovery would otherwise mislabel a finished turn \
         as incomplete and surface a misleading confirm dialog"
    );

    // Case 2a (aborted): ResponseAborted as latest terminal — engine-driven
    // mid-turn kill (e.g. EngineShutdown, recovery_after_restart). Must NOT
    // be clean — the work the user sees on the branch may be half-finished.
    let aborted = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: aborted,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-abort".into(),
            branch: "claude-code/abort".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: aborted,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        !last_turn_ended_cleanly(&pool, aborted).await,
        "ResponseAborted as latest terminal MUST classify as not-clean — \
         the user-visible branch state may be mid-edit"
    );

    // Case 2b (canceled): user clicked Stop. Same not-clean classification.
    let canceled = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: canceled,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-cancel".into(),
            branch: "claude-code/cancel".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: canceled,
        event: ThreadEvent::ResponseCanceled {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        !last_turn_ended_cleanly(&pool, canceled).await,
        "ResponseCanceled as latest terminal MUST classify as not-clean"
    );

    // Case 2c (failed): turn ended with an error (mid-stream API drop, OOM,
    // empty Result). Same not-clean classification.
    let failed = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: failed,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-fail".into(),
            branch: "claude-code/fail".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: failed,
        event: ThreadEvent::ResponseFailed {
            error: "test error".into(),
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        !last_turn_ended_cleanly(&pool, failed).await,
        "ResponseFailed as latest terminal MUST classify as not-clean"
    );

    // Case 1+2 combo: a clean Generated followed by a later Aborted. The
    // LATEST terminal wins — incomplete=true. This is the legitimate
    // sequence the original hardcoded value was meant to catch (clean
    // earlier turn, then a follow-up turn that crashed mid-stream).
    let clean_then_aborted = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: clean_then_aborted,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-mixed".into(),
            branch: "claude-code/mixed".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: clean_then_aborted,
        event: ThreadEvent::ResponseGenerated {
            text: "Turn 1 done.".into(),
            images: Vec::new(),
            model: Some("test-model".into()),
            reasoning_effort: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: clean_then_aborted,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: cc_meta,
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        !last_turn_ended_cleanly(&pool, clean_then_aborted).await,
        "a later Aborted MUST shadow an earlier clean Generated — the LATEST \
         terminal is what reflects the branch's current trustworthiness"
    );

    // Case 4 (cross-channel): CC turn died mid-stream (CC ResponseAborted),
    // then chat agent ran later and produced a clean ResponseGenerated. The
    // chat turn has no bearing on whether the CC branch is mid-edit. Without
    // channel scoping the chat's clean Generated would shadow CC's Aborted
    // and the stale-session recovery would propose CC's branch as
    // incomplete=false, auto-applying CC's half-finished work.
    let chat_meta = EventMeta {
        channel: Some(EventChannel::Chat),
        ..EventMeta::NONE
    };
    let cross_channel = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: cross_channel,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-xchan".into(),
            branch: "claude-code/xchan".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    // CC turn dies mid-stream — ResponseAborted on the CC channel.
    bus.emit(BusEvent::Thread {
        thread_id: cross_channel,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    // Now the chat agent runs and produces a clean Generated on the chat
    // channel — same thread, later sequence.
    bus.emit(BusEvent::Thread {
        thread_id: cross_channel,
        event: ThreadEvent::ResponseGenerated {
            text: "chat answer".into(),
            images: Vec::new(),
            model: Some("test-model".into()),
            reasoning_effort: None,
        },
        meta: chat_meta,
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        !last_turn_ended_cleanly(&pool, cross_channel).await,
        "chat-channel ResponseGenerated MUST NOT shadow a CC-channel \
         ResponseAborted — stale-session recovery is CC-only and the CC \
         branch is genuinely mid-edit"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: a coding-agent turn the user **canceled** must classify as `idle`,
/// not `running`, so the next engine restart leaves it alone.
///
/// Reported 2026-08-04: the user clicked Stop nine seconds into a turn, the
/// timeline showed "Canceled" and a "Response canceled" panel, and then, at the
/// next engine restart nearly four hours later, a "System / Response interrupted"
/// panel with a **Continue** button appeared underneath it. You cannot interrupt
/// a response that already ended.
///
/// Cause: [`BRANCH_CLASSIFICATION_SQL`]'s lifecycle scan only knew
/// `SessionStarted / CodingAgentIdled / SessionEnded / ResponseGenerated`, so
/// the newest *recognised* event on that thread stayed the `SessionStarted`
/// from before the Stop. `recover_orphaned_worktrees` read that as "a turn was
/// in flight when the engine died" and emitted the boundary
/// `ResponseAborted{recovery_after_restart}` + `CodingAgentIdled{engine_restart_interrupt}`
/// pair that renders the panel.
///
/// The cases below pin both directions: every way a turn can *end* classifies
/// `idle`, and every way one can still be *in flight* classifies `running`.
/// The `running` half matters as much as the `idle` half, because
/// over-classifying as idle would silently kill crash recovery and the
/// *Switch to new version* auto-resume.
#[tokio::test]
async fn canceled_turn_classifies_idle_so_restart_does_not_reopen_it() {
    use crate::engine::agent_recovery::BRANCH_CLASSIFICATION_SQL;
    use crate::engine::thread_events::{AbortCause, CancelCause, SessionEndReason};
    use std::collections::HashMap;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    // Start a CC session on `branch` and return its thread id.
    let start = |branch: &str| {
        let branch = branch.to_string();
        let bus = bus.clone();
        let meta = cc_meta.clone();
        async move {
            let thread_id = Uuid::new_v4();
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::SessionStarted {
                    coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                    session_id: format!("sid-{branch}"),
                    branch,
                    repo_id: None,
                    coding_agent_kind: Default::default(),
                    coding_agent_folder: String::new(),
                    app_id: None,
                },
                meta,
            })
            .await
            .expect("emit succeeds")
            .expect("event persisted");
            thread_id
        }
    };
    let emit = |thread_id: Uuid, event: ThreadEvent| {
        let bus = bus.clone();
        let meta = cc_meta.clone();
        async move {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta,
            })
            .await
            .expect("emit succeeds")
            .expect("event persisted");
        }
    };
    let canceled_by_user = ThreadEvent::ResponseCanceled {
        text: String::new(),
        images: Vec::new(),
        model: None,
        reasoning_effort: None,
        cause: CancelCause::UserStop,
    };

    // idle: the reported shape. Stop mid-turn, before CC ever reached an idle.
    let user_stopped = start("claude-code/user-stopped").await;
    emit(user_stopped, canceled_by_user.clone()).await;

    // idle: same, but the turn failed instead of being canceled. Identical hole
    // in the old scan set, identical nonsense panel.
    let failed = start("claude-code/failed").await;
    emit(
        failed,
        ThreadEvent::ResponseFailed {
            error: "stream ended".into(),
        },
    )
    .await;

    // idle: a session that panicked and ended. `SessionEnded` was in the old
    // scan set but matched NEITHER classification arm, and the caller treats
    // "in neither set" as in-flight.
    let panicked = start("claude-code/panicked").await;
    emit(
        panicked,
        ThreadEvent::SessionEnded {
            reason: SessionEndReason::Panic,
        },
    )
    .await;

    // idle: the oldest rows carry no `reason` key at all. They deserialize as
    // LegacyNonTerminal, which is terminal, and the classifier's reason filter
    // must not silently drop them on three-valued logic (`NULL IN (...)`).
    //
    // Inserted raw rather than through the bus on purpose: `SessionEnded.reason`
    // has `#[serde(default = ...)]`, not `skip_serializing_if`, so every event
    // the bus can emit today carries the key. A reason-less row is only
    // reachable as historical data, and this is the shape the query has to
    // survive. (Same precedent as `worktree_cleanup_tests::insert_old_event`.)
    let legacy_no_reason = start("claude-code/legacy-no-reason").await;
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES ($1, 'SessionEnded', '{}'::jsonb, $2, 'thread', $2::text)",
    )
    .bind(Uuid::new_v4())
    .bind(legacy_no_reason)
    .execute(&pool)
    .await
    .expect("legacy reason-less SessionEnded row inserts");

    // running: a turn genuinely in flight when the engine died (no terminal at
    // all). This is what the interrupt panel + Continue exist for.
    start("claude-code/mid-turn").await;

    // running: the *Switch to new version* teardown boundary. `ResponseAborted`
    // must NOT count as turn-ended, or `switch_was_user_initiated` never gets
    // the chance to auto-resume it.
    let switched = start("claude-code/switched").await;
    emit(
        switched,
        ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: AbortCause::EngineShutdown,
        },
    )
    .await;

    // running: CC answered a stale `--resume` with an empty Result. The engine
    // retries against a fresh session, so this SessionEnded is transient.
    let stale_resume = start("claude-code/stale-resume").await;
    emit(
        stale_resume,
        ThreadEvent::SessionEnded {
            reason: SessionEndReason::StaleResume,
        },
    )
    .await;

    // running: the switch teardown boundary followed by a shutdown-reason
    // SessionEnded. No production site emits that pair today (teardown yields
    // Aborted(EngineShutdown) and stops there), but the variant is live and it
    // is the SessionEnded-shaped twin of the boundary: if it ever lands, it must
    // not turn a resumable switch into an idle branch.
    let switched_then_session_ended = start("claude-code/switched-then-session-ended").await;
    emit(
        switched_then_session_ended,
        ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: AbortCause::EngineShutdown,
        },
    )
    .await;
    emit(
        switched_then_session_ended,
        ThreadEvent::SessionEnded {
            reason: SessionEndReason::Shutdown,
        },
    )
    .await;

    // running: canceled, then the user sent a follow-up that was still running
    // when the engine died. A terminal only ends the turn it terminates.
    let canceled_then_resumed = start("claude-code/canceled-then-resumed").await;
    emit(canceled_then_resumed, canceled_by_user).await;
    emit(
        canceled_then_resumed,
        ThreadEvent::CodingAgentUserMessageSent {
            text: "actually, carry on".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
    )
    .await;

    let rows: Vec<(Option<String>, Option<String>)> = sqlx::query_as(&BRANCH_CLASSIFICATION_SQL)
        .fetch_all(&pool)
        .await
        .expect("branch classification query runs");
    let status: HashMap<String, String> = rows
        .into_iter()
        .filter_map(|(branch, status)| Some((branch?, status?)))
        .collect();

    for (branch, expected, why) in [
        (
            "claude-code/user-stopped",
            "idle",
            "the user already ended this turn with Stop; a restart must not \
             re-open it as \"Response interrupted\" with a Continue button",
        ),
        (
            "claude-code/failed",
            "idle",
            "the turn ended on an error the user already saw; there is nothing \
             to continue",
        ),
        (
            "claude-code/panicked",
            "idle",
            "SessionEnded is terminal; landing in neither set made the caller \
             treat it as in-flight",
        ),
        (
            "claude-code/legacy-no-reason",
            "idle",
            "a reason-less SessionEnded is LegacyNonTerminal, so the reason \
             filter must keep it rather than NULL it out of the scan",
        ),
        (
            "claude-code/mid-turn",
            "running",
            "no terminal at all: this is the genuine mid-turn crash the \
             interrupt panel exists for",
        ),
        (
            "claude-code/switched",
            "running",
            "the switch teardown boundary must stay resumable, or \
             Switch to new version silently stops resuming work",
        ),
        (
            "claude-code/stale-resume",
            "running",
            "a stale --resume is retried against a fresh session; the \
             SessionEnded is transient, not a turn boundary",
        ),
        (
            "claude-code/switched-then-session-ended",
            "running",
            "a shutdown-reason SessionEnded is the engine going away mid-turn, \
             so it must not cancel out the switch boundary that precedes it",
        ),
        (
            "claude-code/canceled-then-resumed",
            "running",
            "the follow-up turn was in flight; a terminal only ends the turn \
             it terminates",
        ),
    ] {
        assert_eq!(
            status.get(branch).map(String::as_str),
            Some(expected),
            "branch {branch} must classify as {expected}: {why}"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A *Switch to new version* promises the interrupted thread a resume, and the
/// UI withholds its Continue button on the strength of that promise. So a
/// promise the boot cannot keep has to be WITHDRAWN before the API server opens,
/// or the thread sits `paused` with no way forward.
///
/// The floor takes the resumed set BY ID rather than re-deriving it: a
/// coding-agent resume has only emitted `ContinuationRequested` when this runs,
/// and that type is deliberately absent from `THREAD_START_EVENTS_SQL`, so a
/// query-only exclusion would re-abort a thread that is resuming correctly.
#[tokio::test]
async fn boot_floor_withdraws_only_the_switch_promises_it_did_not_keep() {
    use crate::engine::agent_recovery::settle_unresumed_switch_threads;
    use crate::engine::thread_events::{AbortCause, MessageOrigin};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let device = MessageOrigin::Device {
        device_id: "d1".into(),
        label: "My iPhone".into(),
    };

    // Three threads interrupted by the same switch: one this boot resumed, one it
    // silently declined (over the chat cap / a failed resume / a skipped branch),
    // and one that was archived while its turn was in flight.
    let resumed = Uuid::new_v4();
    let declined = Uuid::new_v4();
    let archived = Uuid::new_v4();
    for thread_id in [resumed, declined, archived] {
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
                mode: crate::engine::thread_events::ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseAborted {
                text: "interrupted by an engine restart".into(),
                images: Vec::new(),
                model: None,
                reasoning_effort: None,
                cause: AbortCause::EngineShutdown,
            },
            meta: EventMeta {
                actor: Some(device.clone()),
                ..EventMeta::NONE
            },
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");
    }

    // Archived DURING the turn, which is the only shape that reaches the floor:
    // the Archive button's own `ThreadArchived` would settle the status and take
    // the thread out of `paused` altogether. Neither resume drain selects an
    // archived thread, so without the floor its switch abort stays the newest
    // boundary forever and unarchiving surfaces a paused thread whose Continue
    // button no later boot could restore.
    sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
        .bind(archived)
        .execute(&pool)
        .await
        .expect("archive the thread");

    let status_of = |thread_id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("summary row exists")
        }
    };

    // Every thread carries the switch fingerprint (device actor + engine
    // shutdown), so every one reads `paused`: nothing has gone wrong yet, the
    // engine just went away and promised to come back.
    for thread_id in [resumed, declined, archived] {
        assert_eq!(
            status_of(thread_id).await,
            "paused",
            "a switch teardown must settle the thread at paused, not failed"
        );
    }

    let withdrawals = |thread_id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 \
                   AND event_type = 'ResponseAborted' \
                   AND payload->>'cause' = 'recovery_after_restart'",
            )
            .bind(thread_id.to_string())
            .fetch_one(&pool)
            .await
            .expect("count query")
        }
    };

    settle_unresumed_switch_threads(&pool, &bus, &std::collections::HashSet::from([resumed])).await;

    assert_eq!(
        withdrawals(resumed).await,
        0,
        "the thread this boot IS resuming must keep its promise: withdrawing it \
         would hand the user a Continue button for a turn already back in flight"
    );
    assert_eq!(
        withdrawals(declined).await,
        1,
        "the thread this boot declined must have its promise withdrawn, which is \
         what re-arms its Continue button"
    );
    assert_eq!(
        withdrawals(archived).await,
        1,
        "an archived thread is selected by NEITHER resume drain, so the floor is \
         its only chance: excluding it here is a permanent dead end"
    );

    // The withdrawal has to be VISIBLE, not just recorded. `recovery_after_restart`
    // is not the switch fingerprint (which needs `engine_shutdown` AND a device),
    // so the status follows the promise off `paused` and onto `failed`: the red
    // dot, a slot in the needs-attention count, and the Continue button the
    // withdrawal exists to hand back. Leaving these two on the reassuring pause
    // glyph is exactly the state the user reported.
    for thread_id in [declined, archived] {
        assert_eq!(
            status_of(thread_id).await,
            "failed",
            "a withdrawn resume promise must stop reading as paused"
        );
    }

    // ...and the CAUSE is what does that, not the actor. The withdrawal names
    // the device that clicked switch, inherited from the abort it withdraws,
    // because the restart it is reporting was that person's. Hardcoding
    // `system` here is what made a user's own *Switch to new version* come back
    // as "System / Response interrupted" over a red thread, and it is the
    // fourth site of that bug rather than the first: see
    // `docs/plans/2026-08-07-teardown-actor-is-one-value-for-the-whole-teardown.md`
    // for the three teardown emits swept two days after this floor was written.
    //
    // Asserted as "no boundary on this thread says system", not merely "the
    // withdrawal says device", because the recurrence is always a NEW emit site
    // appearing beside the fixed ones.
    for thread_id in [declined, archived] {
        let actors: Vec<Option<String>> = sqlx::query_scalar(
            "SELECT payload->'actor'->>'kind' FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseAborted' \
             ORDER BY sequence",
        )
        .bind(thread_id.to_string())
        .fetch_all(&pool)
        .await
        .expect("actor query");
        assert_eq!(
            actors,
            vec![Some("device".to_string()), Some("device".to_string())],
            "both boundaries about ONE user-initiated teardown must name the \
             human who caused it: the switch abort and the withdrawal of its \
             promise are two statements about the same restart"
        );
    }
    assert_eq!(
        status_of(resumed).await,
        "paused",
        "the thread whose promise is being KEPT must stay paused: it is on its \
         way back, and nothing is being asked of the user"
    );

    // The resumed thread's spawn lands its `ContinuationStarted`, which IS in
    // `THREAD_START_EVENTS_SQL`, so from here on the query alone excludes it.
    bus.emit(BusEvent::Thread {
        thread_id: resumed,
        event: ThreadEvent::ContinuationStarted {
            branch: String::new(),
            origin: None,
            reason: Some(crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON.into()),
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    // Idempotent: the withdrawal is itself a ResponseAborted, so the declined
    // thread no longer matches "the newest abort is a switch abort". Without
    // that clause the floor would re-fire on every boot forever, since nothing
    // supersedes the original switch abort in the START-event sense. Run with an
    // EMPTY resumed set so only the query's own guards can hold the line.
    settle_unresumed_switch_threads(&pool, &bus, &std::collections::HashSet::new()).await;
    assert_eq!(
        withdrawals(declined).await,
        1,
        "a second boot must not stack another withdrawal on the same abort"
    );
    assert_eq!(
        withdrawals(resumed).await,
        0,
        "a thread resumed on an earlier boot is superseded by its own \
         ContinuationStarted and must never be swept"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// What one stray `ResponseCanceled` costs a switch, and why the fix belongs
/// upstream of every read in this file.
///
/// The reported shape
/// (`docs/plans/2026-08-06-a-session-that-registers-mid-teardown-is-shutting-down.md`):
/// a *Switch to new version* landed 2 s into a coding-agent session's spawn. The
/// teardown snapshot could not see a session that did not exist yet, so nothing
/// set its `external_terminal_emitted` flag and its per-session `shutting_down`
/// stayed `false`. When the session registered a second later and its
/// `chat_cancel` arm fired on the already-cancelled handle token, it read that
/// bare flag, classified an engine restart as a user Stop, and wrote
/// `ResponseCanceled{user_stop}` on a turn nobody stopped.
///
/// Every recovery read below is correct and unchanged. They simply reached the
/// right conclusion from a wrong input, which is why the plan's non-goals refuse
/// to take `ResponseCanceled` out of `TURN_ENDED_EVENT_TYPES_SQL`: that list is
/// right, and editing it would be a downstream filter over an upstream defect.
///
/// Both halves are asserted against the SAME seed minus that one event, so the
/// test states the cost of the bug rather than merely the shape of the fix.
#[tokio::test]
async fn a_stray_cancel_during_teardown_costs_the_switch_its_auto_resume() {
    use crate::engine::agent_recovery::{
        settle_unresumed_switch_threads, switch_was_user_initiated, BRANCH_CLASSIFICATION_SQL,
    };
    use crate::engine::thread_events::{AbortCause, CancelCause, MessageOrigin};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let device = MessageOrigin::Device {
        device_id: "d1".into(),
        label: "My MacBook".into(),
    };
    // `fixed` is what the engine now writes; `buggy` is what it wrote before.
    let fixed = Uuid::new_v4();
    let buggy = Uuid::new_v4();
    let branch_of = |thread_id: Uuid| format!("claude-code/{thread_id}");

    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    for thread_id in [fixed, buggy] {
        // The user's message, which anchors the turn and is the newest start
        // event, so the teardown abort below out-sequences it.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                text: "so go?".into(),
                user_image_hashes: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: crate::engine::thread_events::ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: cc_meta.clone(),
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");

        // The switch teardown boundary: device actor + engine shutdown, the
        // fingerprint the whole auto-resume contract keys on.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseAborted {
                text: "This response was interrupted by an engine restart.".into(),
                images: Vec::new(),
                model: None,
                reasoning_effort: None,
                cause: AbortCause::EngineShutdown,
            },
            meta: EventMeta {
                actor: Some(device.clone()),
                ..cc_meta.clone()
            },
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");

        // The session finally registers, AFTER the boundary. This is the whole
        // race: `SessionStarted` is what leaves the branch classified `running`.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                session_id: String::new(),
                branch: branch_of(thread_id),
                repo_id: None,
                coding_agent_kind: crate::engine::agent_session::CodingAgentKind::Lucidos,
                coding_agent_folder: String::new(),
                app_id: None,
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            },
            meta: cc_meta.clone(),
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");
    }

    // The one event that differs: the phantom cancel the pre-fix engine wrote.
    // Constructed directly rather than through `emit_response_canceled` on
    // purpose: that helper's idempotency gate would recognise the abort above by
    // request id and skip, which is exactly the suppression the pre-fix engine
    // did NOT have on this path. Going through it would seed a shape the bug
    // never produced.
    bus.emit(BusEvent::Thread {
        thread_id: buggy,
        event: ThreadEvent::ResponseCanceled {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: CancelCause::UserStop,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    // Read 1: the branch classifier decides whether there is a turn to resume.
    let branch_status = |branch: String| {
        let pool = pool.clone();
        async move {
            let rows: Vec<(String, String)> = sqlx::query_as(&BRANCH_CLASSIFICATION_SQL)
                .fetch_all(&pool)
                .await
                .expect("branch classification query");
            rows.into_iter()
                .find(|(b, _)| *b == branch)
                .map(|(_, status)| status)
                .expect("the seeded branch is classified")
        }
    };
    assert_eq!(
        branch_status(branch_of(fixed)).await,
        "running",
        "with no phantom cancel the newest lifecycle event is SessionStarted, so \
         the turn is in flight and the resume gate can pick it up"
    );
    assert_eq!(
        branch_status(branch_of(buggy)).await,
        "idle",
        "the phantom cancel is a turn-ended event, so the classifier reads the \
         turn as finished and no resume is even attempted"
    );

    // Read 2: the switch fingerprint itself is unharmed either way. The cancel
    // does not retire the abort (it is not a start event), which is precisely
    // why the damage is invisible at this layer and only shows up above.
    for thread_id in [fixed, buggy] {
        assert!(
            switch_was_user_initiated(&pool, thread_id).await,
            "the device-attributed teardown abort is still the newest thing on \
             the thread in the START-event sense"
        );
    }

    // Read 3: the promise as the user sees it. Neither thread was resumed by
    // this boot, so the floor withdraws both; the point is that only `buggy`
    // ever reaches this state in production, because `fixed` classifies
    // `running` above and the resume drain claims it first.
    let status_of = |thread_id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("summary row exists")
        }
    };
    for thread_id in [fixed, buggy] {
        assert_eq!(
            status_of(thread_id).await,
            "paused",
            "a switch teardown settles at paused, and the phantom cancel does \
             not disturb that: `ResponseCanceled` is a verdict-preserving arm"
        );
    }

    settle_unresumed_switch_threads(&pool, &bus, &std::collections::HashSet::from([fixed])).await;

    assert_eq!(
        status_of(fixed).await,
        "paused",
        "the resumed thread keeps its promise and stays behind the pause glyph \
         with no Continue button"
    );
    assert_eq!(
        status_of(buggy).await,
        "failed",
        "the reported end state: a transcript that opens 'Paused by restart' and \
         a thread that ends red, in the attention count, asking the user to \
         Continue work the engine had promised to resume itself"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A coding-agent thread does not get a fresh branch per turn: it applies a
/// change and keeps working on the same one. So nothing the branch did on an
/// EARLIER turn may reach the recovery gate, or the thread is retired from the
/// first Apply onward and never auto-resumes again.
///
/// The reported shape (2026-08-09, thread "Persisting Thread Auto-Scroll to
/// Bottom"): a change on the branch was applied at 16:55, the user sent a
/// follow-up at 17:15, and at 17:21:54 they hit *Switch to new version* with
/// the turn mid-flight. Boot classified the branch `running` and the switch
/// fingerprint held, exactly as asserted below, and recovery skipped it anyway
/// with "has no in-flight signal": `in_flight` also consulted
/// `completed_change_branches`, `SELECT DISTINCT branch_name FROM changes WHERE
/// status IN ('applied','discarded')`, which had held that branch since 16:55.
/// No resume was actuated, `settle_unresumed_switch_threads` withdrew the
/// promise, and "Paused by restart" became "System / Response interrupted"
/// over a `failed` thread, reading to the user as a crash they had not caused.
///
/// So the assertions are deliberately spread across all three reads the resume
/// decision is made from, not just the gate that carried the bug. The two DB
/// reads were already correct in the report and are pinned here because the
/// obvious wrong repair is to make one of them agree with the veto: adding
/// `ChangeApplied` to `TURN_ENDED_EVENT_TYPES_SQL` would re-close the same hole
/// from the other side, and it would look reasonable.
#[tokio::test]
async fn an_applied_change_on_the_branch_does_not_cost_a_later_turn_its_resume() {
    use crate::engine::agent_recovery::{
        branch_awaits_recovery, switch_was_user_initiated, BRANCH_CLASSIFICATION_SQL,
    };
    use crate::engine::thread_events::{AbortCause, MessageOrigin};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let branch = format!("claude-code/{thread_id}");
    let change_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    let device = MessageOrigin::Device {
        device_id: "d1".into(),
        label: "My iPhone".into(),
    };

    // Turn 1: a session on the branch, a change, and the user applying it.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: "sid-turn-1".into(),
            branch: branch.clone(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: change_id.to_string(),
            description: Some("turn 1".into()),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: branch.clone(),
            repo_root: "/tmp/repo".into(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            change_id: change_id.to_string(),
            requires_restart: false,
            client_update: false,
            commits: vec!["feat: turn 1".into()],
            thread_title: None,
            actor: Some(device.clone()),
            pre_merge_sha: None,
            post_merge_sha: None,
            path: String::new(),
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    // Turn 2 on the SAME branch: the follow-up, a fresh session, and the user's
    // switch landing mid-turn.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentUserMessageSent {
            text: "follow-up".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: "sid-turn-2".into(),
            branch: branch.clone(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: AbortCause::EngineShutdown,
        },
        meta: EventMeta {
            actor: Some(device),
            ..cc_meta.clone()
        },
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    // Precondition: the branch really does carry a completed change, so this is
    // the seed the veto used to fire on rather than a shape it never saw.
    let completed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM changes WHERE branch_name = $1 AND status = 'applied'",
    )
    .bind(&branch)
    .fetch_one(&pool)
    .await
    .expect("changes row query");
    assert_eq!(
        completed, 1,
        "precondition: turn 1's change is applied and the branch is in the \
         completed set the retired veto read"
    );

    // Read 1: the branch classifier. `ChangeApplied` is not a lifecycle event,
    // so turn 2's `SessionStarted` is still the newest one and the turn is open.
    let rows: Vec<(String, String)> = sqlx::query_as(&BRANCH_CLASSIFICATION_SQL)
        .fetch_all(&pool)
        .await
        .expect("branch classification query");
    let status = rows
        .into_iter()
        .find(|(b, _)| *b == branch)
        .map(|(_, s)| s)
        .expect("the seeded branch is classified");
    assert_eq!(
        status, "running",
        "an applied change from turn 1 must not read as turn 2 having ended"
    );

    // Read 2: the switch fingerprint. The engine promised this turn a resume.
    assert!(
        switch_was_user_initiated(&pool, thread_id).await,
        "the teardown abort carries a device actor and EngineShutdown, so this \
         is a user switch and the boot owes it an auto-resume"
    );

    // Read 3: the gate that broke. `idle_branches` is what read 1 feeds it, and
    // turn 1's change is resolved so nothing is pending.
    let pending = bus
        .changes_projection()
        .get_pending_by_branch(&branch)
        .await
        .expect("pending lookup succeeds");
    assert!(
        pending.is_none(),
        "precondition: the applied change leaves nothing pending, so the gate \
         cannot be rescued by the pending-change arm"
    );
    assert!(
        branch_awaits_recovery(&branch, &std::collections::HashSet::new(), false),
        "the reported failure: recovery must not skip this branch, or no resume \
         is actuated and the boot floor withdraws the promise the user was shown"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
