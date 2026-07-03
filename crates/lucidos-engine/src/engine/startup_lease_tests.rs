//! Tests for the engine startup lease (`acquire_startup_lease`).
//!
//! These exercise the real Postgres advisory lock across independent
//! connections — the exact contention the fix relies on: a booting engine must
//! block on the lease until the previous engine's connection closes, and must
//! fail open (proceed degraded) rather than wedge boot when the predecessor is
//! hung.

use super::acquire_startup_lease;
use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
use std::time::Duration;

/// With no predecessor, the lease is acquired immediately (no added boot
/// latency), and a second attempt while the first is held is denied — the
/// serialization contract. Dropping the holder releases the lock so a later
/// attempt succeeds — the release-on-exit contract.
#[tokio::test]
async fn startup_lease_serializes_and_releases_on_drop() {
    let (pool, db_name) = setup_test_db().await;
    let url = test_db_url(&db_name);

    // First boot: uncontended → acquired right away.
    let lease_a = acquire_startup_lease(&url, Duration::from_secs(5)).await;
    assert!(
        lease_a.is_acquired(),
        "uncontended acquire must hold the lease immediately"
    );

    // Successor while A still holds it: must NOT acquire within its wait window.
    // Short window keeps the test fast; the point is it degrades rather than
    // stealing the lock.
    let lease_b = acquire_startup_lease(&url, Duration::from_millis(600)).await;
    assert!(
        !lease_b.is_acquired(),
        "a second engine must not acquire the lease while the first still holds it (must degrade)"
    );

    // Predecessor exits → its connection closes → lock releases.
    drop(lease_a);

    // A fresh successor now acquires (generous window absorbs the async
    // connection close after drop).
    let lease_c = acquire_startup_lease(&url, Duration::from_secs(10)).await;
    assert!(
        lease_c.is_acquired(),
        "after the holder drops, a new engine must be able to acquire the lease"
    );

    drop(lease_c);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Fail-open: a bad connection URL never panics and never blocks — it returns a
/// degraded (non-acquired) guard so boot proceeds. Guards against a lease-layer
/// failure wedging engine startup.
#[tokio::test]
async fn startup_lease_fails_open_on_connect_error() {
    // Point at a database that does not exist on the test server. `connect`
    // fails fast; the helper must degrade, not hang or panic.
    let bad_url = test_db_url("lucidos_test_nonexistent_startup_lease_db");
    let lease = acquire_startup_lease(&bad_url, Duration::from_secs(2)).await;
    assert!(
        !lease.is_acquired(),
        "a lease-connection failure must degrade (proceed without serialization), not acquire"
    );
}
