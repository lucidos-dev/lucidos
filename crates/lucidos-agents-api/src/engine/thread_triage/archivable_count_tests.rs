//! The stored archivable sub-thread count against the press's own preflight,
//! and the projection's recount requests.

use super::*;
use crate::engine::event_bus::{BusEvent, EmittedEvent};
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};
use tokio::sync::broadcast::Receiver;

async fn spawn(bus: &EventBus, parent: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "work".into(),
            user_image_hashes: vec![],
            device_id: None,
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
    id
}

/// Leave a row as a finished turn does, then apply `set`.
async fn settle(pool: &PgPool, id: Uuid, set: &str) {
    for sql in [
        "UPDATE thread_summaries SET status = 'idle', has_response = TRUE, \
         is_saved = FALSE, coding_agent_change_state = 'none', compose_text = '', \
         live_event_wait_count = 0 WHERE thread_id = $1"
            .to_string(),
        format!("UPDATE thread_summaries SET {set} WHERE thread_id = $1"),
    ] {
        sqlx::query(&sql).bind(id).execute(pool).await.unwrap();
    }
}

async fn emit(bus: &EventBus, id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id: id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

fn question(id: Uuid) -> ThreadEvent {
    ThreadEvent::UserQuestionAsked {
        tool_use_id: format!("q-{id}"),
        cc_session_id: String::new(),
        question: "Which one?".into(),
        options: vec![],
        worktree_path: None,
        multi_select: false,
        owner_approval: None,
        permission_request: None,
    }
}

async fn stored(pool: &PgPool, id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i32>(
        "SELECT archivable_sub_thread_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
    .into()
}

async fn press_count(pool: &PgPool, parent: Uuid) -> i64 {
    let rows = load(pool, TriageScope::Descendants(parent)).await.unwrap();
    completed_preflight(&rows).safe_thread_count as i64
}

fn drain(requests: &mut UnboundedReceiver<Uuid>) -> Vec<Uuid> {
    let mut out = Vec::new();
    while let Ok(id) = requests.try_recv() {
        out.push(id);
    }
    out
}

/// The threads a `ChildrenCountChanged` rebroadcast named, with their count.
fn rebroadcast(events: &mut Receiver<EmittedEvent>) -> Vec<(Uuid, i64)> {
    let mut out = Vec::new();
    while let Ok(e) = events.try_recv() {
        if let BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ChildrenCountChanged { .. },
            ..
        } = e.typed
        {
            let count = e.aggregate.map_or(-1, |a| a.archivable_sub_thread_count);
            out.push((thread_id, count));
        }
    }
    out
}

#[tokio::test]
async fn the_stored_count_equals_the_press_for_every_refusal_input() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, None).await;
    let child = spawn(&bus, Some(parent)).await;
    let done = spawn(&bus, Some(parent)).await;
    settle(&pool, done, "status = 'idle'").await;

    let change_row = |status: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO changes (id, request_id, branch_name, repo_root, description, \
                 file_count, files, requires_restart, status, thread_id) \
                 VALUES ($1, $2, $3, '/tmp', 'd', 0, ARRAY[]::text[], false, $4, $5)",
            )
            .bind(Uuid::new_v4())
            .bind(Uuid::new_v4())
            .bind(format!("b-{}", Uuid::new_v4()))
            .bind(status)
            .bind(child)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    let cases: [(&str, &str); 9] = [
        ("busy", "status = 'running'"),
        ("waiting on an event", "live_event_wait_count = 1"),
        ("pending change", "coding_agent_change_state = 'proposed'"),
        (
            "unproposed work",
            "coding_agent_change_state = 'unproposed'",
        ),
        ("draft", "compose_text = 'half a thought'"),
        ("failed run", "status = 'failed'"),
        ("pinned", "is_saved = TRUE"),
        ("no output", "has_response = FALSE"),
        ("clear", "status = 'idle'"),
    ];
    for (name, set) in cases {
        settle(&pool, child, set).await;
        recount_from(&pool, &bus, &[parent]).await.unwrap();
        let want = if name == "clear" { 2 } else { 1 };
        assert_eq!(press_count(&pool, parent).await, want, "{name}: press");
        assert_eq!(stored(&pool, parent).await, want, "{name}: stored");
    }

    emit(&bus, child, question(child)).await;
    settle(&pool, child, "status = 'idle'").await;
    recount_from(&pool, &bus, &[parent]).await.unwrap();
    assert_eq!(press_count(&pool, parent).await, 1, "orphaned question");
    assert_eq!(stored(&pool, parent).await, 1, "orphaned question");
    sqlx::query("DELETE FROM events WHERE thread_id = $1 AND event_type = 'UserQuestionAsked'")
        .bind(child)
        .execute(&pool)
        .await
        .unwrap();

    for status in ["pending", "set_aside"] {
        change_row(status).await;
        recount_from(&pool, &bus, &[parent]).await.unwrap();
        assert_eq!(press_count(&pool, parent).await, 1, "{status} change row");
        assert_eq!(stored(&pool, parent).await, 1, "{status} change row");
        sqlx::query("DELETE FROM changes WHERE thread_id = $1")
            .bind(child)
            .execute(&pool)
            .await
            .unwrap();
    }

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_grandchilds_flip_recounts_and_rebroadcasts_every_ancestor() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut requests = bus.take_archive_recounts().unwrap();
    let top = spawn(&bus, None).await;
    let mid = spawn(&bus, Some(top)).await;
    let leaf = spawn(&bus, Some(mid)).await;
    settle(&pool, mid, "status = 'idle'").await;
    settle(&pool, leaf, "status = 'idle'").await;
    drain(&mut requests);
    recount_from(&pool, &bus, &[mid]).await.unwrap();
    assert_eq!(stored(&pool, top).await, 2, "mid and leaf are done");
    assert_eq!(stored(&pool, mid).await, 1, "leaf is done");

    let mut events = bus.subscribe();
    emit(&bus, leaf, ThreadEvent::ThreadSaved).await;
    let anchors = drain(&mut requests);
    assert_eq!(anchors, vec![mid], "the pin asks for its parent's recount");
    recount_from(&pool, &bus, &anchors).await.unwrap();
    assert_eq!(
        stored(&pool, mid).await,
        0,
        "a pinned leaf is not archivable"
    );
    assert_eq!(
        stored(&pool, top).await,
        1,
        "mid still counts: an archive leaves a pinned sub-thread open"
    );
    let latest: std::collections::HashMap<Uuid, i64> =
        rebroadcast(&mut events).into_iter().collect();
    assert_eq!(
        latest,
        std::collections::HashMap::from([(top, 1), (mid, 0)]),
        "every moved row is announced with its new count, last"
    );

    emit(&bus, leaf, question(leaf)).await;
    assert_eq!(
        drain(&mut requests),
        vec![mid],
        "a question moves an input the row does not hold"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_step_that_moves_no_archive_input_asks_for_no_recount() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut requests = bus.take_archive_recounts().unwrap();
    let parent = spawn(&bus, None).await;
    let child = spawn(&bus, Some(parent)).await;
    drain(&mut requests);

    emit(
        &bus,
        child,
        ThreadEvent::ThreadTitleGenerated {
            title: "Renamed".into(),
        },
    )
    .await;
    emit(
        &bus,
        child,
        ThreadEvent::TextStreamed {
            text: "token".into(),
        },
    )
    .await;
    assert!(drain(&mut requests).is_empty(), "no family query per step");

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_post_commit_diff_outside_emit_asks_for_a_recount() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut requests = bus.take_archive_recounts().unwrap();
    let parent = spawn(&bus, None).await;
    let child = spawn(&bus, Some(parent)).await;
    settle(&pool, child, "status = 'idle'").await;
    recount_from(&pool, &bus, &[parent]).await.unwrap();
    assert_eq!(stored(&pool, parent).await, 1, "the idle child is done");
    drain(&mut requests);

    assert!(bus
        .refresh_branch_work_and_broadcast(child, true)
        .await
        .unwrap());
    let anchors = drain(&mut requests);
    assert_eq!(anchors, vec![child], "the hook's write asks for a recount");
    recount_from(&pool, &bus, &anchors).await.unwrap();
    assert_eq!(
        stored(&pool, parent).await,
        0,
        "unproposed work needs the user, so the child is not archivable"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_detach_recounts_the_former_parent_itself() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut requests = bus.take_archive_recounts().unwrap();
    let parent = spawn(&bus, None).await;
    let child = spawn(&bus, Some(parent)).await;
    settle(&pool, parent, "status = 'idle'").await;
    settle(&pool, child, "status = 'idle'").await;
    recount_from(&pool, &bus, &[parent]).await.unwrap();
    assert_eq!(stored(&pool, parent).await, 1, "the idle child is done");
    drain(&mut requests);

    emit(
        &bus,
        parent,
        ThreadEvent::ChildThreadDetached {
            child_thread_id: child,
            child_thread_title: None,
        },
    )
    .await;
    let anchors = drain(&mut requests);
    assert!(anchors.contains(&parent), "got {anchors:?}");
    recount_from(&pool, &bus, &anchors).await.unwrap();
    assert_eq!(stored(&pool, parent).await, 0, "the child left the family");

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_boot_recount_fills_existing_families_and_clears_stale_counts() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = spawn(&bus, None).await;
    let child = spawn(&bus, Some(parent)).await;
    let lone = spawn(&bus, None).await;
    settle(&pool, child, "status = 'idle'").await;
    sqlx::query(
        "UPDATE thread_summaries SET archivable_sub_thread_count = \
         CASE WHEN thread_id = $1 THEN 0 ELSE 5 END WHERE thread_id IN ($1, $2)",
    )
    .bind(parent)
    .bind(lone)
    .execute(&pool)
    .await
    .unwrap();

    recount_all(&pool, &bus).await.unwrap();
    assert_eq!(
        stored(&pool, parent).await,
        1,
        "an existing family is filled"
    );
    assert_eq!(stored(&pool, lone).await, 0, "a stale count is cleared");

    teardown_test_db(&db).await;
}
