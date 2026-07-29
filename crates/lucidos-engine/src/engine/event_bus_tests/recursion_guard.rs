use super::super::*;
use super::*;

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
