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
//!    and writes an `AtomicBool`; `health` reads it. An inline probe would put
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

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use super::LucidosEngine;

/// How often the background task probes. Matches the frontend's own health poll,
/// so a recovery surfaces within about one client tick of actually happening.
const PROBE_INTERVAL: Duration = Duration::from_secs(5);

/// Ceiling on one probe. Comfortably under the frontend's 3s `checkHealth`
/// deadline and the gateway's 5s health client, though neither waits on this one
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

/// One round trip to the database, bounded. `true` only when the query actually
/// answered: a timeout, an acquire failure and a query error are all "no answer".
async fn probe_once(pool: &PgPool) -> bool {
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, sqlx::query("SELECT 1").execute(pool)).await,
        Ok(Ok(_))
    )
}

impl LucidosEngine {
    /// Whether the last settled verdict says the database is reachable. Cheap
    /// enough to read per request: one relaxed atomic load, never a query.
    pub fn database_reachable(&self) -> bool {
        self.database_reachable.load(Ordering::Relaxed)
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
                let was = engine.database_reachable();
                let probe_ok = probe_once(engine.pool()).await;
                let (now, next_failures) = apply_probe(was, failures, probe_ok);
                failures = next_failures;
                if now != was {
                    engine.database_reachable.store(now, Ordering::Relaxed);
                    if now {
                        crate::log!("[DbHealth] Database reachable again");
                    } else {
                        crate::log!(
                            "[DbHealth] Database unreachable after {} consecutive failed probes",
                            failures
                        );
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_probe, FAILURES_BEFORE_UNREACHABLE};

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
