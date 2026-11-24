//! Is this engine's database reachable right now? (ADR 0037)
//!
//! An engine outlives its database. In dev the workspace's Postgres is a Docker
//! container, so quitting Docker Desktop leaves every running engine alive with a
//! pool that can no longer connect. Before this module `/api/v1/health` was built
//! entirely from process facts (workspace path, `started_at`, version strings) and
//! never touched the pool, so that engine kept answering `"status": "ok"`:
//!
//!   * the gateway's health probe passed, so nothing on the gateway side noticed;
//!   * the frontend flipped to `connected`, held its boot splash waiting for a
//!     thread list that could never arrive, and painted a black "Loading…" screen
//!     for the full 15s safety cap;
//!   * then ~20 independent startup loads each surfaced their own failure, so one
//!     dead database became a column of "Failed to …" toasts, none of which named
//!     the cause.
//!
//! So the engine states the fact once, and the surfaces above render that instead
//! of inferring it twenty times.
//!
//! Three properties are load-bearing:
//!
//! 1. **The handler never awaits the database.** The probe runs on its own ticker
//!    and writes an atomic; `health` reads it. An inline probe would put
//!    database latency on the endpoint the gateway health-checks with a 5s client
//!    timeout (`stack::build_health_client`), so an outage could start tripping
//!    that deadline as well.
//! 2. **`/api/v1/health` keeps returning 200.** The status code is about the
//!    engine process; this field is about its dependency. Failing the endpoint
//!    would recruit the gateway's respawn machinery against a condition respawning
//!    cannot fix, and it collides with ADR 0014's "never cull an alive engine".
//! 3. **`false` needs positive, repeated evidence.** The engine only reaches
//!    `serve` after connecting AND migrating, so `true` is the honest initial
//!    value, and one slow query must not paint the whole app as down. See
//!    [`apply_probe`] and `.claude/rules/rust.md` on not reading an unanswered
//!    probe as a "no".
//!
//! When the pool cannot answer, the probe also says why, so the slowness warning
//! can name the fix (ADR 0301). A direct connection outside the pool tells a
//! used-up pool from a database that does not answer.
//!
//! The same ticker watches for a wiped schema (ADR 0481). When the migration
//! marker names this database and `_sqlx_migrations` is confirmed gone twice
//! in a row, the engine reports the terminal boot failure and stops. Only a
//! query that answered counts, so an outage never reads as a wipe.

use std::sync::atomic::{AtomicU8, Ordering};

use super::LucidosEngine;
use crate::core::migration_marker::MigrationTable;

/// Consecutive failed probes before the engine reports its database unreachable.
/// Two (so ~10s) rather than one: a single timed-out `SELECT 1` under a saturated
/// host is not evidence of an outage, and the cost of a false positive is the
/// whole client going into its degraded surface.
const FAILURES_BEFORE_UNREACHABLE: u32 = 2;

/// Fold one probe result into the reported state.
///
/// Returns the new `(reachable, consecutive_failures)`. Pure, so the whole
/// hysteresis table is testable without a database.
///
/// Asymmetric on purpose. Going *down* takes [`FAILURES_BEFORE_UNREACHABLE`]
/// consecutive failures, because the claim is expensive to get wrong. Coming
/// *back* takes one success, because a successful `SELECT 1` is proof and there
/// is nothing to protect the user from.
pub fn apply_probe(reachable: bool, consecutive_failures: u32, probe_ok: bool) -> (bool, u32) {
    if probe_ok {
        return (true, 0);
    }
    let failures = consecutive_failures.saturating_add(1);
    (
        reachable && failures < FAILURES_BEFORE_UNREACHABLE,
        failures,
    )
}

/// Consecutive confirmed misses of `_sqlx_migrations` before the engine treats
/// its database as wiped. One miss could be a restore caught mid-flight.
const MISSES_BEFORE_WIPED: u32 = 2;

/// Fold one schema probe into the run of consecutive misses. Returns the new
/// count and whether the database now counts as wiped. An unanswered probe
/// leaves the count alone: it is evidence of nothing.
pub fn apply_schema_probe(misses: u32, table: MigrationTable) -> (u32, bool) {
    let misses = match table {
        MigrationTable::Present => 0,
        MigrationTable::Missing => misses.saturating_add(1),
        MigrationTable::Unknown => misses,
    };
    (misses, misses >= MISSES_BEFORE_WIPED)
}

/// Postgres refuses a new connection with this SQLSTATE when every slot is taken.
const TOO_MANY_CONNECTIONS: &str = "53300";

/// The settled verdict on this engine's database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DatabaseHealth {
    Reachable = 0,
    /// The database itself does not answer: stopped, wedged, or out of disk.
    NotAnswering = 1,
    /// The database answers, but no connection is free for this engine.
    PoolExhausted = 2,
}

impl DatabaseHealth {
    /// The `database_reachable` field of `/api/v1/health`.
    pub fn is_reachable(self) -> bool {
        self == Self::Reachable
    }

    /// The `database_pool_exhausted` field of `/api/v1/health`.
    pub fn is_pool_exhausted(self) -> bool {
        self == Self::PoolExhausted
    }

    /// Why a pool probe failed, from what a direct connection then got. A
    /// refusal for too many connections is a full pool too: the database
    /// answered, and has nothing free.
    pub fn of_failed_probe(direct_answered: bool, refusal_sqlstate: Option<&str>) -> Self {
        if direct_answered || refusal_sqlstate == Some(TOO_MANY_CONNECTIONS) {
            Self::PoolExhausted
        } else {
            Self::NotAnswering
        }
    }

    /// The verdict after [`apply_probe`] decided reachability. `failure` is why
    /// the latest probe failed. It is always set when the verdict is
    /// unreachable, since one success restores reachability.
    pub fn settle(reachable: bool, failure: Option<Self>) -> Self {
        match failure {
            Some(kind) if !reachable => kind,
            _ => Self::Reachable,
        }
    }
}

/// [`DatabaseHealth`] in one atomic, so the handler reads it without a lock and
/// the reachable and pool-exhausted fields can never disagree.
#[derive(Debug)]
pub struct DatabaseHealthCell(AtomicU8);

impl Default for DatabaseHealthCell {
    /// Reachable: the engine only reaches `serve` after connecting and
    /// migrating, so anything else would be a claim without evidence.
    fn default() -> Self {
        Self(AtomicU8::new(DatabaseHealth::Reachable as u8))
    }
}

impl DatabaseHealthCell {
    fn load(&self) -> DatabaseHealth {
        match self.0.load(Ordering::Relaxed) {
            1 => DatabaseHealth::NotAnswering,
            2 => DatabaseHealth::PoolExhausted,
            _ => DatabaseHealth::Reachable,
        }
    }

    fn store(&self, health: DatabaseHealth) {
        self.0.store(health as u8, Ordering::Relaxed);
    }
}

impl LucidosEngine {
    /// The last settled verdict on the database. Cheap enough to read per
    /// request: one relaxed atomic load, never a query. Read it once per
    /// response, so the fields derived from it cannot disagree.
    pub fn database_health(&self) -> DatabaseHealth {
        self.database_health.load()
    }

    /// Publish the verdict from the background probe.
    pub fn set_database_health(&self, health: DatabaseHealth) {
        self.database_health.store(health);
    }
}

#[cfg(test)]
mod tests {
    use super::{DatabaseHealth, DatabaseHealthCell};

    #[test]
    fn the_cell_round_trips_every_verdict_and_starts_reachable() {
        let cell = DatabaseHealthCell::default();
        assert_eq!(cell.load(), DatabaseHealth::Reachable);
        for health in [
            DatabaseHealth::NotAnswering,
            DatabaseHealth::PoolExhausted,
            DatabaseHealth::Reachable,
        ] {
            cell.store(health);
            assert_eq!(cell.load(), health);
        }
    }
}
