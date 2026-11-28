use super::*;

async fn next_thread_frame(
    rx: &mut tokio::sync::broadcast::Receiver<EmittedEvent>,
    event_type: &str,
) -> EmittedEvent {
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("a frame within 5s")
            .expect("the bus stays open");
        if let BusEvent::Thread { event, .. } = &frame.typed {
            if event.event_type() == event_type {
                return frame;
            }
        }
    }
}

async fn next_system_frame(
    rx: &mut tokio::sync::broadcast::Receiver<EmittedEvent>,
    event_type: &str,
) -> EmittedEvent {
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("a frame within 5s")
            .expect("the bus stays open");
        if let BusEvent::System(se) = &frame.typed {
            if se.stored_event_type() == event_type {
                return frame;
            }
        }
    }
}

#[tokio::test]
async fn a_thread_event_carries_the_emitting_runs_trigger_depth() {
    use crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut frames = bus.subscribe();

    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "an ordinary turn").await;
    assert_eq!(
        next_thread_frame(&mut frames, "MessageReceived")
            .await
            .depth,
        0,
        "an event emitted outside any fire is nobody's chain link"
    );

    EVENT_TRIGGER_DEPTH
        .scope(2, async {
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ResponseGenerated {
                    text: "the fire's own answer".into(),
                    images: vec![],
                    model: None,
                    reasoning_effort: None,
                },
                meta: EventMeta {
                    channel: Some(EventChannel::Chat),
                    ..EventMeta::NONE
                },
            })
            .await
            .expect("emit inside the fire");
        })
        .await;
    assert_eq!(
        next_thread_frame(&mut frames, "ResponseGenerated")
            .await
            .depth,
        2,
        "a fire's own event carries the fire's depth, so the cap can see it"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_system_frame_carries_the_emitting_runs_trigger_depth() {
    use crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut frames = bus.subscribe();

    let recorded = |bus: &EventBus, status: &'static str| {
        let bus = bus.clone();
        async move {
            bus.emit(BusEvent::System(SystemEvent::TriggerExecuted {
                trigger_id: "nightly-backup".into(),
                payload: serde_json::json!({ "status": status }),
            }))
            .await
            .expect("emit")
        }
    };

    recorded(&bus, "success").await;
    assert_eq!(
        next_system_frame(&mut frames, "TriggerExecuted")
            .await
            .depth,
        0,
        "a scheduled run outside any chain starts one at zero"
    );

    EVENT_TRIGGER_DEPTH
        .scope(2, recorded(&bus, "failure"))
        .await;
    assert_eq!(
        next_system_frame(&mut frames, "TriggerExecuted")
            .await
            .depth,
        2,
        "the frame a fire records is a link in that fire's chain"
    );

    teardown_test_db(&db_name).await;
}

const CAP: usize = crate::engine::thread_queue::DEFAULT_MAX_CONCURRENT_CHILDREN_PER_THREAD;

async fn guard(pool: &sqlx::PgPool, parent: Uuid) -> Result<i32, String> {
    crate::engine::LucidosEngine::check_thread_recursion_guard(pool, parent, CAP).await
}

async fn spawn_children(bus: &EventBus, parent: Uuid, n: usize) -> Vec<Uuid> {
    let mut children = Vec::new();
    for i in 0..n {
        let child = Uuid::new_v4();
        emit_thread_message(bus, child, Some(parent), &format!("child {i}")).await;
        children.push(child);
    }
    children
}

async fn finish(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("finish");
}

async fn set_row(pool: &sqlx::PgPool, thread_id: Uuid, status: &str, live_waits: i32) {
    sqlx::query(
        "UPDATE thread_summaries SET status = $2, live_event_wait_count = $3 \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .bind(status)
    .bind(live_waits)
    .execute(pool)
    .await
    .expect("set row");
}
#[tokio::test]
async fn test_recursion_guard_allows_shallow_threads() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root task").await;

    let result = guard(&pool, root).await;
    assert!(result.is_ok(), "depth 0→1 should be allowed");
    assert_eq!(result.unwrap(), 1);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_enforces_max_depth() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let mut chain: Vec<Uuid> = Vec::new();
    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;
    chain.push(root);

    for i in 1..=crate::engine::child_limits::MAX_THREAD_DEPTH {
        let child = Uuid::new_v4();
        emit_thread_message(
            &bus,
            child,
            Some(*chain.last().unwrap()),
            &format!("child {}", i),
        )
        .await;
        chain.push(child);
    }

    let deepest = *chain.last().unwrap();
    let result = guard(&pool, deepest).await;
    assert!(result.is_err(), "spawning beyond max depth should fail");
    let err = result.unwrap_err();
    assert!(
        err.contains("Maximum thread nesting depth"),
        "error should mention depth limit: {}",
        err
    );

    let second_to_last = chain[chain.len() - 2];
    let result2 = guard(&pool, second_to_last).await;
    assert!(
        result2.is_ok(),
        "spawning at exactly max depth should succeed"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_depth_stored_in_projection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    let child = Uuid::new_v4();
    let grandchild = Uuid::new_v4();

    emit_thread_message(&bus, root, None, "root").await;
    emit_thread_message(&bus, child, Some(root), "child").await;
    emit_thread_message(&bus, grandchild, Some(child), "grandchild").await;

    let root_depth: i32 =
        sqlx::query_scalar("SELECT depth FROM thread_summaries WHERE thread_id = $1")
            .bind(root)
            .fetch_one(&pool)
            .await
            .unwrap();
    let child_depth: i32 =
        sqlx::query_scalar("SELECT depth FROM thread_summaries WHERE thread_id = $1")
            .bind(child)
            .fetch_one(&pool)
            .await
            .unwrap();
    let grandchild_depth: i32 =
        sqlx::query_scalar("SELECT depth FROM thread_summaries WHERE thread_id = $1")
            .bind(grandchild)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(root_depth, 0, "root thread should be depth 0");
    assert_eq!(child_depth, 1, "child thread should be depth 1");
    assert_eq!(grandchild_depth, 2, "grandchild thread should be depth 2");

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_unknown_parent_treated_as_root() {
    let (pool, db_name) = setup_test_db().await;

    let unknown = Uuid::new_v4();
    let result = guard(&pool, unknown).await;
    assert!(result.is_ok(), "unknown parent should default to depth 0");
    assert_eq!(result.unwrap(), 1);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_graceful_error_message() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let mut parent = Uuid::new_v4();
    emit_thread_message(&bus, parent, None, "root").await;
    for i in 1..=crate::engine::child_limits::MAX_THREAD_DEPTH {
        let child = Uuid::new_v4();
        emit_thread_message(&bus, child, Some(parent), &format!("level {}", i)).await;
        parent = child;
    }

    let result = guard(&pool, parent).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("Cannot spawn further child threads"),
        "error should guide the LLM: {}",
        err
    );
    assert!(
        err.contains("complete the task in this thread"),
        "error should suggest alternative: {}",
        err
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_parallel_children_within_limit() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;

    for i in 0..CAP {
        let child = Uuid::new_v4();
        emit_thread_message(&bus, child, Some(root), &format!("child {}", i)).await;

        let result = guard(&pool, child).await;
        assert!(result.is_ok(), "child {}'s children should be allowed", i);
        assert_eq!(result.unwrap(), 2);
    }

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_enforces_max_live_children() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;
    spawn_children(&bus, root, CAP).await;

    let err = guard(&pool, root)
        .await
        .expect_err("a full set of live children refuses");
    assert!(
        err.contains("at the same time") && err.contains("A slot frees"),
        "the refusal names a limit at one moment and how a slot frees: {err}"
    );
    assert!(
        !err.to_lowercase().contains("top-level") && !err.to_lowercase().contains("top level"),
        "the refusal must not send the work out of the family: {err}"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn finished_children_free_their_slots() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;
    for child in spawn_children(&bus, root, CAP).await {
        finish(&bus, child).await;
    }
    assert_eq!(
        guard(&pool, root).await,
        Ok(1),
        "ten finished children hold nothing"
    );

    let failed = spawn_children(&bus, root, 1).await[0];
    set_row(&pool, failed, "failed", 0).await;
    assert_eq!(guard(&pool, root).await, Ok(1));

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn every_live_state_holds_a_slot() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    for (status, live_waits) in [
        ("waiting_for_user_answer", 0),
        ("paused", 0),
        ("idle", 1),
        ("failed", 1),
    ] {
        let root = Uuid::new_v4();
        emit_thread_message(&bus, root, None, "root").await;
        let children = spawn_children(&bus, root, CAP).await;
        set_row(&pool, children[0], status, live_waits).await;
        assert!(
            guard(&pool, root).await.is_err(),
            "a child at {status} with {live_waits} live waits must hold its slot"
        );

        set_row(&pool, children[0], "idle", 0).await;
        assert_eq!(
            guard(&pool, root).await,
            Ok(1),
            "and frees it once finished"
        );
    }

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_queued_child_holds_a_slot() {
    use crate::engine::thread_queue::{ThreadQueueKind, ThreadQueueRequest};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;
    spawn_children(&bus, root, CAP - 1).await;
    assert_eq!(guard(&pool, root).await, Ok(1));

    let queued = Uuid::new_v4();
    let request = ThreadQueueRequest::SubThread {
        hook_args: Default::default(),
        prompt: "queued work".into(),
        child_thread_id: queued,
        depth: 0,
        parent_thread_id: Some(root),
        spawning_event_id: None,
        wake_mode: None,
        title: None,
        model: None,
        reasoning_effort: None,
        provider: None,
        pre_emitted_origin: None,
        origin: None,
    };
    bus.emit(BusEvent::System(SystemEvent::ThreadQueued {
        entry_id: Uuid::new_v4(),
        kind: ThreadQueueKind::SubThread,
        trigger_id: None,
        trigger_name: None,
        thread_id: request.thread_id(),
        summary: "queued work".into(),
        request: serde_json::to_value(&request).unwrap(),
        requeued: false,
        actor: None,
    }))
    .await
    .expect("queue the child");

    assert!(
        guard(&pool, root).await.is_err(),
        "the queued child is the tenth live one"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_guard_enforces_the_cap_it_is_given() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;
    spawn_children(&bus, root, 2).await;

    let check = |cap| crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, root, cap);
    assert!(check(2).await.is_err());
    assert_eq!(check(3).await, Ok(1));

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_children_limit_per_parent_not_global() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;

    let mut first_child = Uuid::new_v4();
    for i in 0..CAP {
        let child = Uuid::new_v4();
        emit_thread_message(&bus, child, Some(root), &format!("root child {}", i)).await;
        if i == 0 {
            first_child = child;
        }
    }

    let result = guard(&pool, first_child).await;
    assert!(
        result.is_ok(),
        "child should have its own independent children budget"
    );

    teardown_test_db(&db_name).await;
}
