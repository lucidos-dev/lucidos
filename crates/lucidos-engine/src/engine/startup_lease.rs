//! Engine **startup lease** — a per-database advisory lock that serializes
//! engine startup so a fresh engine never runs its recovery/reset sweeps against
//! a database the *previous* engine is still mutating.
//!
//! ## Why this exists
//!
//! A *Switch to new version* (and any respawn) is not atomic: the gateway's
//! `respawn_stack` (`crates/lucidos-gateway/src/server.rs`) SIGUSR1s the old
//! engine and then *immediately* spawns the new one — it reaps the old child in a
//! detached task, so it does NOT wait for the old engine to exit. The new engine
//! therefore boots and runs recovery (`recover_orphaned_worktrees` →
//! `recover_orphaned_threads`) **while the old engine is still gracefully shutting
//! down against the same per-workspace Postgres**: still emitting the
//! device-attributed `ResponseAborted` boundary that marks a user-initiated switch
//! (`abort_in_flight_for_restart`, emitted only at teardown), and still draining
//! its interrupted Claude Code subprocesses (which stream final
//! `CodingAgentToolResult` / `CodingAgentTextStreamed` events).
//!
//! That race produces the exact bug this module fixes: recovery reads the DB
//! before the device-abort boundary lands (so `switch_was_user_initiated` is false
//! → the crash path / manual Continue runs instead of auto-resume), and the old
//! CC subprocess's no-actor drain events land *after* recovery's terminal event
//! and re-project `status → running` — leaving a phantom `running` thread with no
//! live process.
//!
//! ## The lease
//!
//! Each engine acquires a session-scoped Postgres advisory lock on a dedicated
//! connection and holds it for its **entire process lifetime**. The successor
//! blocks on `acquire_startup_lease` *before* running any reset/recovery, and
//! unblocks exactly when the predecessor's process exits: on a graceful shutdown
//! the predecessor holds the lock through its `abort_in_flight_for_restart` +
//! CC-drain steps (those run before its serve future resolves and before its
//! `run()` returns, which is what drops the lease), and on a crash the predecessor
//! connection dies and the lock releases instantly. Either way, recovery on the
//! successor runs against a quiescent DB.
//!
//! Advisory locks are scoped to the current database, and each Lucidos workspace
//! has its own database (`lucidos_<slug>`), so one constant key never collides
//! across workspaces. (Precedent: `core/repositories.rs` uses
//! `pg_advisory_xact_lock`.)
//!
//! **Fail-open:** a hung predecessor must never wedge boot forever. If the lock is
//! still held after `max_wait`, or the lease connection can't be opened, we log
//! and proceed WITHOUT the lock (degraded, back to today's behavior — the settle
//! floor and the next restart still recover). `max_wait` sits well inside the
//! gateway's 120 s `BOOT_GRACE`, so waiting here never triggers a gateway respawn.

use sqlx::Connection;
use std::time::Duration;

/// Advisory-lock key for the engine startup lease. Arbitrary but distinctive —
/// the ASCII bytes of `"LUCIDOS\0"` (`0x4C55_4349_444F_5300`), a positive `i64`.
/// Database-scoped, so a single constant is safe across all workspaces.
const STARTUP_LEASE_KEY: i64 = 0x4C55_4349_444F_5300;

/// How long a booting engine waits for the previous engine to release the lease
/// before proceeding degraded. Comfortably above the predecessor's graceful
/// shutdown worst case (~10 s CC drain + ~10 s HTTP drain) and comfortably below
/// the gateway's `BOOT_GRACE` (120 s), so the wait never provokes a respawn.
pub const DEFAULT_MAX_WAIT: Duration = Duration::from_secs(45);

/// Poll interval while the previous engine still holds the lease.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// A held startup lease. Keep it alive for the engine's process lifetime — drop
/// closes the dedicated connection, which releases the session-scoped advisory
/// lock so the NEXT engine's `acquire_startup_lease` can proceed.
pub struct StartupLease {
    /// The connection holding the advisory lock. `None` in the degraded path
    /// (couldn't connect, query failed, or timed out waiting for a hung
    /// predecessor): no lock is held and boot proceeds anyway.
    _conn: Option<sqlx::PgConnection>,
    acquired: bool,
}

impl StartupLease {
    /// True when the advisory lock is actually held (serialization is in effect).
    /// False in the degraded fail-open path. Used for the boot log and tests.
    pub fn is_acquired(&self) -> bool {
        self.acquired
    }
}

/// Acquire the per-database engine startup lease, blocking (bounded by `max_wait`)
/// until the previous engine releases it. Hold the returned guard for the whole
/// process lifetime. Never panics and never blocks past `max_wait`: any failure
/// (connect error, query error, timeout) degrades to a non-acquired guard so boot
/// proceeds. See the module docs for the full contract.
pub async fn acquire_startup_lease(database_url: &str, max_wait: Duration) -> StartupLease {
    let mut conn = match sqlx::postgres::PgConnection::connect(database_url).await {
        Ok(c) => c,
        Err(e) => {
            log!(
                "[StartupLease] Could not open lease connection — proceeding WITHOUT startup serialization: {}",
                e
            );
            return StartupLease {
                _conn: None,
                acquired: false,
            };
        }
    };

    let deadline = tokio::time::Instant::now() + max_wait;
    let mut announced_wait = false;
    loop {
        match sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
            .bind(STARTUP_LEASE_KEY)
            .fetch_one(&mut conn)
            .await
        {
            Ok(true) => {
                if announced_wait {
                    log!("[StartupLease] Acquired — previous engine has exited; recovery may proceed");
                }
                return StartupLease {
                    _conn: Some(conn),
                    acquired: true,
                };
            }
            Ok(false) => {
                if tokio::time::Instant::now() >= deadline {
                    log!(
                        "[StartupLease] Previous engine still holds the lease after {:?} — proceeding WITHOUT serialization (degraded; recovery may race a hung predecessor)",
                        max_wait
                    );
                    return StartupLease {
                        _conn: None,
                        acquired: false,
                    };
                }
                if !announced_wait {
                    log!(
                        "[StartupLease] Previous engine still alive — waiting up to {:?} for it to exit before running recovery",
                        max_wait
                    );
                    announced_wait = true;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            Err(e) => {
                log!(
                    "[StartupLease] Lease query failed — proceeding WITHOUT serialization: {}",
                    e
                );
                return StartupLease {
                    _conn: None,
                    acquired: false,
                };
            }
        }
    }
}

#[cfg(test)]
#[path = "startup_lease_tests.rs"]
mod startup_lease_tests;
