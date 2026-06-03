//! Tests for [`super::EventBus`]. Split from the former single
//! `event_bus_tests.rs` into concern modules (see the `mod` declarations
//! below). Shared setup helpers live here so every concern module can reach
//! them via `use super::*`.

use super::*;
pub(crate) use crate::engine::thread_events::{
    ActorMode, EventChannel, EventMeta, MessageOrigin, SessionEndReason, ThreadEvent,
};
pub(crate) use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};

/// Create a parent Chat thread and a child thread, returning (parent_id, child_id).
async fn spawn_parent_child(bus: &EventBus, child_channel: EventChannel) -> (Uuid, Uuid) {
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

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
            channel: Some(child_channel),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    (parent_id, child_id)
}

async fn emit_cc_session_started(bus: &EventBus, child_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
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
}

async fn assert_active_children(pool: &PgPool, parent_id: Uuid, expected: i32, msg: &str) {
    let count: i32 = sqlx::query_scalar(
        "SELECT active_children_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(parent_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(count, expected, "{}", msg);
}

async fn assert_children_counters(
    pool: &PgPool,
    parent_id: Uuid,
    expected_active: i32,
    expected_total: i32,
    msg: &str,
) {
    let (active, total): (i32, i32) = sqlx::query_as(
        "SELECT active_children_count, total_children_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(parent_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(active, expected_active, "{} — active_children_count", msg);
    assert_eq!(total, expected_total, "{} — total_children_count", msg);
}

/// Helper: emit a MessageReceived event for a thread with an optional parent.
async fn emit_thread_message(bus: &EventBus, thread_id: Uuid, parent: Option<Uuid>, text: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: text.into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: parent,
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
}

/// Helper: emit CodingAgentIdled with the given flags. When `has_changes=true`,
/// also emits a synthetic `ChangeProposed` to mirror the production CC
/// lifecycle (idle → propose). `coding_agent_proposed` is set exclusively by
/// `ChangeProposed`, so without the follow-up the projection column never
/// flips. Callers that want to assert the bare-idle (no-proposal) state
/// should construct events inline rather than use this helper.
async fn emit_cc_idle(
    bus: &EventBus,
    thread_id: Uuid,
    has_changes: bool,
    cc_session_id: Option<&str>,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: cc_session_id.map(Into::into),
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
    if has_changes {
        emit_change_proposed(bus, thread_id, "claude-code/test", false).await;
    }
}

/// Emit a vanilla `ChangeProposed` for `thread_id` and wait for it to land.
/// `change_id` is derived from `thread_id` so multiple proposals on the same
/// thread reuse the same row. The string `change_id` is not a UUID, so
/// `write_proposed_aggregate` short-circuits without inserting into `changes` —
/// fine for projection tests that only care about `thread_summaries` flips.
async fn emit_change_proposed(
    bus: &EventBus,
    thread_id: Uuid,
    branch: &str,
    requires_restart: bool,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: format!("test-cid-{}", thread_id),
            description: Some("Test change".into()),
            files: vec!["test.rs".into()],
            requires_restart,
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
}

/// Emit a legacy per-commit `ChangeProposed` (empty change_id, commit_sha set).
/// The new projection treats these as inert in `thread_summaries`; they only
/// flow through `write_proposed_per_commit` (UPDATE-only on `changes`).
async fn emit_change_proposed_per_commit(
    bus: &EventBus,
    thread_id: Uuid,
    branch: &str,
    commit_sha: &str,
    description: Option<&str>,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: String::new(),
            commit_sha: Some(commit_sha.into()),
            branch_name: branch.into(),
            description: description.map(Into::into),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            repo_root: String::new(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// Seed `thread_summaries.coding_agent_has_diff = TRUE` directly. In production
/// `session_seed` sets this against on-disk git reality; tests that need it as
/// a precondition (without going through the seed sweep) flip it manually.
async fn force_has_diff(pool: &sqlx::PgPool, thread_id: Uuid) {
    sqlx::query("UPDATE thread_summaries SET coding_agent_has_diff = TRUE WHERE thread_id = $1")
        .bind(thread_id)
        .execute(pool)
        .await
        .unwrap();
}

/// Read `blocking_descendant_count` for `thread_id`.
async fn read_blocking_descendant_count(pool: &PgPool, thread_id: Uuid) -> i32 {
    sqlx::query_scalar(
        "SELECT blocking_descendant_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Sibling helper to `read_blocking_descendant_count` for the attention
/// counter. Used by tests that pin the REVIEW-bubble behavior (parent
/// surfaces to Review when a descendant needs user attention, even if a
/// sibling descendant is still running and so keeps the row blocking).
async fn read_attention_descendant_count(pool: &PgPool, thread_id: Uuid) -> i32 {
    sqlx::query_scalar(
        "SELECT attention_descendant_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Spawn parent + CC child, bring the child to Idle (post-session, no pending
/// changes). Returns (parent_id, child_id); parent's blocking_descendant_count
/// is 0 at return — the test starts from a clean baseline.
async fn spawn_parent_with_idle_cc_child(bus: &EventBus, pool: &PgPool) -> (Uuid, Uuid) {
    let (parent_id, child_id) = spawn_parent_child(bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(bus, child_id).await;
    emit_cc_idle(bus, child_id, false, None).await;
    assert_eq!(
        read_blocking_descendant_count(pool, parent_id).await,
        0,
        "baseline: idle CC child contributes 0 to parent's blocking_descendant_count"
    );
    (parent_id, child_id)
}

/// Drain all SSE broadcasts the bus has accumulated since subscription,
/// returning a flat list of `(thread_id, event_type, aggregate)` tuples.
/// Caller filters by `thread_id` / aggregate presence to find specific
/// rebroadcasts. Ignores system events.
fn drain_aggregate_broadcasts(
    rx: &mut tokio::sync::broadcast::Receiver<EmittedEvent>,
) -> Vec<(Uuid, String, Option<crate::core::store::ThreadAggregate>)> {
    let mut out = Vec::new();
    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::Thread {
            thread_id, event, ..
        } = &emitted.typed
        {
            out.push((*thread_id, event.event_type().to_string(), emitted.aggregate.clone()));
        }
    }
    out
}

/// Emit a `MessageReceived` on the ClaudeCode channel. Companion to
/// `emit_thread_message` (Chat channel) — the new blocking-count tests below
/// build CC parent/child trees, and the channel choice flips
/// `thread_summaries.source` which drives the `source = 'claude_code'` scope
/// of the orphan-waiting reset under test.
async fn emit_cc_message_received(
    bus: &EventBus,
    thread_id: Uuid,
    parent: Option<Uuid>,
    text: &str,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: text.into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: parent,
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
}

/// Spawn a CC parent + N running CC children. Each child is at
/// `status='running'` post-`SessionStarted` — the parent's
/// `blocking_descendant_count` equals `n` on return. Used by the
/// blocking-count regression tests below.
async fn spawn_cc_parent_with_n_running_cc_children(
    bus: &EventBus,
    n: usize,
) -> (Uuid, Vec<Uuid>) {
    let parent_id = Uuid::new_v4();
    emit_cc_message_received(bus, parent_id, None, "parent").await;
    emit_cc_session_started(bus, parent_id).await;

    let children: Vec<Uuid> = (0..n).map(|_| Uuid::new_v4()).collect();
    for &child_id in &children {
        emit_cc_message_received(bus, child_id, Some(parent_id), "sub").await;
        emit_cc_session_started(bus, child_id).await;
    }
    (parent_id, children)
}

async fn emit_cc_recovery_pair(bus: &EventBus, child_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::RecoveryAfterRestart,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: Some(
                crate::engine::agent_recovery::ENGINE_RESTART_INTERRUPT_REASON.to_string(),
            ),
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

mod serialization_sse;
mod serialization_persistence;
mod fan_out_callback;
mod session_lifecycle;
mod active_children_count;
mod idle_reconcile_counts;
mod section_review;
mod recursion_guard;
mod has_response;
mod origin_and_resume;
mod change_apply_archive;
mod proposed_apply_cycle;
mod initiator_actor;
mod review_presence_status;
mod thread_state_and_eviction;
mod has_diff_and_actor;
mod blocking_attention_counts;
mod ancestor_rebroadcast;
mod recovery_and_pipeline;
