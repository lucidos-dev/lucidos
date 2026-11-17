use crate::api::actor::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
};
use crate::test_support::{
    boot_mock_engine, in_engine_child, seed_coding_thread, setup_test_db, teardown_test_db,
};
use axum::{body::Body, http::Request};
use tower::ServiceExt;

/// Home may pause across families, but gains no authority to unpause.
#[tokio::test]
async fn pause_reach_and_owner_only_unpause_through_the_router() {
    if !in_engine_child(
        "api::threads::automation_pause::tests::pause_reach_and_owner_only_unpause_through_the_router",
    )
    .await
    {
        return;
    }
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    let home = crate::engine::home_thread::ensure_home_thread(bus, &pool)
        .await
        .unwrap();
    let unrelated = seed_coding_thread(bus, None).await;
    let target = seed_coding_thread(bus, None).await;
    let child = seed_coding_thread(bus, Some(unrelated)).await;
    let scheduler = crate::scheduler::SchedulerManager::new(engine.clone(), pool.clone())
        .await
        .unwrap();
    let app = crate::api::create_router(
        engine.clone(),
        pool.clone(),
        engine.event_store().clone(),
        crate::memory::EmbedderSlot::empty(),
        None,
        engine.workspace_path().to_path_buf(),
        std::sync::Arc::new(tokio::sync::Mutex::new(scheduler)),
        chrono::Utc::now(),
        &crate::net_config::BindChoice::Loopback,
    );
    init_agent_origin_secret("automation-pause-test-secret".into());
    for (caller, thread, method, expected) in [
        (Some(unrelated), target, "POST", 403),
        (None, target, "POST", 403),
        (Some(unrelated), child, "POST", 200),
        (Some(unrelated), unrelated, "POST", 200),
        (Some(home), target, "POST", 200),
        (Some(home), target, "DELETE", 403),
    ] {
        let token = mint_agent_origin_token(caller, 0, None).unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/v1/threads/{thread}/automation-pause"))
                    .header(HEADER_AGENT_ORIGIN_TOKEN, token)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"reason":"checkpoint"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected, "{caller:?} {method}");
        if expected == 200 {
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body, serde_json::json!({"paused": true, "changed": true}));
        }
    }
    assert!(crate::engine::automation_pause::automation_is_paused(&pool, target).await);
    drop(app);
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}
