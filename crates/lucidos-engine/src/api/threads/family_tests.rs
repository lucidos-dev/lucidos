// ── Family cascade tests ──────────────────────────────────────────────────
//
// Two layers:
//
// 1. `classify_family` is a pure function over a `Vec<FamilyRow>`, so every
//    rejection path is testable without touching Postgres. We hand-build
//    family snapshots and assert the StatusCode plus the JSON shape.
//
// 2. `load_family` walks a real recursive CTE under `FOR UPDATE`. We exercise
//    it end to end through `EventBus`, which feeds the projection that
//    populates `thread_summaries`, then drive `classify_family` against the
//    loaded snapshot. That is what proves the SQL and the row parser round-trip
//    every column the decision consults.
//
// Most cases pass `FamilyVerb::Archive`, because archive is the verb whose
// behaviour was already pinned here. The delete-specific arms live at the
// bottom, beside the one case where the two verbs answer differently.

use super::{
    classify_family, coding_agent_members, every_member, external_repo_pending, load_family,
    not_yet_archived, FamilyDecision, FamilyRow, FamilyVerb,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
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
            provider: None,
            voice_session_id: None,
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

/// Put a parent back to idle after a child of its finished.
///
/// A child terminal wakes its parent into `running`
/// (`event_bus/parent_callback.rs`), which is the parent's reaction turn. In
/// production that turn ends and settles the row. Nothing drains the callback
/// channel here, so a fixture that wants a quiescent family has to end it
/// itself. Skip this and the parent is a blocking descendant, which is the
/// right answer to the wrong question: these tests are about descendants.
///
/// The terminal has to match the parent's own kind, since only
/// `CodingAgentIdled` settles a coding-agent row.
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

/// Spawn a CC child of `parent_id`. `bring_to_idle` controls whether the
/// child is left in Running (true = emit `CodingAgentIdled` so the
/// projection drops status to idle). CC is used because it reaches every
/// state the cascade gate reads, pending changes included. A chat child
/// holds `archive_state='inbox'` on its own terminal event too, covered by
/// `archive_cascade_carries_an_idle_chat_child`.
async fn spawn_child(bus: &EventBus, pool: &PgPool, parent_id: Uuid, bring_to_idle: bool) -> Uuid {
    let child_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
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
        settle_after_child_completion(bus, pool, parent_id).await;
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
    settle_after_child_completion(bus, pool, parent_id).await;
    child_id
}

/// Spawn an agent-driven chat child of `parent_id` and finish it. Its
/// `ResponseGenerated` leaves the row at `archive_state='inbox'`, which is
/// the state the cascade then has to carry.
async fn spawn_idle_chat_child(bus: &EventBus, pool: &PgPool, parent_id: Uuid) -> Uuid {
    let child_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "delegated task".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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

async fn locked_family(pool: &PgPool, thread_uuid: Uuid) -> Vec<FamilyRow> {
    let mut tx = pool.begin().await.unwrap();
    let family = load_family(&mut tx, thread_uuid).await.unwrap();
    tx.commit().await.unwrap();
    family
}

/// Archive's two subsets, or a panic saying `why` the family should have been
/// admitted. `Proceed` is fieldless, so each verb derives what it needs.
fn expect_proceed(family: &[FamilyRow], target: Uuid, why: &str) -> (Vec<Uuid>, Vec<Uuid>) {
    match classify_family(family, target, FamilyVerb::Archive) {
        FamilyDecision::Proceed => (not_yet_archived(family), external_repo_pending(family)),
        FamilyDecision::Reject { status, body } => panic!("{why}; got {status} {body}"),
    }
}

/// The refusal a verb answers with, or a panic saying `why` one was owed.
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

// ── Tests ─────────────────────────────────────────────────────────────

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
    // A finished chat child sits at `archive_state='inbox'` now, so the
    // cascade is the only thing that can archive it. Two ways this breaks:
    // the child blocks the parent's Archive button, or it is dropped from
    // `to_archive` and stranded in the inbox with its parent gone.
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
    let running_child = spawn_child(&bus, &pool, parent_id, false).await; // status='running'

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
    // Pure-logic test: hand-build a family where the parent itself is
    // Running. Avoids the extra DB churn since the decision function
    // doesn't care how the rows arrived.
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
    // Archiving an already-archived thread is a no-op SUCCESS, not a 409.
    // The parent gate rejects only live work (Running); an already-archived
    // target falls through to Proceed with an empty `to_archive` (the filter
    // already excludes already-archived rows). Without this, a frontend whose
    // `meta.section` has desynced to 'inbox' (a missed `ThreadArchived` SSE or
    // a failed archive HTTP response on a flaky PWA leaves the backend at
    // 'archived' but the client at 'inbox') shows a stale Archive button that
    // 409s on every tap; the client's catch then rolls the optimistic flip
    // back to 'inbox', re-showing the button — a permanently stuck control.
    // Idempotent archive lets the click converge to 'archived'.
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
    // Partial-cascade state: the parent is already archived but a descendant
    // resurfaced to inbox (e.g. a late `CodingAgentIdled` after the family was
    // archived). Re-archiving the parent must archive the resurfaced
    // descendant rather than reject — the idempotent parent gate is what makes
    // this reachable (the old `archive_state == Archived` rejection 409'd the
    // whole family and left the descendant stranded in Review).
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
    // External-repo CC is the carve-out: Apply can't merge into a foreign
    // repo, so Archive (with `ChangeApplied` cleanup) is the only way to
    // dismiss the change row. The parent gate must admit it AND it must
    // appear in `external_repo_pending` so the handler emits
    // `ChangeApplied` before `ThreadArchived`.
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
async fn archive_allows_parent_waiting_for_user_answer() {
    // CC parent paused on AskUserQuestion is archivable — the cascade
    // loop's per-thread cancel-stamp resolves the dangling QuestionCard
    // before ThreadArchived fires.
    let parent_id = Uuid::new_v4();
    let family = vec![row(
        parent_id,
        true,
        "waiting_for_user_answer",
        "inbox",
        false,
        false,
    )];
    let (to_archive, external_repo_pending) = expect_proceed(
        &family,
        parent_id,
        "expected Proceed when parent CC is WaitingForUserAnswer",
    );
    assert_eq!(to_archive, vec![parent_id]);
    assert!(external_repo_pending.is_empty());
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
    // The CTE returns an empty Vec when the target doesn't exist.
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
    // Pure-logic guard: a snapshot with the target idle, an idle child,
    // and an already-archived grandchild proceeds with [target, child].
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
            provider: None,
            voice_session_id: None,
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
    settle_after_child_completion(&bus, &pool, parent_id).await;

    // Precondition 1: the cascade gate must NOT block this thread.
    // status=idle + section=inbox passes `is_blocking`, so the orphaned
    // question rides through — which is exactly why we need the
    // defensive helper in the per-thread loop.
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
            provider: None,
            voice_session_id: None,
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

    let (to_archive, external_repo_pending) = expect_proceed(
        &family,
        target,
        "expected Proceed: external-repo CC with pending changes is exempt",
    );
    assert_eq!(to_archive.len(), 2);
    assert_eq!(external_repo_pending, vec![ext_cc]);
}

// ── The delete verb ───────────────────────────────────────────────────
//
// Delete asks this same classifier, so everything above binds it too. What
// follows is the one state the two verbs answer differently, plus the parity
// assertion that keeps them from drifting anywhere else.

/// The single divergence. Archive admits a parked parent and cancel-stamps its
/// question card in its own cascade step. Delete cannot: there is nothing to
/// stamp when the card is about to go. The subprocess parked on that question
/// would be left with nobody to answer it.
#[test]
fn delete_refuses_a_parent_waiting_on_an_answer_where_archive_admits_it() {
    let target = Uuid::new_v4();
    let mut parked = idle_chat(target);
    parked.status = "waiting_for_user_answer".into();
    let family = vec![parked];

    let (status, body) = expect_reject(
        &family,
        target,
        FamilyVerb::Delete,
        "delete must refuse a parent parked on a question",
    );
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason"], "parent_not_deletable");
    assert_eq!(body["parent_status"], "waiting_for_user_answer");

    let (to_archive, _) = expect_proceed(
        &family,
        target,
        "archive must still admit a parent parked on a question",
    );
    assert_eq!(to_archive, vec![target]);
}

/// Everything except that one state must answer identically, or the lift into
/// one classifier bought nothing. Each family below is refused, and the two
/// verbs must agree on the refusal slug as well as on the fact of it.
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

    // And the running-parent case, where the slug is deliberately per-verb.
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

/// Delete sweeps the WHOLE family, including members archive would skip.
/// `not_yet_archived` is archive's subset and would silently strand an
/// already-archived sub-thread's rows behind.
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
