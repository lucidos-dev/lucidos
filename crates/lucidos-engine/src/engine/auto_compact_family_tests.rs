use crate::engine::event_bus::EventBus;
use crate::engine::thread_family::family_root;
use crate::test_support::{seed_chat_thread, setup_test_db, teardown_test_db};

#[tokio::test]
async fn a_subthread_uses_the_root_family_arm() {
    let (pool, db) = setup_test_db().await;
    let (bus, _) = EventBus::new(pool.clone());
    let root = seed_chat_thread(&bus, "root").await;
    let grandchild = seed_chat_thread(&bus, "child").await;
    sqlx::query("UPDATE thread_summaries SET parent_thread_id = $1 WHERE thread_id = $2")
        .bind(root)
        .bind(grandchild)
        .execute(&pool)
        .await
        .unwrap();
    let arm = crate::engine::auto_compact_trial::arm_for_family;
    assert_eq!(
        arm(family_root(&pool, grandchild).await.unwrap()),
        arm(root),
        "a sub-thread runs in its root's arm"
    );
    teardown_test_db(&db).await;
}
