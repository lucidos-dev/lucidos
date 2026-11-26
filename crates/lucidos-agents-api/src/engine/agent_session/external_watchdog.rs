use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::engine::change_ops::now_epoch_millis;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::MessageOrigin;
use crate::engine::types::AgentSession;

use super::lifecycle::{watchdog_gate, WatchdogGate, WATCHDOG_HUNG_TOOL_CEILING_MS};
use crate::engine::thread_lifecycle::ThreadStatus;

pub(crate) const EXTERNAL_WATCHDOG_LIMIT_MS: i64 = 12 * 60 * 1000;

pub(super) const EXTERNAL_WATCHDOG_TICK_SECS: u64 = 30;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ExternalWatchdogDecision {
    Skip,
    Resume,
    ResumeIfRunning,
}

pub(super) struct ExternalWatchdogInput {
    pub is_waiting: bool,
    pub last_event_at_ms: i64,
    pub tools_in_flight: i32,
    pub loop_ended: bool,
    pub now_ms: i64,
    pub limit_ms: i64,
    pub ceiling_ms: i64,
}

pub(super) fn external_watchdog_decision(input: ExternalWatchdogInput) -> ExternalWatchdogDecision {
    if input.loop_ended {
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

const AGENT_PRODUCED_OUTPUT_EVENT_TYPES: &[&str] = &[
    "CodingAgentTextStreamed",
    "CodingAgentToolCalled",
    "CodingAgentToolResult",
    "ContextCaptured",
];

fn quiet_window_secs(limit_ms: i64) -> i64 {
    ((limit_ms + 999) / 1000).max(1)
}

fn orphans_without_live_session(candidates: Vec<Uuid>, live: &HashSet<Uuid>) -> Vec<Uuid> {
    candidates
        .into_iter()
        .filter(|tid| !live.contains(tid))
        .collect()
}

pub fn spawn_external_watchdog(
    engine: &crate::engine::LucidosEngine,
) -> tokio::task::JoinHandle<()> {
    ExternalWatchdog::new(
        engine.agent_sessions.clone(),
        Arc::new(engine.event_bus.clone()),
        engine.pool().clone(),
        EXTERNAL_WATCHDOG_LIMIT_MS,
        WATCHDOG_HUNG_TOOL_CEILING_MS,
    )
    .spawn()
}

pub(crate) struct ExternalWatchdog {
    agent_sessions: Arc<tokio::sync::Mutex<HashMap<Uuid, AgentSession>>>,
    event_bus: Arc<EventBus>,
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
            let mut interval =
                tokio::time::interval(Duration::from_secs(EXTERNAL_WATCHDOG_TICK_SECS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            log_legacy!(
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

    pub(crate) async fn tick(&self) {
        let now_ms = now_epoch_millis();

        // INVARIANT: Two-pass: snapshot under one lock, mutate+remove under a second
        // lock, then emit OUTSIDE the lock. Holding the mutex across the
        // `event_bus.emit` await would block every `agent_session` insert
        // for the duration of a DB write.
        let live: HashSet<Uuid>;
        let mut candidates: Vec<StuckSession> = {
            let sessions = self.agent_sessions.lock().await;
            live = sessions.keys().copied().collect();
            sessions
                .iter()
                .filter_map(|(tid, s)| {
                    if !s.is_running() {
                        return None;
                    }
                    let last_ms = s.last_event_at.load(std::sync::atomic::Ordering::Relaxed);
                    let tif = s.tools_in_flight.load(std::sync::atomic::Ordering::Relaxed);
                    let decision = external_watchdog_decision(ExternalWatchdogInput {
                        is_waiting: s.is_waiting,
                        last_event_at_ms: last_ms,
                        tools_in_flight: tif,
                        loop_ended: !s.is_live(),
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
                        external_continuation: s.external_continuation_requested.clone(),
                        idle_notify: s.idle_notify.clone(),
                        agent_cancel: s.agent_cancel.clone(),
                        last_event_at: s.last_event_at.clone(),
                        snapshot_last_ms: last_ms,
                        needs_running_check,
                    })
                })
                .collect()
        };

        if candidates.iter().any(|c| c.needs_running_check) {
            let mut keep: Vec<StuckSession> = Vec::with_capacity(candidates.len());
            for c in candidates.into_iter() {
                if c.needs_running_check {
                    let still_running =
                        crate::engine::thread_liveness::thread_is_running(&self.pool, c.thread_id)
                            .await
                            .unwrap_or_else(|e| {
                                log_legacy!(
                                    "[ExternalWatchdog] thread={} running-state probe failed: {}; not recovering this tick",
                                    c.thread_id,
                                    e
                                );
                                false
                            });
                    if !still_running {
                        log_legacy!(
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
        self.settle_orphaned_running(&live).await;

        if candidates.is_empty() {
            return;
        }

        self.recover_stuck(candidates).await;
    }

    async fn settle_orphaned_running(&self, live_at_snapshot: &HashSet<Uuid>) {
        let candidates: Vec<Uuid> = match sqlx::query_scalar::<_, Uuid>(&format!(
            "SELECT ts.thread_id FROM thread_summaries ts \
             WHERE ts.is_coding_agent = true \
               AND ts.status = {} \
               AND ts.last_activity < now() - make_interval(secs => $1) \
               AND ( \
                   SELECT e.event_type FROM events e \
                   WHERE e.aggregate_id = ts.thread_id::text \
                     AND e.event_type <> ALL($3) \
                   ORDER BY e.sequence DESC LIMIT 1 \
               ) = ANY($2) \
               AND NOT EXISTS ( \
                   SELECT 1 FROM thread_queue q \
                   WHERE q.thread_id = ts.thread_id AND q.status = 'queued' \
               )",
            ThreadStatus::Running.sql_literal(),
        ))
        .bind(quiet_window_secs(self.limit_ms))
        .bind(AGENT_PRODUCED_OUTPUT_EVENT_TYPES)
        .bind(crate::engine::thread_events::ThreadEvent::SIDE_QUESTION_EVENT_TYPES)
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                log_legacy!("[ExternalWatchdog] orphan settle query failed: {}", e);
                return;
            }
        };

        let orphans = {
            let sessions = self.agent_sessions.lock().await;
            let live_now: HashSet<Uuid> = sessions.keys().copied().collect();
            orphans_without_live_session(candidates, &live_now)
        };
        for thread_id in orphans_without_live_session(orphans, live_at_snapshot) {
            match crate::engine::successor::between_successor_sessions(&self.pool, thread_id).await
            {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    log_legacy!(
                        "[ExternalWatchdog] successor gap probe for {} failed: {}; not settling",
                        thread_id,
                        e
                    );
                    continue;
                }
            }
            match crate::engine::claude_code::settle_stuck_running_thread(
                &self.pool,
                &self.event_bus,
                thread_id,
                Some(MessageOrigin::system()),
                crate::engine::claude_code::SettleTerminal::StuckProjection,
            )
            .await
            {
                Ok(true) => log_legacy!(
                    "[ExternalWatchdog] thread={} was `running` with no live session and \
                     nothing for {}min, settled",
                    thread_id,
                    self.limit_ms / 60_000,
                ),
                Ok(false) => {}
                Err(e) => log_legacy!(
                    "[ExternalWatchdog] settling orphaned thread={} failed: {}",
                    thread_id,
                    e
                ),
            }
        }
    }

    async fn recover_stuck(&self, candidates: Vec<StuckSession>) {
        let mut to_emit: Vec<StuckSession> = Vec::with_capacity(candidates.len());
        {
            let mut sessions = self.agent_sessions.lock().await;
            for c in candidates {
                let current_last_ms = c.last_event_at.load(std::sync::atomic::Ordering::Relaxed);
                if current_last_ms > c.snapshot_last_ms {
                    log_legacy!(
                        "[ExternalWatchdog] thread={} produced a fresh event since the snapshot (recovered) — leaving it alone",
                        c.thread_id,
                    );
                    continue;
                }
                let ours = owns_session_entry(&sessions, &c);
                // INVARIANT: Order matters: set the suppression flag BEFORE cancelling.
                // `agent_cancel.cancel()` can wake the driver_task / run_session
                // teardown on another worker, and that path checks
                // `external_terminal_already_emitted` before emitting its own
                // `ResponseAborted`. If the flag were still false in that window
                // it would emit a duplicate terminal on top of our
                // `ContinuationRequested`. The Release store before the (itself
                // synchronizing) `cancel()` guarantees any observer of the
                // cancellation also sees the flag set.
                //
                // The continuation flag rides the same ordering: it tells the
                // wedged loop's completion that the "external terminal" is a
                // RECOVERY continuation (not a restart abort / concurrent
                // cancel), so a conflict-resolution session hands its merge
                // duty off instead of aborting the apply and tearing down the
                // merge worktree under the continuation we're about to emit.
                //
                // That flag is the half a replacement forfeits: it promises a
                // `ContinuationRequested` that only the `to_emit` push below
                // sends. Setting it and then declining to emit strands the
                // wedged session's completion in a hand-off to nothing.
                if ours {
                    c.external_continuation
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                c.external_terminal
                    .store(true, std::sync::atomic::Ordering::Release);
                c.agent_cancel.cancel();
                c.idle_notify.notify_waiters();
                if !ours {
                    log_legacy!(
                        "[ExternalWatchdog] thread={} was taken over by a replacement session \
                         since the snapshot: cancelled the wedged subprocess, left the \
                         replacement's entry and its turn alone",
                        c.thread_id,
                    );
                    continue;
                }
                sessions.remove(&c.thread_id);
                to_emit.push(c);
            }
        }

        for s in to_emit {
            log_legacy!(
                "[ExternalWatchdog] thread={} stuck for {}s (limit={}min) — \
                 in-loop watchdog never fired (likely wedged event handler); \
                 cancelling the coding-agent subprocess + dropping session entry \
                 + emitting ContinuationRequested for auto-resume",
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

fn owns_session_entry(sessions: &HashMap<Uuid, AgentSession>, c: &StuckSession) -> bool {
    sessions
        .get(&c.thread_id)
        .is_some_and(|s| Arc::ptr_eq(&s.last_event_at, &c.last_event_at))
}

/// INVARIANT: One stuck-session snapshot — produced by tick's first pass, consumed by
/// the mutate + emit passes. Captured copies of the Arcs let those passes
/// run after the snapshot lock is released.
struct StuckSession {
    thread_id: Uuid,
    elapsed_ms: i64,
    external_terminal: Arc<std::sync::atomic::AtomicBool>,
    external_continuation: Arc<std::sync::atomic::AtomicBool>,
    idle_notify: Arc<tokio::sync::Notify>,
    agent_cancel: tokio_util::sync::CancellationToken,
    last_event_at: Arc<std::sync::atomic::AtomicI64>,
    snapshot_last_ms: i64,
    needs_running_check: bool,
}

#[cfg(test)]
#[path = "external_watchdog_tests.rs"]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 2_000_000_000_000;

    const CEILING: i64 = 45 * 60 * 1000;

    fn input(
        now: i64,
        last: i64,
        is_waiting: bool,
        tif: i32,
        loop_ended: bool,
    ) -> ExternalWatchdogInput {
        ExternalWatchdogInput {
            is_waiting,
            last_event_at_ms: last,
            tools_in_flight: tif,
            loop_ended,
            now_ms: now,
            limit_ms: EXTERNAL_WATCHDOG_LIMIT_MS,
            ceiling_ms: CEILING,
        }
    }

    #[test]
    fn exited_session_stale_resumes_if_running() {
        let stale = NOW - 15 * 60 * 1000;
        let out = external_watchdog_decision(input(NOW, stale, false, 0, true));
        assert_eq!(out, ExternalWatchdogDecision::ResumeIfRunning);
    }

    #[test]
    fn exited_session_fresh_is_skipped() {
        let fresh = NOW - 60 * 1000;
        let out = external_watchdog_decision(input(NOW, fresh, false, 0, true));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    #[test]
    fn exited_session_zero_heartbeat_is_skipped() {
        let out = external_watchdog_decision(input(NOW, 0, false, 0, true));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    #[test]
    fn waiting_session_is_skipped() {
        let stale = NOW - 15 * 60 * 1000;
        let out = external_watchdog_decision(input(NOW, stale, true, 0, false));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    #[test]
    fn tools_in_flight_skips() {
        let stale = NOW - 15 * 60 * 1000;
        let out = external_watchdog_decision(input(NOW, stale, false, 1, false));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    #[test]
    fn fresh_session_is_skipped() {
        let fresh = NOW - 60 * 1000;
        let out = external_watchdog_decision(input(NOW, fresh, false, 0, false));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    #[test]
    fn stuck_session_resumes() {
        let stale = NOW - 15 * 60 * 1000;
        let out = external_watchdog_decision(input(NOW, stale, false, 0, false));
        assert_eq!(out, ExternalWatchdogDecision::Resume);
    }

    #[test]
    fn exactly_at_limit_does_not_fire() {
        let at_limit = NOW - EXTERNAL_WATCHDOG_LIMIT_MS;
        let out = external_watchdog_decision(input(NOW, at_limit, false, 0, false));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    #[test]
    fn one_ms_past_limit_fires() {
        let stale = NOW - EXTERNAL_WATCHDOG_LIMIT_MS - 1;
        let out = external_watchdog_decision(input(NOW, stale, false, 0, false));
        assert_eq!(out, ExternalWatchdogDecision::Resume);
    }

    #[test]
    fn zero_last_event_skips() {
        let out = external_watchdog_decision(input(NOW, 0, false, 0, false));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }

    #[test]
    fn quiet_window_never_rounds_down_to_zero_seconds() {
        assert_eq!(quiet_window_secs(EXTERNAL_WATCHDOG_LIMIT_MS), 720);
        assert_eq!(quiet_window_secs(1000), 1);
        for sub_second in [0, 1, 50, 999] {
            assert_eq!(
                quiet_window_secs(sub_second),
                1,
                "a {sub_second}ms limit must still exclude anything active in the \
                 last second, never collapse to an unguarded 0s window",
            );
        }
    }

    #[test]
    fn the_external_limit_stays_above_the_in_loop_limit() {
        const {
            assert!(
                EXTERNAL_WATCHDOG_LIMIT_MS
                    > crate::engine::agent_session::lifecycle::WATCHDOG_INACTIVITY_LIMIT_MS,
                "the in-loop watchdog must get the first crack"
            )
        };
    }

    #[test]
    fn tools_in_flight_past_ceiling_resumes_if_running() {
        let past_ceiling = NOW - CEILING - 1;
        let out = external_watchdog_decision(input(NOW, past_ceiling, false, 3, false));
        assert_eq!(out, ExternalWatchdogDecision::ResumeIfRunning);
    }

    #[test]
    fn tools_in_flight_within_ceiling_skips() {
        let within_ceiling = NOW - CEILING + 60 * 1000;
        let out = external_watchdog_decision(input(NOW, within_ceiling, false, 3, false));
        assert_eq!(out, ExternalWatchdogDecision::Skip);
    }
}
