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
        self.shutting_down
            .load(std::sync::atomic::Ordering::SeqCst)
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
        use crate::engine::thread_events::{self, EventChannel, EventMeta};

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
            thread_events::emit_response_aborted(
                &self.event_bus,
                *thread_id,
                thread_events::AbortCause::EngineShutdown,
                "This response was interrupted by an engine restart.".to_string(),
                vec![],
                None,
                None,
                EventMeta {
                    request_event_id: originating_event_id,
                    actor: actor.clone(),
                    ..EventMeta::NONE
                },
                "[Restart] ResponseAborted (chat)",
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
        let cc_originating_ids: Vec<Option<uuid::Uuid>> =
            futures::future::join_all(cc_thread_ids.iter().map(|tid| {
                crate::engine::agent_session::latest_originating_event_id(
                    &self.pool,
                    *tid,
                    crate::engine::agent_session::CC_ORIGINATING_EVENT_TYPES,
                )
            }))
            .await;
        for ((thread_id, originating_event_id), flag) in cc_thread_ids
            .iter()
            .zip(cc_originating_ids)
            .zip(external_emitted_flags)
        {
            thread_events::emit_response_aborted(
                &self.event_bus,
                *thread_id,
                thread_events::AbortCause::EngineShutdown,
                String::new(),
                vec![],
                None,
                None,
                EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    request_event_id: originating_event_id,
                    actor: actor.clone(),
                    ..EventMeta::NONE
                },
                "[Restart] ResponseAborted (cc)",
            )
            .await;
            // Set AFTER the emit lands so any Result arriving from this point
            // on observes the flag and skips its own duplicate emit.
            flag.store(true, std::sync::atomic::Ordering::Release);
        }
    }

    /// Emit ResponseAborted for all active non-CC threads during engine shutdown.
    /// CC threads are handled separately by `shutdown_agent_sessions`.
    ///
    /// After emitting, cancels all threads so their tasks can clean up. The
    /// agentic loop may also emit ResponseCanceled on cancellation — having both
    /// is harmless (ResponseAborted takes precedence in status derivation).
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
        // CC cleanup reads `shutting_down` to emit SessionEnded { reason: "shutdown" }
        // instead of "completed" — the frontend uses this to show "Aborted".
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
        for (tid, interrupt, _) in &sessions {
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
