//! External watchdog: scans `agent_sessions` periodically from OUTSIDE any
//! per-thread loop, force-resuming any session whose `last_event_at` has
//! drifted past the limit while the gate (is_waiting / tools_in_flight) says
//! we'd otherwise have fired.
//!
//! The in-loop watchdog at `lifecycle::WATCHDOG_INACTIVITY_LIMIT_MS` is the
//! fast first line — it lives inside `run_session`'s `select!` and fires
//! after 10 min of silence. The 2026-05-16 incident (thread `ef2685a9`,
//! 68-min silent stuck thread) proved that the in-loop watchdog cannot fire
//! when the `select!` itself is wedged (e.g. an event-handler await waiting
//! on a slow subscriber / projection). This module is the floor: it ticks
//! from its own `tokio::spawn`, so a wedged event loop can't starve it.
//!
//! On fire it does NOT emit `ResponseAborted` (no user-visible "Aborted"
//! terminal). Instead it drops the entry from `agent_sessions` and emits
//! `ContinuationRequested { reason: AUTO_RECOVERY_AFTER_HANG_REASON }`, which the
//! spawn dispatcher consumes and turns into a fresh `--resume`. Same outcome
//! as the in-loop watchdog's auto-recovery path — but reachable from
//! outside the wedged loop.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::engine::change_ops::now_epoch_millis;
use crate::engine::event_bus::EventBus;
use crate::engine::types::AgentSession;

use super::lifecycle::{watchdog_gate, WatchdogGate};

/// 12 minutes. Longer than the in-loop's 10-min limit: the 2-min grace gives
/// the in-loop watchdog the first crack. When the in-loop fires successfully
/// it cancels the agent token, the loop exits, and the entry is removed from
/// `agent_sessions` — the external tick is then a no-op. The external tick
/// only fires when the in-loop didn't (wedged `select!`).
pub(crate) const EXTERNAL_WATCHDOG_LIMIT_MS: i64 = 12 * 60 * 1000;

/// 30 s. Coarse enough not to thrash the `agent_sessions` mutex, fine enough
/// that worst-case detection latency is `EXTERNAL_WATCHDOG_LIMIT_MS +
/// EXTERNAL_WATCHDOG_TICK_SECS`.
pub(super) const EXTERNAL_WATCHDOG_TICK_SECS: u64 = 30;

/// Outcome of one external-watchdog evaluation for a single session.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ExternalWatchdogDecision {
    /// Session is healthy, exited-and-still-being-cleaned-up, or otherwise
    /// should not be touched.
    Skip,
    /// Session is stuck mid-turn with no tool in flight — drop from
    /// `agent_sessions` and emit `ContinuationRequested` unconditionally.
    Resume,
    /// Recover ONLY after a projection re-check confirms the thread is still
    /// `running`. Covers two cases the pure gate can't fully judge:
    ///   - hung tool past the ceiling (`tools_in_flight > 0`) — could be a
    ///     genuine hang OR a pending question/permission card the user hasn't
    ///     answered (`waiting_for_user_answer`); only the former is recoverable.
    ///   - process exited but the session is still stale past the limit — the
    ///     in-loop cleanup is wedged / never settled the projection.
    ResumeIfRunning,
}

pub(super) struct ExternalWatchdogInput {
    pub is_waiting: bool,
    pub last_event_at_ms: i64,
    pub tools_in_flight: i32,
    pub process_exited: bool,
    pub now_ms: i64,
    pub limit_ms: i64,
    pub ceiling_ms: i64,
}

/// Pure decision. For an exited session the in-loop normally owns cleanup, so
/// skip — UNLESS it is still stale past the limit, which means the in-loop
/// cleanup is wedged / never ran (`ResumeIfRunning`, gated on a still-running
/// re-check at the call site). Otherwise reuse `watchdog_gate`: `Fire` →
/// `Resume`, `FirePastCeiling` → `ResumeIfRunning`, everything else → `Skip`.
/// Sharing the gate with the in-loop guarantees the two watchdogs agree on what
/// "stuck" means; only the timeout differs.
pub(super) fn external_watchdog_decision(
    input: ExternalWatchdogInput,
) -> ExternalWatchdogDecision {
    if input.process_exited {
        let stale = input.last_event_at_ms > 0
            && input.now_ms.saturating_sub(input.last_event_at_ms) > input.limit_ms;
        return if stale {
            ExternalWatchdogDecision::ResumeIfRunning
        } else {
            ExternalWatchdogDecision::Skip
        };
    }
    match watchdog_gate(
        input.is_waiting,
        input.last_event_at_ms,
        input.now_ms,
        input.limit_ms,
        input.ceiling_ms,
        input.tools_in_flight,
    ) {
        WatchdogGate::Fire => ExternalWatchdogDecision::Resume,
        WatchdogGate::FirePastCeiling(_) => ExternalWatchdogDecision::ResumeIfRunning,
        _ => ExternalWatchdogDecision::Skip,
    }
}

/// Owns the periodic scan + ContinuationRequested emission. Constructed once at
/// engine startup; lives for the duration of the process.
pub(crate) struct ExternalWatchdog {
    agent_sessions: Arc<tokio::sync::Mutex<HashMap<Uuid, AgentSession>>>,
    event_bus: Arc<EventBus>,
    /// Used by the `ResumeIfRunning` re-check (`thread_is_running`) so a hung
    /// tool past the ceiling, or an exited-but-wedged session, is only recovered
    /// when the projection still shows `running` (not `waiting_for_user_answer`,
    /// not already settled).
    pool: sqlx::PgPool,
    limit_ms: i64,
    ceiling_ms: i64,
}

impl ExternalWatchdog {
    pub(crate) fn new(
        agent_sessions: Arc<tokio::sync::Mutex<HashMap<Uuid, AgentSession>>>,
        event_bus: Arc<EventBus>,
        pool: sqlx::PgPool,
        limit_ms: i64,
        ceiling_ms: i64,
    ) -> Self {
        Self {
            agent_sessions,
            event_bus,
            pool,
            limit_ms,
            ceiling_ms,
        }
    }

    pub(crate) fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(
                EXTERNAL_WATCHDOG_TICK_SECS,
            ));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // `interval` always fires at t=0; discard so the first tick
            // lands at `EXTERNAL_WATCHDOG_TICK_SECS` instead of racing
            // engine startup.
            interval.tick().await;
            log!(
                "[ExternalWatchdog] starting (limit={}min, tick={}s)",
                self.limit_ms / 60_000,
                EXTERNAL_WATCHDOG_TICK_SECS,
            );
            loop {
                interval.tick().await;
                self.tick().await;
            }
        })
    }

    /// One scan of `agent_sessions`. Public for tests so they don't sleep
    /// 30 s per assertion.
    pub(crate) async fn tick(&self) {
        let now_ms = now_epoch_millis();

        // Two-pass: snapshot under one lock, mutate+remove under a second
        // lock, then emit OUTSIDE the lock. Holding the mutex across the
        // `event_bus.emit` await would block every `agent_session` insert
        // for the duration of a DB write.
        let mut candidates: Vec<StuckSession> = {
            let sessions = self.agent_sessions.lock().await;
            sessions
                .iter()
                .filter_map(|(tid, s)| {
                    let last_ms = s
                        .last_event_at
                        .load(std::sync::atomic::Ordering::Relaxed);
                    let tif = s
                        .tools_in_flight
                        .load(std::sync::atomic::Ordering::Relaxed);
                    let decision = external_watchdog_decision(ExternalWatchdogInput {
                        is_waiting: s.is_waiting,
                        last_event_at_ms: last_ms,
                        tools_in_flight: tif,
                        process_exited: s.process_exited,
                        now_ms,
                        limit_ms: self.limit_ms,
                        ceiling_ms: self.ceiling_ms,
                    });
                    let needs_running_check = match decision {
                        ExternalWatchdogDecision::Resume => false,
                        ExternalWatchdogDecision::ResumeIfRunning => true,
                        ExternalWatchdogDecision::Skip => return None,
                    };
                    Some(StuckSession {
                        thread_id: *tid,
                        elapsed_ms: now_ms.saturating_sub(last_ms),
                        external_terminal: s.external_terminal_emitted.clone(),
                        idle_notify: s.idle_notify.clone(),
                        needs_running_check,
                    })
                })
                .collect()
        };

        // `ResumeIfRunning` candidates (hung tool past ceiling, or exited-but-
        // wedged) recover ONLY if the projection still shows `running` — a
        // pending question/permission card (`waiting_for_user_answer`) or an
        // already-settled thread must not be euthanized. The DB read is rare
        // (only at the ceiling / for an exited stale session) and runs outside
        // the `agent_sessions` lock.
        if candidates.iter().any(|c| c.needs_running_check) {
            let mut keep: Vec<StuckSession> = Vec::with_capacity(candidates.len());
            for c in candidates.into_iter() {
                if c.needs_running_check {
                    let still_running =
                        crate::engine::claude_code::thread_is_running(&self.pool, c.thread_id)
                            .await
                            .unwrap_or(false);
                    if !still_running {
                        log!(
                            "[ExternalWatchdog] thread={} past ceiling / exited-stale but no longer `running` (awaiting user answer / already settled) — not recovering",
                            c.thread_id,
                        );
                        continue;
                    }
                }
                keep.push(c);
            }
            candidates = keep;
        }
        let stuck = candidates;

        if stuck.is_empty() {
            return;
        }

        // `external_terminal_emitted=true` is the same suppression flag
        // `abort_in_flight_for_restart` uses — without it, the wedged
        // in-loop's eventual safety-net would emit a duplicate
        // ResponseAborted on top of our ContinuationRequested.
        {
            let mut sessions = self.agent_sessions.lock().await;
            for s in &stuck {
                s.external_terminal
                    .store(true, std::sync::atomic::Ordering::Release);
                s.idle_notify.notify_waiters();
                sessions.remove(&s.thread_id);
            }
        }

        for s in stuck {
            log!(
                "[ExternalWatchdog] thread={} stuck for {}s (limit={}min) — \
                 in-loop watchdog never fired (likely wedged event handler); \
                 dropping session entry + emitting ContinuationRequested for auto-resume",
                s.thread_id,
                s.elapsed_ms / 1000,
                self.limit_ms / 60_000,
            );
            crate::engine::thread_events::emit_continuation_requested_or_log(
                &self.event_bus,
                s.thread_id,
                crate::engine::agent_recovery::AUTO_RECOVERY_AFTER_HANG_REASON,
                None,
                "[ExternalWatchdog] ContinuationRequested (auto-recovery after stuck session)",
            )
            .await;
        }
    }
}

/// One stuck-session snapshot — produced by tick's first pass, consumed by
/// the mutate + emit passes. Captured copies of the Arcs let those passes
/// run after the snapshot lock is released.
struct StuckSession {
    thread_id: Uuid,
    elapsed_ms: i64,
    external_terminal: Arc<std::sync::atomic::AtomicBool>,
    idle_notify: Arc<tokio::sync::Notify>,
    /// `true` for `ResumeIfRunning` candidates — recover only after a
    /// `thread_is_running` re-check. `false` for unconditional `Resume`.
    needs_running_check: bool,
}

#[cfg(test)]
#[path = "external_watchdog_tests.rs"]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Realistic epoch-millis base (≈ 2033) so `now - 15 min` stays positive
    /// — `watchdog_gate` short-circuits to `SkipBadHeartbeat` on `last <= 0`,
    /// which would let every test pass for the wrong reason if `now` were
    /// small.
    const NOW: i64 = 2_000_000_000_000;

    /// Hung-tool ceiling for these decision tests. Larger than
    /// `EXTERNAL_WATCHDOG_LIMIT_MS` so "stale past the limit" (12 min) and
    /// "stale past the ceiling" (45 min) are distinct regimes.
    const CEILING: i64 = 45 * 60 * 1000;

    fn input(
        now: i64,
        last: i64,
        is_waiting: bool,
        tif: i32,
        exited: bool,
    ) -> ExternalWatchdogInput {
        ExternalWatchdogInput {
            is_waiting,
            last_event_at_ms: last,
            tools_in_flight: tif,
            process_exited: exited,
            now_ms: now,
            limit_ms: EXTERNAL_WATCHDOG_LIMIT_MS,
            ceiling_ms: CEILING,
        }
    }

    /// Phase B: an exited session whose projection is still stale past the
    /// limit means the in-loop cleanup is wedged / never settled — recover it
    /// (after a `thread_is_running` re-check at the call site). `ResumeIfRunning`
    /// (not `Resume`) so an already-settled thread isn't double-terminated.
    #[test]
    fn exited_session_stale_resumes_if_running() {
        let stale = NOW - 15 * 60 * 1000;
        let out = external_watchdog_decision(input(NOW, stale, false, 0, true));
        assert_eq!(out, ExternalWatchdogDecision::ResumeIfRunning);
    }

    /// An exited session that is NOT yet stale is left to the in-loop cleanup —
    /// recovering here would race it (the original `process_exited` skip).
    #[test]
    fn exited_session_fresh_is_skipped() {
        let fresh = NOW - 60 * 1000;
        let out = external_watchdog_decision(input(NOW, fresh, false, 0, true));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    /// An exited session with an uninitialized heartbeat (last=0) is not stale
    /// (defensive) — skip, don't recover on a 0 read.
    #[test]
    fn exited_session_zero_heartbeat_is_skipped() {
        let out = external_watchdog_decision(input(NOW, 0, false, 0, true));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    /// is_waiting=true → SkipIsWaiting in the shared gate → Skip here. The
    /// session is parked at a turn boundary waiting for the user; legitimate
    /// silence.
    #[test]
    fn waiting_session_is_skipped() {
        let stale = NOW - 15 * 60 * 1000;
        let out = external_watchdog_decision(input(NOW, stale, true, 0, false));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    /// A tool is mid-execution. Could be Bash running cargo build, an
    /// AskUserQuestion the user hasn't answered, anything. Legitimate
    /// silence — never euthanize.
    #[test]
    fn tools_in_flight_skips() {
        let stale = NOW - 15 * 60 * 1000;
        let out = external_watchdog_decision(input(NOW, stale, false, 1, false));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    /// 1 min ago — far inside the 12-min limit.
    #[test]
    fn fresh_session_is_skipped() {
        let fresh = NOW - 60 * 1000;
        let out = external_watchdog_decision(input(NOW, fresh, false, 0, false));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    /// 15 min ago, gate clean: must fire. This is the only path that
    /// produces `Resume`.
    #[test]
    fn stuck_session_resumes() {
        let stale = NOW - 15 * 60 * 1000;
        let out = external_watchdog_decision(input(NOW, stale, false, 0, false));
        assert_eq!(out, ExternalWatchdogDecision::Resume);
    }

    /// `watchdog_gate` uses `>` (strict). Exactly at the limit must NOT
    /// fire — otherwise a 12-min sleep on the wall clock would flag healthy
    /// idleness as stuck.
    #[test]
    fn exactly_at_limit_does_not_fire() {
        let at_limit = NOW - EXTERNAL_WATCHDOG_LIMIT_MS;
        let out = external_watchdog_decision(input(NOW, at_limit, false, 0, false));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    /// One ms past the limit must fire. Anchors the threshold so a future
    /// off-by-one in `watchdog_gate` (e.g. flipping `>` to `>=`) trips here.
    #[test]
    fn one_ms_past_limit_fires() {
        let stale = NOW - EXTERNAL_WATCHDOG_LIMIT_MS - 1;
        let out = external_watchdog_decision(input(NOW, stale, false, 0, false));
        assert_eq!(out, ExternalWatchdogDecision::Resume);
    }

    /// Last-event=0 is the uninitialized-heartbeat sentinel from the
    /// underlying gate. Must skip (not fire) so a freshly-constructed session
    /// whose first event hasn't arrived isn't euthanized.
    #[test]
    fn zero_last_event_skips() {
        let out = external_watchdog_decision(input(NOW, 0, false, 0, false));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    /// Phase A (the root-cause fix): a tool in flight past the hung-tool ceiling
    /// no longer skips forever — `ResumeIfRunning` so the tick recovers it after
    /// confirming the thread is still `running` (not a pending user card).
    #[test]
    fn tools_in_flight_past_ceiling_resumes_if_running() {
        let past_ceiling = NOW - CEILING - 1;
        let out = external_watchdog_decision(input(NOW, past_ceiling, false, 3, false));
        assert_eq!(out, ExternalWatchdogDecision::ResumeIfRunning);
    }

    /// Within the ceiling, tools in flight remain a legitimate skip even though
    /// past the 12-min limit (this is the prior `tools_in_flight_skips` regime).
    #[test]
    fn tools_in_flight_within_ceiling_skips() {
        let within_ceiling = NOW - CEILING + 60 * 1000; // 1 min shy of ceiling
        let out = external_watchdog_decision(input(NOW, within_ceiling, false, 3, false));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }
}
