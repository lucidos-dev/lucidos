// ── Thread delete ─────────────────────────────────────────────────────────
//
// The cascade GATE is tested in `family_tests.rs`, which archive and delete
// share. What is tested here is the removal itself: what goes, in what order,
// and what survives it.
//
// The handler needs a whole `LucidosEngine` (providers, embedder, scheduler),
// which this harness does not stand up. So the tests drive
// `delete_family_rows` against a real Postgres seeded through `EventBus`, which
// is every destructive statement in the route. The caller gate is tested where
// it lives, in `api/actor_tests.rs`.

use super::*;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::memory::pgvector::{MemorySource, PgVectorIndex};
use crate::test_support::{setup_test_db, teardown_test_db};
use sqlx::PgPool;

/// A marker no other fixture writes, so an assertion that it is absent from the
/// audit payload means the payload carries no thread content.
const BODY_MARKER: &str = "zebra-quilt-marker";

/// Seed one chat thread with a body carrying [`BODY_MARKER`], and return its id
/// plus the id of its `MessageReceived`.
async fn seed_thread(bus: &EventBus, parent: Option<uuid::Uuid>) -> (Uuid, Uuid) {
    let thread_id = Uuid::new_v4();
    let event_id = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                text: format!("please remember {BODY_MARKER}"),
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
        .unwrap()
        .expect("MessageReceived is persisted, so it has an id")
        .event_id;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "noted".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    (thread_id, event_id)
}

/// Index one memory fact against `event_id`, the shape `memory_consumer` writes.
async fn index_fact(index: &PgVectorIndex, event_id: Uuid, summary: &str) -> Uuid {
    let id = Uuid::new_v4();
    index
        .index_entry(
            id,
            &MemorySource::Event { id: event_id },
            "topic",
            summary,
            0.5,
            &["marker".to_string()],
            &vec![0.01f32; 384],
            "test-embedder",
            chrono::Utc::now(),
            1,
        )
        .await
        .unwrap();
    id
}

async fn count(pool: &PgPool, sql: &str, id: Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Run the whole transaction the way the handler does, and commit.
async fn delete_family(pool: &PgPool, ids: &[Uuid]) -> DeletedCounts {
    let mut tx = pool.begin().await.unwrap();
    let counts = delete_family_rows(&mut tx, ids).await.unwrap();
    tx.commit().await.unwrap();
    counts
}

// ── I1: memory before events ──────────────────────────────────────────

/// A `memory_entries` row's `source` is an event id, so the events are the only
/// way to find one. The delete must take it, and must take it while the events
/// it names are still there.
#[tokio::test]
async fn memory_rows_are_gone_after_a_delete() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let index = PgVectorIndex::new(pool.clone()).await.unwrap();

    let (doomed, doomed_event) = seed_thread(&bus, None).await;
    let (_kept, kept_event) = seed_thread(&bus, None).await;
    let doomed_fact = index_fact(&index, doomed_event, "the doomed thread said this").await;
    let kept_fact = index_fact(&index, kept_event, "the kept thread said this").await;

    let counts = delete_family(&pool, &[doomed]).await;
    assert_eq!(counts.memory, 1, "exactly the doomed thread's fact");

    assert!(
        index.get_by_id(doomed_fact).await.unwrap().is_none(),
        "the fact sourced to the deleted thread must be gone"
    );
    assert!(
        index.get_by_id(kept_fact).await.unwrap().is_some(),
        "and a bystander's fact must be untouched"
    );

    teardown_test_db(&db_name).await;
}

/// The ordering is the invariant, not an implementation detail, and no
/// behavioural test can see it: run the statements the other way round and this
/// suite still passes, while every `memory_entries` row the thread sourced
/// becomes an unreachable orphan. So the source is what is asserted.
#[test]
fn the_memory_statement_precedes_the_events_statement() {
    const SRC: &str = include_str!("delete.rs");
    let memory = SRC
        .find("DELETE FROM memory_entries")
        .expect("the memory sweep must still be here");
    let events = SRC
        .find("DELETE FROM events")
        .expect("the events sweep must still be here");
    assert!(
        memory < events,
        "memory_entries must be deleted BEFORE events. Its `source` is an event \
         id, so the events are the only way to find the rows; swap these and \
         every fact the thread taught is orphaned forever."
    );
}

// ── I2: nothing survives ──────────────────────────────────────────────

/// One assertion per table the cascade sweeps. A row left behind points at a
/// thread nobody can open.
#[tokio::test]
async fn nothing_survives_a_delete() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let index = PgVectorIndex::new(pool.clone()).await.unwrap();

    let (parent, parent_event) = seed_thread(&bus, None).await;
    let (child, _) = seed_thread(&bus, Some(parent)).await;
    index_fact(&index, parent_event, "something learned").await;

    // A row in every other table the sweep names.
    sqlx::query(
        "INSERT INTO notifications (id, task_id, title, message, thread_id, event_id) \
         VALUES ($1, $2, 'test', 'a notification', $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(parent)
    .bind(parent_event)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO changes (id, request_id, branch_name, repo_root, description, \
         file_count, files, requires_restart, status, thread_id) \
         VALUES ($1, $2, 'b', '/tmp', 'd', 0, ARRAY[]::text[], false, 'applied', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(child)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO thread_queue (id, kind, thread_id, request) \
         VALUES ($1, 'sub-thread', $2, '{}'::jsonb)",
    )
    .bind(Uuid::new_v4())
    .bind(child)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO standing_applies (thread_id) VALUES ($1)")
        .bind(child)
        .execute(&pool)
        .await
        .unwrap();

    delete_family(&pool, &[parent, child]).await;

    for id in [parent, child] {
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM events WHERE thread_id = $1",
                id
            )
            .await,
            0,
            "events.thread_id"
        );
        let by_aggregate: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE aggregate = 'thread' AND aggregate_id = $1",
        )
        .bind(id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(by_aggregate, 0, "events.aggregate_id");
        for table in [
            "thread_summaries",
            "notifications",
            "changes",
            "thread_queue",
            "standing_applies",
        ] {
            let left: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM {table} WHERE thread_id = $1"
            ))
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(left, 0, "a row survived in {table}");
        }
    }
    let orphan_memory: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM memory_entries WHERE source->>'type' = 'event'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(orphan_memory, 0, "memory_entries");

    teardown_test_db(&db_name).await;
}

/// A workspace whose pgvector extension never loaded has no `memory_entries`
/// table, and inside a transaction a missing relation aborts everything after
/// it. Without the guard, delete would fail on exactly the workspaces with no
/// memory to delete.
#[tokio::test]
async fn a_workspace_with_no_memory_table_still_deletes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (thread_id, _) = seed_thread(&bus, None).await;
    // Deliberately no `PgVectorIndex::new`, so the table is absent.

    let counts = delete_family(&pool, &[thread_id]).await;
    assert_eq!(counts.memory, 0);
    assert!(counts.events > 0, "the events still went");

    teardown_test_db(&db_name).await;
}

// ── I6: one transaction ───────────────────────────────────────────────

/// Every destructive statement is in one transaction, so a failure anywhere
/// leaves the family whole. The rollback is driven here rather than injected
/// into the function, because a caller abandoning the transaction IS the
/// production failure mode: any `?` in the handler between the first statement
/// and the commit drops `tx` unclosed.
#[tokio::test]
async fn a_failed_delete_rolls_everything_back() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let index = PgVectorIndex::new(pool.clone()).await.unwrap();

    let (thread_id, event_id) = seed_thread(&bus, None).await;
    let fact = index_fact(&index, event_id, "a fact that must survive a rollback").await;

    let mut tx = pool.begin().await.unwrap();
    let counts = delete_family_rows(&mut tx, &[thread_id]).await.unwrap();
    assert!(counts.memory > 0 && counts.events > 0, "it really deleted");
    tx.rollback().await.unwrap();

    assert!(
        index.get_by_id(fact).await.unwrap().is_some(),
        "the memory row must come back, or the ordering invariant is unrecoverable"
    );
    assert!(
        count(
            &pool,
            "SELECT COUNT(*) FROM events WHERE thread_id = $1",
            thread_id
        )
        .await
            > 0,
        "and so must the events"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM thread_summaries WHERE thread_id = $1",
            thread_id
        )
        .await,
        1,
        "and the row the drawer reads"
    );

    teardown_test_db(&db_name).await;
}

// ── I3 and I4: the audit record ───────────────────────────────────────

/// The payload is the only thing left, so it must not re-file what the delete
/// removed. Ids and counts, and no text from the thread.
#[tokio::test]
async fn the_audit_record_carries_no_content() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, _) = seed_thread(&bus, None).await;
    let counts = delete_family(&pool, &[thread_id]).await;
    bus.emit(BusEvent::System(SystemEvent::ThreadsDeleted {
        thread_ids: vec![thread_id],
        event_count: counts.events,
        memory_count: counts.memory,
        worktrees_removed: 0,
        actor: None,
    }))
    .await
    .unwrap();

    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE event_type = 'ThreadsDeleted' ORDER BY sequence DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let text = payload.to_string();
    assert!(
        !text.contains(BODY_MARKER),
        "the thread's own words must not survive in the record that it was deleted: {text}"
    );
    for forbidden in ["title", "summary", "message", "text"] {
        assert!(
            !text.contains(forbidden),
            "the payload names a content field '{forbidden}': {text}"
        );
    }
    assert!(text.contains(&thread_id.to_string()), "but the id is there");

    teardown_test_db(&db_name).await;
}

/// The record sits on aggregate `ops` with `aggregate_id` `global`, and a thread
/// sweep matches `aggregate = 'thread'` plus a `thread_id`. So deleting thread B
/// cannot reach the record of deleting thread A.
#[tokio::test]
async fn a_later_delete_cannot_reach_an_earlier_record() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (first, _) = seed_thread(&bus, None).await;
    let (second, _) = seed_thread(&bus, None).await;

    delete_family(&pool, &[first]).await;
    bus.emit(BusEvent::System(SystemEvent::ThreadsDeleted {
        thread_ids: vec![first],
        event_count: 2,
        memory_count: 0,
        worktrees_removed: 0,
        actor: None,
    }))
    .await
    .unwrap();

    delete_family(&pool, &[second]).await;

    let records: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = 'ThreadsDeleted'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        records, 1,
        "the first delete's record must survive the second delete"
    );

    teardown_test_db(&db_name).await;
}

// ── The surviving parent ──────────────────────────────────────────────

/// The cascade cuts exactly one edge: the target's link to a parent outside the
/// family. That parent is read before the delete, because it lives on a row
/// that is about to go.
#[tokio::test]
async fn the_surviving_parent_is_read_before_the_row_goes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (grandparent, _) = seed_thread(&bus, None).await;
    let (parent, _) = seed_thread(&bus, Some(grandparent)).await;
    let (child, _) = seed_thread(&bus, Some(parent)).await;

    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        parent_outside_family(&mut tx, parent).await.unwrap(),
        Some(grandparent)
    );
    assert_eq!(
        parent_outside_family(&mut tx, child).await.unwrap(),
        Some(parent),
        "a member inside the family still answers; the handler only asks about \
         the target"
    );
    assert_eq!(
        parent_outside_family(&mut tx, grandparent).await.unwrap(),
        None,
        "a top thread has nothing to repair"
    );
    tx.commit().await.unwrap();

    teardown_test_db(&db_name).await;
}

/// The counters a surviving ancestor holds for the subtree that just vanished.
/// Nothing else repairs them: the projection's propagation keys on a live event,
/// and a delete emits none on the threads it removes.
#[tokio::test]
async fn a_surviving_parent_stops_counting_the_deleted_subtree() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (parent, _) = seed_thread(&bus, None).await;
    let child = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: child,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "work".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent),
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

    // The child is Running, so the parent counts it on both columns.
    let (blocking, active): (i32, i32) = sqlx::query_as(
        "SELECT blocking_descendant_count, active_children_count \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(parent)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (blocking, active),
        (1, 1),
        "the parent starts out counting it"
    );

    delete_family(&pool, &[child]).await;
    EventBus::rebuild_children_counts(&pool).await.unwrap();
    EventBus::rebuild_blocking_descendant_count(&pool)
        .await
        .unwrap();

    let (blocking, active): (i32, i32) = sqlx::query_as(
        "SELECT blocking_descendant_count, active_children_count \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(parent)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (blocking, active),
        (0, 0),
        "a stale blocking count hides the parent's own Archive and Delete for good"
    );

    teardown_test_db(&db_name).await;
}

// ── The worktree and the branch ───────────────────────────────────────

/// Decision 8, and the one place the reclamation rule is inverted. The owner
/// asked for the thread and its work to be gone, so the branch goes even
/// holding commits nothing merged.
#[test]
fn the_delete_asks_for_the_branch_whatever_it_holds() {
    const SRC: &str = include_str!("delete.rs");
    assert!(
        SRC.contains("BranchDisposal::Always"),
        "delete must pass BranchDisposal::Always. With WhenMerged the branch \
         survives the thread, and once the events are gone nothing can resolve \
         it back to one."
    );
    assert!(
        !SRC.contains("BranchDisposal::WhenMerged"),
        "the reclamation default has no business on this path"
    );
}

/// The branch is read from the events, so it MUST be read before they go. The
/// cleanup worker can already have taken the worktree while keeping a branch
/// that holds unmerged commits. The recorded name is then the only handle left
/// on it.
#[tokio::test]
async fn a_coding_agent_members_branch_is_read_before_the_events_go() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, _) = seed_thread(&bus, None).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-1".into(),
            branch: "lucidos-claude-code-first".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    // A resume writes a second one. The newest wins, because that is the branch
    // the session was last on.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-2".into(),
            branch: "lucidos-claude-code-second".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let members = recorded_branches(&mut tx, &[thread_id]).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(
        members[0].branch.as_deref(),
        Some("lucidos-claude-code-second")
    );

    // After the delete there is nothing left to read it from, which is why the
    // handler reads it inside the same transaction.
    delete_family_rows(&mut tx, &[thread_id]).await.unwrap();
    let after = recorded_branches(&mut tx, &[thread_id]).await.unwrap();
    assert_eq!(after[0].branch, None, "the handle is gone with the events");
    tx.commit().await.unwrap();

    teardown_test_db(&db_name).await;
}

/// A chat thread with no session records no branch, and the reclaim has nothing
/// to chase. Guards the `None` arm against a future change that unwraps it.
#[tokio::test]
async fn a_thread_that_never_started_a_session_records_no_branch() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (thread_id, _) = seed_thread(&bus, None).await;
    let mut tx = pool.begin().await.unwrap();
    let members = recorded_branches(&mut tx, &[thread_id]).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(members[0].branch, None);

    teardown_test_db(&db_name).await;
}

/// A worktree the cleanup worker already reclaimed must not end the reclaim.
/// Skipping there left a branch nothing could ever name again, which is the
/// half of decision 8 the worktree alone does not cover.
#[test]
fn a_missing_worktree_still_chases_the_branch() {
    const SRC: &str = include_str!("delete.rs");
    let skip = SRC
        .find("if !worktree.exists() {")
        .expect("the reclaim must still test for the worktree");
    let after = &SRC[skip..skip + 200];
    assert!(
        after.contains("delete_orphaned_branch"),
        "a missing worktree must fall through to the recorded branch, not \
         `continue`. The worker reclaims a spent tree while KEEPING a branch \
         that holds unmerged commits, so that is the state an old coding-agent \
         thread reaches delete in."
    );
}

/// The parent keeps a disclosure chevron gated on `total_children_count`, and
/// no rebuild reduces it: until delete existed nothing removed a child. A count
/// of one with no child left draws a chevron that expands to nothing.
#[tokio::test]
async fn a_surviving_parent_stops_counting_children_it_no_longer_has() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (parent, _) = seed_thread(&bus, None).await;
    let (child, _) = seed_thread(&bus, Some(parent)).await;

    let before: i32 = sqlx::query_scalar(
        "SELECT total_children_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(parent)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, 1, "the parent starts out counting it");

    delete_family(&pool, &[child]).await;
    // The statement the handler runs, against the surviving parent.
    sqlx::query(
        "UPDATE thread_summaries p SET total_children_count = ( \
             SELECT COUNT(*) FROM thread_summaries c WHERE c.parent_thread_id = p.thread_id \
         ) WHERE p.thread_id = $1",
    )
    .bind(parent)
    .execute(&pool)
    .await
    .unwrap();

    let after: i32 = sqlx::query_scalar(
        "SELECT total_children_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(parent)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after, 0, "or the chevron expands to nothing, for good");

    teardown_test_db(&db_name).await;
}

/// The two in-memory drops and the projection repair all have to land BEFORE
/// the frame that tells every client to re-read. A client that re-reads first
/// sees the stale counts, and nothing re-broadcasts afterwards.
#[test]
fn the_audit_frame_is_the_last_thing_the_handler_does() {
    const SRC: &str = include_str!("delete.rs");
    let emit = SRC
        .find("SystemEvent::ThreadsDeleted {")
        .expect("the emit must still be here");
    for earlier in [
        "drop_live_subscriptions(&state, &ids)",
        "reclaim_worktrees(&state, &coding_agents)",
        "repair_ancestor_counts(&state)",
    ] {
        let at = SRC
            .find(earlier)
            .unwrap_or_else(|| panic!("{earlier} must still be called"));
        assert!(
            at < emit,
            "{earlier} must run before the ThreadsDeleted emit: the frame is \
             what makes every client re-read"
        );
    }
}

/// The drop must stay silent. Archive ends a subscription with an
/// `EventWaitCanceled`, and delete cannot, because the thread has no rows
/// left. A thread event on that id would re-insert an `events` row and raise a
/// `thread_summaries` row through the projection's upsert.
#[test]
fn dropping_a_deleted_threads_waits_records_nothing() {
    const SRC: &str = include_str!("../../engine/event_wait/dispatcher.rs");
    let start = SRC
        .find("pub async fn drop_waits_for_thread(")
        .expect("the drop-only path must still exist");
    let body = &SRC[start..];
    let end = body.find("\n    }\n").expect("the function must close");
    let body = &body[..end];
    assert!(
        !body.contains("emit"),
        "drop_waits_for_thread must not emit. An EventWaitCanceled on a deleted \
         thread resurrects it through the projection's upsert:\n{body}"
    );
    assert!(
        body.contains("live_waits.take("),
        "it must still take the entries out of the live set"
    );
}
