// ── Archive cascade tests ─────────────────────────────────────────────────
//
// Two layers:
//
// 1. `classify_archive_decision` is a pure function operating on a
//    `Vec<FamilyRow>` — every rejection path is testable without touching
//    Postgres. We hand-build family snapshots and assert the StatusCode +
//    JSON shape.
//
// 2. `load_family_for_archive` walks a real recursive CTE under `FOR UPDATE`.
//    We exercise it end-to-end via `EventBus` (which feeds the projection
//    that populates `thread_summaries`), then drive `classify_archive_decision`
//    against the loaded snapshot. This validates that the SQL + the parser
//    correctly round-trip every column the decision consults.

use super::{
    classify_archive_decision, load_family_for_archive, ArchiveDecision, FamilyRow,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, EventChannel, EventMeta, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};
use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

// ── Pure-logic fixtures ────────────────────────────────────────────────

fn row(
    id: Uuid,
    is_coding_agent: bool,
    status: &str,
    archive_state: &str,
    coding_agent_proposed: bool,
    coding_agent_is_external_repo: bool,
) -> FamilyRow {
    FamilyRow {
        thread_id: id,
        is_coding_agent,
        status: status.to_string(),
        archive_state: archive_state.to_string(),
        coding_agent_proposed,
        coding_agent_is_external_repo,
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

// ── DB-backed fixtures (mirror event_bus_tests helpers) ────────────────

/// Spawn a top-level chat parent. Returns its thread_id. Brings the row
/// to status='idle' so it's archive-eligible.
async fn spawn_idle_parent(bus: &EventBus) -> Uuid {
    let parent_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "do something".into(),
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
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    // Bring it to idle so the parent's own resolve_actions admits Archive.
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

/// Spawn a CC child of `parent_id`. `bring_to_idle` controls whether the
/// child is left in Running (true = emit `CodingAgentIdled` so the
/// projection drops status to idle). CC is used instead of Chat because
/// chat sub-threads' terminal events route to `archive_state='archived'`
/// by `resolve_transition` design — agentic-loop children don't surface
/// in REVIEW — so the cascading-archive test scenarios need CC
/// descendants to model the surfacing path.
async fn spawn_child(bus: &EventBus, parent_id: Uuid, bring_to_idle: bool) -> Uuid {
    let child_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "child task".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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
    // SessionStarted upserts is_coding_agent=TRUE on the row so the
    // family snapshot classifies the child correctly.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: format!("test-session-{}", child_id),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    if bring_to_idle {
        // Drop the row to status='idle' so it's not blocking. archive_state
        // is already 'inbox' from the MessageReceived INSERT (the column
        // default since migration 20260518132821).
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
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }
    // `bring_to_idle=false` leaves the row at status='running',
    // archive_state='inbox' — the exact "blocking" combo per
    // `is_blocking`. No follow-up MessageReceived needed: the
    // SessionStarted upsert above kept the row in Running, and the new
    // column default ensures archive_state is already 'inbox'.
    child_id
}

/// Spawn a CC child of `parent_id`, bring its session to Idle. Drop a
/// `ChangeProposed` if `with_pending_changes` so the row exits Idle into
/// Waiting + coding_agent_proposed=true.
async fn spawn_cc_child(
    bus: &EventBus,
    parent_id: Uuid,
    with_pending_changes: bool,
) -> Uuid {
    let child_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "cc task".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
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
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    if with_pending_changes {
        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event: ThreadEvent::ChangeProposed {
                change_id: format!("test-cid-{}", child_id),
                description: Some("Test change".into()),
                files: vec!["test.rs".into()],
                requires_restart: false,
                origin: None,
                commit_sha: None,
                branch_name: "claude-code/test".into(),
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
    }
    child_id
}

/// Emit `ThreadArchived` so the row's archive_state flips to 'archived'.
async fn archive_via_event(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn load_family(pool: &PgPool, thread_uuid: Uuid) -> Vec<FamilyRow> {
    let mut tx = pool.begin().await.unwrap();
    let family = load_family_for_archive(&mut tx, thread_uuid).await.unwrap();
    tx.commit().await.unwrap();
    family
}

// ── Tests ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn archive_with_no_descendants_archives_parent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;
    let family = load_family(&pool, parent_id).await;

    let ArchiveDecision::Proceed {
        to_archive,
        external_repo_pending,
    } = classify_archive_decision(&family, parent_id)
    else {
        panic!("expected Proceed for an idle childless parent");
    };
    assert_eq!(to_archive, vec![parent_id]);
    assert!(external_repo_pending.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn archive_with_idle_descendants_archives_all() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;
    let child_a = spawn_child(&bus, parent_id, true).await;
    let child_b = spawn_child(&bus, parent_id, true).await;
    let grandchild = spawn_child(&bus, child_a, true).await;

    let family = load_family(&pool, parent_id).await;
    assert_eq!(family.len(), 4, "parent + 2 children + 1 grandchild");

    let ArchiveDecision::Proceed {
        to_archive,
        external_repo_pending,
    } = classify_archive_decision(&family, parent_id)
    else {
        panic!("expected Proceed when every descendant is idle");
    };
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
    let running_child = spawn_child(&bus, parent_id, false).await; // status='running'

    let family = load_family(&pool, parent_id).await;
    let ArchiveDecision::Reject { status, body } =
        classify_archive_decision(&family, parent_id)
    else {
        panic!("expected Reject when a descendant is running");
    };
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
    let cc_child = spawn_cc_child(&bus, parent_id, true).await;

    let family = load_family(&pool, parent_id).await;
    let ArchiveDecision::Reject { status, body } =
        classify_archive_decision(&family, parent_id)
    else {
        panic!("expected Reject when a CC descendant has pending changes");
    };
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
    // Pure-logic test: hand-build a family where the parent itself is
    // Running. Avoids the extra DB churn since the decision function
    // doesn't care how the rows arrived.
    let parent_id = Uuid::new_v4();
    let family = vec![running_chat(parent_id)];
    let ArchiveDecision::Reject { status, body } =
        classify_archive_decision(&family, parent_id)
    else {
        panic!("expected Reject when parent is running");
    };
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason"], "parent_not_archivable");
    assert_eq!(body["parent_status"], "running");
}

#[tokio::test]
async fn archive_rejects_parent_cc_with_pending_changes() {
    // In-workspace CC parent with a pending change is NOT archivable. The
    // user must Apply or Discard the change first. Without this gate the
    // ThreadArchived projection clears coding_agent_proposed via
    // CLEAR_CODING_AGENT_FLAGS, the change row is left dangling in the
    // changes table, and the thread routes to Archive instead of Review —
    // the cca058432 "pending changes survive into Review when archived"
    // contract was never wired up (the projection always cleared the
    // column the routing depended on). Aligns with `resolve_actions`,
    // which already returns [Discard, Apply] — never Archive — in this
    // state.
    let parent_id = Uuid::new_v4();
    let family = vec![cc_with_pending(parent_id)];
    let ArchiveDecision::Reject { status, body } =
        classify_archive_decision(&family, parent_id)
    else {
        panic!("expected Reject when parent CC has its own pending changes");
    };
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason"], "parent_has_pending_changes");
}

#[tokio::test]
async fn archive_allows_parent_external_repo_cc_with_pending_changes() {
    // External-repo CC is the carve-out: Apply can't merge into a foreign
    // repo, so Archive (with `ChangeApplied` cleanup) is the only way to
    // dismiss the change row. The parent gate must admit it AND it must
    // appear in `external_repo_pending` so the handler emits
    // `ChangeApplied` before `ThreadArchived`.
    let parent_id = Uuid::new_v4();
    let mut row = cc_with_pending(parent_id);
    row.coding_agent_is_external_repo = true;
    let family = vec![row];
    let ArchiveDecision::Proceed {
        to_archive,
        external_repo_pending,
    } = classify_archive_decision(&family, parent_id)
    else {
        panic!("expected Proceed when parent CC is external-repo with pending changes");
    };
    assert_eq!(to_archive, vec![parent_id]);
    assert_eq!(external_repo_pending, vec![parent_id]);
}

#[tokio::test]
async fn archive_allows_parent_waiting_for_user_answer() {
    // CC parent paused on AskUserQuestion is archivable — the cascade
    // loop's per-thread cancel-stamp resolves the dangling QuestionCard
    // before ThreadArchived fires.
    let parent_id = Uuid::new_v4();
    let family = vec![row(parent_id, true, "waiting_for_user_answer", "inbox", false, false)];
    let ArchiveDecision::Proceed {
        to_archive,
        external_repo_pending,
    } = classify_archive_decision(&family, parent_id)
    else {
        panic!("expected Proceed when parent CC is WaitingForUserAnswer");
    };
    assert_eq!(to_archive, vec![parent_id]);
    assert!(external_repo_pending.is_empty());
}

#[tokio::test]
async fn archive_skips_already_archived_descendants() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;
    let live_child = spawn_child(&bus, parent_id, true).await;
    let archived_child = spawn_child(&bus, parent_id, true).await;
    archive_via_event(&bus, archived_child).await;

    let family = load_family(&pool, parent_id).await;
    assert_eq!(family.len(), 3);

    let ArchiveDecision::Proceed {
        to_archive,
        external_repo_pending,
    } = classify_archive_decision(&family, parent_id)
    else {
        panic!("expected Proceed when only an already-archived descendant exists");
    };
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
    // The CTE returns an empty Vec when the target doesn't exist.
    let missing = Uuid::new_v4();
    let family: Vec<FamilyRow> = vec![];
    let ArchiveDecision::Reject { status, body } =
        classify_archive_decision(&family, missing)
    else {
        panic!("expected Reject for missing target");
    };
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["reason"], "thread_not_found");
}

#[tokio::test]
async fn idle_descendants_alongside_archived_and_self() {
    // Pure-logic guard: a snapshot with the target idle, an idle child,
    // and an already-archived grandchild proceeds with [target, child].
    let target = Uuid::new_v4();
    let child = Uuid::new_v4();
    let gc = Uuid::new_v4();
    let family = vec![idle_chat(target), idle_chat(child), archived(gc)];
    let ArchiveDecision::Proceed {
        to_archive,
        external_repo_pending,
    } = classify_archive_decision(&family, target)
    else {
        panic!("expected Proceed");
    };
    assert!(external_repo_pending.is_empty());
    assert_eq!(to_archive.len(), 2);
    assert!(to_archive.contains(&target));
    assert!(to_archive.contains(&child));
    assert!(!to_archive.contains(&gc));
}

/// Reproduce the orphan-question scenario the cascade now defends against:
/// CC asks a question, then `CodingAgentIdled` lands without an answer.
/// The row settles at `status=idle, archive_state=inbox` — `is_blocking`
/// admits it, so the cascade Proceeds. The dangling QuestionCard then
/// needs `resolve_pending_question_as_canceled` to cancel-stamp it
/// (otherwise its answer buttons render clickable on the archived thread,
/// fix commit 3440bed36).
///
/// **Test limitation.** A full end-to-end assertion would invoke
/// `archive_thread` and observe a `UserQuestionAnswered { Canceled }`
/// landing for the orphaned question. That requires a real
/// `LucidosEngine` (Vertex/OpenAI provider, embedder, scheduler,
/// app manager) — the cascade test harness here only stands up an
/// `EventBus` against a throwaway Postgres. Instead we assert the
/// two preconditions the production wiring relies on: the cascade
/// admits the orphaned-question thread (so the helper must run), AND
/// `lookup_pending_question_tool_use_id` (the helper's first step)
/// returns the orphaned tool_use_id (so the helper has something to
/// cancel-stamp). End-to-end coverage lives in the API e2e suite.
#[tokio::test]
async fn orphaned_question_does_not_block_cascade_and_is_lookup_visible() {
    use crate::engine::agent_question::lookup_pending_question_tool_use_id;
    use crate::engine::thread_events::QuestionOption;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;

    // Spawn a CC child, then orphan a UserQuestionAsked on it by emitting
    // CodingAgentIdled without a matching UserQuestionAnswered.
    let child_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "cc task".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-orphan".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
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
            }],
            worktree_path: None,
            multi_select: false,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    // Idle without UserQuestionAnswered — this is what orphans the card.
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
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Precondition 1: the cascade gate must NOT block this thread.
    // status=idle + section=inbox passes `is_blocking`, so the orphaned
    // question rides through — which is exactly why we need the
    // defensive helper in the per-thread loop.
    let family = load_family(&pool, parent_id).await;
    assert_eq!(family.len(), 2, "parent + orphaned-question child");
    let ArchiveDecision::Proceed {
        to_archive,
        external_repo_pending,
    } = classify_archive_decision(&family, parent_id)
    else {
        panic!("cascade must admit the orphaned-question child — otherwise the helper would never run");
    };
    assert!(external_repo_pending.is_empty());
    assert_eq!(to_archive.len(), 2);
    assert!(to_archive.contains(&parent_id));
    assert!(to_archive.contains(&child_id));

    // Precondition 2: the helper's lookup query finds the orphaned
    // tool_use_id — without this, the cancel-stamp would silently
    // no-op and the bug would re-occur.
    let found = lookup_pending_question_tool_use_id(&pool, child_id).await;
    assert_eq!(
        found.as_deref(),
        Some(orphan_tool_use_id),
        "lookup must surface the orphaned question so resolve_pending_question_as_canceled has something to cancel-stamp"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A CC sub-thread that's spawned + Running but hasn't fired a
/// section-transitioning event yet must still block the parent's cascade
/// — exercises the new column-default leg (`archive_state='inbox'` on
/// fresh rows).
#[tokio::test]
async fn archive_rejects_when_fresh_cc_child_is_running_without_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = spawn_idle_parent(&bus).await;
    let fresh_child = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: fresh_child,
        event: ThreadEvent::MessageReceived {
            text: "fresh cc child".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "fresh-session".into(),
            branch: "claude-code/fresh".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let family = load_family(&pool, parent_id).await;
    let ArchiveDecision::Reject { status, body } =
        classify_archive_decision(&family, parent_id)
    else {
        panic!("cascade must reject when a fresh CC child is actively running");
    };
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason"], "descendants_blocking");
    let blocking = body["blocking"].as_array().unwrap();
    assert_eq!(blocking.len(), 1);
    assert_eq!(blocking[0]["thread_id"], fresh_child.to_string());
    assert_eq!(blocking[0]["status"], "running");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Defense in depth: a row that ends up at Running + Archived (legacy
/// pre-migration row, race, manual DB edit) must still be rejected by
/// the cascade — verified at the pure-logic layer so the assertion
/// doesn't depend on the column default.
#[tokio::test]
async fn archive_rejects_legacy_running_archived_descendant() {
    let parent_id = Uuid::new_v4();
    let legacy_child = Uuid::new_v4();
    let family = vec![
        idle_chat(parent_id),
        row(legacy_child, true, "running", "archived", false, false),
    ];
    let ArchiveDecision::Reject { status, body } =
        classify_archive_decision(&family, parent_id)
    else {
        panic!("cascade must reject a legacy Running+Archived descendant");
    };
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason"], "descendants_blocking");
    let blocking = body["blocking"].as_array().unwrap();
    assert_eq!(blocking.len(), 1);
    assert_eq!(blocking[0]["thread_id"], legacy_child.to_string());
}

#[tokio::test]
async fn external_repo_cc_with_pending_changes_does_not_block() {
    // Pure-logic guard for the is_blocking carve-out at the cascade layer:
    // an external-repo CC descendant with pending changes must pass
    // validation AND show up in `external_repo_pending` so the handler
    // emits ChangeApplied for it before the ThreadArchived emit.
    let target = Uuid::new_v4();
    let ext_cc = Uuid::new_v4();
    let mut ext_row = idle_cc(ext_cc);
    ext_row.coding_agent_is_external_repo = true;
    ext_row.coding_agent_proposed = true;
    ext_row.status = "waiting".into();
    let family = vec![idle_chat(target), ext_row];

    let ArchiveDecision::Proceed {
        to_archive,
        external_repo_pending,
    } = classify_archive_decision(&family, target)
    else {
        panic!("expected Proceed — external-repo CC with pending changes is exempt");
    };
    assert_eq!(to_archive.len(), 2);
    assert_eq!(external_repo_pending, vec![ext_cc]);
}
