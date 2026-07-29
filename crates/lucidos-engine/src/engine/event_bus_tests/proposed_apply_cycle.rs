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
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
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

/// Seed a real pending `changes` row by replaying the production CC lifecycle
/// (session → aggregate `ChangeProposed`) with a valid-UUID change_id so the
/// projection's `write_proposed_aggregate` actually inserts the row. The
/// shared `emit_change_proposed` helper builds a non-UUID id that
/// `parse_change_id` rejects, so it can't be used when a test needs a real
/// row to claim. Returns the change_id.
async fn seed_pending_change(bus: &EventBus, thread_id: Uuid, branch: &str) -> Uuid {
    start_cc_session(bus, thread_id, branch, None).await;
    let change_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: change_id.to_string(),
            description: Some("Fix".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: branch.into(),
            repo_root: "/tmp".into(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    change_id
}

fn change_applied_event(change_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChangeApplied {
        change_id: change_id.to_string(),
        requires_restart: false,
        client_update: false,
        commits: vec![],
        thread_title: None,
        actor: None,
        pre_merge_sha: None,
        post_merge_sha: None,
        path: String::new(),
    }
}

async fn count_change_applied(pool: &PgPool, change_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE event_type = 'ChangeApplied' AND payload->>'change_id' = $1",
    )
    .bind(change_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Bug: one logical apply must emit `ChangeApplied` at most once per change_id.
/// A sequential re-apply — an HTTP retry, the Apply-All driver re-firing, or a
/// post-restart recovery path re-emitting minutes later (the observed
/// 4-minute-gap, no-actor second emit) — used to persist a SECOND
/// `ChangeApplied`, rendering two "Change applied" timeline entries with no
/// input/output between them. The EventBus single-fire guard claims the
/// change row and suppresses the duplicate.
#[tokio::test]
async fn change_applied_emits_once_on_sequential_reapply() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = seed_pending_change(&bus, thread_id, "claude-code/once-seq").await;

    // First apply persists the event and flips the change row to applied.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: change_applied_event(change_id),
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    // Second apply (re-emit) of the same change_id must be suppressed.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: change_applied_event(change_id),
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(
        count_change_applied(&pool, change_id).await,
        1,
        "ChangeApplied must be persisted at most once per change_id — a sequential \
         re-apply must not produce a second 'Change applied' timeline entry"
    );

    let status: String = sqlx::query_scalar("SELECT status FROM changes WHERE id = $1")
        .bind(change_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "applied", "the change row is applied after the first emit");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Bug: two concurrent applies of one pending change (an Apply double-click, a
/// client retry, or the user racing the Apply-All driver) both read
/// status='pending' and both used to emit `ChangeApplied` (~0.6s apart, the
/// classic double-fire). The `FOR UPDATE` claim in the single-fire guard
/// serializes them so exactly one event persists.
#[tokio::test]
async fn change_applied_emits_once_under_concurrent_apply() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = seed_pending_change(&bus, thread_id, "claude-code/once-conc").await;

    let (r1, r2) = tokio::join!(
        bus.emit(BusEvent::Thread {
            thread_id,
            event: change_applied_event(change_id),
            meta: EventMeta::NONE,
        }),
        bus.emit(BusEvent::Thread {
            thread_id,
            event: change_applied_event(change_id),
            meta: EventMeta::NONE,
        }),
    );
    r1.unwrap();
    r2.unwrap();

    assert_eq!(
        count_change_applied(&pool, change_id).await,
        1,
        "concurrent applies of one change must collapse to exactly one ChangeApplied"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Guard must NOT suppress legitimate single emits whose change_id has no
/// `changes` row to claim (a throwaway/test id, or a row whose
/// `ChangeProposed` never landed — e.g. the external-repo archive carve-out
/// and the Tier-3 stash→take slow path both emit `ChangeApplied` for a
/// change with no aggregate row). These must still emit exactly once.
#[tokio::test]
async fn change_applied_without_changes_row_still_emits_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/no-row", None).await;

    // Valid UUID, but no ChangeProposed was emitted → no `changes` row exists.
    let change_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: change_applied_event(change_id),
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(
        count_change_applied(&pool, change_id).await,
        1,
        "a ChangeApplied with no changes row to dedup against must still emit once"
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

/// Propose-time reconcile (Invariants 1 & 2) at the projection level.
///
/// A coding-agent thread had a pending change on branch A (a merge-conflict
/// re-run left it orphaned). When `propose_change` proposes on a NEW branch B,
/// it enforces "≤1 pending change per thread" by discarding A's pending change
/// BEFORE emitting the new `ChangeProposed`. This models that exact sequence and
/// locks the two guarantees:
///   1. Only branch B's change is left pending — the orphan is gone, so the
///      frontend's `hasPendingChanges` no longer suppresses Archive.
///   2. `coding_agent_proposed` is TRUE afterwards — the discard's `ClearAll`
///      runs BEFORE the propose's `SetChanges`, so the flag reflects the new
///      change (the ordering the source guard below pins in `propose_change`).
///
/// This is the exact stuck state from thread `a4d52fd0` — two pending change
/// rows across branches, one applied, one orphaned pending — reduced to its
/// projection contract. See
/// docs/plans/2026-07-01-orphaned-pending-change-blocks-archive.md.
#[tokio::test]
async fn propose_time_reconcile_keeps_single_pending_and_proposed_flag() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    // Old branch A: a pending change (the orphan-to-be).
    let change_a = seed_pending_change(&bus, thread_id, "claude-code/old-A").await;

    // New branch B proposal. `propose_change` reconciles FIRST — (a) discard the
    // stale other-branch change …
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeDiscarded {
            change_id: change_a.to_string(),
            actor: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    // … (b) THEN emit the new branch's ChangeProposed.
    let change_b = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: change_b.to_string(),
            description: Some("Fix v2".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: "claude-code/new-B".into(),
            repo_root: "/tmp".into(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Invariant 1: exactly one pending change, on the new branch.
    let pending: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, branch_name FROM changes WHERE thread_id = $1 AND status = 'pending' \
         ORDER BY created_at",
    )
    .bind(thread_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        pending.len(),
        1,
        "only the new-branch change may remain pending — the orphan blocks Archive; got {:?}",
        pending
    );
    assert_eq!(pending[0].0, change_b, "the surviving pending change is branch B's");
    assert_eq!(pending[0].1, "claude-code/new-B");

    // The orphan is discarded (not lingering as pending).
    let a_status: String = sqlx::query_scalar("SELECT status FROM changes WHERE id = $1")
        .bind(change_a)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(a_status, "discarded", "the stale branch-A change must be discarded");

    // Invariant 2: coding_agent_proposed reflects the NEW change (discard-before-propose).
    let proposed: bool =
        sqlx::query_scalar("SELECT coding_agent_proposed FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        proposed,
        "coding_agent_proposed must be TRUE after a cross-branch propose — the sibling \
         discard's ClearAll must run BEFORE the new ChangeProposed's SetChanges"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Source-order guard for the propose-time reconcile ORDERING (Invariant 2).
///
/// `propose_change` cannot be exercised through a live `LucidosEngine` (nothing
/// in the crate builds one outside `main.rs`), so — as with the merge-helper
/// signature test — we pin the load-bearing property structurally: the sibling
/// reconcile call must appear BEFORE the `ChangeProposed` emit. If a refactor
/// reorders them, `ChangeDiscarded`'s `ClearAll` would wipe the
/// `coding_agent_proposed` flag the proposal just set, re-introducing the
/// "applied but no Apply button / no Archive" class of bug.
#[test]
fn propose_change_reconciles_stale_branches_before_change_proposed_emit() {
    let src = include_str!("../change_ops/propose.rs");
    let discard_pos = src
        .find("discard_pending_for_thread_except")
        .expect("propose_change must reconcile stale other-branch pending changes");
    let proposed_pos = src
        .find("ThreadEvent::ChangeProposed")
        .expect("propose_change must still emit ChangeProposed");
    assert!(
        discard_pos < proposed_pos,
        "the sibling reconcile must run BEFORE the ChangeProposed emit — otherwise \
         ChangeDiscarded's ClearAll wipes the coding_agent_proposed flag this proposal sets. \
         See docs/plans/2026-07-01-orphaned-pending-change-blocks-archive.md"
    );
}

/// Guard where the apply-time net (`discard_orphaned_pending_siblings`) lives.
///
/// It belongs inside `apply_change` — gated on a real `ApplyStatus::Applied`
/// transition — so every caller (HTTP handler, the no-live `apply_now` fast /
/// stale paths, the Apply-All driver, the post-hardening re-entry) reconciles
/// uniformly and correctly. It must ALSO live in `apply_now_success` (the live
/// in-place merge path bypasses `apply_change`). It must NOT be scattered into
/// the HTTP handler (an ungated call there discarded a newer sibling on a `Noop`
/// re-apply — data loss) and must NOT be wired into the shared
/// `emit_change_applied` (the external-repo archive loop calls that per pending
/// change, so reconciling there would double-terminate siblings).
#[test]
fn apply_time_reconcile_lives_in_apply_change_gated_not_in_handler_or_emitter() {
    let apply = include_str!("../change_ops/apply.rs");
    assert!(
        apply.contains("discard_orphaned_pending_siblings"),
        "apply_change must reconcile orphaned sibling pending changes on a successful apply"
    );
    assert!(
        apply.contains("ApplyStatus::Applied"),
        "the apply-time reconcile must be gated on ApplyStatus::Applied — reconciling on \
         Noop/Hardening/Conflict discards newer sibling work (data loss)"
    );
    let apply_now = include_str!("../agent_session/apply_now.rs");
    assert!(
        apply_now.contains("discard_orphaned_pending_siblings"),
        "apply_now_success (live in-place merge) must reconcile orphaned siblings — it bypasses apply_change"
    );
    let http = include_str!("../../api/changes.rs");
    assert!(
        !http.contains("discard_orphaned_pending_siblings"),
        "the HTTP apply_change handler must NOT reconcile directly — apply_change does it, gated on Applied"
    );
    let emitters = include_str!("../change_ops_emitters.rs");
    assert!(
        !emitters.contains("discard_orphaned_pending_siblings"),
        "emit_change_applied must NOT reconcile siblings — it is shared with the external-repo \
         archive loop, which applies multiple pending changes per thread in sequence"
    );
}

/// Guard where the apply-time dev refresh (background engine rebuild /
/// served-`dist` re-snapshot) lives.
///
/// The mirror image of the reconcile guard above: this one must live in EXACTLY
/// ONE place — hanging off the shared `emit_change_applied`, the single emit
/// every merge path performs exactly once. It used to sit on the `apply_change`
/// wrapper instead, which three merge paths bypass (`apply_now_success` for the
/// live in-place merge, the async in-place conflict recovery, and the Tier-2/3
/// conflict-resolution cleanup in `run_session/completion.rs`) — so an
/// engine-affecting Apply from a live coding-agent session merged with no
/// rebuild started, no building spinner, and a "Switch to new version" offering
/// a binary older than the merge.
///
/// A SECOND trigger site is not merely redundant, it re-breaks the feature:
/// `trigger_background_rebuild` coalesces by aborting the in-flight build, and
/// the aborted task's `flock` guard may not have dropped before the replacement
/// probes it — the replacement then reads `SkippedLocked`, falls back to
/// `BuildState::Idle`, and no build runs at all.
#[test]
fn apply_time_dev_refresh_lives_only_on_the_shared_change_applied_emit() {
    let emitters = include_str!("../change_ops_emitters.rs");
    assert!(
        emitters.contains("post_apply_dev_refresh"),
        "emit_change_applied must drive the post-apply dev refresh — it is the one emit \
         every merge path performs exactly once"
    );
    for (label, src) in [
        ("change_ops/apply.rs", include_str!("../change_ops/apply.rs")),
        (
            "agent_session/apply_now.rs",
            include_str!("../agent_session/apply_now.rs"),
        ),
        (
            "agent_session/run_session/completion.rs",
            include_str!("../agent_session/run_session/completion.rs"),
        ),
        ("api/changes.rs", include_str!("../../api/changes.rs")),
    ] {
        assert!(
            !src.contains("trigger_background_rebuild()")
                && !src.contains("refresh_served_frontend_after_rebuild()"),
            "{label} must NOT trigger the rebuild / frontend re-snapshot directly — \
             emit_change_applied owns it. A second trigger site can abort the first \
             build into a SkippedLocked no-op, leaving no build running at all."
        );
    }
}

/// Guard that the post-apply dev refresh runs only for the ACCEPTED
/// `ChangeApplied`, never for a duplicate the bus suppressed.
///
/// The bus's single-fire guard `FOR UPDATE`-locks the change row and returns
/// `Ok(None)` for a recognized duplicate — which is a routine occurrence, not a
/// pathology: the ~0.6s double-fire race, Apply-All retries, the
/// conflict-recovery cleanup, and post-restart re-emits all hit it. `emit_or_log`
/// throws that verdict away, so emitting through it and then refreshing
/// unconditionally would let the DUPLICATE coalesce (abort) the rebuild the
/// ACCEPTED emit just started — and the replacement can read `SkippedLocked` and
/// fall back to `BuildState::Idle`, leaving no build running. That is the very
/// symptom this funnel was added to fix, re-entered through another door.
#[test]
fn post_apply_refresh_runs_only_for_the_accepted_change_applied() {
    let emitters = include_str!("../change_ops_emitters.rs");
    let emit_pos = emitters
        .find("ThreadEvent::ChangeApplied {")
        .expect("emit_change_applied must emit ChangeApplied");
    let tail = &emitters[emit_pos..];
    assert!(
        tail.contains("Ok(res) => res.is_some()"),
        "emit_change_applied must emit via `emit` (not `emit_or_log`) and read the \
         Ok(Some)/Ok(None) verdict — `emit_or_log` discards the single-fire guard's \
         suppression, so a duplicate would still fire the refresh"
    );
    let accepted_pos = tail
        .find("if accepted {")
        .expect("the post-apply refresh must be gated on the emit being accepted");
    let refresh_pos = tail
        .find("post_apply_dev_refresh(")
        .expect("emit_change_applied must call the post-apply refresh");
    assert!(
        accepted_pos < refresh_pos,
        "the `accepted` gate must wrap the post_apply_dev_refresh call, not follow it"
    );
}

/// Guard that `discard_change` feeds the Apply-All driver a terminal signal.
///
/// When the sibling reconcile (or a concurrent user discard) drops a change that
/// is a live Apply-All batch member, the driver must be told so the batch
/// advances. Otherwise it spawns `apply_change` on the now-`discarded` row, which
/// returns `Err` with no terminal event — the batch never completes and the
/// "Applying changes…" toast sticks forever. Symmetric to `emit_change_applied`'s
/// `notify_apply_all(Applied …)` (a no-op for non-members).
#[test]
fn discard_change_notifies_apply_all_driver_so_batch_advances() {
    let discard = include_str!("../change_ops/discard.rs");
    assert!(
        discard.contains("notify_apply_all") && discard.contains("DISCARDED_MEMBER_REASON"),
        "discard_change must notify the Apply-All driver (Failed / DISCARDED_MEMBER_REASON) so a \
         batch member discarded mid-batch advances the batch instead of stalling it"
    );
}
