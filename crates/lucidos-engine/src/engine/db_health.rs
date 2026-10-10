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

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sqlx::{Connection, PgConnection, PgPool};

use super::LucidosEngine;

/// How often the background task probes. Matches the frontend's own health poll,
/// so a recovery surfaces within about one client tick of actually happening.
const PROBE_INTERVAL: Duration = Duration::from_secs(5);

/// Ceiling on one probe. Comfortably under the frontend's `HEALTH_PROBE_TIMEOUT_MS`
/// and the gateway's `build_health_client` timeout, though neither waits on this one
/// (property 1 above). It exists so a wedged connection cannot stall the ticker.
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

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
    fn of_failed_probe(direct_answered: bool, refusal_sqlstate: Option<&str>) -> Self {
        if direct_answered || refusal_sqlstate == Some(TOO_MANY_CONNECTIONS) {
            Self::PoolExhausted
        } else {
            Self::NotAnswering
        }
    }

    /// The verdict after [`apply_probe`] decided reachability. `failure` is why
    /// the latest probe failed. It is always set when the verdict is
    /// unreachable, since one success restores reachability.
    fn settle(reachable: bool, failure: Option<Self>) -> Self {
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

impl LucidosEngine {
    /// The last settled verdict on the database. Cheap enough to read per
    /// request: one relaxed atomic load, never a query. Read it once per
    /// response, so the fields derived from it cannot disagree.
    pub fn database_health(&self) -> DatabaseHealth {
        self.database_health.load()
    }

    /// Start the background reachability probe. Spawned once at boot, alongside
    /// the other periodic engine tasks in `main.rs`.
    ///
    /// Unlike `spawn_served_frontend_sync` this is NOT dev-only: a packaged
    /// install's bundled Postgres can die too, and the client's degraded surface
    /// is the same either way (only the remedy sentence differs, which the
    /// frontend picks from the existing `packaged` flag).
    pub fn spawn_db_health_probe(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let engine = self.clone();
        tokio::spawn(async move {
            let mut failures: u32 = 0;
            let mut ticker = tokio::time::interval(PROBE_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if engine.is_shutting_down() {
                    return;
                }
                let was = engine.database_health.load();
                let failure = probe_once(engine.pool()).await;
                let (reachable, next_failures) =
                    apply_probe(was.is_reachable(), failures, failure.is_none());
                failures = next_failures;
                let now = DatabaseHealth::settle(reachable, failure);
                if now != was {
                    engine.database_health.store(now);
                    match now {
                        DatabaseHealth::Reachable => {
                            crate::log!("[DbHealth] Database reachable again")
                        }
                        DatabaseHealth::NotAnswering => crate::log!(
                            "[DbHealth] Database unreachable after {} consecutive failed probes",
                            failures
                        ),
                        DatabaseHealth::PoolExhausted => crate::log!(
                            "[DbHealth] Database answers, but no pooled connection is free \
                             after {} consecutive failed probes",
                            failures
                        ),
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_probe, DatabaseHealth, DatabaseHealthCell, FAILURES_BEFORE_UNREACHABLE};

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
        assert_eq!(failures, FAILURES_BEFORE_UNREACHABLE);
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
