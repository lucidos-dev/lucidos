use super::*;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

#[tokio::test]
async fn every_internal_fault_kind_is_persisted_and_broadcast_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();
    for kind in FaultKind::ALL {
        for _ in 0..2 {
            bus.emit(BusEvent::System(SystemEvent::InternalFaultRecorded {
                kind,
            }))
            .await
            .unwrap();
        }
    }
    assert_eq!(count(&bus).await.unwrap(), FaultKind::ALL.len() as i64);
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE event_type = 'InternalFaultRecorded'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, FaultKind::ALL.len() as i64);
    for _ in FaultKind::ALL {
        rx.try_recv().unwrap();
    }
    assert!(rx.try_recv().is_err());
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn internal_faults_dedupe_across_buses_restart_and_reset_on_sent() {
    let (pool, db) = setup_test_db().await;
    let (a, _) = EventBus::new(pool.clone());
    let (b, _) = EventBus::new(pool.clone());
    let receipt = uuid::Uuid::new_v4();
    a.emit_internal_fault(receipt, FaultKind::Panic)
        .await
        .unwrap();
    let emit = |bus: EventBus| async move {
        bus.emit(BusEvent::System(SystemEvent::InternalFaultRecorded {
            kind: FaultKind::Panic,
        }))
        .await
        .unwrap();
    };
    tokio::join!(emit(a.clone()), emit(b));
    assert_eq!(count(&a).await.unwrap(), 1);
    a.emit(BusEvent::System(SystemEvent::InternalFaultRecorded {
        kind: FaultKind::TaskPanic,
    }))
    .await
    .unwrap();
    assert_eq!(count(&a).await.unwrap(), 2);
    a.emit(BusEvent::System(SystemEvent::DiagnosticBundleSent {
        destination: "maintainer".into(),
        report_code: Some("test-report".into()),
        size_bytes: 123,
        sources: vec!["engine".into()],
        actor: None,
    }))
    .await
    .unwrap();
    let (restarted, _) = EventBus::new(pool.clone());
    assert_eq!(count(&restarted).await.unwrap(), 0);
    restarted
        .emit_internal_fault(receipt, FaultKind::Panic)
        .await
        .unwrap();
    assert_eq!(count(&restarted).await.unwrap(), 0);
    emit(restarted.clone()).await;
    assert_eq!(count(&restarted).await.unwrap(), 1);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn deduplicated_receipts_also_survive_a_report_boundary() {
    let (pool, db) = setup_test_db().await;
    let (bus, _) = EventBus::new(pool);
    let first = uuid::Uuid::new_v4();
    let duplicate = uuid::Uuid::new_v4();
    bus.emit_internal_fault(first, FaultKind::Panic)
        .await
        .unwrap();
    bus.emit_internal_fault(duplicate, FaultKind::Panic)
        .await
        .unwrap();
    bus.emit(BusEvent::System(SystemEvent::DiagnosticBundleSent {
        destination: "maintainer".into(),
        report_code: None,
        size_bytes: 123,
        sources: vec!["engine".into()],
        actor: None,
    }))
    .await
    .unwrap();
    bus.emit_internal_fault(duplicate, FaultKind::Panic)
        .await
        .unwrap();
    assert_eq!(count(&bus).await.unwrap(), 0);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn failed_fault_emit_retains_the_inbox_and_rolls_back_its_receipt() {
    let (pool, db) = setup_test_db().await;
    let (bus, _) = EventBus::new(pool.clone());
    crate::test_support::refuse_event_type(&pool, "InternalFaultRecorded").await;
    let root = tempfile::tempdir().unwrap();
    let inbox = Inbox::new(root.path());
    inbox.record(FaultKind::TaskPanic).unwrap();
    assert!(drain(&inbox, &bus).await.is_err());
    assert_eq!(inbox.pending().unwrap().len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM internal_fault_receipts")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TRIGGER refuse_event ON events")
        .execute(&pool)
        .await
        .unwrap();
    drain(&inbox, &bus).await.unwrap();
    assert_eq!(count(&bus).await.unwrap(), 1);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn pending_internal_faults_survive_boot_and_replay_only_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _) = EventBus::new(pool);
    let root = tempfile::tempdir().unwrap();
    let inbox = Inbox::new(root.path());
    inbox.record(FaultKind::BootFailure).unwrap();
    let pending = inbox.pending().unwrap();
    assert_eq!(pending.len(), 1);
    drain(&inbox, &bus).await.unwrap();
    assert_eq!(count(&bus).await.unwrap(), 1);
    drain(&inbox, &bus).await.unwrap();
    assert_eq!(count(&bus).await.unwrap(), 1);
    assert!(inbox.pending().unwrap().is_empty());
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn snapshot_barrier_uses_the_only_worker_and_propagates_drain_errors() {
    let (pool, db) = setup_test_db().await;
    let (bus, _) = EventBus::new(pool.clone());
    let root = tempfile::tempdir().unwrap();
    let inbox = Inbox::new(root.path());
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let worker = tokio::spawn(run_worker(inbox.clone(), bus.clone(), rx));
    inbox.record(FaultKind::Panic).unwrap();
    let (reply, result) = tokio::sync::oneshot::channel();
    tx.send(reply).await.unwrap();
    result.await.unwrap().unwrap();
    assert_eq!(count(&bus).await.unwrap(), 1);
    assert!(inbox.pending().unwrap().is_empty());
    crate::test_support::refuse_event_type(&pool, "InternalFaultRecorded").await;
    inbox.record(FaultKind::TaskPanic).unwrap();
    let (reply, result) = tokio::sync::oneshot::channel();
    tx.send(reply).await.unwrap();
    assert!(result.await.unwrap().is_err());
    assert_eq!(inbox.pending().unwrap().len(), 1);
    drop(tx);
    worker.await.unwrap();
    teardown_test_db(&db).await;
}
