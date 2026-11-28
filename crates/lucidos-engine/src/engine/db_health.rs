use std::sync::atomic::{AtomicU8, Ordering};

use super::LucidosEngine;
use crate::core::migration_marker::MigrationTable;

/// INVARIANT: Consecutive failed probes before the engine reports its database unreachable.
/// Two (so ~10s) rather than one: a single timed-out `SELECT 1` under a saturated
/// host is not evidence of an outage, and the cost of a false positive is the
/// whole client going into its degraded surface.
const FAILURES_BEFORE_UNREACHABLE: u32 = 2;

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

/// INVARIANT: Consecutive confirmed misses of `_sqlx_migrations` before the engine treats
/// its database as wiped. One miss could be a restore caught mid-flight.
const MISSES_BEFORE_WIPED: u32 = 2;

pub fn apply_schema_probe(misses: u32, table: MigrationTable) -> (u32, bool) {
    let misses = match table {
        MigrationTable::Present => 0,
        MigrationTable::Missing => misses.saturating_add(1),
        MigrationTable::Unknown => misses,
    };
    (misses, misses >= MISSES_BEFORE_WIPED)
}

const TOO_MANY_CONNECTIONS: &str = "53300";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DatabaseHealth {
    Reachable = 0,
    NotAnswering = 1,
    PoolExhausted = 2,
}

impl DatabaseHealth {
    pub fn is_reachable(self) -> bool {
        self == Self::Reachable
    }

    pub fn is_pool_exhausted(self) -> bool {
        self == Self::PoolExhausted
    }

    pub fn of_failed_probe(direct_answered: bool, refusal_sqlstate: Option<&str>) -> Self {
        if direct_answered || refusal_sqlstate == Some(TOO_MANY_CONNECTIONS) {
            Self::PoolExhausted
        } else {
            Self::NotAnswering
        }
    }

    pub fn settle(reachable: bool, failure: Option<Self>) -> Self {
        match failure {
            Some(kind) if !reachable => kind,
            _ => Self::Reachable,
        }
    }
}

#[derive(Debug)]
pub struct DatabaseHealthCell(AtomicU8);

impl Default for DatabaseHealthCell {
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
    /// INVARIANT: The last settled verdict on the database. Cheap enough to read per
    /// request: one relaxed atomic load, never a query. Read it once per
    /// response, so the fields derived from it cannot disagree.
    pub fn database_health(&self) -> DatabaseHealth {
        self.database_health.load()
    }

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
