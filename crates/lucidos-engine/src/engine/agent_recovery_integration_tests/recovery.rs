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
