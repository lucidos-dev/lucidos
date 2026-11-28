use super::super::*;

impl LucidosEngine {
    pub fn mark_shutting_down(&self) {
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// INVARIANT: True once shutdown / restart has begun. Every trigger-fire path reads
    /// this one signal: event dispatch, both cron paths, and the Thread Queue's
    /// admission. A fire started now would call back into the HTTP API being
    /// torn down, producing a spurious "<trigger> failed" push.
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// INVARIANT: Teardown-time boundary emission for the *Switch to new version* flow.
    /// Called from `main.rs::shutdown_signal` at ACTUAL engine teardown (the
    /// SIGUSR1 graceful shutdown), NOT at switch-request time — so nothing shows
    /// "Switched/Aborted" while the old engine is still alive through a dev
    /// rebuild. Walks the in-flight chat AND CC threads and emits the boundary
    /// events with the `actor` the switch handler stashed (via
    /// `take_restart_actor`): a device actor → "Paused by restart"; `None`
    /// (bare stop.sh / external SIGUSR1) → "System / Response interrupted".
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
        // INVARIANT: Stop the scheduler firing event-triggers before we emit the abort
        // events below — those would otherwise fan out to triggers whose
        // scripts hit the API mid-restart and fail.
        self.mark_shutting_down();

        let (all_cc_thread_ids, cc_thread_ids, external_emitted_flags): (
            std::collections::HashSet<uuid::Uuid>,
            Vec<uuid::Uuid>,
            Vec<std::sync::Arc<std::sync::atomic::AtomicBool>>,
        ) = {
            let guard = self.agent_sessions.lock().await;
            let all = guard.keys().copied().collect();
            let (ids, flags) = guard
                .iter()
                .filter(|(_, s)| {
                    s.is_in_flight() && !matches!(s.phase, crate::engine::SessionPhase::Applying)
                })
                .map(|(tid, s)| (*tid, s.external_terminal_emitted.clone()))
                .unzip();
            (all, ids, flags)
        };

        let chat_thread_ids =
            partition_chat_thread_ids(&self.processing_thread_ids(), &all_cc_thread_ids);

        // INVARIANT: ---- Chat threads ---------------------------------------------------
        // Resolve each thread's anchor in parallel, because sequential awaits
        // would serialize N round-trips on a busy restart. The running turn
        // recorded its own `request_event_id` on the handle, so the abort
        // terminates the turn that is actually in flight and the loop's
        // follow-up cancel is suppressed by the gate in
        // `emit_response_canceled`. The `CHAT_ORIGINATING_EVENT_TYPES` query
        // behind it is the fallback for a thread whose turn never got that far;
        // the CTC entry in that list is what makes a chat agent woken by a child
        // finishing resolve to the CTC rather than a stale older
        // `MessageReceived` from a previous turn. Without an anchor
        // `groupIntoExchanges` routes the abort into the wrong (already-
        // completed) exchange and the live `UserQuestionAsked` exchange never
        // gets the abort step.
        let chat_originating_ids: Vec<Option<uuid::Uuid>> =
            futures::future::join_all(chat_thread_ids.iter().map(|tid| {
                crate::engine::in_flight_request_event_id(
                    &self.active_threads,
                    &self.pool,
                    *tid,
                    crate::engine::thread_events::CHAT_ORIGINATING_EVENT_TYPES,
                )
            }))
            .await;
        for (thread_id, originating_event_id) in chat_thread_ids.iter().zip(chat_originating_ids) {
            let boundary = emit_teardown_abort_unless_question_parked(
                &self.pool,
                &self.event_bus,
                *thread_id,
                None,
                "This response was interrupted by an engine restart.".to_string(),
                originating_event_id,
                actor.clone(),
            )
            .await;
            if boundary.settles_turn() {
                self.force_evict_chat_thread(*thread_id);
            }
        }

        // INVARIANT: ---- CC threads -----------------------------------------------------
        // Pre-emit ONLY the boundary `ResponseAborted{actor: device}` so the
        // post-restart timeline reads "Paused by restart" on the AbortPanel.
        // The synthetic `CodingAgentIdled{engine_restart_interrupt}` that
        // drives the spawn-dispatcher classifier is left to the post-restart
        // recovery sweep — it owns the decision of whether to preserve the
        // worktree (the worktree is `--resume`'d by the user's Continue click)
        // or clean it up. Pre-emitting that idle event from here would push
        // the branch into `idle_branches` on restart and trigger a worktree
        // cleanup, breaking the Continue flow.
        // The anchor comes off the handle, same as the chat bucket, for a
        // session the chat path spawned: it holds its `ThreadGuard` across the
        // whole session (`process_cc.rs`) and `run_session` stamps that same
        // `EventMeta::request_event_id = Some(origin_id)` on every CC event,
        // which the `msg_rx` follow-up arm does NOT rewrite, so handle and
        // session agree for the session's whole life. CC parents woken from a
        // finished child need no special case: `notify_parent_of_child_completion`
        // passes the CTC id as `pre_emitted_origin` regardless of whether
        // `parent_is_coding_agent`, and that id is what the handle records.
        // A session the spawn dispatcher resumed calls `run_direct_agent`
        // directly and registers no handle at all, so it takes the
        // `CC_ORIGINATING_EVENT_TYPES` fallback (the chat list plus CCUMS),
        // exactly as every coding-agent thread did before.
        //
        // Per-thread work runs concurrently, same rationale as the chat
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
                    let originating_event_id = crate::engine::in_flight_request_event_id(
                        &self.active_threads,
                        &self.pool,
                        *thread_id,
                        crate::engine::thread_events::CC_ORIGINATING_EVENT_TYPES,
                    )
                    .await;
                    let boundary = emit_teardown_abort_unless_question_parked(
                        &self.pool,
                        &self.event_bus,
                        *thread_id,
                        Some(crate::engine::thread_events::EventChannel::ClaudeCode),
                        String::new(),
                        originating_event_id,
                        actor,
                    )
                    .await;
                    if boundary.settles_turn() {
                        flag.store(true, std::sync::atomic::Ordering::Release);
                    }
                }
            },
        ))
        .await;
    }

    pub async fn shutdown_active_threads(&self) {
        let active_ids = self.processing_thread_ids();
        if active_ids.is_empty() {
            return;
        }
        let actor = self
            .teardown_actor()
            .unwrap_or_else(crate::engine::thread_events::MessageOrigin::system);
        let all_cc_thread_ids: std::collections::HashSet<uuid::Uuid> =
            self.agent_sessions.lock().await.keys().copied().collect();
        for thread_id in partition_chat_thread_ids(&active_ids, &all_cc_thread_ids) {
            if crate::engine::question_park_sql::thread_has_unanswered_question(
                &self.pool, thread_id,
            )
            .await
            {
                log_legacy!(
                    "[Shutdown] Preserving thread {} — parked on an unanswered question",
                    thread_id
                );
                continue;
            }
            log_legacy!(
                "[Shutdown] Emitting ResponseAborted for active thread {}",
                thread_id
            );
            let request_event_id = crate::engine::in_flight_request_event_id(
                &self.active_threads,
                &self.pool,
                thread_id,
                crate::engine::thread_events::CHAT_ORIGINATING_EVENT_TYPES,
            )
            .await;
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
                    meta: crate::engine::thread_events::EventMeta {
                        request_event_id,
                        actor: Some(actor.clone()),
                        ..crate::engine::thread_events::EventMeta::NONE
                    },
                })
                .await
            {
                log_legacy!(
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
            log_legacy!("[Engine] Error closing browsers on shutdown: {}", e);
        }
    }

    pub async fn shutdown_agent_sessions(&self) {
        // INVARIANT: Mark all sessions as shutting down and collect their interrupt/stop handles.
        // The flag is what makes the teardown boundary an abort rather than a cancel:
        // `stop_terminal_kind` reads it and yields `Aborted(EngineShutdown)`, which the
        // frontend renders as "Response interrupted" and recovery reads back as the
        // *Switch to new version* fingerprint. It does NOT produce a `SessionEnded`
        // (the post-loop cleanup bails out early during shutdown), and the retired
        // `SessionEndReason::Completed` this comment used to contrast with is long gone.
        // Thread, interrupt, stop, and the startup cancel of a run still starting.
        type SessionHandles = (
            uuid::Uuid,
            std::sync::Arc<tokio::sync::Notify>,
            std::sync::Arc<tokio::sync::Notify>,
            Option<tokio_util::sync::CancellationToken>,
        );
        let sessions: Vec<SessionHandles> = {
            let guard = self.agent_sessions.lock().await;
            for s in guard.values() {
                s.shutting_down
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            guard
                .iter()
                .map(|(tid, s)| {
                    let startup_cancel = match &s.phase {
                        crate::engine::SessionPhase::Starting { startup_cancel } => {
                            Some(startup_cancel.clone())
                        }
                        crate::engine::SessionPhase::Running { .. }
                        | crate::engine::SessionPhase::Applying => None,
                    };
                    (*tid, s.interrupt.clone(), s.stop.clone(), startup_cancel)
                })
                .collect()
        };

        if sessions.is_empty() {
            return;
        }

        log_legacy!(
            "[Shutdown] Gracefully stopping {} Claude Code session(s)...",
            sessions.len()
        );

        // INVARIANT: Phase 1: Interrupt all active sessions (like pressing Esc).
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
        let parked: Vec<bool> = futures::future::join_all(sessions.iter().map(|(tid, _, _, _)| {
            crate::engine::question_park_sql::preserve_question_park_at_shutdown(
                &self.pool,
                "session sweep",
                *tid,
                true,
            )
        }))
        .await;
        for ((tid, interrupt, stop, startup_cancel), is_parked) in sessions.iter().zip(parked) {
            if is_parked {
                stop.notify_one();
                continue;
            }
            log_legacy!("[Shutdown] Interrupting Claude Code session {}", tid);
            match startup_cancel {
                Some(startup_cancel) => startup_cancel.cancel(),
                None => interrupt.notify_one(),
            }
        }

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let remaining = self.agent_sessions.lock().await.len();
            if remaining == 0 {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                log_legacy!(
                    "[Shutdown] {} session(s) still active after timeout — force-stopping",
                    remaining
                );
                for (tid, _, stop, _) in &sessions {
                    if self.agent_sessions.lock().await.contains_key(tid) {
                        stop.notify_one();
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        log_legacy!("[Shutdown] Coding-agent sessions stopped.");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeardownBoundary {
    Preserved,
    Aborted,
    Unwritten,
}

impl TeardownBoundary {
    pub fn settles_turn(self) -> bool {
        self != Self::Unwritten
    }
}

const TEARDOWN_ABORT_ATTEMPTS: u32 = 3;

pub async fn emit_teardown_abort_unless_question_parked(
    pool: &sqlx::PgPool,
    event_bus: &crate::engine::event_bus::EventBus,
    thread_id: uuid::Uuid,
    channel: Option<crate::engine::thread_events::EventChannel>,
    text: String,
    originating_event_id: Option<uuid::Uuid>,
    actor: Option<crate::engine::thread_events::MessageOrigin>,
) -> TeardownBoundary {
    use crate::engine::thread_events::{EventMeta, ThreadEvent};

    if crate::engine::question_park_sql::thread_has_unanswered_question(pool, thread_id).await {
        log_legacy!(
            "[Restart] Preserving thread {} — parked on an unanswered question, no boundary abort",
            thread_id
        );
        return TeardownBoundary::Preserved;
    }
    let mut attempt = 1;
    loop {
        let outcome = event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ResponseAborted {
                    text: text.clone(),
                    images: vec![],
                    model: None,
                    reasoning_effort: None,
                    cause: crate::engine::thread_events::AbortCause::EngineShutdown,
                },
                meta: EventMeta {
                    channel,
                    request_event_id: originating_event_id,
                    actor: actor.clone(),
                    ..EventMeta::NONE
                },
            })
            .await;
        match outcome {
            Ok(Some(_)) => return TeardownBoundary::Aborted,
            Ok(None) => {
                log_legacy!(
                    "[Restart] Boundary abort for thread {} was not persisted",
                    thread_id
                );
                return TeardownBoundary::Unwritten;
            }
            Err(e) if attempt < TEARDOWN_ABORT_ATTEMPTS && rolled_back(e.as_ref()) => {
                log_legacy!(
                    "[Restart] Boundary abort for thread {} rolled back on attempt {}, retrying: {}",
                    thread_id,
                    attempt,
                    e
                );
                attempt += 1;
            }
            Err(e) => {
                log_legacy!(
                    "[Restart] Boundary abort for thread {} failed: {}",
                    thread_id,
                    e
                );
                return TeardownBoundary::Unwritten;
            }
        }
    }
}

fn rolled_back(err: &(dyn std::error::Error + 'static)) -> bool {
    err.downcast_ref::<sqlx::Error>()
        .and_then(|e| match e {
            sqlx::Error::Database(db) => db.code(),
            _ => None,
        })
        .is_some_and(|code| code.starts_with("40"))
}
