use super::{
    classify_family, coding_agent_members, every_member, external_repo_pending, load_family,
    not_yet_archived, FamilyDecision, FamilyRow, FamilyVerb, HOME_THREAD,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::engine::thread_lifecycle::ChangeStateKind;
use crate::test_support::{setup_test_db, teardown_test_db};
use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

fn row(
    id: Uuid,
    is_coding_agent: bool,
    status: &str,
    archive_state: &str,
    proposed: bool,
    coding_agent_is_external_repo: bool,
) -> FamilyRow {
    FamilyRow {
        thread_id: id,
        is_coding_agent,
        status: status.to_string(),
        archive_state: archive_state.to_string(),
        coding_agent_change_state: if proposed {
            ChangeStateKind::Proposed
        } else {
            ChangeStateKind::None
        },
        coding_agent_is_external_repo,
        holds_live_wait: false,
        is_saved: false,
        is_home: false,
    }
}

fn cc_holding_work(id: Uuid) -> FamilyRow {
    FamilyRow {
        coding_agent_change_state: ChangeStateKind::Unproposed,
        holds_live_wait: true,
        ..row(id, true, "idle", "inbox", false, false)
    }
}

fn idle_chat(id: Uuid) -> FamilyRow {
    row(id, false, "idle", "inbox", false, false)
}

fn running_chat(id: Uuid) -> FamilyRow {
    row(id, false, "running", "inbox", false, false)
}

fn idle_cc(id: Uuid) -> FamilyRow {
    row(id, true, "idle", "inbox", false, false)
}

fn cc_with_pending(id: Uuid) -> FamilyRow {
    row(id, true, "waiting", "inbox", true, false)
}

fn archived(id: Uuid) -> FamilyRow {
    row(id, false, "idle", "archived", false, false)
}

async fn spawn_idle_parent(bus: &EventBus) -> Uuid {
    let parent_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "do something".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::ResponseGenerated {
            text: "ok".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    parent_id
}

async fn settle_after_child_completion(bus: &EventBus, pool: &PgPool, parent_id: Uuid) {
    let is_cc: bool =
        sqlx::query_scalar("SELECT is_coding_agent FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(pool)
            .await
            .unwrap();
    let event = if is_cc {
        ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some(format!("test-session-{}", parent_id)),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        }
    } else {
        ThreadEvent::ResponseGenerated {
            text: "reacted to the child".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        }
    };
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn spawn_child(bus: &EventBus, pool: &PgPool, parent_id: Uuid, bring_to_idle: bool) -> Uuid {
    let child_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "child task".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: format!("test-session-{}", child_id),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    if bring_to_idle {
        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event: ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some(format!("test-session-{}", child_id)),
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
        settle_after_child_completion(bus, pool, parent_id).await;
    }
    child_id
}

async fn spawn_cc_child(
    bus: &EventBus,
    pool: &PgPool,
    parent_id: Uuid,
    with_pending_changes: bool,
) -> Uuid {
    let child_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "cc task".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: with_pending_changes,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("test-session".into()),
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
    if with_pending_changes {
        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event: ThreadEvent::ChangeProposed {
                content: Default::default(),
                change_id: Uuid::new_v4().to_string(),
                description: Some("Test change".into()),
                files: vec!["test.rs".into()],
                requires_restart: false,
                origin: None,
                commit_sha: None,
                branch_name: format!("claude-code/test-{child_id}"),
                repo_root: "/tmp".into(),
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
    }
    settle_after_child_completion(bus, pool, parent_id).await;
    child_id
}

async fn spawn_idle_chat_child(bus: &EventBus, pool: &PgPool, parent_id: Uuid) -> Uuid {
    let child_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "delegated task".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseGenerated {
            text: "done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    settle_after_child_completion(bus, pool, parent_id).await;
    child_id
}

async fn archive_via_event(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn locked_family(pool: &PgPool, thread_uuid: Uuid) -> Vec<FamilyRow> {
    let mut tx = pool.begin().await.unwrap();
    let family = load_family(&mut tx, thread_uuid).await.unwrap();
    tx.commit().await.unwrap();
    family
}

fn expect_proceed(family: &[FamilyRow], target: Uuid, why: &str) -> (Vec<Uuid>, Vec<Uuid>) {
    match classify_family(family, target, FamilyVerb::Archive) {
        FamilyDecision::Proceed => (not_yet_archived(family), external_repo_pending(family)),
        FamilyDecision::Reject { status, body } => panic!("{why}; got {status} {body}"),
    }
}

fn expect_reject(
    family: &[FamilyRow],
    target: Uuid,
    verb: FamilyVerb,
    why: &str,
) -> (StatusCode, serde_json::Value) {
    match classify_family(family, target, verb) {
        FamilyDecision::Reject { status, body } => (status, body),
        FamilyDecision::Proceed => panic!("{why}"),
    }
}

#[tokio::test]
async fn load_family_locks_every_member_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;
    let child_id = spawn_child(&bus, &pool, parent_id, true).await;

    let mut holder = pool.begin().await.unwrap();
    let family = load_family(&mut holder, parent_id).await.unwrap();
    assert_eq!(
        every_member(&family),
        vec![parent_id, child_id],
        "the snapshot lists the parent first"
    );

    let mut writer = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '200ms'")
        .execute(&mut *writer)
        .await
        .unwrap();
    let update = "UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1";
    let blocked = sqlx::query(update)
        .bind(child_id)
        .execute(&mut *writer)
        .await;
    assert!(
        blocked.is_err(),
        "a write to a locked descendant must wait for the snapshot's transaction"
    );
    writer.rollback().await.unwrap();
    holder.rollback().await.unwrap();

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn archive_with_no_descendants_archives_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;
    let family = locked_family(&pool, parent_id).await;

    let (to_archive, external_repo_pending) = expect_proceed(
        &family,
        parent_id,
        "expected Proceed for an idle childless parent",
    );
    assert_eq!(to_archive, vec![parent_id]);
    assert!(external_repo_pending.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn archive_cascade_carries_an_idle_chat_child() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;
    let child_id = spawn_idle_chat_child(&bus, &pool, parent_id).await;

    let child_section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(child_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        child_section, "inbox",
        "a finished chat child keeps the inbox state it ran with",
    );

    let family = locked_family(&pool, parent_id).await;
    let (to_archive, _) = expect_proceed(
        &family,
        parent_id,
        "an idle chat child must not block its parent's archive",
    );
    assert!(
        to_archive.contains(&child_id),
        "the cascade must carry the child, or it is stranded in the inbox",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn archive_with_idle_descendants_archives_all() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;
    let child_a = spawn_child(&bus, &pool, parent_id, true).await;
    let child_b = spawn_child(&bus, &pool, parent_id, true).await;
    let grandchild = spawn_child(&bus, &pool, child_a, true).await;

    let family = locked_family(&pool, parent_id).await;
    assert_eq!(family.len(), 4, "parent + 2 children + 1 grandchild");

    let (to_archive, external_repo_pending) = expect_proceed(
        &family,
        parent_id,
        "expected Proceed when every descendant is idle",
    );
    assert_eq!(
        to_archive.len(),
        4,
        "every member must be in to_archive: {:?}",
        to_archive
    );
    for id in [parent_id, child_a, child_b, grandchild] {
        assert!(to_archive.contains(&id), "{} missing from to_archive", id);
    }
    assert!(external_repo_pending.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn archive_rejects_when_descendant_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;
    let running_child = spawn_child(&bus, &pool, parent_id, false).await;

    let family = locked_family(&pool, parent_id).await;
    let (status, body) = expect_reject(
        &family,
        parent_id,
        FamilyVerb::Archive,
        "expected Reject when a descendant is running",
    );
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason"], "descendants_blocking");
    let blocking = body["blocking"].as_array().unwrap();
    assert_eq!(blocking.len(), 1);
    assert_eq!(blocking[0]["thread_id"], running_child.to_string());
    assert_eq!(blocking[0]["status"], "running");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn archive_rejects_when_descendant_has_pending_changes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;
    let cc_child = spawn_cc_child(&bus, &pool, parent_id, true).await;

    let family = locked_family(&pool, parent_id).await;
    let (status, body) = expect_reject(
        &family,
        parent_id,
        FamilyVerb::Archive,
        "expected Reject when a CC descendant has pending changes",
    );
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason"], "descendants_blocking");
    let blocking = body["blocking"].as_array().unwrap();
    assert_eq!(blocking.len(), 1);
    assert_eq!(blocking[0]["thread_id"], cc_child.to_string());
    assert_eq!(blocking[0]["has_pending_changes"], true);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn archive_rejects_when_parent_running() {
    let parent_id = Uuid::new_v4();
    let family = vec![running_chat(parent_id)];
    let (status, body) = expect_reject(
        &family,
        parent_id,
        FamilyVerb::Archive,
        "expected Reject when parent is running",
    );
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason"], "parent_not_archivable");
    assert_eq!(body["parent_status"], "running");
}

#[tokio::test]
async fn archive_already_archived_parent_is_idempotent() {
    let parent_id = Uuid::new_v4();
    let family = vec![archived(parent_id)];
    let (to_archive, external_repo_pending) = expect_proceed(
        &family,
        parent_id,
        "expected Proceed (idempotent) when the parent is already archived",
    );
    assert!(
        to_archive.is_empty(),
        "an already-archived parent has nothing to re-emit: {:?}",
        to_archive
    );
    assert!(external_repo_pending.is_empty());
}

#[tokio::test]
async fn archive_already_archived_parent_archives_resurfaced_descendant() {
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    let family = vec![archived(parent_id), idle_cc(child_id)];
    let (to_archive, external_repo_pending) = expect_proceed(
        &family,
        parent_id,
        "expected Proceed with the resurfaced descendant",
    );
    assert_eq!(to_archive, vec![child_id]);
    assert!(external_repo_pending.is_empty());
}

#[tokio::test]
async fn archive_rejects_parent_cc_with_pending_changes() {
    let parent_id = Uuid::new_v4();
    let family = vec![cc_with_pending(parent_id)];
    let (status, body) = expect_reject(
        &family,
        parent_id,
        FamilyVerb::Archive,
        "expected Reject when parent CC has its own pending changes",
    );
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason"], "parent_has_pending_changes");
}

#[tokio::test]
async fn archive_allows_parent_external_repo_cc_with_pending_changes() {
    let parent_id = Uuid::new_v4();
    let mut row = cc_with_pending(parent_id);
    row.coding_agent_is_external_repo = true;
    let family = vec![row];
    let (to_archive, external_repo_pending) = expect_proceed(
        &family,
        parent_id,
        "expected Proceed when parent CC is external-repo with pending changes",
    );
    assert_eq!(to_archive, vec![parent_id]);
    assert_eq!(external_repo_pending, vec![parent_id]);
}

#[tokio::test]
async fn archive_skips_already_archived_descendants() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;
    let live_child = spawn_child(&bus, &pool, parent_id, true).await;
    let archived_child = spawn_child(&bus, &pool, parent_id, true).await;
    archive_via_event(&bus, archived_child).await;

    let family = locked_family(&pool, parent_id).await;
    assert_eq!(family.len(), 3);

    let (to_archive, external_repo_pending) = expect_proceed(
        &family,
        parent_id,
        "expected Proceed when only an already-archived descendant exists",
    );
    assert!(external_repo_pending.is_empty());
    assert_eq!(
        to_archive.len(),
        2,
        "already-archived rows must be excluded: {:?}",
        to_archive
    );
    assert!(to_archive.contains(&parent_id));
    assert!(to_archive.contains(&live_child));
    assert!(
        !to_archive.contains(&archived_child),
        "already-archived child must not be re-emitted"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn archive_target_not_found_returns_404() {
    let missing = Uuid::new_v4();
    let family: Vec<FamilyRow> = vec![];
    let (status, body) = expect_reject(
        &family,
        missing,
        FamilyVerb::Archive,
        "expected Reject for missing target",
    );
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["reason"], "thread_not_found");
}

#[tokio::test]
async fn idle_descendants_alongside_archived_and_self() {
    let target = Uuid::new_v4();
    let child = Uuid::new_v4();
    let gc = Uuid::new_v4();
    let family = vec![idle_chat(target), idle_chat(child), archived(gc)];
    let (to_archive, external_repo_pending) = expect_proceed(&family, target, "expected Proceed");
    assert!(external_repo_pending.is_empty());
    assert_eq!(to_archive.len(), 2);
    assert!(to_archive.contains(&target));
    assert!(to_archive.contains(&child));
    assert!(!to_archive.contains(&gc));
}

#[tokio::test]
async fn orphaned_question_does_not_block_cascade_and_is_lookup_visible() {
    use crate::engine::agent_question::lookup_pending_question_tool_use_id;
    use crate::engine::thread_events::QuestionOption;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;

    let child_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "cc task".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-orphan".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    let orphan_tool_use_id = "tool-use-orphan-123";
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: orphan_tool_use_id.into(),
            cc_session_id: "sid-orphan".into(),
            question: "What now?".into(),
            options: vec![QuestionOption {
                id: "opt-0".into(),
                label: "A".into(),
                description: None,
                preview: None,
                widget: None,
                quiet: false,
                quiet_reply: None,
            }],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            permission_request: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("sid-orphan".into()),
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
    settle_after_child_completion(&bus, &pool, parent_id).await;

    let family = locked_family(&pool, parent_id).await;
    assert_eq!(family.len(), 2, "parent + orphaned-question child");
    let (to_archive, external_repo_pending) = expect_proceed(
        &family,
        parent_id,
        "cascade must admit the orphaned-question child, otherwise the helper would never run",
    );
    assert!(external_repo_pending.is_empty());
    assert_eq!(to_archive.len(), 2);
    assert!(to_archive.contains(&parent_id));
    assert!(to_archive.contains(&child_id));

    let found = lookup_pending_question_tool_use_id(&pool, child_id).await;
    assert_eq!(
        found.as_deref(),
        Some(orphan_tool_use_id),
        "lookup must surface the orphaned question so the archive handler has something to cancel-stamp"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn archive_rejects_when_fresh_cc_child_is_running_without_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;
    let fresh_child = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: fresh_child,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "fresh cc child".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: fresh_child,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "fresh-session".into(),
            branch: "claude-code/fresh".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let family = locked_family(&pool, parent_id).await;
    let (status, body) = expect_reject(
        &family,
        parent_id,
        FamilyVerb::Archive,
        "cascade must reject when a fresh CC child is actively running",
    );
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason"], "descendants_blocking");
    let blocking = body["blocking"].as_array().unwrap();
    assert_eq!(blocking.len(), 1);
    assert_eq!(blocking[0]["thread_id"], fresh_child.to_string());
    assert_eq!(blocking[0]["status"], "running");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn archive_rejects_legacy_running_archived_descendant() {
    let parent_id = Uuid::new_v4();
    let legacy_child = Uuid::new_v4();
    let family = vec![
        idle_chat(parent_id),
        row(legacy_child, true, "running", "archived", false, false),
    ];
    let (status, body) = expect_reject(
        &family,
        parent_id,
        FamilyVerb::Archive,
        "cascade must reject a legacy Running+Archived descendant",
    );
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason"], "descendants_blocking");
    let blocking = body["blocking"].as_array().unwrap();
    assert_eq!(blocking.len(), 1);
    assert_eq!(blocking[0]["thread_id"], legacy_child.to_string());
}

#[tokio::test]
async fn external_repo_cc_with_pending_changes_does_not_block() {
    let target = Uuid::new_v4();
    let ext_cc = Uuid::new_v4();
    let mut ext_row = idle_cc(ext_cc);
    ext_row.coding_agent_is_external_repo = true;
    ext_row.coding_agent_change_state = ChangeStateKind::Proposed;
    ext_row.status = "waiting".into();
    let family = vec![idle_chat(target), ext_row];

    let (to_archive, external_repo_pending) = expect_proceed(
        &family,
        target,
        "expected Proceed: external-repo CC with pending changes is exempt",
    );
    assert_eq!(to_archive.len(), 2);
    assert_eq!(external_repo_pending, vec![ext_cc]);
}

#[test]
fn both_verbs_refuse_a_parent_waiting_on_an_answer() {
    let target = Uuid::new_v4();
    for parked in [idle_chat(target), idle_cc(target)] {
        let family = vec![FamilyRow {
            status: "waiting_for_user_answer".into(),
            ..parked
        }];
        for (verb, reason) in [
            (FamilyVerb::Archive, "parent_not_archivable"),
            (FamilyVerb::Delete, "parent_not_deletable"),
        ] {
            let (status, body) = expect_reject(
                &family,
                target,
                verb,
                "a parent parked on a question must be refused",
            );
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(body["reason"], reason);
            assert_eq!(body["parent_status"], "waiting_for_user_answer");
        }
    }
}

#[test]
fn archive_and_delete_refuse_the_same_family() {
    let target = Uuid::new_v4();
    let running_child = Uuid::new_v4();
    let pending_child = Uuid::new_v4();

    let mut running_parent = idle_chat(target);
    running_parent.status = "running".into();

    let families: Vec<(&str, Vec<FamilyRow>)> = vec![
        (
            "a running descendant",
            vec![idle_chat(target), running_chat(running_child)],
        ),
        (
            "a descendant holding a pending change",
            vec![idle_chat(target), cc_with_pending(pending_child)],
        ),
        (
            "the parent holding a pending change",
            vec![cc_with_pending(target)],
        ),
    ];

    for (what, family) in families {
        let (archive_status, archive_body) =
            expect_reject(&family, target, FamilyVerb::Archive, what);
        let (delete_status, delete_body) = expect_reject(&family, target, FamilyVerb::Delete, what);
        assert_eq!(
            archive_status, delete_status,
            "the two verbs disagreed on the status for {what}"
        );
        assert_eq!(
            archive_body["reason"], delete_body["reason"],
            "the two verbs disagreed on the reason for {what}"
        );
    }

    let running_family = vec![running_parent];
    let (_, archive_body) = expect_reject(
        &running_family,
        target,
        FamilyVerb::Archive,
        "a running parent",
    );
    let (_, delete_body) = expect_reject(
        &running_family,
        target,
        FamilyVerb::Delete,
        "a running parent",
    );
    assert_eq!(archive_body["reason"], "parent_not_archivable");
    assert_eq!(delete_body["reason"], "parent_not_deletable");
}

#[test]
fn delete_takes_every_member_including_the_already_archived() {
    let target = Uuid::new_v4();
    let archived_child = Uuid::new_v4();
    let cc_child = Uuid::new_v4();
    let family = vec![
        idle_chat(target),
        archived(archived_child),
        idle_cc(cc_child),
    ];

    assert_eq!(
        every_member(&family),
        vec![target, archived_child, cc_child],
        "delete must take the archived member too"
    );
    assert_eq!(
        not_yet_archived(&family),
        vec![target, cc_child],
        "archive still skips it, which is why the two subsets are separate"
    );
    assert_eq!(coding_agent_members(&family), vec![cc_child]);
}

#[test]
fn both_verbs_refuse_the_home_thread() {
    let home = Uuid::new_v4();
    let family = vec![FamilyRow {
        is_home: true,
        ..idle_chat(home)
    }];
    for verb in [FamilyVerb::Archive, FamilyVerb::Delete] {
        let (status, body) = expect_reject(&family, home, verb, "the home thread must be refused");
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["reason"], HOME_THREAD);
    }
}

#[test]
fn a_cascade_refuses_a_family_holding_the_home_thread() {
    let target = Uuid::new_v4();
    let home = Uuid::new_v4();
    let family = vec![
        idle_chat(target),
        FamilyRow {
            is_home: true,
            ..idle_chat(home)
        },
    ];
    for verb in [FamilyVerb::Archive, FamilyVerb::Delete] {
        let (_, body) = expect_reject(&family, target, verb, "a home member must refuse");
        assert_eq!(body["reason"], HOME_THREAD);
    }
}

#[tokio::test]
async fn the_locked_snapshot_carries_the_home_marker() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let home = crate::engine::home_thread::ensure_home_thread(&bus, &pool)
        .await
        .unwrap();
    let other = spawn_idle_parent(&bus).await;

    let family = locked_family(&pool, home).await;
    assert!(family[0].is_home);
    let (_, body) = expect_reject(&family, home, FamilyVerb::Delete, "home is refused");
    assert_eq!(body["reason"], HOME_THREAD);
    assert!(!locked_family(&pool, other).await[0].is_home);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn every_refusal_carries_the_blocker_the_menu_shows() {
    use crate::engine::thread_lifecycle::Blocker;

    let target = Uuid::new_v4();
    let child = Uuid::new_v4();
    let waiting = |id| row(id, false, "waiting_for_user_answer", "inbox", false, false);
    let home = FamilyRow {
        is_home: true,
        ..idle_chat(target)
    };
    let cases: Vec<(&str, Vec<FamilyRow>, Blocker, &str)> = vec![
        ("home", vec![home], Blocker::Home, HOME_THREAD),
        (
            "running parent",
            vec![running_chat(target)],
            Blocker::Running,
            "parent_not_archivable",
        ),
        (
            "waiting parent",
            vec![waiting(target)],
            Blocker::Question,
            "parent_not_archivable",
        ),
        (
            "pending parent",
            vec![cc_with_pending(target)],
            Blocker::PendingChange,
            "parent_has_pending_changes",
        ),
        (
            "archived parent still holding a change",
            vec![row(target, true, "idle", "archived", true, false)],
            Blocker::PendingChange,
            "parent_has_pending_changes",
        ),
        (
            "running child",
            vec![idle_chat(target), running_chat(child)],
            Blocker::DescendantRunning,
            "descendants_blocking",
        ),
        (
            "child awaiting an answer",
            vec![idle_chat(target), waiting(child)],
            Blocker::DescendantQuestion,
            "descendants_blocking",
        ),
        (
            "pending child",
            vec![idle_chat(target), cc_with_pending(child)],
            Blocker::DescendantPendingChange,
            "descendants_blocking",
        ),
        (
            "parent parked with held work",
            vec![cc_holding_work(target)],
            Blocker::HeldProposal,
            "parent_holds_held_proposal",
        ),
        (
            "child parked with held work",
            vec![idle_chat(target), cc_holding_work(child)],
            Blocker::DescendantHeldProposal,
            "descendants_blocking",
        ),
    ];
    for (label, family, blocker, reason) in cases {
        let (status, body) = expect_reject(&family, target, FamilyVerb::Archive, label);
        assert_eq!(status, StatusCode::CONFLICT, "{label}");
        assert_eq!(body["blocker"], blocker.as_str(), "{label}");
        assert_eq!(body["reason"], reason, "{label}");
        let (_, delete_body) = expect_reject(&family, target, FamilyVerb::Delete, label);
        assert_eq!(delete_body["blocker"], blocker.as_str(), "{label} (delete)");
    }
}

#[test]
fn the_slug_names_the_strongest_sub_thread_reason() {
    let (target, waiting, running) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let family = vec![
        idle_chat(target),
        row(
            waiting,
            false,
            "waiting_for_user_answer",
            "inbox",
            false,
            false,
        ),
        running_chat(running),
    ];
    let (_, body) = expect_reject(&family, target, FamilyVerb::Archive, "two blockers");
    assert_eq!(body["blocker"], "descendant_running");
    assert_eq!(body["blocking"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        body["blocking"][0]["thread_id"],
        running.to_string(),
        "the first listed member holds the named blocker, so Show sub-thread opens it"
    );
}
