use super::super::*;
use super::*;

/// Regression: ChangeApplied followed by CodingAgentIdled(has_changes=false) must leave
/// the thread in 'idle'. Pre-Option-B, CodingAgentIdled unconditionally set
/// status='waiting', so the reset_worktree_and_idle emission after apply would override
/// the idle status from ChangeApplied — leaving the thread stuck on restart. Under
/// Option B every CC settle terminus is 'idle'; this regression is structurally
/// impossible but the test still exercises the apply → idle → re-idle path.
#[tokio::test]
async fn change_applied_then_idle_no_changes_stays_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/fix", None).await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    let (status, has_changes): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        status, "idle",
        "CodingAgentIdled+ChangeProposed settle to 'idle' (Option B)"
    );
    assert!(has_changes);

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: "c1".into(),
            description: Some("Fix bug".into()),
            files: vec!["src/main.rs".into()],
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
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
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
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (status, has_changes): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "idle", "ChangeApplied → idle");
    assert!(!has_changes, "ChangeApplied clears coding_agent_proposed");

    // reset_worktree_and_idle emits CodingAgentIdled { has_changes: false }
    // THIS IS THE REGRESSION SCENARIO: previously this set status back to 'waiting'
    emit_cc_idle(&bus, thread_id, false, Some("sid-1")).await;

    let (status, has_changes): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        status, "idle",
        "CodingAgentIdled(no changes) after ChangeApplied must stay idle"
    );
    assert!(!has_changes, "coding_agent_proposed stays false");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// After ChangeApplied, the thread stays 'inbox' so the Archive button appears.
/// CC flags are cleared, so resolveActions returns ['archive'] instead of ['apply','discard'].
/// A subsequent CodingAgentIdled(no changes) is idempotent — section stays 'inbox'.
#[tokio::test]
async fn change_applied_keeps_inbox_shows_archive() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/fix", None).await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    // After CodingAgentIdled(has_changes=true), section should be 'inbox'
    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(section, "inbox", "CodingAgentIdled with changes → inbox");

    // Propose and apply a change
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: "c1".into(),
            description: Some("Fix bug".into()),
            files: vec!["src/main.rs".into()],
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
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
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
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // ChangeApplied does NOT change section — thread stays 'inbox' for Archive button
    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "inbox",
        "ChangeApplied must NOT clear inbox — Archive button needs to appear"
    );

    // CC flags should be cleared (ClearAll)
    let coding_agent_proposed: bool = sqlx::query_scalar(
        "SELECT coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !coding_agent_proposed,
        "ChangeApplied must clear coding_agent_proposed"
    );

    // A subsequent CodingAgentIdled(no changes) keeps section as 'inbox' (idempotent)
    emit_cc_idle(&bus, thread_id, false, Some("sid-1")).await;

    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "inbox",
        "CodingAgentIdled(no changes) keeps section inbox"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// ChangeDiscarded also keeps the thread in inbox so Archive button appears.
/// Mirror of change_applied_keeps_inbox_shows_archive for the discard path.
#[tokio::test]
async fn change_discarded_keeps_inbox_shows_archive() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/fix", None).await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: "c1".into(),
            description: Some("Fix bug".into()),
            files: vec!["src/main.rs".into()],
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
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeDiscarded {
            change_id: "c1".into(),
            actor: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (section, coding_agent_proposed): (String, bool) = sqlx::query_as(
        "SELECT archive_state, coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        section, "inbox",
        "ChangeDiscarded must NOT clear inbox — Archive button needs to appear"
    );
    assert!(
        !coding_agent_proposed,
        "ChangeDiscarded must clear coding_agent_proposed"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Full Apply → Archive flow: Apply keeps thread in inbox, Archive moves to archived (ARCHIVE section).
#[tokio::test]
async fn apply_then_archive_moves_to_archive_section() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/fix", None).await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: "c1".into(),
            description: Some("Fix".into()),
            files: vec!["a.rs".into()],
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
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Apply
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
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
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "inbox",
        "After Apply: stays in inbox for Archive button"
    );

    // Archive
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (section, status): (String, String) =
        sqlx::query_as("SELECT archive_state, status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(section, "archived", "After Done: moved to ARCHIVE");
    assert_eq!(status, "idle", "After Done: status is idle");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Engine restart with an unresolved CodingAgentPermissionRequest in the
/// event log: `recover_orphan_cc_permission_requests` must emit a paired
/// CodingAgentPermissionResolved so the PermissionCard transitions out of
/// its pending state. Without this fix, the in-memory waiter for the dead
/// Claude Code subprocess is gone and clicking Allow/Deny in the UI 404s forever.
#[tokio::test]
async fn startup_resolves_orphan_permission_request() {
    use crate::engine::agent_recovery::recover_orphan_cc_permission_requests;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/orphan", None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-orphan".into(),
            tool_use_id: "tu-orphan".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({"file_path": "/tmp/x"}),
            summary: "Edit /tmp/x".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Pre-restart state: thread is held in waiting_for_user_answer, no Resolved persisted.
    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "waiting_for_user_answer",
        "PermissionRequest must put the thread in waiting_for_user_answer"
    );

    let resolved_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE event_type = 'CodingAgentPermissionResolved' \
           AND payload->>'request_id' = 'req-orphan'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(resolved_count, 0, "no Resolved exists pre-recovery");

    // Simulate engine restart recovery.
    recover_orphan_cc_permission_requests(&pool, &bus).await;

    // Post-recovery: a Resolved event was emitted, projection moved status off
    // waiting_for_user_answer (to 'running'; main.rs's running→idle reset
    // settles it from there in production, but that step is out of scope here).
    let resolved_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE event_type = 'CodingAgentPermissionResolved' \
           AND payload->>'request_id' = 'req-orphan'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        resolved_count, 1,
        "recovery must emit exactly one Resolved per orphan request"
    );

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "running",
        "Resolved projection must clear the waiting_for_user_answer status"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Already-resolved permission requests must NOT be re-resolved on startup.
/// Otherwise restart amplifies the Resolved log with duplicate events.
#[tokio::test]
async fn startup_skips_already_resolved_permission_requests() {
    use crate::engine::agent_recovery::recover_orphan_cc_permission_requests;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/already-resolved", None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-done".into(),
            tool_use_id: "tu-done".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({}),
            summary: "Edit".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionResolved {
            request_id: "req-done".into(),
            allowed: true,
            reason: None,
            persist_scope: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    recover_orphan_cc_permission_requests(&pool, &bus).await;

    let resolved_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE event_type = 'CodingAgentPermissionResolved' \
           AND payload->>'request_id' = 'req-done'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        resolved_count, 1,
        "recovery must not duplicate an already-resolved request"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// User types a new message while a CC permission card is pending:
/// `resolve_pending_permissions_as_superseded` must emit a paired
/// `CodingAgentPermissionResolved { allowed: false }` so the card's buttons
/// stop dangling, and fan a deny out to the in-memory waiter so the blocked
/// MCP handler returns. The projection then flips the thread off
/// `waiting_for_user_answer`.
#[tokio::test]
async fn typed_message_supersedes_pending_permission() {
    use crate::engine::cc_permission::{
        resolve_pending_permissions_as_superseded, PermissionEntry, PermissionState,
        SUPERSEDED_REASON,
    };
    use std::sync::Mutex;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/superseded", None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-super".into(),
            tool_use_id: "tu-super".into(),
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": "ls"}),
            summary: "Bash ls".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "waiting_for_user_answer");

    // Seed a live in-memory waiter for this request so we can assert the deny
    // fan-out reaches it (the still-blocked MCP handler).
    let pending = Mutex::new(PermissionState::default());
    let mut rx = {
        let mut state = pending.lock().unwrap();
        let (tx, rx) = tokio::sync::broadcast::channel(1);
        let key = (
            thread_id,
            "Bash".to_string(),
            "{\"command\":\"ls\"}".to_string(),
        );
        state.by_dedup_key.insert(
            key.clone(),
            PermissionEntry {
                thread_id,
                request_id: "req-super".into(),
                tool_name: "Bash".into(),
                input: serde_json::json!({"command": "ls"}),
                tx,
            },
        );
        state.by_request_id.insert("req-super".into(), key);
        rx
    };

    resolve_pending_permissions_as_superseded(&pool, &bus, &pending, thread_id, None).await;

    // The in-memory waiter received a deny.
    assert_eq!(
        rx.recv().await.ok(),
        Some(false),
        "blocked MCP handler must be unblocked with a deny"
    );

    // Exactly one Resolved emitted, carrying allowed=false + the superseded reason.
    let (resolved_count, allowed, reason): (i64, Option<bool>, Option<String>) = sqlx::query_as(
        "SELECT COUNT(*), \
                    bool_and((payload->>'allowed')::bool), \
                    max(payload->>'reason') \
             FROM events \
             WHERE event_type = 'CodingAgentPermissionResolved' \
               AND payload->>'request_id' = 'req-super'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        resolved_count, 1,
        "exactly one Resolved per superseded request"
    );
    assert_eq!(allowed, Some(false), "superseded resolution must be a deny");
    assert_eq!(reason.as_deref(), Some(SUPERSEDED_REASON));

    // Projection moved the thread off waiting_for_user_answer.
    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "running",
        "Resolved must clear waiting_for_user_answer"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// `resolve_pending_permissions_as_superseded` is a no-op when the thread has
/// no pending permission (already resolved, or never had one) — it must not
/// amplify the event log with spurious Resolved rows.
#[tokio::test]
async fn supersede_is_noop_when_no_pending_permission() {
    use crate::engine::cc_permission::{
        resolve_pending_permissions_as_superseded, PermissionState,
    };
    use std::sync::Mutex;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/noop", None).await;

    // A request that's ALREADY resolved (user clicked Allow).
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-resolved".into(),
            tool_use_id: "tu-resolved".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({}),
            summary: "Edit".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionResolved {
            request_id: "req-resolved".into(),
            allowed: true,
            reason: None,
            persist_scope: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let pending = Mutex::new(PermissionState::default());
    resolve_pending_permissions_as_superseded(&pool, &bus, &pending, thread_id, None).await;

    let resolved_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE event_type = 'CodingAgentPermissionResolved' \
           AND payload->>'request_id' = 'req-resolved'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        resolved_count, 1,
        "no extra Resolved for an already-resolved request"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Live permission answer: a `CodingAgentPermissionResolved` while the thread is
/// genuinely parked (`waiting_for_user_answer`) MUST resume it to `running` —
/// the in-memory MCP waiter is alive and about to continue. Guards against the
/// non-resurrecting fix over-reaching and breaking the normal Allow-click resume.
#[tokio::test]
async fn permission_resolution_resumes_waiting_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/live-resume", None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-live".into(),
            tool_use_id: "tu-live".into(),
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": "ls"}),
            summary: "Bash ls".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "waiting_for_user_answer",
        "request parks the thread"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionResolved {
            request_id: "req-live".into(),
            allowed: true,
            reason: None,
            persist_scope: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "running",
        "resolving a live (waiting) card must resume the session to running"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The zombie-`running` bug: a permission card left dangling by a cleanly-idled
/// session (e.g. a workflow whose parallel subagent's card outlived the main
/// turn), tapped later. The stale `CodingAgentPermissionResolved` must NOT
/// resurrect the idle thread into a dead `running` with no live session — it may
/// only flip to `running` from `waiting_for_user_answer`.
#[tokio::test]
async fn permission_resolution_does_not_resurrect_idle_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/zombie", None).await;

    // Card raised, then the session idles WITHOUT the card being answered.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-zombie".into(),
            tool_use_id: "tu-zombie".into(),
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": "sed -n '1,10p' x"}),
            summary: "Bash sed".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    emit_cc_idle(&bus, thread_id, false, None).await;

    let status_after_idle: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status_after_idle, "idle",
        "a no-changes idle after a pending card leaves the thread idle"
    );

    // Hours later the user taps the still-rendered card → a stale Resolved.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionResolved {
            request_id: "req-zombie".into(),
            allowed: true,
            reason: None,
            persist_scope: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let status_after_resolve: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status_after_resolve, "idle",
        "a stale permission resolution must NOT resurrect the idle thread to running"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// `resolve_pending_permissions_as_session_ended` (called from
/// `emit_coding_agent_idled`) must clear a dangling card at the turn boundary:
/// emit a paired `CodingAgentPermissionResolved { allowed: false }` with the
/// session-ended reason, fan a deny out to the still-blocked in-memory waiter,
/// and leave the thread `idle` (the non-resurrecting projection means clearing
/// the card can't zombie it).
#[tokio::test]
async fn idle_sweep_clears_pending_permission_without_resurrecting() {
    use crate::engine::cc_permission::{
        resolve_pending_permissions_as_session_ended, PermissionEntry, PermissionState,
        SESSION_ENDED_REASON,
    };
    use std::sync::Mutex;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/idle-sweep", None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-idle".into(),
            tool_use_id: "tu-idle".into(),
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": "sed x"}),
            summary: "Bash sed".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    emit_cc_idle(&bus, thread_id, false, None).await;

    // Seed a live in-memory waiter so we can assert the deny fan-out reaches the
    // still-blocked MCP handler (as it would for a dangling parallel-subagent
    // card whose subprocess hasn't torn down yet).
    let pending = Mutex::new(PermissionState::default());
    let mut rx = {
        let mut state = pending.lock().unwrap();
        let (tx, rx) = tokio::sync::broadcast::channel(1);
        let key = (
            thread_id,
            "Bash".to_string(),
            "{\"command\":\"sed x\"}".to_string(),
        );
        state.by_dedup_key.insert(
            key.clone(),
            PermissionEntry {
                thread_id,
                request_id: "req-idle".into(),
                tool_name: "Bash".into(),
                input: serde_json::json!({"command": "sed x"}),
                tx,
            },
        );
        state.by_request_id.insert("req-idle".into(), key);
        rx
    };

    resolve_pending_permissions_as_session_ended(&pool, &bus, &pending, thread_id, None).await;

    assert_eq!(
        rx.recv().await.ok(),
        Some(false),
        "the still-blocked MCP waiter must be unblocked with a deny"
    );

    let (resolved_count, allowed, reason): (i64, Option<bool>, Option<String>) = sqlx::query_as(
        "SELECT COUNT(*), \
                bool_and((payload->>'allowed')::bool), \
                max(payload->>'reason') \
         FROM events \
         WHERE event_type = 'CodingAgentPermissionResolved' \
           AND payload->>'request_id' = 'req-idle'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        resolved_count, 1,
        "exactly one Resolved for the dangling card"
    );
    assert_eq!(allowed, Some(false), "the idle sweep resolves as a deny");
    assert_eq!(reason.as_deref(), Some(SESSION_ENDED_REASON));

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "idle",
        "clearing the card at idle must leave the thread idle, not running"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Validates the legacy orphan-waiting startup sweep in main.rs:325. Since
/// Option B (`STATUS_FROM_PROPOSED_CHANGE = 'idle'`) the production path no
/// longer parks CC threads in `'waiting'` — this sweep is now exclusively a
/// safety net for historical rows from before the Option B migration. The
/// test forces both shapes of legacy row into 'waiting' and verifies the
/// sweep clears the no-proposal one while leaving the with-proposal one for
/// the migration to handle (the migration moves it to 'idle'; the legacy
/// sweep deliberately doesn't, because clearing the CC flags would lose
/// the pending change).
#[tokio::test]
async fn startup_resets_orphaned_waiting_threads_without_changes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    // Thread A: simulate a pre-Option-B legacy row stuck in 'waiting' with
    // no pending change (the bug the sweep was originally written to catch).
    let thread_a = Uuid::new_v4();
    start_cc_session(&bus, thread_a, "claude-code/a", None).await;
    emit_cc_idle(&bus, thread_a, false, None).await;
    sqlx::query("UPDATE thread_summaries SET status = 'waiting' WHERE thread_id = $1")
        .bind(thread_a)
        .execute(&pool)
        .await
        .unwrap();

    // Thread B: simulate a pre-Option-B legacy row with a real pending
    // change. Replay the production lifecycle (CodingAgentIdled +
    // ChangeProposed) — which under Option B settles to 'idle' — then force
    // it back to 'waiting' to mimic the historical shape.
    let thread_b = Uuid::new_v4();
    start_cc_session(&bus, thread_b, "claude-code/b", None).await;
    bus.emit(BusEvent::Thread {
        thread_id: thread_b,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: true,
            cc_session_id: None,
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
    emit_change_proposed(&bus, thread_b, "claude-code/b", true).await;
    sqlx::query("UPDATE thread_summaries SET status = 'waiting' WHERE thread_id = $1")
        .bind(thread_b)
        .execute(&pool)
        .await
        .unwrap();

    // Simulate engine restart: run the orphan-waiting cleanup query from main.rs.
    sqlx::query(
        "UPDATE thread_summaries SET status = 'idle', \
             coding_agent_proposed = FALSE, coding_agent_requires_restart = FALSE, \
             coding_agent_is_external_repo = FALSE, coding_agent_applying = FALSE \
             WHERE status = 'waiting' AND coding_agent_proposed = FALSE AND source = 'claude_code'",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Thread A: was stuck in waiting with no proposal — sweep clears it.
    let status_a: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_a)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status_a, "idle",
        "Startup must reset orphaned waiting thread to idle"
    );

    // Thread B: has a real pending change — the legacy sweep deliberately
    // skips it (clearing CC flags here would lose the proposal). The
    // Option B migration (20260519203328) catches this case separately by
    // flipping status='waiting' → 'idle' for any source='claude_code'
    // regardless of coding_agent_proposed.
    let (status_b, has_changes_b, requires_restart_b): (String, bool, bool) = sqlx::query_as(
            "SELECT status, coding_agent_proposed, coding_agent_requires_restart FROM thread_summaries WHERE thread_id = $1"
        ).bind(thread_b).fetch_one(&pool).await.unwrap();
    assert_eq!(
        status_b, "waiting",
        "Legacy sweep must NOT touch rows with coding_agent_proposed=true"
    );
    assert!(has_changes_b, "coding_agent_proposed preserved");
    assert!(
        requires_restart_b,
        "coding_agent_requires_restart preserved"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// ThreadArchived must clear all CC flags and set status to idle.
/// Previously ThreadArchived was a no-op, leaving archived threads stuck in waiting.
#[tokio::test]
async fn thread_archived_clears_cc_flags_and_goes_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/feat", None).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: true,
            requires_restart: true,
            cc_session_id: None,
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
    // Replay the production lifecycle: idle → propose keeps status='idle'
    // (a proposed change is an artifact, not a parked loop) and sets
    // coding_agent_proposed.
    emit_change_proposed(&bus, thread_id, "claude-code/feat", true).await;

    let (status, has_changes): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "idle");
    assert!(has_changes);

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

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
    assert_eq!(status, "idle", "ThreadArchived must set idle");
    assert!(
        !has_changes,
        "ThreadArchived must clear coding_agent_proposed"
    );
    assert!(
        !requires_restart,
        "ThreadArchived must clear coding_agent_requires_restart"
    );
    assert!(
        !is_external,
        "ThreadArchived must clear coding_agent_is_external_repo"
    );
    assert!(!applying, "ThreadArchived must clear coding_agent_applying");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn thread_archived_clears_is_saved() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/feat", None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadSaved,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let saved: bool =
        sqlx::query_scalar("SELECT is_saved FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(saved, "ThreadSaved must set is_saved=true");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (saved, archive_state): (bool, String) =
        sqlx::query_as("SELECT is_saved, archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        !saved,
        "ThreadArchived must clear is_saved so the row leaves the Saved section"
    );
    assert_eq!(
        archive_state, "archived",
        "ThreadArchived must set archive_state='archived' (the sole archive flag)"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// ThreadArchived must NOT zero the children counters. The counters are a
/// cache derived from real child events; the children still exist after
/// archive, and family-lift routing surfaces the parent again whenever any
/// child is still active. Zeroing makes the parent's disclosure chevron
/// disappear (it's gated on `totalChildrenCount > 0`), so the user sees the
/// nested children but can't collapse them.
#[tokio::test]
async fn thread_archived_preserves_children_counters() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, _child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    assert_children_counters(
        &pool,
        parent_id,
        1,
        1,
        "precondition: parent has 1 active/1 total child",
    )
    .await;

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_children_counters(
        &pool,
        parent_id,
        1,
        1,
        "ThreadArchived must preserve children counters — children still exist and remain active",
    )
    .await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Archive click invariant: ThreadArchived must emit LAST, after any trailing
/// CodingAgentIdled from CC cleanup, or the lifecycle side effect re-marks the
/// thread to inbox and the archive is silently undone.
#[tokio::test]
async fn cc_idled_then_archived_ends_in_default_section() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/dismiss-order", None).await;
    emit_cc_idle(&bus, thread_id, false, None).await;
    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "inbox",
        "CodingAgentIdled must surface CC thread to inbox"
    );

    emit_cc_idle(&bus, thread_id, false, None).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "archived",
        "ThreadArchived emitted LAST must leave section=default — Archive click would otherwise be silently undone by trailing CodingAgentIdled"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Bug-pinning counter-test: if this ever asserts `archived` instead of `inbox`,
/// the lifecycle stopped re-marking on CodingAgentIdled and archive_thread can
/// drop its ordering hack.
#[tokio::test]
async fn archived_then_cc_idled_undoes_archive() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/wrong-order", None).await;
    emit_cc_idle(&bus, thread_id, false, None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    emit_cc_idle(&bus, thread_id, false, None).await;

    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "inbox",
        "trailing CodingAgentIdled re-surfaces the thread to inbox — this is why archive_thread must end CC FIRST"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Projection invariant: a `ResponseCanceled` arriving AFTER `ThreadArchived`
/// flips the thread's section back to 'inbox', undoing the archive.
///
/// `archive_thread` itself no longer produces this sequence — `stop_agent` is
/// called with `StopReason::Archive` which sets `archiving=true` so the stop
/// arm suppresses `ResponseCanceled` (`ThreadArchived` is the terminator).
/// This test pins the projection rule for any OTHER caller that emits a
/// trailing `ResponseCanceled` (e.g. an in-flight cancel that races the
/// archive). The lifecycle rule (`ResponseCanceled → to_inbox`) intentionally
/// surfaces the thread so the user can act; if archive_thread regresses and
/// starts emitting a trailing `ResponseCanceled`, this test catches it.
#[tokio::test]
async fn archived_then_response_canceled_undoes_archive() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/stop-race", None).await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
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

    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        section, "inbox",
        "trailing ResponseCanceled re-surfaces the thread to inbox — archive_thread must await the cancel before emitting ThreadArchived"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
