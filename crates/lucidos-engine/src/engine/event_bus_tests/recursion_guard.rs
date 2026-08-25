use super::super::*;
use super::*;

/// The next broadcast frame carrying a thread event of this type, so an
/// interleaved projection broadcast cannot make the assertion below flaky.
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

/// The next broadcast frame carrying a system event of this type.
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

/// The plumbing Bug 2 was missing. `MAX_EVENT_TRIGGER_DEPTH` can only reach a
/// thread event if the emitting run's chain depth does, and the bus is where
/// that depth is read.
///
/// The scope here is the one `thread_queue::executor` puts around a fire, and
/// the read is the one `EventBus::emit` does. A user's own turn is outside any
/// scope and must stay at zero, or the cap would start refusing ordinary work.
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

/// The same read, for the system carrier. `TriggerExecuted` is the frame a
/// fire's own completion writes. A trigger may subscribe to it, so it is the
/// one that has to carry the fire's depth.
///
/// `thread_queue::executor` records it INSIDE the fire's
/// `EVENT_TRIGGER_DEPTH.scope`, which is what makes this read answer the fire's
/// depth. Recording it after the scope resolved stamped 0 and left such a
/// trigger uncapped.
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

// --- Recursion guard tests ---
#[tokio::test]
async fn test_recursion_guard_allows_shallow_threads() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root task").await;

    // Spawning a child from root (depth 0 → child depth 1) should succeed
    let result = crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, root).await;
    assert!(result.is_ok(), "depth 0→1 should be allowed");
    assert_eq!(result.unwrap(), 1);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_enforces_max_depth() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    // Build a chain of threads at increasing depths
    let mut chain: Vec<Uuid> = Vec::new();
    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;
    chain.push(root);

    // Build chain up to MAX_THREAD_DEPTH
    for i in 1..=crate::engine::chat::MAX_THREAD_DEPTH {
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

    // Last thread in chain is at MAX_THREAD_DEPTH — spawning from it should fail
    let deepest = *chain.last().unwrap();
    let result = crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, deepest).await;
    assert!(result.is_err(), "spawning beyond max depth should fail");
    let err = result.unwrap_err();
    assert!(
        err.contains("Maximum thread nesting depth"),
        "error should mention depth limit: {}",
        err
    );

    // But spawning from the second-to-last should still succeed
    let second_to_last = chain[chain.len() - 2];
    let result2 =
        crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, second_to_last).await;
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

    // Verify depths in thread_summaries
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

    // Check guard for a thread_id that doesn't exist in thread_summaries
    let unknown = Uuid::new_v4();
    let result = crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, unknown).await;
    assert!(result.is_ok(), "unknown parent should default to depth 0");
    assert_eq!(result.unwrap(), 1);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_graceful_error_message() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    // Build a chain at max depth
    let mut parent = Uuid::new_v4();
    emit_thread_message(&bus, parent, None, "root").await;
    for i in 1..=crate::engine::chat::MAX_THREAD_DEPTH {
        let child = Uuid::new_v4();
        emit_thread_message(&bus, child, Some(parent), &format!("level {}", i)).await;
        parent = child;
    }

    // Try to spawn from the deepest — should get clear error
    let result = crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, parent).await;
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

    // Spawn children up to the limit — all should succeed
    for i in 0..crate::engine::chat::MAX_CHILDREN_PER_THREAD {
        let child = Uuid::new_v4();
        emit_thread_message(&bus, child, Some(root), &format!("child {}", i)).await;

        // Each child should be allowed to spawn its own children
        let result = crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, child).await;
        assert!(result.is_ok(), "child {}'s children should be allowed", i);
        assert_eq!(result.unwrap(), 2);
    }

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_enforces_max_children() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;

    // Fill up the children limit
    for i in 0..crate::engine::chat::MAX_CHILDREN_PER_THREAD {
        let child = Uuid::new_v4();
        emit_thread_message(&bus, child, Some(root), &format!("child {}", i)).await;
    }

    // Now trying to spawn from root should fail — max children reached
    let result = crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, root).await;
    assert!(result.is_err(), "should reject when max children reached");
    let err = result.unwrap_err();
    assert!(
        err.contains("Maximum child threads per parent"),
        "error should mention children limit: {}",
        err
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn test_recursion_guard_children_limit_per_parent_not_global() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let root = Uuid::new_v4();
    emit_thread_message(&bus, root, None, "root").await;

    // Fill root's children limit
    let mut first_child = Uuid::new_v4();
    for i in 0..crate::engine::chat::MAX_CHILDREN_PER_THREAD {
        let child = Uuid::new_v4();
        emit_thread_message(&bus, child, Some(root), &format!("root child {}", i)).await;
        if i == 0 {
            first_child = child;
        }
    }

    // Root is full, but first_child should still be able to spawn its own children
    let result =
        crate::engine::LucidosEngine::check_thread_recursion_guard(&pool, first_child).await;
    assert!(
        result.is_ok(),
        "child should have its own independent children budget"
    );

    teardown_test_db(&db_name).await;
}
