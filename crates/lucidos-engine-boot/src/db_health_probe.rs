//! Periodic database reachability and schema probes (ADRs 0037 and 0481).

use std::sync::Arc;
use std::time::Duration;

use lucidos_core::core::migration_marker::{self, MigrationTable};
use lucidos_engine::engine::db_health::{apply_probe, apply_schema_probe, DatabaseHealth};
use lucidos_engine::engine::LucidosEngine;
use sqlx::{Connection, PgConnection, PgPool};

/// How often the background task probes. Matches the frontend's own health poll,
/// so a recovery surfaces within about one client tick of actually happening.
const PROBE_INTERVAL: Duration = Duration::from_secs(5);

/// Ceiling on one probe. This bound keeps a wedged connection from stalling
/// the ticker. It is shorter than the frontend and gateway health timeouts.
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// One round trip through the pool, bounded. `None` when the query answered.
/// Otherwise a direct connection says why it did not, bounded too, so a tick
/// never lasts much past twice [`PROBE_TIMEOUT`].
async fn probe_once(pool: &PgPool) -> Option<DatabaseHealth> {
    let pooled = tokio::time::timeout(PROBE_TIMEOUT, sqlx::query("SELECT 1").execute(pool)).await;
    if matches!(pooled, Ok(Ok(_))) {
        return None;
    }
    Some(probe_outside_pool(pool).await)
}

/// Whether the database answers a connection of its own, with the pool's
/// connect options and one timeout over connect and query together.
async fn probe_outside_pool(pool: &PgPool) -> DatabaseHealth {
    let options = pool.connect_options();
    let attempt = async {
        let mut conn = PgConnection::connect_with(&options).await?;
        sqlx::query("SELECT 1").execute(&mut conn).await?;
        conn.close().await
    };
    match tokio::time::timeout(PROBE_TIMEOUT, attempt).await {
        Ok(Ok(())) => DatabaseHealth::of_failed_probe(true, None),
        Ok(Err(sqlx::Error::Database(e))) => {
            DatabaseHealth::of_failed_probe(false, e.code().as_deref())
        }
        Ok(Err(_)) | Err(_) => DatabaseHealth::of_failed_probe(false, None),
    }
}

/// Report the wiped database and stop, when the migration marker agrees
/// it is this database. `false` when it does not, and the engine runs on.
async fn stop_on_wiped_database(engine: &LucidosEngine) -> bool {
    let workspace = engine.workspace_path();
    let named = tokio::time::timeout(
        PROBE_TIMEOUT,
        migration_marker::marker_names(
            workspace,
            &lucidos_core::core::database_url(),
            engine.pool(),
        ),
    )
    .await;
    if named != Ok(true) {
        return false;
    }
    lucidos_core::log_legacy!("[DbHealth] The database lost its migration table; stopping");
    lucidos_core::boot_failure::report(&lucidos_core::boot_failure::wiped_database_message(
        workspace,
    ))
    .await;
    request_own_shutdown();
    true
}

/// Start the background reachability probe. Spawned once at boot, alongside
/// the other periodic engine tasks in `main.rs`.
///
/// Packaged installs run this probe too. The frontend uses the same degraded
/// surface in both cases and picks the remedy text from the `packaged` flag.
pub fn spawn(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut failures: u32 = 0;
        let mut schema_misses: u32 = 0;
        let mut ticker = tokio::time::interval(PROBE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if engine.is_shutting_down() {
                return;
            }
            let was = engine.database_health();
            let failure = probe_once(engine.pool()).await;
            let (reachable, next_failures) =
                apply_probe(was.is_reachable(), failures, failure.is_none());
            failures = next_failures;
            // Only after the pool answered, and bounded like it, so a tick
            // still never lasts much past twice `PROBE_TIMEOUT`.
            let table = if failure.is_none() {
                tokio::time::timeout(
                    PROBE_TIMEOUT,
                    migration_marker::migration_table(engine.pool()),
                )
                .await
                .unwrap_or(MigrationTable::Unknown)
            } else {
                MigrationTable::Unknown
            };
            let (misses, wiped) = apply_schema_probe(schema_misses, table);
            schema_misses = misses;
            if wiped && stop_on_wiped_database(&engine).await {
                return;
            }
            let now = DatabaseHealth::settle(reachable, failure);
            if now != was {
                engine.set_database_health(now);
                match now {
                    DatabaseHealth::Reachable => {
                        lucidos_core::log_legacy!("[DbHealth] Database reachable again")
                    }
                    DatabaseHealth::NotAnswering => lucidos_core::log_legacy!(
                        "[DbHealth] Database unreachable after {} consecutive failed probes",
                        failures
                    ),
                    DatabaseHealth::PoolExhausted => lucidos_core::log_legacy!(
                        "[DbHealth] Database answers, but no pooled connection is free \
                         after {} consecutive failed probes",
                        failures
                    ),
                }
            }
        }
    })
}

/// Stop through the engine's own graceful path, the SIGUSR1 `main.rs` handles,
/// so agent sessions and background tasks are torn down too.
#[cfg(unix)]
fn request_own_shutdown() {
    // SAFETY: kill takes no pointers; getpid names this process.
    unsafe {
        libc::kill(libc::getpid(), libc::SIGUSR1);
    }
}

#[cfg(not(unix))]
fn request_own_shutdown() {
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_missing_table_is_not_a_wipe_and_two_in_a_row_are() {
        assert_eq!(apply_schema_probe(0, MigrationTable::Missing), (1, false));
        assert_eq!(apply_schema_probe(1, MigrationTable::Missing), (2, true));
    }

    #[test]
    fn a_present_table_resets_the_run() {
        assert_eq!(apply_schema_probe(1, MigrationTable::Present), (0, false));
        assert_eq!(apply_schema_probe(0, MigrationTable::Missing), (1, false));
    }

    /// The load-bearing negative case: during a Docker or socket outage the
    /// query errors, and that must never count toward a wipe (ADR 0037).
    #[test]
    fn an_unanswered_probe_leaves_the_run_alone() {
        assert_eq!(apply_schema_probe(0, MigrationTable::Unknown), (0, false));
        assert_eq!(apply_schema_probe(1, MigrationTable::Unknown), (1, false));
        let (misses, wiped) = apply_schema_probe(1, MigrationTable::Unknown);
        assert_eq!(
            apply_schema_probe(misses, MigrationTable::Missing),
            (2, true)
        );
        assert!(!wiped);
    }

    #[test]
    fn a_direct_connection_that_answers_means_the_pool_is_used_up() {
        assert_eq!(
            DatabaseHealth::of_failed_probe(true, None),
            DatabaseHealth::PoolExhausted
        );
        assert_eq!(
            DatabaseHealth::of_failed_probe(false, Some("53300")),
            DatabaseHealth::PoolExhausted,
            "too many connections: the database answered and has nothing free"
        );
    }

    #[test]
    fn a_direct_connection_that_gets_nothing_means_the_database_does_not_answer() {
        assert_eq!(
            DatabaseHealth::of_failed_probe(false, None),
            DatabaseHealth::NotAnswering
        );
        assert_eq!(
            DatabaseHealth::of_failed_probe(false, Some("57P03")),
            DatabaseHealth::NotAnswering,
            "any other refusal is the database, not the pool"
        );
    }

    #[test]
    fn the_kind_of_failure_shows_only_once_the_verdict_is_unreachable() {
        use DatabaseHealth::*;
        assert_eq!(DatabaseHealth::settle(true, None), Reachable);
        assert_eq!(
            DatabaseHealth::settle(true, Some(PoolExhausted)),
            Reachable,
            "one failure is not an outage, so it names no kind"
        );
        assert_eq!(
            DatabaseHealth::settle(false, Some(PoolExhausted)),
            PoolExhausted
        );
        assert_eq!(
            DatabaseHealth::settle(false, Some(NotAnswering)),
            NotAnswering
        );
    }

    #[test]
    fn a_healthy_probe_reports_reachable_and_clears_the_tally() {
        assert_eq!(apply_probe(true, 0, true), (true, 0));
        assert_eq!(apply_probe(true, 1, true), (true, 0));
    }

    #[test]
    fn one_failure_is_not_enough_to_claim_an_outage() {
        // The load-bearing negative case: a single timed-out probe on a saturated
        // host must not put the whole client into its degraded surface.
        assert_eq!(apply_probe(true, 0, false), (true, 1));
    }

    #[test]
    fn the_second_consecutive_failure_settles_the_verdict() {
        let (reachable, failures) = apply_probe(true, 1, false);
        assert!(!reachable, "two consecutive failures is the threshold");
        assert_eq!(failures, 2);
    }

    #[test]
    fn a_single_success_restores_it() {
        // Asymmetric by design: proof of life needs no hysteresis.
        assert_eq!(apply_probe(false, 7, true), (true, 0));
    }

    #[test]
    fn a_settled_outage_stays_settled_without_re_announcing() {
        // Already false: further failures keep it false and keep counting, so the
        // caller's `now != was` guard logs the transition exactly once.
        let (reachable, failures) = apply_probe(false, 2, false);
        assert!(!reachable);
        assert_eq!(failures, 3);
    }

    #[test]
    fn a_long_outage_cannot_overflow_the_tally() {
        // Saturating rather than wrapping: a wrap would take the count back under
        // the threshold and flip a dead database to "reachable".
        let (reachable, failures) = apply_probe(false, u32::MAX, false);
        assert!(!reachable);
        assert_eq!(failures, u32::MAX);
    }
}
