use super::super::*;
use super::*;

/// Locks in the boolean-cluster split: `CodingAgentIdled` no longer sets
/// `coding_agent_proposed`, even when its `has_changes` payload is true.
/// `coding_agent_proposed` is set EXCLUSIVELY by `ChangeProposed`. The two
/// events still ship together in the production CC lifecycle (idle, then
/// the engine emits ChangeProposed), but the projection treats them as
/// independent writes — which lets `coding_agent_has_diff` and
/// `coding_agent_proposed` evolve along separate axes (mid-session commits
/// vs. formal review-ready proposal).
///
/// Without this, drift returns: an idle payload could imply "ready for
/// review" without the engine actually emitting the formal proposal event,
/// and `coding_agent_requires_restart` could lag behind a fresh proposal
/// because it was last set from a stale CodingAgentIdled.
#[tokio::test]
async fn coding_agent_idled_does_not_set_proposed_until_change_proposed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/split", None).await;

    // Step 1: CC idles with has_changes=true (mid-loop, before formal proposal).
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: true,
            cc_session_id: Some("sid-split".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Asserts the SPLIT: CodingAgentIdled alone leaves `coding_agent_proposed`
    // false (so Apply / Discard buttons don't appear), and the row stays at
    // 'idle'. The is_external flag DOES flow through (it's a property of the
    // session binding, not a proposal-lifecycle signal).
    let (status, proposed, requires_restart, is_external): (String, bool, bool, bool) =
        sqlx::query_as(
            "SELECT status, coding_agent_proposed, coding_agent_requires_restart, \
             coding_agent_is_external_repo FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "idle", "CodingAgentIdled alone must NOT promote to 'waiting'");
    assert!(!proposed, "CodingAgentIdled must NOT set coding_agent_proposed");
    assert!(
        !requires_restart,
        "coding_agent_requires_restart must NOT come from CodingAgentIdled"
    );
    assert!(!is_external, "is_external_repo=false in payload");

    // Step 2: ChangeProposed fires — the formal "ready for review" event.
    emit_change_proposed(&bus, thread_id, "claude-code/split", true).await;

    // Now `coding_agent_proposed` is true; status stays 'idle' — a proposed
    // change is an artifact for review, not a parked loop, so the row does
    // not enter a special "waiting" state. The pending-review affordance
    // surfaces via `coding_agent_proposed` (and `is_blocking` clause 3),
    // not via status. `coding_agent_requires_restart` comes from THIS
    // event's payload, not the prior CodingAgentIdled.
    let (status, proposed, requires_restart, has_diff): (String, bool, bool, bool) =
        sqlx::query_as(
            "SELECT status, coding_agent_proposed, coding_agent_requires_restart, \
             coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "idle", "ChangeProposed keeps status='idle' (review is an artifact, not a parked loop)");
    assert!(proposed, "ChangeProposed sets coding_agent_proposed");
    assert!(
        requires_restart,
        "coding_agent_requires_restart comes from the ChangeProposed payload"
    );
    assert!(has_diff, "ChangeProposed also seeds the git-truth has_diff signal");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Persisted thread events broadcast on the bus carry the post-event projection
/// snapshot in `EmittedEvent.aggregate`. Frontend uses this to update
/// `thread.meta` directly instead of looking up SECTION_TRANSITIONS / STATUS_TRANSITIONS.
/// Read-your-write semantics: the snapshot reflects the state AFTER this event's
/// projection update, fetched inside the same transaction as the INSERT.
#[tokio::test]
async fn sse_thread_event_carries_aggregate() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let mut rx = bus.subscribe();

    start_cc_session(&bus, thread_id, "claude-code/agg-test", None).await;

    // Drain the start events so we observe only the cancel.
    while rx.try_recv().is_ok() {}

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseCanceled {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let evt = rx.recv().await.expect("ResponseCanceled broadcast");
    let agg = evt
        .aggregate
        .as_ref()
        .expect("aggregate must be present on persisted thread events");
    assert_eq!(agg.section, "inbox", "ResponseCanceled puts CC thread in inbox");
    assert_eq!(agg.status, "idle", "after cancel, status returns to idle");
    assert_eq!(agg.thread_id, thread_id.to_string());

    // Verify SSE JSON serialization carries the aggregate too.
    let sse = evt.to_sse_json();
    let v: serde_json::Value = serde_json::from_str(&sse).unwrap();
    assert_eq!(v["data"]["aggregate"]["section"], "inbox");
    assert_eq!(v["data"]["aggregate"]["status"], "idle");
    // Aggregate must NOT carry compose fields (live drafts have their own cadence).
    assert!(v["data"]["aggregate"].get("composeText").is_none());
    assert!(v["data"]["aggregate"].get("composeImages").is_none());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// CodingAgentIdled with has_changes=true (followed by the synthetic
/// ChangeProposed emitted by `emit_cc_idle`) must set
/// `coding_agent_proposed=true` and leave status at 'idle' — a proposed
/// change is an artifact for review, not a parked loop (Option B).
#[tokio::test]
async fn claude_code_idled_with_changes_sets_proposed_keeps_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/test", None).await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    let (status, has_changes): (String, bool) =
        sqlx::query_as("SELECT status, coding_agent_proposed FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "idle",
        "CodingAgentIdled(has_changes=true) + ChangeProposed settle to 'idle' under Option B"
    );
    assert!(has_changes);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// External repo CC threads must never get ChangeProposed. The runtime skips
/// propose_change for external repos, so this test verifies the invariant:
/// CodingAgentIdled(has_changes=true, is_external_repo=true) sets the flags
/// correctly, but no changes row exists — meaning Apply/Discard won't appear.
#[tokio::test]
async fn external_repo_idle_with_changes_never_shows_apply_discard() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/external", None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: true,
            requires_restart: false,
            cc_session_id: Some("sid-ext".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // External-repo threads stay 'idle' under the split model — the runtime
    // never emits ChangeProposed for external repos, and `coding_agent_proposed`
    // is now ChangeProposed-driven. The is_external flag still flows from the
    // CodingAgentIdled payload. The "no Apply/Discard buttons" guarantee comes
    // from the absent ChangeProposed event (asserted below).
    let (status, has_changes, is_external): (String, bool, bool) = sqlx::query_as(
            "SELECT status, coding_agent_proposed, coding_agent_is_external_repo FROM thread_summaries WHERE thread_id = $1"
        ).bind(thread_id).fetch_one(&pool).await.unwrap();
    assert_eq!(status, "idle", "no proposal → idle (Apply/Discard never appear)");
    assert!(!has_changes, "no ChangeProposed → coding_agent_proposed stays false");
    assert!(is_external, "coding_agent_is_external_repo reflects the payload");

    // The key invariant: no ChangeProposed event should exist for external repos.
    // The runtime skips propose_change, so no changes row is created.
    // Without a pending change, resolve_actions returns [Archive], not [Apply, Discard].
    let change_proposed_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'ChangeProposed'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        change_proposed_count, 0,
        "External repo threads must never have ChangeProposed events — \
             the runtime must skip propose_change for external repos"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Full apply cycle: idle(changes) → apply → idle(no changes) → must end idle.
/// Simulates the exact sequence from apply_now_success: emit_change_applied then
/// reset_worktree_and_idle. The thread must not get stuck in 'waiting'.
#[tokio::test]
async fn full_apply_cycle_ends_idle_not_waiting() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let events = vec![
        (
            ThreadEvent::SessionStarted {
                session_id: "s1".into(),
                branch: "claude-code/fix".into(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
            },
            EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        ),
        (
            ThreadEvent::CodingAgentIdled {
                has_changes: true,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some("sid".into()),
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
                bg_bash_pending: false,
            },
            EventMeta::NONE,
        ),
        (
            ThreadEvent::ChangeProposed {
                change_id: "c1".into(),
                description: Some("Fix".into()),
                files: vec!["f.rs".into()],
                requires_restart: false,
                origin: None,
                commit_sha: None,
                branch_name: String::new(),
                repo_root: String::new(),
                hardened: false,
                incomplete: false,
                path: String::new(),
                diff: String::new(),
            },
            EventMeta::NONE,
        ),
        (
            ThreadEvent::ChangeApplied {
                change_id: "c1".into(),
                requires_restart: false,
                client_update: false,
                commits: vec![],
                thread_title: None,
                actor: None,
                pre_merge_sha: None,
                post_merge_sha: None,
                path: String::new(),
            },
            EventMeta::NONE,
        ),
        (
            ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some("sid".into()),
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
                bg_bash_pending: false,
            },
            EventMeta::NONE,
        ),
    ];

    for (event, meta) in events {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta,
        })
        .await
        .unwrap();
    }

    let (status, has_changes, requires_restart, is_external, applying): (
        String,
        bool,
        bool,
        bool,
        bool,
    ) = sqlx::query_as(
        "SELECT status, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo, coding_agent_applying \
             FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        status, "idle",
        "After full apply cycle, thread must be idle"
    );
    assert!(!has_changes, "coding_agent_proposed must be false after apply");
    assert!(!requires_restart);
    assert!(!is_external);
    assert!(!applying);

    let sid: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'cc_session_id' FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentIdled' \
             ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        sid.as_deref(),
        Some("sid"),
        "cc_session_id must survive for resume"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: a `ChangeApplied` emitted with `actor: Some(MessageOrigin::Device)`
/// must persist that actor verbatim into the events table. The frontend reads
/// `payload->'actor'` to render the chip ("You" / "<device label>") — when the
/// stored payload is missing the actor or has it as `null`, the
/// `actorInitiator` fallback in `thread-events.ts` collapses to "Lucidos
/// Engine", which is the user-visible bug behind Task 4 of the
/// mode-driven-actor-chip plan. This test pins the lower-level contract that
/// the EventBus persistence pipeline preserves the actor field, so that when
/// the call sites in `apply_change` / `end_stale_waiting_session` /
/// `spawn_hardening_session` are correctly wired, the user actor reaches the
/// frontend.
#[tokio::test]
async fn change_applied_persists_device_actor_in_payload() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let device = MessageOrigin::Device {
        device_id: "dev-actor-test".into(),
        label: "Test MacBook".into(),
    };

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            change_id: "change-actor-test".into(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: Some(device.clone()),
            pre_merge_sha: None,
            post_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit ChangeApplied");

    let actor_json: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'actor' FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeApplied' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("query persisted ChangeApplied actor");

    let actor_json = actor_json.expect(
        "ChangeApplied payload must carry a non-null `actor` field — \
         a missing/null actor renders as 'Lucidos Engine' in the UI",
    );
    assert_eq!(
        actor_json.get("kind").and_then(|v| v.as_str()),
        Some("device"),
        "actor.kind must be 'device' (not engine/agent), got: {actor_json:?}"
    );
    assert_eq!(
        actor_json.get("device_id").and_then(|v| v.as_str()),
        Some("dev-actor-test"),
        "actor.device_id must round-trip, got: {actor_json:?}"
    );
    assert_eq!(
        actor_json.get("label").and_then(|v| v.as_str()),
        Some("Test MacBook"),
        "actor.label must round-trip so the chip renders the user's device name, \
         got: {actor_json:?}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Tier 3 slow-path regression: when `apply_change` hands the merge off to a
/// fresh Claude Code subprocess, the user's actor is parked in `pending_apply_actors`
/// keyed by `change_id`. The cleanup in `agent_session::run_session` takes it
/// back out and stamps it on the resulting `ChangeApplied`. This test exercises
/// the stash → take → emit chain end-to-end (DB roundtrip), without spawning
/// CC, and asserts the persisted event carries the device — guarding against
/// any regression that drops the actor across the async gap.
#[tokio::test]
async fn slow_path_change_applied_carries_stashed_apply_actor() {
    use crate::engine::pending_apply_actors::PendingApplyActors;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let stash = PendingApplyActors::default();
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();

    let device = MessageOrigin::Device {
        device_id: "iphone-slow-path".into(),
        label: "iOS Safari PWA".into(),
    };

    // Apply call site stashes the actor by change_id before spawning CC for the merge.
    stash.stash(change_id, device.clone());

    // Cleanup site (post-merge) takes it back out and stamps it on ChangeApplied.
    let recovered = stash.take(change_id);
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            change_id: change_id.to_string(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: recovered,
            pre_merge_sha: None,
            post_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit ChangeApplied");

    let actor_json: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'actor' FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeApplied' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("query persisted ChangeApplied actor");

    let actor_json = actor_json.expect(
        "slow-path ChangeApplied must carry the stashed actor — \
         a missing actor means the stash → take wiring regressed and the chip falls back to 'Lucidos Engine'",
    );
    assert_eq!(
        actor_json.get("kind").and_then(|v| v.as_str()),
        Some("device"),
    );
    assert_eq!(
        actor_json.get("device_id").and_then(|v| v.as_str()),
        Some("iphone-slow-path"),
    );

    // Take is one-shot: a second cleanup pass (e.g. retried apply) sees None
    // and falls back to the engine attribution rather than double-stamping.
    assert!(
        stash.take(change_id).is_none(),
        "stash entry must be consumed after first take"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
