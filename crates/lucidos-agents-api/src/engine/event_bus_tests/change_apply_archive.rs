use super::super::*;
use super::*;

#[tokio::test]
async fn change_applied_then_idle_no_changes_stays_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/fix", None).await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    let (status, has_changes): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
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
            content: Default::default(),
            change_id: test_change_id(thread_id).to_string(),
            description: Some("Fix bug".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            incomplete: false,
            set_aside: false,
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
            content: Default::default(),
            change_id: test_change_id(thread_id).to_string(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            landing_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (status, has_changes): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "idle", "ChangeApplied → idle");
    assert!(!has_changes, "ChangeApplied takes the thread off proposed");

    emit_cc_idle(&bus, thread_id, false, Some("sid-1")).await;

    let (status, has_changes): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        status, "idle",
        "CodingAgentIdled(no changes) after ChangeApplied must stay idle"
    );
    assert!(!has_changes, "the thread stays unproposed");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn change_applied_keeps_inbox_shows_archive() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/fix", None).await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    let section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(section, "inbox", "CodingAgentIdled with changes → inbox");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            content: Default::default(),
            change_id: test_change_id(thread_id).to_string(),
            description: Some("Fix bug".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            incomplete: false,
            set_aside: false,
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
            content: Default::default(),
            change_id: test_change_id(thread_id).to_string(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            landing_merge_sha: None,
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
        "ChangeApplied must NOT clear inbox — Archive button needs to appear"
    );

    let proposed: bool = sqlx::query_scalar(
        "SELECT coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!proposed, "ChangeApplied must take the thread off proposed");

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
            content: Default::default(),
            change_id: test_change_id(thread_id).to_string(),
            description: Some("Fix bug".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            incomplete: false,
            set_aside: false,
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
            change_id: test_change_id(thread_id).to_string(),
            actor: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (section, proposed): (String, bool) = sqlx::query_as(
        "SELECT archive_state, coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
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
        !proposed,
        "ChangeDiscarded must take the thread off proposed"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

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
            content: Default::default(),
            change_id: test_change_id(thread_id).to_string(),
            description: Some("Fix".into()),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            incomplete: false,
            set_aside: false,
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
            content: Default::default(),
            change_id: test_change_id(thread_id).to_string(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            landing_merge_sha: None,
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
            grant_offer: None,
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

    recover_orphan_cc_permission_requests(&pool, &bus).await;

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
            grant_offer: None,
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
            standing_deny: None,
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

#[tokio::test]
async fn typed_message_supersedes_pending_permission() {
    use crate::engine::cc_permission::{
        resolve_pending_permissions_as_superseded, PermissionAnswer, PermissionEntry,
        PermissionState, SUPERSEDED_REASON,
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
            grant_offer: None,
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
                grant_offer: Default::default(),
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

    assert_eq!(
        rx.recv().await.ok(),
        Some(PermissionAnswer::Withdrawn(SUPERSEDED_REASON)),
        "blocked MCP handler must be unblocked without reading as a user deny"
    );

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

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-resolved".into(),
            tool_use_id: "tu-resolved".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({}),
            summary: "Edit".into(),
            grant_offer: None,
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
            standing_deny: None,
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
            grant_offer: None,
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
            standing_deny: None,
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

#[tokio::test]
async fn permission_resolution_does_not_resurrect_idle_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/zombie", None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-zombie".into(),
            tool_use_id: "tu-zombie".into(),
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": "sed -n '1,10p' x"}),
            summary: "Bash sed".into(),
            grant_offer: None,
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

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentPermissionResolved {
            request_id: "req-zombie".into(),
            allowed: true,
            reason: None,
            persist_scope: None,
            standing_deny: None,
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

#[tokio::test]
async fn idle_sweep_clears_pending_permission_without_resurrecting() {
    use crate::engine::cc_permission::{
        resolve_pending_permissions_as_session_ended, PermissionAnswer, PermissionEntry,
        PermissionState, SESSION_ENDED_REASON,
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
            grant_offer: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    emit_cc_idle(&bus, thread_id, false, None).await;

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
                grant_offer: Default::default(),
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
        Some(PermissionAnswer::Withdrawn(SESSION_ENDED_REASON)),
        "the still-blocked MCP waiter must be unblocked without reading as a user deny"
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

#[tokio::test]
async fn startup_resets_orphaned_waiting_threads_without_changes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_a = Uuid::new_v4();
    start_cc_session(&bus, thread_a, "claude-code/a", None).await;
    emit_cc_idle(&bus, thread_a, false, None).await;
    sqlx::query("UPDATE thread_summaries SET status = 'waiting' WHERE thread_id = $1")
        .bind(thread_a)
        .execute(&pool)
        .await
        .unwrap();

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
            turn_end: None,
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

    sqlx::query(
        "UPDATE thread_summaries SET status = 'idle', coding_agent_is_external_repo = FALSE \
         WHERE status = 'waiting' AND coding_agent_change_state <> 'proposed' \
           AND source = 'claude_code'",
    )
    .execute(&pool)
    .await
    .unwrap();

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

    let (status_b, has_changes_b, requires_restart_b): (String, bool, bool) = sqlx::query_as(
            "SELECT status, coding_agent_change_state = 'proposed', coding_agent_requires_restart FROM thread_summaries WHERE thread_id = $1"
        ).bind(thread_b).fetch_one(&pool).await.unwrap();
    assert_eq!(
        status_b, "waiting",
        "Legacy sweep must NOT touch a proposed row"
    );
    assert!(has_changes_b, "the proposed state is preserved");
    assert!(
        requires_restart_b,
        "coding_agent_requires_restart preserved"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn thread_archived_goes_idle_and_the_proposal_follows_the_changes_row() {
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
            turn_end: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    emit_change_proposed(&bus, thread_id, "claude-code/feat", true).await;

    let (status, has_changes): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
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

    let (status, has_changes, requires_restart, is_external): (String, bool, bool, bool) =
        sqlx::query_as(
            "SELECT status, coding_agent_change_state = 'proposed', coding_agent_requires_restart, coding_agent_is_external_repo \
             FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "idle", "ThreadArchived must set idle");
    assert!(
        !is_external,
        "ThreadArchived must clear coding_agent_is_external_repo"
    );
    assert!(
        has_changes && requires_restart,
        "the change is still pending, so the proposal flags stay: they follow \
         the changes row, not the archive"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            content: Default::default(),
            change_id: test_change_id(thread_id).to_string(),
            requires_restart: true,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            landing_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    let (has_changes, requires_restart): (bool, bool) = sqlx::query_as(
        "SELECT coding_agent_change_state = 'proposed', coding_agent_requires_restart \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!has_changes && !requires_restart);

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
