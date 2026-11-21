use super::*;
use axum::body::Body;
use lucidos_gateway_stamp::HEADER_GATEWAY_STAMP;

#[tokio::test]
async fn receiver_requires_diagnostic_stamp_before_reading_body() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (bus, _) = crate::engine::event_bus::EventBus::new(pool);
    static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let valid = super::super::gateway_stamp::test_support::diagnostic_headers(
        "source",
        "once",
        &"a".repeat(64),
    );
    let mut wrong_purpose = valid.clone();
    wrong_purpose.insert(
        HEADER_GATEWAY_STAMP,
        super::super::gateway_stamp::test_support::stamp_for("device")
            .parse()
            .unwrap(),
    );
    let mut changed = valid;
    changed.insert(
        lucidos_gateway_stamp::HEADER_DIAGNOSTIC_ORIGIN,
        "other".parse().unwrap(),
    );
    for headers in [HeaderMap::new(), wrong_purpose, changed] {
        let stream = futures::stream::poll_fn(
            |_| -> std::task::Poll<Option<Result<Vec<u8>, std::io::Error>>> {
                panic!("unauthenticated body was polled")
            },
        );
        let mut req = Request::new(Body::from_stream(stream));
        *req.headers_mut() = headers;
        let err = ingest(&bus, req, &SLOTS).await.unwrap_err();
        assert_eq!(err.status, StatusCode::FORBIDDEN);
    }
    crate::test_support::teardown_test_db(&db).await;
}

fn signed_request(bytes: Vec<u8>, digest_bytes: &[u8], delivery: &str) -> Request {
    let headers = super::super::gateway_stamp::test_support::diagnostic_headers(
        "source",
        delivery,
        &format!("{:x}", Sha256::digest(digest_bytes)),
    );
    let mut req = Request::new(Body::from(bytes));
    *req.headers_mut() = headers;
    req
}

#[tokio::test]
async fn receiver_bounds_body_rejects_digest_changes_and_hides_decoder_errors() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (bus, _) = crate::engine::event_bus::EventBus::new(pool);
    static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let invalid = zstd::stream::encode_all(b"{PRIVATE_MARKER".as_slice(), 3).unwrap();
    for request in [
        signed_request(invalid.clone(), &invalid, "invalid"),
        signed_request(invalid, b"changed", "digest"),
    ] {
        let err = ingest(&bus, request, &SLOTS).await.unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(!format!("{err:?}").contains("PRIVATE_MARKER"));
    }
    let large = vec![0; MAX_COMPRESSED_BYTES + 1];
    let err = ingest(&bus, signed_request(large, b"large", "large"), &SLOTS)
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::PAYLOAD_TOO_LARGE);
    let permit = SLOTS.acquire().await.unwrap();
    let err = ingest(&bus, signed_request(vec![], b"", "busy"), &SLOTS)
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::TOO_MANY_REQUESTS);
    drop(permit);
    crate::test_support::teardown_test_db(&db).await;
}

fn bundle_bytes() -> Vec<u8> {
    let value = serde_json::json!({
        "manifest": {"format_version": 1, "sources": {}, "trimmed_lines": 0,
            "gateway_version": "test", "gateway_build": "test", "release": "test",
            "versions": {}, "missing_versions": {}, "failure_window_days": 7,
            "failure_row_limit": 2000, "configured": {}, "engine_healthy": true},
        "timeline": (0..100).map(|i| serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339(), "source": "engine", "pid": 42,
            "message": format!("example {i}")
        })).collect::<Vec<_>>()
    });
    zstd::stream::encode_all(serde_json::to_vec(&value).unwrap().as_slice(), 3).unwrap()
}

#[tokio::test]
async fn receipt_retries_are_atomic_and_query_is_agent_only_and_bounded() {
    let _query_lock = crate::test_support::DIAGNOSTIC_QUERY_TEST_LOCK.lock().await;
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (bus, _) = crate::engine::event_bus::EventBus::new(pool.clone());
    static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let bytes = bundle_bytes();
    let id = ingest(
        &bus,
        signed_request(bytes.clone(), &bytes, "receipt"),
        &SLOTS,
    )
    .await
    .unwrap();
    let retry = ingest(
        &bus,
        signed_request(bytes.clone(), &bytes, "receipt"),
        &SLOTS,
    )
    .await
    .unwrap();
    assert_eq!(id, retry);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE event_type = 'DiagnosticBundleReceived'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let changed = bundle_bytes();
    let err = ingest(
        &bus,
        signed_request(changed.clone(), &changed, "receipt"),
        &SLOTS,
    )
    .await
    .unwrap_err();
    assert_eq!(err.status, StatusCode::CONFLICT);
    assert_eq!(
        query_inbox(&pool, id, None, &HeaderMap::new())
            .await
            .unwrap_err()
            .status,
        StatusCode::FORBIDDEN
    );
    crate::core::subprocess_origin::init_agent_origin_secret("subprocess-test-secret".to_string());
    let mut headers = HeaderMap::new();
    headers.insert(
        crate::core::subprocess_origin::HEADER_AGENT_ORIGIN_TOKEN,
        crate::core::subprocess_origin::mint_agent_origin_token(None, 0, None)
            .unwrap()
            .parse()
            .unwrap(),
    );
    let result = query_inbox(&pool, id, Some("limit=5"), &headers)
        .await
        .unwrap();
    assert_eq!(result.returned, 5);
    assert_eq!(result.earlier_matches, 95);
    assert_eq!(
        query_inbox(&pool, id, Some("since=8d"), &headers)
            .await
            .unwrap_err()
            .status,
        StatusCode::BAD_REQUEST
    );
    headers.insert(
        super::super::app_reach::APP_ID_HEADER,
        "habit-tracker".parse().unwrap(),
    );
    assert_eq!(
        query_inbox(&pool, id, None, &headers)
            .await
            .unwrap_err()
            .status,
        StatusCode::FORBIDDEN
    );
    headers.remove(super::super::app_reach::APP_ID_HEADER);
    bus.expire_diagnostic_bundles().await.unwrap();
    sqlx::query("DELETE FROM diagnostic_bundles WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        query_inbox(&pool, id, None, &headers)
            .await
            .unwrap_err()
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        ingest(
            &bus,
            signed_request(bytes.clone(), &bytes, "receipt"),
            &SLOTS
        )
        .await
        .unwrap(),
        id
    );
    assert_eq!(
        query_inbox(&pool, id, None, &headers)
            .await
            .unwrap_err()
            .status,
        StatusCode::NOT_FOUND
    );
    crate::test_support::teardown_test_db(&db).await;
}
