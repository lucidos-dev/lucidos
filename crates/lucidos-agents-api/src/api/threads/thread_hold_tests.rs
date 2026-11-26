use crate::api::gateway_stamp::test_support::stamped_device_headers;
use crate::core::subprocess_origin::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
};
use crate::engine::thread_events::{MessageOrigin, ThreadEvent};
use crate::engine::thread_hold::thread_hold;
use crate::test_support::{
    boot_mock_engine, in_engine_child, seed_coding_thread, setup_test_db, teardown_test_db,
};
use axum::{body::Body, http::Request};
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Clone, Copy, Debug)]
enum Caller {
    Agent(Option<Uuid>),
    Owner,
}

#[tokio::test]
async fn hold_reach_and_resume_authority_through_the_router() {
    if !in_engine_child(
        "api::threads::thread_hold::tests::hold_reach_and_resume_authority_through_the_router",
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
        std::sync::Arc::new(crate::api::hooks::NoEventHooks),
    );
    init_agent_origin_secret("thread-hold-test-secret".into());
    let call = |caller: Caller, thread: Uuid, method: &'static str| {
        let app = app.clone();
        async move {
            let mut request = Request::builder()
                .method(method)
                .uri(format!("/api/v1/threads/{thread}/hold"))
                .header("content-type", "application/json");
            match caller {
                Caller::Agent(source) => {
                    request = request.header(
                        HEADER_AGENT_ORIGIN_TOKEN,
                        mint_agent_origin_token(source, 0, None).unwrap(),
                    );
                }
                Caller::Owner => {
                    for (name, value) in &stamped_device_headers("test-device") {
                        request = request.header(name, value);
                    }
                }
            }
            let response = app
                .oneshot(
                    request
                        .body(Body::from(r#"{"reason":"checkpoint"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            response.status().as_u16()
        }
    };
    for (caller, thread, expected) in [
        (Caller::Agent(Some(unrelated)), target, 403),
        (Caller::Agent(None), target, 403),
        (Caller::Agent(Some(home)), target, 200),
        (Caller::Agent(Some(unrelated)), unrelated, 200),
    ] {
        assert_eq!(call(caller, thread, "POST").await, expected, "{caller:?}");
    }
    let reached = thread_hold(&pool, child).await.unwrap().expect("reached");
    assert_eq!(reached.root_thread_id, unrelated);

    for caller in [
        Caller::Agent(Some(home)),
        Caller::Agent(Some(unrelated)),
        Caller::Agent(Some(target)),
    ] {
        assert_eq!(call(caller, target, "DELETE").await, 403, "{caller:?}");
    }
    let mut owner_message = crate::test_support::message_received("resume when ready");
    if let ThreadEvent::MessageReceived { origin, .. } = &mut owner_message {
        *origin = Some(MessageOrigin::Device {
            device_id: "test-device".into(),
        });
    }
    crate::test_support::seed_thread_event(bus, target, owner_message).await;
    assert_eq!(
        call(Caller::Agent(Some(target)), target, "DELETE").await,
        200,
        "the thread's own agent resumes inside the owner's turn"
    );
    assert!(thread_hold(&pool, target).await.unwrap().is_none());

    assert_eq!(
        call(Caller::Owner, child, "DELETE").await,
        400,
        "a reached sub-thread resumes from its root"
    );
    assert_eq!(call(Caller::Owner, unrelated, "DELETE").await, 200);
    assert!(thread_hold(&pool, unrelated).await.unwrap().is_none());
    assert!(
        thread_hold(&pool, child).await.unwrap().is_none(),
        "Resume lifts the sub-thread its Hold reached"
    );
    drop(app);
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}
