use super::*;
use crate::engine::event_bus::EventBus;
use crate::test_support::{setup_test_db, teardown_test_db};
use lucidos_diagnostic_log::bundle::{Bundle, Manifest, Source, TimelineLine};
use std::collections::BTreeMap;

fn bytes() -> Vec<u8> {
    let bundle = Bundle {
        manifest: Manifest {
            format_version: 1,
            sources: BTreeMap::new(),
            trimmed_lines: 0,
            gateway_version: "test".into(),
            gateway_build: "test".into(),
            release: "test".into(),
            versions: BTreeMap::new(),
            missing_versions: BTreeMap::new(),
            failure_window_days: 7,
            failure_row_limit: 2000,
            configured: BTreeMap::new(),
            engine_healthy: None,
        },
        timeline: (0..3000)
            .map(|i| TimelineLine {
                ts: format!("2026-11-16T12:{:02}:{:02}.000Z", i / 60, i % 60),
                source: Source::Engine,
                pid: 42,
                message: format!("example {i}"),
            })
            .collect(),
    };
    zstd::stream::encode_all(serde_json::to_vec(&bundle).unwrap().as_slice(), 1).unwrap()
}

#[test]
fn decoding_refuses_compressed_and_expanded_overflow_and_invalid_schema() {
    assert!(decode(&bytes()).is_ok());
    assert!(decode_with_limits(&bytes(), 1, 1024).is_err());
    assert!(decode_with_limits(&bytes(), 1024 * 1024, 100).is_err());
    for input in [b"not zstd".as_slice(), b"{}", b"null"] {
        let compressed = zstd::stream::encode_all(input, 1).unwrap();
        assert!(decode(&compressed).is_err());
    }
    let mut invalid = decode(&bytes()).unwrap();
    invalid.manifest.format_version = 2;
    let encoded =
        zstd::stream::encode_all(serde_json::to_vec(&invalid).unwrap().as_slice(), 1).unwrap();
    assert!(decode(&encoded).is_err());
}

#[tokio::test]
async fn receipt_and_event_are_atomic_deduplicated_across_buses_and_restart() {
    let (pool, db) = setup_test_db().await;
    let (a, _) = EventBus::new(pool.clone());
    let (b, _) = EventBus::new(pool.clone());
    let mut rx = a.subscribe();
    let mut other_rx = b.subscribe();
    let receipt = Incoming::new("workspace:example", "delivery-1", &bytes()).unwrap();
    let (first, second) = tokio::join!(
        a.receive_diagnostic_bundle(&receipt),
        b.receive_diagnostic_bundle(&receipt)
    );
    assert_eq!(first.unwrap(), second.unwrap());
    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM diagnostic_bundles")
        .fetch_one(&pool)
        .await
        .unwrap();
    let events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE event_type = 'DiagnosticBundleReceived'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((stored, events), (1, 1));
    // Exactly one bus publishes; a retry publishes neither.
    let published = rx.try_recv().is_ok() as usize + other_rx.try_recv().is_ok() as usize;
    assert_eq!(published, 1);
    assert!(rx.try_recv().is_err());
    assert!(other_rx.try_recv().is_err());
    let (restarted, _) = EventBus::new(pool.clone());
    restarted.receive_diagnostic_bundle(&receipt).await.unwrap();
    assert!(rx.try_recv().is_err());
    let mut different = decode(&bytes()).unwrap();
    different.timeline[0].message = "different content".into();
    let different =
        zstd::stream::encode_all(serde_json::to_vec(&different).unwrap().as_slice(), 1).unwrap();
    let conflicting = Incoming::new("workspace:example", "delivery-1", &different).unwrap();
    assert!(restarted
        .receive_diagnostic_bundle(&conflicting)
        .await
        .is_err());
    let isolated = Incoming::new("workspace:another", "delivery-1", &different).unwrap();
    assert_ne!(
        restarted
            .receive_diagnostic_bundle(&isolated)
            .await
            .unwrap(),
        receipt.id
    );
    assert!(
        crate::core::event_subscription::is_subscribable_system_event(
            &crate::engine::event_bus::SystemEvent::DiagnosticBundleReceived {
                bundle_id: receipt.id,
                origin: receipt.origin.clone(),
                digest: receipt.digest.clone(),
                size_bytes: receipt.payload.len() as u64
            }
        )
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn failed_event_rolls_back_payload_and_expired_receipt_cannot_resurrect() {
    let _query_lock = crate::test_support::DIAGNOSTIC_QUERY_TEST_LOCK.lock().await;
    let (pool, db) = setup_test_db().await;
    let (bus, _) = EventBus::new(pool.clone());
    let receipt = Incoming::new("drop", "report-1", &bytes()).unwrap();
    sqlx::query("ALTER TABLE events ADD CONSTRAINT refuse_diagnostics CHECK (event_type <> 'DiagnosticBundleReceived')").execute(&pool).await.unwrap();
    assert!(bus.receive_diagnostic_bundle(&receipt).await.is_err());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM diagnostic_bundles")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    sqlx::query("ALTER TABLE events DROP CONSTRAINT refuse_diagnostics")
        .execute(&pool)
        .await
        .unwrap();
    bus.receive_diagnostic_bundle(&receipt).await.unwrap();
    sqlx::query("UPDATE diagnostic_bundles SET received = now() - interval '91 days'")
        .execute(&pool)
        .await
        .unwrap();
    let filter = crate::diagnostic_log::query::parse(None, chrono::Utc::now()).unwrap();
    assert!(query_received(&pool, receipt.id, &filter)
        .await
        .unwrap()
        .is_none());
    assert_eq!(bus.expire_diagnostic_bundles().await.unwrap(), 1);
    assert_eq!(bus.expire_diagnostic_bundles().await.unwrap(), 0);
    bus.receive_diagnostic_bundle(&receipt).await.unwrap();
    assert!(query_received(&pool, receipt.id, &filter)
        .await
        .unwrap()
        .is_none());
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE event_type = 'DiagnosticBundleReceived'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    teardown_test_db(&db).await;
}

#[test]
fn query_reuses_diagnostic_log_filters_and_line_byte_budgets() {
    let decoded = decode(&bytes()).unwrap();
    let now = chrono::DateTime::parse_from_rfc3339("2026-11-16T13:00:00Z")
        .unwrap()
        .to_utc();
    let q = crate::diagnostic_log::query::parse(
        Some("since=2h&limit=5000&byte_limit=9999999&contains=example"),
        now,
    )
    .unwrap();
    let result = query(decoded, &q).unwrap();
    assert_eq!(result.returned, 2000);
    assert_eq!(result.earlier_matches, 1000);
    assert!(result.byte_size <= 256 * 1024);
    assert_eq!(result.lines.last().unwrap().msg, "example 2999");
}
