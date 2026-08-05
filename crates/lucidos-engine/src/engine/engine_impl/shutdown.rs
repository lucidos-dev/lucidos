//! Restart-abort and graceful shutdown of active threads, browser, agent sessions.
//!
//! Part of the `LucidosEngine` inherent impl, split from engine_impl.rs.

use super::super::*;

impl LucidosEngine {
    /// Mark the engine as shutting down / restarting. Idempotent — the flag is
    /// never cleared because the process is on its way out. Called at the very
    /// start of both shutdown paths (the `main.rs` signal handler and
    /// `abort_in_flight_for_restart`) so the scheduler stops firing
    /// event-triggers before any terminator cleanup event is emitted.
    pub fn mark_shutting_down(&self) {
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// True once shutdown / restart has begun. The scheduler's event subscriber
    /// reads this to skip event-trigger dispatch: a trigger fired from a
    /// shutdown-cleanup event would spawn a script that calls back into the
    /// HTTP API being torn down, producing a spurious "<trigger> failed" push.
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Teardown-time boundary emission for the *Switch to new version* flow.
    /// Called from `main.rs::shutdown_signal` at ACTUAL engine teardown (the
    /// SIGUSR1 graceful shutdown), NOT at switch-request time — so nothing shows
    /// "Switched/Aborted" while the old engine is still alive through a dev
    /// rebuild. Walks the in-flight chat AND CC threads and emits the boundary
    /// events with the `actor` the switch handler stashed (via
    /// `take_restart_actor`): a device actor → "You restarted"; `None` (bare
    /// stop.sh / external SIGUSR1) → "⚙ System restarted".
    ///
    /// For chat threads: emits `ResponseAborted { actor: <actor> }` with
    /// `request_event_id` pointing to the originating MessageReceived/
    /// TriggerStarted.
    ///
    /// For CC threads: emits both `ResponseAborted` AND the synthetic
    /// `CodingAgentIdled { reason: engine_restart_interrupt }` so the spawn
    /// dispatcher's classifier (which runs after restart on recovery) sees a
    /// thread that's already terminated and skips re-emitting the same pair.
    /// `actor` flows onto both events.
    ///
    /// Idempotent — reading the per-event guard inside the projection ensures
    /// a duplicate restart click does not double-emit.
    pub async fn abort_in_flight_for_restart(
        self: &std::sync::Arc<Self>,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) {
        // Stop the scheduler firing event-triggers before we emit the abort
        // events below — those would otherwise fan out to triggers whose
        // scripts hit the API mid-restart and fail.
        self.mark_shutting_down();

        // `all_cc_thread_ids` covers idle coding-agent sessions too — their run loop
        // stays registered in `active_threads` between turns, so they show up
        // in `processing_thread_ids()` and must be excluded from the chat
        // bucket. `cc_thread_ids` (in-flight only) and `external_emitted_flags`
        // drive the pre-emit; the flag tells `run_session`'s classify/safety
        // paths to skip a duplicate emit (see `external_terminal_emitted`).
        let (all_cc_thread_ids, cc_thread_ids, external_emitted_flags): (
            std::collections::HashSet<uuid::Uuid>,
            Vec<uuid::Uuid>,
            Vec<std::sync::Arc<std::sync::atomic::AtomicBool>>,
        ) = {
            let guard = self.agent_sessions.lock().await;
            let all = guard.keys().copied().collect();
            let (ids, flags) = guard
                .iter()
                .filter(|(_, s)| s.is_in_flight())
                .map(|(tid, s)| (*tid, s.external_terminal_emitted.clone()))
                .unzip();
            (all, ids, flags)
        };

        let chat_thread_ids =
            partition_chat_thread_ids(&self.processing_thread_ids(), &all_cc_thread_ids);

        // ---- Chat threads ---------------------------------------------------
        // Look up originating event ids in parallel — sequential awaits would
        // serialize N round-trips on a busy restart. The CTC entry in
        // `CHAT_ORIGINATING_EVENT_TYPES` is what makes a chat agent woken by a
        // child finishing get its abort stamped with the CTC's id rather than
        // a stale older `MessageReceived` from a previous turn — without it
        // `groupIntoExchanges` routes the abort into the wrong (already-
        // completed) exchange and the live `UserQuestionAsked` exchange never
        // gets the abort step.
        let chat_originating_ids: Vec<Option<uuid::Uuid>> =
            futures::future::join_all(chat_thread_ids.iter().map(|tid| {
                crate::engine::agent_session::latest_originating_event_id(
                    &self.pool,
                    *tid,
                    crate::engine::agent_session::CHAT_ORIGINATING_EVENT_TYPES,
                )
            }))
            .await;
        for (thread_id, originating_event_id) in chat_thread_ids.iter().zip(chat_originating_ids) {
            // Same preserve guard as the coding-agent branch below: a chat thread
            // parked on an unanswered question is preserved (no boundary abort) so
            // its card stays answerable and answering resumes. The chat agent's
            // `ask_user_question` blocks the loop on the same wait registry as CC,
            // so a question-parked chat thread is still in `processing_thread_ids()`
            // and would otherwise get the device "Restarted" abort here (the
            // reproduced chat screenshot). `None` channel = chat bucket.
            emit_teardown_abort_unless_question_parked(
                &self.pool,
                &self.event_bus,
                *thread_id,
                None,
                "This response was interrupted by an engine restart.".to_string(),
                originating_event_id,
                actor.clone(),
            )
            .await;
            // Drop the thread from `active_threads` so the subsequent
            // `shutdown_active_threads` sweep doesn't see it in
            // `processing_thread_ids()` and emit a second System abort on top
            // of the device "Restarted" panel we just persisted. CC's side of
            // this is the `external_terminal_emitted` flag on `AgentSession`
            // because `run_session` keeps running and re-reads it; the chat
            // loop has no equivalent re-read — it just exits when its token
            // is cancelled, so eviction is enough.
            self.force_evict_chat_thread(*thread_id);
        }

        // ---- CC threads -----------------------------------------------------
        // Pre-emit ONLY the boundary `ResponseAborted{actor: device}` so the
        // post-restart timeline reads "You restarted" on the AbortPanel.
        // The synthetic `CodingAgentIdled{engine_restart_interrupt}` that
        // drives the spawn-dispatcher classifier is left to the post-restart
        // recovery sweep — it owns the decision of whether to preserve the
        // worktree (the worktree is `--resume`'d by the user's Continue click)
        // or clean it up. Pre-emitting that idle event from here would push
        // the branch into `idle_branches` on restart and trigger a worktree
        // cleanup, breaking the Continue flow.
        // CC parents woken from a finished child also have CTC as their
        // turn's `request_event_id` — `notify_parent_of_child_completion`
        // passes the CTC id as `pre_emitted_origin` regardless of whether
        // `parent_is_coding_agent`, and `run_session` stamps
        // `EventMeta::request_event_id = Some(origin_id)` on every CC event.
        // `CC_ORIGINATING_EVENT_TYPES` is the chat list + CCUMS, so it covers
        // both regular CC follow-ups and wake-from-child.
        //
        // Per-thread work runs concurrently — same rationale as the chat
        // lookups above (sequential awaits would serialize N round-trips on a
        // busy restart), and each thread's chain needs two queries now: the
        // originating-id lookup plus the question-parked check inside
        // `emit_teardown_abort_unless_question_parked`. Within one thread's
        // future the order is fixed: the flag is set AFTER the emit lands so
        // any Result arriving from that point on observes it and skips its own
        // duplicate emit. Set on the question-parked skip too: the shutdown
        // interrupt still makes CC produce a Result that classifies
        // `Aborted(EngineShutdown)`, and the stop arm's `stop_terminal_kind`
        // does the same — without the flag, either in-loop path would land the
        // very abort the skip just avoided.
        futures::future::join_all(cc_thread_ids.iter().zip(external_emitted_flags).map(
            |(thread_id, flag)| {
                let actor = actor.clone();
                async move {
                    let originating_event_id =
                        crate::engine::agent_session::latest_originating_event_id(
                            &self.pool,
                            *thread_id,
                            crate::engine::agent_session::CC_ORIGINATING_EVENT_TYPES,
                        )
                        .await;
                    emit_teardown_abort_unless_question_parked(
                        &self.pool,
                        &self.event_bus,
                        *thread_id,
                        Some(crate::engine::thread_events::EventChannel::ClaudeCode),
                        String::new(),
                        originating_event_id,
                        actor,
                    )
                    .await;
                    flag.store(true, std::sync::atomic::Ordering::Release);
                }
            },
        ))
        .await;
    }

    /// Emit ResponseAborted for all active non-CC threads during engine shutdown.
    /// CC threads are handled separately by `shutdown_agent_sessions`.
    ///
    /// After emitting, cancels all threads so their tasks can clean up. The
    /// agentic loop may also emit ResponseCanceled on cancellation, and the
    /// idempotency gate in `emit_response_canceled` can't suppress it here (the
    /// abort above carries no `request_event_id` to match on). Having both is
    /// harmless: the exchange label prefers Aborted over Canceled regardless of
    /// order, and the `thread_summaries.status` column
    /// (and IS last-write-wins) keeps the abort's verdict: `'paused'`, since
    /// `EngineShutdown` is a transient cause, preserved because the
    /// `ResponseCanceled` projection arm is `preserving_verdict`. Both halves
    /// are load-bearing; the status half used to be missing, which erased the
    /// interrupted thread's status dot.
    ///
    /// Stamps `actor: System` so the AbortPanel renders ⚙ System — the host
    /// system killed these in-flight responses (engine shutdown). The
    /// user-driven `/api/v1/restart` path pre-emits with `actor: Device {..}`
    /// BEFORE shutdown for in-flight threads it knows about; this fallback
    /// covers anything that started after that pre-emit.
    pub async fn shutdown_active_threads(&self) {
        let active_ids = self.processing_thread_ids();
        if active_ids.is_empty() {
            return;
        }
        // CC threads (in-flight or idle) are handled by shutdown_agent_sessions.
        let all_cc_thread_ids: std::collections::HashSet<uuid::Uuid> =
            self.agent_sessions.lock().await.keys().copied().collect();
        for thread_id in partition_chat_thread_ids(&active_ids, &all_cc_thread_ids) {
            // Preserve guard (defense-in-depth): a thread parked on an unanswered
            // question is a resumable checkpoint, never aborted. `abort_in_flight_for_restart`
            // already skips + evicts these on the user-switch path; this covers a
            // thread that reached `processing_thread_ids()` after that pre-emit.
            // Same shared predicate as every other restart-abort path.
            if crate::engine::agent_recovery::thread_has_unanswered_question(&self.pool, thread_id)
                .await
            {
                log!(
                    "[Shutdown] Preserving thread {} — parked on an unanswered question",
                    thread_id
                );
                continue;
            }
            log!(
                "[Shutdown] Emitting ResponseAborted for active thread {}",
                thread_id
            );
            // Direct .emit (not emit_response_aborted): wants the Err for the per-thread log below.
            if let Err(e) = self
                .event_bus
                .emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::ResponseAborted {
                        text: "This response was interrupted by an engine shutdown.".to_string(),
                        images: vec![],
                        model: None,
                        reasoning_effort: None,
                        cause: crate::engine::thread_events::AbortCause::EngineShutdown,
                    },
                    meta: crate::engine::thread_events::EventMeta::with_actor(Some(
                        crate::engine::thread_events::MessageOrigin::system(),
                    )),
                })
                .await
            {
                log!(
                    "[Shutdown] Failed to emit ResponseAborted for thread {}: {}",
                    thread_id,
                    e
                );
            }
        }
        self.cancel_all_threads(None);
    }

    pub async fn shutdown_browser(&self) {
        if let Err(e) = self.browser_runtime.close_all().await {
            log!("[Engine] Error closing browsers on shutdown: {}", e);
        }
    }

    /// Gracefully stop all running coding-agent sessions.
    /// Sends interrupt to active sessions, waits for them to produce
    /// a Result event and go idle (persisting cc_session_id in CodingAgentIdled),
    /// then cancels remaining sessions.
    pub async fn shutdown_agent_sessions(&self) {
        // Mark all sessions as shutting down and collect their interrupt/stop handles.
        // The flag is what makes the teardown boundary an abort rather than a cancel:
        // `stop_terminal_kind` reads it and yields `Aborted(EngineShutdown)`, which the
        // frontend renders as "Response interrupted" and recovery reads back as the
        // *Switch to new version* fingerprint. It does NOT produce a `SessionEnded`
        // (the post-loop cleanup bails out early during shutdown), and the retired
        // `SessionEndReason::Completed` this comment used to contrast with is long gone.
        let sessions: Vec<(
            uuid::Uuid,
            std::sync::Arc<tokio::sync::Notify>,
            std::sync::Arc<tokio::sync::Notify>,
        )> = {
            let guard = self.agent_sessions.lock().await;
            for s in guard.values() {
                s.shutting_down
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            guard
                .iter()
                .map(|(tid, s)| (*tid, s.interrupt.clone(), s.stop.clone()))
                .collect()
        };

        if sessions.is_empty() {
            return;
        }

        log!(
            "[Shutdown] Gracefully stopping {} Claude Code session(s)...",
            sessions.len()
        );

        // Phase 1: Interrupt all active sessions (like pressing Esc).
        // This makes CC stop current work and emit a Result event, which triggers
        // CodingAgentIdled (with cc_session_id) to be persisted to DB.
        //
        // EXCEPT a session parked on an unanswered `AskUserQuestion`. Esc is a
        // USER gesture, and CC applies it to the tool it is blocked on: the
        // pending `AskUserQuestion`. CC then records a rejection the user never
        // made ("The user doesn't want to proceed with this tool use") as a
        // `CodingAgentToolResult` and races on past the question. That single
        // event lands after the `UserQuestionAsked`, so it strikes the card
        // through client-side AND ends the park server-side, while the terminal
        // that would normally follow is suppressed by
        // `external_terminal_emitted`. What is left is a thread with a dead
        // question, no terminator, and a permanent "Working" (the 2026-08-01
        // report; see
        // `docs/plans/2026-08-01-preserve-question-parked-session-through-teardown.md`).
        //
        // This was the one restart path with no preserve guard. Parked sessions
        // go straight to the hard stop instead, whose arm skips its own terminal
        // and text flush for exactly this reason
        // (`preserve_question_park_at_shutdown`) while still cancelling the
        // runtime, so the subprocess dies with the engine and the poll below is
        // not spent waiting on a session that was never asked to stop.
        // Probe concurrently, then notify synchronously. Same rationale as the
        // per-thread lookups in `abort_in_flight_for_restart` above: serial
        // awaits here would hold every session's Esc behind N round-trips on a
        // busy restart.
        let parked: Vec<bool> = futures::future::join_all(sessions.iter().map(|(tid, _, _)| {
            crate::engine::agent_recovery::preserve_question_park_at_shutdown(
                &self.pool,
                "session sweep",
                *tid,
                true,
            )
        }))
        .await;
        for ((tid, interrupt, stop), is_parked) in sessions.iter().zip(parked) {
            if is_parked {
                stop.notify_one();
                continue;
            }
            log!("[Shutdown] Interrupting Claude Code session {}", tid);
            interrupt.notify_one();
        }

        // Phase 2: Poll until all sessions are gone or 10 seconds elapse.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let remaining = self.agent_sessions.lock().await.len();
            if remaining == 0 {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                log!(
                    "[Shutdown] {} session(s) still active after timeout — force-stopping",
                    remaining
                );
                // The stop arm reads `is_shutdown=true` (set above) and emits
                // `Aborted(EngineShutdown)` for actively-working sessions, nothing
                // for idle sessions — never `ResponseCanceled` here.
                for (tid, _, stop) in &sessions {
                    if self.agent_sessions.lock().await.contains_key(tid) {
                        stop.notify_one();
                    }
                }
                // Brief wait for stop cleanup
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        log!("[Shutdown] Coding-agent sessions stopped.");
    }
}

/// Teardown boundary emit for one in-flight thread — coding-agent OR chat:
/// emits the `ResponseAborted { cause: EngineShutdown, actor }` that
/// post-restart recovery reads as "user-initiated switch" — UNLESS the thread
/// is parked on an unanswered question, which is preserved instead.
///
/// A question-parked thread is a stable, resumable checkpoint (decision 7 of
/// `docs/plans/2026-07-01-new-engine-version-switch-flow.md`): it survives any
/// restart as `waiting_for_user_answer` with its card answerable, and answering
/// resumes (coding agents via `ContinuationRequested` → `--resume`; chat via
/// the answer-resume path in `answer_pending_question`). The session cannot be
/// filtered out by `is_in_flight()` — the subprocess/loop is alive MID-TURN,
/// blocked inside the AskUserQuestion hook / `walk_question_batch` — so without
/// this check the boundary abort landed on every user switch: it rendered
/// "interrupted" over the live card and, worse, counted as a terminal in
/// recovery's `thread_has_unanswered_question`, defeating the preserve guard.
///
/// `channel` selects the bucket: `Some(EventChannel::ClaudeCode)` for a
/// coding-agent thread (empty `text`), `None` for a chat thread (which carries
/// the human-readable interrupted text). Both consult the SAME shared
/// `thread_has_unanswered_question` predicate — the DRY guard so a chat and a
/// coding-agent restart can't diverge on what "parked" means.
///
/// Returns whether the abort was emitted. For coding-agent threads the caller
/// must set the session's `external_terminal_emitted` flag in BOTH cases — the
/// preserved session's run loop still sees the shutdown interrupt (both the
/// Result classify and the stop arm produce `Aborted(EngineShutdown)`), and
/// only that flag keeps those in-loop paths from landing the abort this skip
/// just avoided.
pub(crate) async fn emit_teardown_abort_unless_question_parked(
    pool: &sqlx::PgPool,
    event_bus: &crate::engine::event_bus::EventBus,
    thread_id: uuid::Uuid,
    channel: Option<crate::engine::thread_events::EventChannel>,
    text: String,
    originating_event_id: Option<uuid::Uuid>,
    actor: Option<crate::engine::thread_events::MessageOrigin>,
) -> bool {
    use crate::engine::thread_events::{self, EventMeta};

    if crate::engine::agent_recovery::thread_has_unanswered_question(pool, thread_id).await {
        log!(
            "[Restart] Preserving thread {} — parked on an unanswered question, no boundary abort",
            thread_id
        );
        return false;
    }
    thread_events::emit_response_aborted(
        event_bus,
        thread_id,
        thread_events::AbortCause::EngineShutdown,
        text,
        vec![],
        None,
        None,
        EventMeta {
            channel,
            request_event_id: originating_event_id,
            actor,
            ..EventMeta::NONE
        },
        "[Restart] ResponseAborted (teardown)",
    )
    .await;
    true
}

#[cfg(test)]
#[path = "shutdown_tests.rs"]
mod shutdown_tests;
