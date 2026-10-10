//! Tests for the disk monitor: one fresh-reading alert per crossing, and no
//! wait on a stalled database.

use super::common::*;
use crate::engine::event_bus::EventBus;
use crate::test_support::{setup_test_db, teardown_test_db};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

const LOW_DISK_TITLE: &str = "Low disk space on your machine";

async fn low_disk_alerts(
    rx: tokio::sync::broadcast::Receiver<crate::engine::event_bus::EmittedEvent>,
) -> Vec<CapturedNotification> {
    drain_notifications(rx, Duration::from_millis(200))
        .await
        .into_iter()
        .filter(|n| n.title == LOW_DISK_TITLE)
        .collect()
}

#[tokio::test]
async fn crossing_below_soft_sends_one_alert_with_the_reading() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (_tmp, root) = fresh_workspace().await;
    let (probe, _reading) = settable_probe(Some(12 * GB));
    let mut monitor = make_monitor(bus.clone(), root, probe);

    let rx = bus.subscribe();
    monitor.check_once().await;

    let alerts = low_disk_alerts(rx).await;
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    let body = &alerts[0].message;
    assert!(body.contains("Only 12.0 GB free"), "{body}");
    assert!(
        body.contains("volume hosting"),
        "body must call out the volume, not Lucidos itself: {body}"
    );
    // A tap has to land on the page that answers the alert.
    assert_eq!(alerts[0].settings_view(), Some("disk-usage"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The incident: a reading taken before a slow step was reported after disk
/// had recovered. The re-probe before the emit must win, and the monitor must
/// stay armed for a real crossing later.
#[tokio::test]
async fn a_stale_low_reading_followed_by_recovery_sends_no_alert() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (_tmp, root) = fresh_workspace().await;
    let mut monitor = make_monitor(
        bus.clone(),
        root,
        scripted_probe(&[12 * GB, 232 * GB, 9 * GB]),
    );

    let rx = bus.subscribe();
    monitor.check_once().await; // reads 12 GB, then 232 GB before emitting
    let after_recovery = low_disk_alerts(rx).await;
    assert!(
        after_recovery.is_empty(),
        "no alert when the fresh reading is above soft: {after_recovery:?}"
    );

    let rx = bus.subscribe();
    monitor.check_once().await; // a real crossing: 9 GB on both reads
    let alerts = low_disk_alerts(rx).await;
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert!(alerts[0].message.contains("Only 9.0 GB free"), "{alerts:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn staying_below_soft_does_not_re_alert() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (_tmp, root) = fresh_workspace().await;
    let (probe, reading) = settable_probe(Some(12 * GB));
    let mut monitor = make_monitor(bus.clone(), root, probe);

    let rx = bus.subscribe();
    monitor.check_once().await;
    *reading.lock().unwrap() = Some(8 * GB);
    monitor.check_once().await;
    // A failed probe must not re-arm the alert either.
    *reading.lock().unwrap() = None;
    monitor.check_once().await;
    *reading.lock().unwrap() = Some(10 * GB);
    monitor.check_once().await;

    let alerts = low_disk_alerts(rx).await;
    assert_eq!(alerts.len(), 1, "one alert per crossing: {alerts:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Recovery re-arms the alert only past the margin above soft, so jitter
/// across the threshold does not ping the user each minute.
#[tokio::test]
async fn the_alert_re_arms_only_after_recovery_past_the_margin() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (_tmp, root) = fresh_workspace().await;
    let (probe, reading) = settable_probe(None);
    let mut monitor = make_monitor(bus.clone(), root, probe);
    let soft = super::FREE_DISK_SOFT_BYTES;

    let rx = bus.subscribe();
    for free in [
        soft - GB / 10,     // crossing: alert
        soft + GB / 2,      // above soft, inside the band: stays sent
        soft - GB / 10,     // jitter back below: no alert
        soft + 2 * GB + GB, // past the band: re-arms
        soft - GB / 10,     // a new crossing: alert
    ] {
        *reading.lock().unwrap() = Some(free);
        monitor.check_once().await;
    }

    let alerts = low_disk_alerts(rx).await;
    assert_eq!(alerts.len(), 2, "{alerts:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn ample_free_disk_sends_no_alert() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (_tmp, root) = fresh_workspace().await;
    let (probe, _reading) = settable_probe(Some(232 * GB));
    let mut monitor = make_monitor(bus.clone(), root, probe);

    let rx = bus.subscribe();
    monitor.check_once().await;

    let notifications = drain_notifications(rx, Duration::from_millis(200)).await;
    assert!(notifications.is_empty(), "{notifications:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Small Lucidos footprint + low volume: the body steers the user to look
/// elsewhere on their machine, not at Lucidos.
#[tokio::test]
async fn a_tiny_lucidos_footprint_blames_the_machine() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (_tmp, root) = fresh_workspace().await; // no worktrees: footprint 0
    let (probe, _reading) = settable_probe(Some(12 * GB));
    let mut monitor = make_monitor(bus.clone(), root, probe);

    let rx = bus.subscribe();
    monitor.check_once().await;

    let alerts = low_disk_alerts(rx).await;
    assert_eq!(alerts.len(), 1);
    assert!(alerts[0].message.contains("other apps"), "{alerts:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Large Lucidos footprint + low volume: the body steers to the Disk Usage
/// page. The footprint comes from disk alone, with no thread lookup.
#[tokio::test]
async fn a_large_lucidos_footprint_suggests_cleanup() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (_tmp, root) = fresh_workspace().await;
    let _wt = add_worktree_for_thread(&root, Uuid::new_v4(), true).await;
    let (probe, _reading) = settable_probe(Some(12 * GB));
    let mut monitor = make_monitor(bus.clone(), root, probe);
    // A 1-byte boundary forces the large branch without writing gigabytes.
    monitor.large_footprint_bytes = 1;

    let rx = bus.subscribe();
    monitor.check_once().await;

    let alerts = low_disk_alerts(rx).await;
    assert_eq!(alerts.len(), 1);
    assert!(
        alerts[0].message.contains("Settings → System → Disk Usage"),
        "{alerts:?}"
    );
    assert!(alerts[0].message.contains("clean idle ones"), "{alerts:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// On a hung database the emit gives up within its bound, and the alert stays
/// armed. The retry reports the reading of the retry, never the first one.
#[tokio::test]
async fn a_stalled_emit_is_bounded_and_retried_with_a_fresh_reading() {
    let (_tmp, root) = fresh_workspace().await;
    let (stalled_bus, _stalled_rx) = EventBus::new(unresponsive_pool().await);
    let (probe, reading) = settable_probe(Some(12 * GB));
    let mut monitor = make_monitor(Arc::new(stalled_bus), root, probe);
    monitor.emit_timeout = Duration::from_millis(200);

    let started = Instant::now();
    monitor.check_once().await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a stalled emit must not hold the monitor: took {:?}",
        started.elapsed()
    );

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    monitor.bus = bus.clone();
    *reading.lock().unwrap() = Some(9 * GB);

    let rx = bus.subscribe();
    monitor.check_once().await;
    let alerts = low_disk_alerts(rx).await;
    assert_eq!(alerts.len(), 1, "the failed alert must retry: {alerts:?}");
    assert!(alerts[0].message.contains("Only 9.0 GB free"), "{alerts:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The cleanup cycle blocks on a hung database for every worktree. The
/// monitor is its own task and must alert while the cycle is still stuck.
#[tokio::test]
async fn a_cleanup_cycle_stalled_on_the_database_does_not_delay_the_alert() {
    let (_tmp, root) = fresh_workspace().await;
    // A `thread-<8hex>` directory is enough: the thread lookup comes first.
    std::fs::create_dir_all(crate::engine::git_ops::worktrees_dir(&root).join("thread-deadbeef"))
        .unwrap();
    let stalled_pool = unresponsive_pool().await;
    let (stalled_bus, _stalled_rx) = EventBus::new(stalled_pool.clone());
    let mut worker = make_worker(stalled_pool, Arc::new(stalled_bus), root.clone());
    worker.free_soft_bytes = u64::MAX;
    let cycle = tokio::spawn(async move { worker.run_once().await });

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (probe, _reading) = settable_probe(Some(12 * GB));
    let mut monitor = make_monitor(bus.clone(), root, probe);

    let rx = bus.subscribe();
    tokio::time::timeout(Duration::from_secs(5), monitor.check_once())
        .await
        .expect("the monitor must not wait on the stalled cycle");
    let alerts = low_disk_alerts(rx).await;
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert!(
        !cycle.is_finished(),
        "the cleanup cycle should still be blocked on the hung database"
    );

    cycle.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// One wake per worsening. A wake per tick would run a full cycle every
/// minute through a long low-disk episode.
#[tokio::test]
async fn the_monitor_wakes_cleanup_once_per_worsening() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, root) = fresh_workspace().await;
    let (probe, reading) = settable_probe(None);
    let mut monitor = make_monitor(Arc::new(bus), root, probe);
    let wake = monitor.cleanup_wake.clone();

    let mut woken = Vec::new();
    for free in [
        Some(100 * GB), // comfortable
        Some(12 * GB),  // into soft: wake
        Some(12 * GB),  // still soft
        None,           // a failed probe changes nothing
        Some(3 * GB),   // into hard: wake
        Some(3 * GB),   // still hard
        Some(12 * GB),  // recovering to soft
        Some(100 * GB), // comfortable again
        Some(12 * GB),  // a new crossing: wake
    ] {
        *reading.lock().unwrap() = free;
        monitor.check_once().await;
        woken.push(was_woken(&wake).await);
    }
    assert_eq!(
        woken,
        [false, true, false, false, true, false, false, false, true]
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The wake is in memory, so a hung database cannot hold it back.
#[tokio::test]
async fn the_monitor_wakes_cleanup_while_the_database_is_stalled() {
    let (_tmp, root) = fresh_workspace().await;
    let (stalled_bus, _stalled_rx) = EventBus::new(unresponsive_pool().await);
    let (probe, _reading) = settable_probe(Some(12 * GB));
    let mut monitor = make_monitor(Arc::new(stalled_bus), root, probe);
    monitor.emit_timeout = Duration::from_millis(200);
    let wake = monitor.cleanup_wake.clone();

    tokio::time::timeout(Duration::from_secs(5), monitor.check_once())
        .await
        .expect("a stalled emit must not hold the monitor");
    assert!(was_woken(&wake).await);
}
