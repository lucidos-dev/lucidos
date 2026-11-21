use super::*;
use crate::api::permission_requests::tests::{agent_thread, answer_and_tap, ask_prepared, grant};
use crate::engine::permission::menu::{ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL};
use crate::test_support::{setup_test_db, teardown_test_db};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn change(channel: &str) -> Request {
    Request {
        method: "PUT".into(),
        path: "/release-check".into(),
        body: json!({"channel":channel}),
        reason: "Test this update".into(),
    }
}

#[test]
fn route_table_and_request_shape_are_closed() {
    for (method, path) in [
        ("POST", "/release-check"),
        ("GET", "/release-check"),
        ("PUT", "/gateway/status"),
        ("GET", "/pairing"),
        ("GET", "/gateway/status?x"),
        ("GET", "/gateway/../status"),
    ] {
        let mut req = change("local");
        req.method = method.into();
        req.path = path.into();
        req.body = Value::Null;
        assert!(req.checked_route().is_err(), "{method} {path}");
    }
    for extra in [json!({"port":1234}), json!({"url":"http://example.com"})] {
        let mut req = json!({"method":"GET","path":"/gateway/status"});
        req.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(serde_json::from_value::<Request>(req).is_err());
    }
    let a: Request = serde_json::from_str(r#"{"method":"PUT","path":"/release-check","body":{"channel":"local","nested":{"z":1,"a":2}},"reason":"test"}"#).unwrap();
    let b: Request = serde_json::from_str(r#"{"method":"PUT","path":"/release-check","body":{"nested":{"a":2,"z":1},"channel":"local"},"reason":"another reason"}"#).unwrap();
    assert_eq!(
        a.ask(a.checked_route().unwrap()).unwrap().breadth,
        b.ask(b.checked_route().unwrap()).unwrap().breadth
    );
}

#[tokio::test]
async fn only_exact_owner_consent_reaches_the_gateway_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    let other = agent_thread(&bus).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let app = Router::new()
        .route(
            "/~/api/v1/control/gateway/status",
            axum::routing::get(|| async { Json(json!({"release_check":{"channel":"stable"}})) }),
        )
        .route(
            "/~/api/v1/control/release-check",
            axum::routing::put(move || {
                count.fetch_add(1, Ordering::SeqCst);
                async { Json(json!({"channel":"local"})) }
            }),
        );
    let server = lucidos_local_token::test_support::serve_https(app);
    let port = server.port().to_string();
    let read = Request {
        method: "GET".into(),
        path: "/gateway/status".into(),
        body: Value::Null,
        reason: String::new(),
    };
    assert_eq!(
        execute(&bus, thread, read, &port).await.unwrap().0["response"]["release_check"]["channel"],
        "stable"
    );
    let first = execute(&bus, thread, change("local"), &port)
        .await
        .unwrap()
        .0;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let card = ask_prepared(&bus, thread, first["request_id"].as_str().unwrap()).await;
    let declined = card.pick(DONT_ALLOW_LABEL);
    answer_and_tap(&bus, thread, &card, declined).await;
    assert!(execute(&bus, thread, change("local"), &port)
        .await
        .unwrap()
        .0["request_id"]
        .is_string());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let req = change("local");
    grant(
        &bus,
        thread,
        req.ask(req.checked_route().unwrap()).unwrap(),
        ALLOW_ONCE_LABEL,
    )
    .await;
    let (reopened, _rx) = EventBus::new(pool.clone());
    assert!(execute(&reopened, other, change("local"), &port)
        .await
        .unwrap()
        .0["request_id"]
        .is_string());
    assert!(execute(&reopened, thread, change("stable"), &port)
        .await
        .unwrap()
        .0["request_id"]
        .is_string());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        execute(&reopened, thread, change("local"), &port)
            .await
            .unwrap()
            .0["response"]["channel"],
        "local"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(execute(&reopened, thread, change("local"), &port)
        .await
        .unwrap()
        .0["request_id"]
        .is_string());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.abort();
    pool.close().await;
    teardown_test_db(&db).await;
}
