use super::lifecycle::{stop_terminal_kind, TerminalKind};
use crate::engine::thread_events::{EventChannel, MessageOrigin, SessionEndReason};
use crate::engine::LucidosEngine;
use uuid::Uuid;

impl LucidosEngine {
    pub(crate) fn clear_cc_debounce(&self, thread_id: Uuid) {
        if let Ok(mut spawns) = self.last_cc_spawn.lock() {
            spawns.remove(&thread_id);
        }
    }

    /// Which backend drives this thread, from the `thread_summaries`
    /// projection. NULL / missing row / DB error all default to `ClaudeCode`
    /// — every thread persisted before the column existed was CC, and the
    /// callers (event stamping on engine-internal flows) prefer a default
    /// stamp over a hard failure.
    pub(crate) async fn thread_coding_agent(&self, thread_id: Uuid) -> crate::runtime::CodingAgent {
        let stored: Option<String> =
            sqlx::query_scalar("SELECT coding_agent FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&self.pool)
                .await
                .unwrap_or_else(|e| {
                    crate::log!(
                "[AgentSession] thread_coding_agent({}) DB error: {} — defaulting to ClaudeCode",
                thread_id,
                e
            );
                    None
                })
                .flatten();
        stored
            .map(|s| crate::runtime::CodingAgent::parse(&s))
            .unwrap_or(crate::runtime::CodingAgent::ClaudeCode)
    }

    /// Signal the agent runtime to terminate, then flush any un-persisted text
    /// as a final stream event. The runtime task watches `cancel` and kills its
    /// child; we don't own the process here.
    pub(crate) async fn kill_cc_and_flush(
        cancel: &tokio_util::sync::CancellationToken,
        claude_text_buf: &str,
        last_text_persisted_len: usize,
        event_bus: &crate::engine::event_bus::EventBus,
        thread_id: Uuid,
        meta: &crate::engine::thread_events::EventMeta,
        coding_agent: crate::runtime::CodingAgent,
    ) {
        cancel.cancel();
        if !claude_text_buf.is_empty() {
            let delta =
                &claude_text_buf[claude_text_buf.floor_char_boundary(last_text_persisted_len)..];
            if !delta.is_empty() {
                event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event:
                                crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed {
                                    text: delta.to_string(),
                                    coding_agent,
                                },
                            meta: meta.clone(),
                        },
                        "[AgentSession] CodingAgentTextStreamed flush on cancel",
                    )
                    .await;
            }
        }
    }

    /// Is this session shutting down? **The only definition**, for every reader
    /// that classifies a turn: the two halves are each wrong alone, and the
    /// interesting failures are two readers disagreeing. (`entry_guard.rs` reads
    /// the per-session flag bare on purpose, asking who OWNS the terminal rather
    /// than what it should be; see that field's doc in `engine::types`.)
    ///
    /// `per_session` is the caller's own load of
    /// [`crate::engine::types::AgentSession::shutting_down`], a **snapshot**
    /// signal: `shutdown_agent_sessions` sets it on the sessions present in
    /// `agent_sessions` when its pass ran, and on nobody else. A session
    /// inserted after that pass carries `false` while the engine is very much
    /// going down. Taking the loaded `bool` rather than the atomic keeps the
    /// "one read per decision" discipline with the caller, where the read is.
    ///
    /// `is_shutting_down()` is the **durable** half: `mark_shutting_down()` is
    /// the first statement of `abort_in_flight_for_restart`, so it is already
    /// true before the teardown boundary is even emitted. Neither half is
    /// sufficient alone: the per-session flag misses a late registration, and
    /// the global one is not per-session at all.
    ///
    /// Reading either half alone has cost us twice, and both were the same
    /// late-registering session:
    ///
    /// * **thread-9e37697e** (data loss): `finalize_direct_agent` read only the
    ///   per-session flag, ran full normal cleanup on a session whose
    ///   `spawn_or_resume` finished after the pass, and `git branch -D`'d a
    ///   still-resumable branch.
    /// * **The 2026-08-06 switch report**: `emit_stop_terminal` read only the
    ///   per-session flag when choosing the terminal event, so a *Switch to new
    ///   version* that landed during a 2.8 s session spawn wrote
    ///   `ResponseCanceled{user_stop}` on a turn nobody stopped. That cancel is
    ///   in `TURN_ENDED_EVENT_TYPES_SQL`, so the next boot classified the branch
    ///   `idle`, declined the auto-resume, and withdrew the "Paused by restart"
    ///   promise the transcript had already made.
    ///
    /// See `docs/plans/2026-08-06-a-session-that-registers-mid-teardown-is-shutting-down.md`.
    pub(crate) fn session_is_shutting_down(&self, per_session: bool) -> bool {
        per_session || self.is_shutting_down()
    }

    /// [`Self::session_is_shutting_down`] for a caller that does not already
    /// hold the session's flag: looks the session up and reads it. A thread
    /// with no entry answers on the durable half alone.
    ///
    /// **Both flags are monotonic** (neither is ever cleared: the per-session
    /// one is only ever set `true`, and `mark_shutting_down` says the global one
    /// stays set because the process is on its way out). So this only ever goes
    /// false → true, and asking again LATER can only be more true. A caller with
    /// two decisions to make should therefore ask again at the second one rather
    /// than reuse the first answer, whenever the later decision is the
    /// destructive one.
    pub(crate) async fn thread_session_is_shutting_down(&self, thread_id: Uuid) -> bool {
        let per_session = {
            let guard = self.agent_sessions.lock().await;
            guard
                .get(&thread_id)
                .is_some_and(|s| s.shutting_down.load(std::sync::atomic::Ordering::Relaxed))
        };
        self.session_is_shutting_down(per_session)
    }

    /// Stamp `actor` on `meta` when an aborted-by-the-host terminal fires and no
    /// actor has been set already. Lets the AbortPanel attribute the abort:
    /// 'System' for a process-killed one, the device for a user-initiated
    /// *Switch to new version*, and neither for engine-deliberate work like a
    /// hardening retrigger, which sets its own actor upstream.
    ///
    /// The actor is a PARAMETER rather than a hardcoded `System` because the two
    /// callers are not asking the same question, and the difference is
    /// load-bearing. `completion.rs`'s safety net really is the host giving up
    /// on a hung session, so it passes `System`. `emit_stop_terminal`'s abort arm
    /// is the engine teardown, whose actor is
    /// [`LucidosEngine::teardown_actor`](crate::engine::LucidosEngine::teardown_actor):
    /// a device there is half the switch fingerprint
    /// (`agent_recovery::SWITCH_TEARDOWN_ABORT_SQL`) and is what buys the session
    /// its `paused` verdict and its auto-resume. Hardcoding `System` here cost
    /// exactly that to any session registered after
    /// `shutdown_agent_sessions` took its flag pass.
    pub(super) fn stamp_host_actor_if_aborted(
        meta: &mut crate::engine::thread_events::EventMeta,
        is_aborted: bool,
        actor: crate::engine::thread_events::MessageOrigin,
    ) {
        if is_aborted && meta.actor.is_none() {
            meta.actor = Some(actor);
        }
    }

    /// Build the terminal event for a CC turn from its classified kind.
    pub(super) fn make_terminal_event(
        kind: TerminalKind,
        text: String,
        model: Option<String>,
        reasoning_effort: Option<String>,
    ) -> crate::engine::thread_events::ThreadEvent {
        match kind {
            TerminalKind::Generated => {
                crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                    text,
                    images: vec![],
                    model,
                    reasoning_effort,
                }
            }
            TerminalKind::Canceled(cause) => {
                crate::engine::thread_events::ThreadEvent::ResponseCanceled {
                    text,
                    images: vec![],
                    model,
                    reasoning_effort,
                    cause,
                }
            }
            TerminalKind::Aborted(cause) => {
                crate::engine::thread_events::ThreadEvent::ResponseAborted {
                    text,
                    images: vec![],
                    model,
                    reasoning_effort,
                    cause,
                }
            }
            // ResponseFailed has no text/model fields — the partial response is
            // already in the timeline as CodingAgentTextStreamed events. The
            // error string is what the frontend renders next to the red dot.
            TerminalKind::Failed { error } => {
                crate::engine::thread_events::ThreadEvent::ResponseFailed { error }
            }
        }
    }

    /// Run the stop-arm body shared by both `stop.notified()` and
    /// `chat_cancel.cancelled()` in `run_session`. Kills the Claude Code subprocess,
    /// flushes any unpersisted text, then emits the terminal event chosen by
    /// `stop_terminal_kind` (deduping against the restart pre-emit when
    /// emitting Aborted, stamping `actor: System` when appropriate). `arm`
    /// is a stable label ("stop" / "chat_cancel") used in log lines and the
    /// dedup site so traces stay distinguishable across the two select arms.
    ///
    /// `suppress_user_terminal` is set by the caller when the stop signal
    /// originated from Apply / Discard / Archive — those paths emit their
    /// own lifecycle event (`ChangeApplied` / `ChangeDiscarded` /
    /// `ThreadArchived`) and must NOT also emit `ResponseCanceled`.
    ///
    /// At engine teardown the whole body is skipped for a session parked on an
    /// unanswered `AskUserQuestion` (see
    /// [`crate::engine::agent_recovery::preserve_question_park_at_shutdown`]):
    /// such a session must land NOTHING after its `UserQuestionAsked`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn emit_stop_terminal(
        &self,
        arm: &'static str,
        thread_id: Uuid,
        is_waiting: bool,
        is_shutdown: bool,
        suppress_user_terminal: bool,
        interrupt_is_redirect: bool,
        agent_cancel: &tokio_util::sync::CancellationToken,
        claude_text_buf: &str,
        last_text_persisted_len: usize,
        meta: &crate::engine::thread_events::EventMeta,
        external_terminal_emitted: &std::sync::atomic::AtomicBool,
        normalized_model: &Option<String>,
        cc_reasoning_effort: &Option<String>,
        coding_agent: crate::runtime::CodingAgent,
    ) {
        // Resolve "is this session shutting down" ONCE, and use that one value
        // for every decision below. Two reads of the same question inside this
        // function is exactly what produced the 2026-08-06 switch report: the
        // preserve guard asked the widened question and `stop_terminal_kind` got
        // the bare per-session flag, so a restart that landed mid-spawn wrote
        // `ResponseCanceled{user_stop}` on a turn nobody stopped. The parameter
        // arrives as the per-session snapshot; from here on `is_shutdown` is the
        // real answer.
        let is_shutdown = self.session_is_shutting_down(is_shutdown);
        if crate::engine::agent_recovery::preserve_question_park_at_shutdown(
            &self.pool,
            arm,
            thread_id,
            is_shutdown,
        )
        .await
        {
            agent_cancel.cancel();
            return;
        }
        Self::kill_cc_and_flush(
            agent_cancel,
            claude_text_buf,
            last_text_persisted_len,
            &self.event_bus,
            thread_id,
            meta,
            coding_agent,
        )
        .await;
        let Some(mut kind) = stop_terminal_kind(is_shutdown, is_waiting, suppress_user_terminal)
        else {
            crate::log!(
                "[AgentSession] {} arm: session {} stopped without a terminal event \
                 (idle, user-action, or idle shutdown)",
                arm,
                thread_id
            );
            return;
        };
        // Refine a real-Cancel into a redirect cancel when this stop came from a
        // Codex mid-turn follow-up redirect that escalated (CC ignored the
        // graceful interrupt) — neutral render instead of "Canceled ✕".
        if interrupt_is_redirect {
            if let TerminalKind::Canceled(cause) = &mut kind {
                *cause = crate::engine::thread_events::CancelCause::SupersededByFollowup;
            }
        }
        let is_aborted = matches!(kind, TerminalKind::Aborted(_));
        // Dedup BOTH Aborted and Canceled — eviction-path pre-emits set the
        // flag with actor=System, so a follow-up Canceled here would mask the
        // engine-initiated abort as a user cancel.
        if external_terminal_already_emitted(
            &self.pool,
            external_terminal_emitted,
            thread_id,
            meta.request_event_id,
            is_shutdown,
            arm,
        )
        .await
        {
            return;
        }
        let terminal_event = Self::make_terminal_event(
            kind,
            claude_text_buf.to_string(),
            normalized_model.clone(),
            cc_reasoning_effort.clone(),
        );
        // `is_aborted` here means exactly `Aborted(EngineShutdown)`:
        // `stop_terminal_kind` yields an abort only when `is_shutdown`. So the
        // actor is the TEARDOWN's, the same one the pre-emit stamped, which is
        // what lets a session registered after `shutdown_agent_sessions` took
        // its flag pass still read "Paused by restart" and still auto-resume. No
        // stashed actor (a bare stop.sh, an external SIGUSR1) falls back to
        // System, and the AbortPanel reads System. User-driven Canceled
        // inherits the existing meta.
        let mut emit_meta = meta.clone();
        Self::stamp_host_actor_if_aborted(
            &mut emit_meta,
            is_aborted,
            self.teardown_actor()
                .unwrap_or_else(crate::engine::thread_events::MessageOrigin::system),
        );
        let log_label = format!("[AgentSession] terminal event ({})", arm);
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: terminal_event,
                    meta: emit_meta,
                },
                &log_label,
            )
            .await;
    }

    /// Emit a `CodingAgentPromptSent` event for automated coding-agent sessions (hardening,
    /// merge conflict, recovery) and return the event ID for use as `origin_id`.
    /// This ensures all events emitted by the coding-agent session have a valid
    /// `request_event_id` pointing to a real persisted event.
    ///
    /// `origin` flows onto the event so engine-internal callers (orphan recovery,
    /// hardening retrigger) can stamp themselves with `MessageOrigin::Engine`.
    /// Pass `None` when the surrounding flow already carries the origin (e.g.
    /// merge-conflict prompts emitted as part of an apply chain that has its
    /// own actor stamping).
    pub(crate) async fn emit_automated_prompt(
        &self,
        thread_id: Uuid,
        prompt: &str,
        origin: Option<MessageOrigin>,
    ) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let coding_agent = self.thread_coding_agent(thread_id).await;
        let result = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::CodingAgentPromptSent {
                    text: prompt.to_string(),
                    coding_agent,
                    origin,
                },
                meta: crate::engine::thread_events::EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    ..crate::engine::thread_events::EventMeta::NONE
                },
            })
            .await?;
        Ok(result.expect("CodingAgentPromptSent is persisted").event_id)
    }

    /// Spawn a CC task with a panic guard that emits `ResponseFailed` + `SessionEnded`
    /// if the task panics. Without this, a panic in a spawned CC task would leave
    /// the thread stuck in "running" state forever with no frontend notification.
    pub(crate) fn spawn_cc_task_guarded(
        engine: std::sync::Arc<LucidosEngine>,
        thread_id: Uuid,
        future: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let handle = tokio::spawn(future);
        // Fire-and-forget: the watcher cleans up on panic; nobody awaits it.
        // Dropping the JoinHandle detaches the already-spawned watcher (it keeps running).
        drop(Self::monitor_cc_task(engine, thread_id, handle));
    }

    /// Monitor a spawned CC task's JoinHandle. If it panics, emit `ResponseFailed`
    /// and `SessionEnded` and clean up the session entry. Shared by
    /// `spawn_cc_task_guarded`, the chat HTTP handler, and the Thread Queue's
    /// coding-agent execution path. Returns the watcher's own handle —
    /// resolves once the monitored task AND any panic cleanup finish, so a
    /// caller can await full completion (the queue executor does, to hold
    /// the capacity slot for the session's duration).
    pub(crate) fn monitor_cc_task(
        engine: std::sync::Arc<LucidosEngine>,
        thread_id: Uuid,
        handle: tokio::task::JoinHandle<()>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(join_err) = handle.await {
                if join_err.is_panic() {
                    let panic_msg = match join_err.into_panic().downcast::<String>() {
                        Ok(s) => *s,
                        Err(e) => match e.downcast::<&str>() {
                            Ok(s) => s.to_string(),
                            Err(_) => "unknown panic".to_string(),
                        },
                    };
                    crate::log!(
                        "[AgentSession] CC task panicked for thread {}: {}",
                        thread_id,
                        panic_msg
                    );
                    engine
                        .event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                    error: format!("Internal error: {}", panic_msg),
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[AgentSession] ResponseFailed after panic",
                        )
                        .await;
                    engine
                        .event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::SessionEnded {
                                    reason: SessionEndReason::Panic,
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[AgentSession] SessionEnded after panic",
                        )
                        .await;
                    engine.agent_sessions.lock().await.remove(&thread_id);
                }
            }
        })
    }
}

/// True iff an engine-internal path already emitted the boundary
/// `ResponseAborted` for this turn, so `run_session`'s terminal arms (Result
/// classify, cancel, chat_cancel, safety net) skip their own emit and the user
/// sees one panel instead of two.
///
/// Two arms, because the flag alone cannot answer for every session:
///
/// * **The in-memory flag**, set by `abort_in_flight_for_restart`
///   (`/api/v1/restart`) and `emit_stuck_thread_eviction_abort`
///   (`register_thread_queued`'s 60 s timeout). It covers a boundary emitted
///   while this session was already in `agent_sessions`.
/// * **The events table**, consulted only during an effective shutdown. It
///   covers the opposite order: a boundary emitted BEFORE the session existed.
///   Both restart emitters iterate a snapshot of `agent_sessions`, so a session
///   still spawning is invisible to them and has no flag of its own to set.
///   That is the 2026-08-06 switch report: a session that registered one second
///   after the teardown boundary stacked a second terminator next to it, and
///   that extra event then cost the thread its auto-resume.
///
/// **The DB arm needs BOTH halves of "covers this turn", and neither alone.**
/// It goes through [`crate::engine::agent_recovery::boundary_abort_covers_turn`]:
/// a `ResponseAborted` that out-sequences the thread's newest start event AND
/// carries this turn's `request_event_id`.
///
/// * The **anchor alone** is what `thread_events::has_terminator_for` (the gate
///   `emit_response_canceled` uses) would give, and it is wrong here: a
///   coding-agent session keeps ONE `request_event_id` across follow-up turns,
///   so it would read the first turn's terminator as covering the second.
///   Observed shape, thread 950f5ebb: `SessionStarted`, `MessageReceived`,
///   `ResponseGenerated`, `MessageReceived`, `ResponseGenerated`, two genuine
///   terminators under one anchor.
/// * The **window alone** is turn-exact only for turns that carry a start
///   event, and two ordinary shapes carry none (a parent woken by
///   `ChildThreadCompleted`, and an `answered_after_idle` continuation, which
///   withholds its `ContinuationStarted` by design). For those a previous
///   turn's abort sits inside the window forever, so the window alone would
///   read a stale boundary as covering a live turn and leave it with no
///   terminator at all.
///
/// A turn with no anchor cannot prove either, so it emits.
///
/// Gated on `is_shutdown` so the normal path pays no query: outside a teardown
/// the flag is the whole answer, exactly as before. A free function taking the
/// pool, so its tests drive it against a seeded pool rather than needing a
/// whole `LucidosEngine`.
pub(super) async fn external_terminal_already_emitted(
    pool: &sqlx::PgPool,
    flag: &std::sync::atomic::AtomicBool,
    thread_id: Uuid,
    request_event_id: Option<Uuid>,
    is_shutdown: bool,
    site: &'static str,
) -> bool {
    if flag.load(std::sync::atomic::Ordering::Acquire) {
        crate::log!(
            "[AgentSession] Skipping terminal emit ({}) for thread {}: external pre-emit already landed",
            site,
            thread_id
        );
        return true;
    }
    if !is_shutdown {
        return false;
    }
    let Some(anchor) = request_event_id else {
        return false;
    };
    if crate::engine::agent_recovery::boundary_abort_covers_turn(pool, thread_id, anchor).await {
        crate::log!(
            "[AgentSession] Skipping terminal emit ({}) for thread {}: this session registered \
             after the teardown boundary, which already covers turn {}",
            site,
            thread_id,
            anchor
        );
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `stamp_host_actor_if_aborted` stamps the actor its caller supplies (NOT
    /// `Engine{OrphanRecovery}`) on aborted terminals. The safety-net caller
    /// supplies `System`: the host killed the process, and the engine just emits
    /// the marker. Engine actor stays for engine-deliberate work like hardening
    /// retrigger or scheduler.
    #[test]
    fn stamp_host_actor_stamps_the_supplied_actor_when_aborted_and_no_actor() {
        use crate::engine::thread_events::{EventMeta, MessageOrigin};
        let mut meta = EventMeta::NONE;
        LucidosEngine::stamp_host_actor_if_aborted(&mut meta, true, MessageOrigin::system());
        assert!(matches!(meta.actor, Some(MessageOrigin::System)));
    }

    /// The teardown caller supplies the device that clicked *Switch to new
    /// version*, and that must reach the event: a `Device` actor on an
    /// `EngineShutdown` abort IS the switch fingerprint
    /// (`agent_recovery::SWITCH_TEARDOWN_ABORT_SQL`), so it is what buys the
    /// session its `paused` verdict and its auto-resume. Hardcoding `System`
    /// here is what cost a chat thread both on 2026-08-07.
    #[test]
    fn stamp_host_actor_stamps_the_teardown_device_when_aborted() {
        use crate::engine::thread_events::{AbortCause, EventMeta, MessageOrigin};
        let device = MessageOrigin::Device {
            device_id: "d-1".into(),
            label: "iOS Safari PWA".into(),
        };
        let mut meta = EventMeta::NONE;
        LucidosEngine::stamp_host_actor_if_aborted(&mut meta, true, device.clone());
        assert_eq!(meta.actor, Some(device));
        assert!(AbortCause::EngineShutdown.promises_auto_resume(meta.actor.as_ref()));
    }

    /// Non-aborted terminals (Generated, Canceled) carry the inbound meta
    /// untouched — Generated is a normal turn end, Canceled is user-driven.
    /// Stamping a host actor on those would mis-attribute the AbortPanel.
    #[test]
    fn stamp_host_actor_no_op_when_not_aborted() {
        use crate::engine::thread_events::{EventMeta, MessageOrigin};
        let mut meta = EventMeta::NONE;
        LucidosEngine::stamp_host_actor_if_aborted(&mut meta, false, MessageOrigin::system());
        assert!(meta.actor.is_none());
    }

    /// If a more specific actor is already set (e.g. device for /api/v1/restart
    /// pre-emit), don't overwrite it. The pre-emit's device attribution must
    /// survive so the AbortPanel reads "Paused by restart", not "System".
    #[test]
    fn stamp_host_actor_does_not_overwrite_existing() {
        use crate::engine::thread_events::{EventMeta, MessageOrigin};
        let device = MessageOrigin::Device {
            device_id: "d-1".into(),
            label: "iOS Safari PWA".into(),
        };
        let mut meta = EventMeta {
            actor: Some(device.clone()),
            ..EventMeta::NONE
        };
        LucidosEngine::stamp_host_actor_if_aborted(&mut meta, true, MessageOrigin::system());
        assert_eq!(meta.actor, Some(device));
    }

    /// Source-scan tripwire: **`is_shutdown` always holds the effective
    /// answer**, never the raw per-session flag.
    ///
    /// This is the shape of the 2026-08-06 switch report, and a behavioural
    /// test cannot reach it: every consumer is buried inside `run_session`,
    /// which needs a whole live `LucidosEngine` plus a subprocess. What went
    /// wrong was purely textual. One function bound `is_shutdown` from the bare
    /// atomic, asked the widened question in one place and the narrow one three
    /// lines later, and the narrow read is the one that chose the terminal
    /// event. `run_session` then used the same NAME for both meanings in the
    /// same file, which is what made the discrepancy invisible on review.
    ///
    /// So the rule is about the name: in the three production files that decide
    /// a terminal, every `let is_shutdown = …` must be produced by
    /// `session_is_shutting_down`. Widening at a site that later re-widens is
    /// harmless (the OR is idempotent) and worth it, because a reader must
    /// never have to trace a call chain to learn which of the two questions a
    /// variable answers.
    ///
    /// Scoped to those three deliberately. Test files bind the name to a plain
    /// literal to drive the pure truth tables (`cancel_lifecycle_tests.rs`), and
    /// `run_session/entry_guard.rs` reads the per-session flag on purpose (see
    /// its own doc); neither is a terminal decision.
    #[test]
    fn is_shutdown_is_always_the_effective_answer() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("engine")
            .join("agent_session");
        let files = [
            dir.join("runtime_helpers.rs"),
            dir.join("run_session").join("run.rs"),
            dir.join("run_session").join("completion.rs"),
        ];
        let mut violations: Vec<String> = Vec::new();
        let mut checked = 0usize;
        for path in &files {
            let content = std::fs::read_to_string(path).expect("read source");
            let lines: Vec<&str> = content.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if !line.trim_start().starts_with("let is_shutdown =") {
                    continue;
                }
                checked += 1;
                // The binding's own statement, which may wrap over several
                // lines or open a block. Three lines is enough for every
                // current shape and keeps the scan from swallowing the next
                // statement.
                let window = lines[i..(i + 4).min(lines.len())].join(" ");
                if !window.contains("session_is_shutting_down") {
                    violations.push(format!(
                        "{}:{}: {}",
                        path.file_name().unwrap().to_string_lossy(),
                        i + 1,
                        line.trim()
                    ));
                }
            }
        }
        assert!(
            checked >= 6,
            "the scan found only {checked} `is_shutdown` bindings, so it has \
             stopped matching the source it is supposed to guard"
        );
        assert!(
            violations.is_empty(),
            "`is_shutdown` must be the effective answer \
             (`session_is_shutting_down`), not the raw per-session flag. A bare \
             read here classifies an engine restart as a user Stop for any \
             session that registered after the teardown snapshot:\n{}",
            violations.join("\n")
        );
    }

    // -- external_terminal_already_emitted ---------------------------------
    //
    // The DB arm is the half added for the 2026-08-06 switch report, where a
    // session registered AFTER the teardown boundary and so could never have
    // been handed the in-memory flag. Each test states one leg of the
    // predicate; between them they pin that the suppression is scoped to the
    // TURN, not to the session and not to the request id.
    mod suppression {
        use crate::engine::event_bus::{BusEvent, EventBus};
        use crate::engine::thread_events::{
            AbortCause, ActorMode, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
        };
        use crate::test_support::{setup_test_db, teardown_test_db};
        use std::sync::atomic::AtomicBool;
        use uuid::Uuid;

        fn cc_meta() -> EventMeta {
            EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            }
        }

        /// A user message: a start event, so it retires every abort before it.
        /// Returns its own id, which is the anchor the turn it opens will carry.
        async fn seed_start(bus: &EventBus, thread_id: Uuid) -> Uuid {
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::MessageReceived {
                    voice_session_id: None,
                    text: "so go?".into(),
                    user_image_hashes: vec![],
                    device_id: None,
                    device: None,
                    image_description: None,
                    parent_thread_id: None,
                    spawning_event_id: None,
                    mode: ActorMode::Human,
                    model: None,
                    reasoning_effort: None,
                    origin: None,
                },
                meta: cc_meta(),
            })
            .await
            .expect("emit succeeds")
            .expect("event persisted")
            .event_id
        }

        /// The switch teardown boundary the restart pre-emit lands, anchored on
        /// the turn it is tearing down (what `in_flight_request_event_id`
        /// resolves for the real emit).
        async fn seed_teardown_boundary(bus: &EventBus, thread_id: Uuid, anchor: Uuid) {
            crate::engine::thread_events::emit_response_aborted(
                bus,
                thread_id,
                AbortCause::EngineShutdown,
                String::new(),
                vec![],
                None,
                None,
                EventMeta {
                    request_event_id: Some(anchor),
                    actor: Some(MessageOrigin::Device {
                        device_id: "d-1".into(),
                        label: "My MacBook".into(),
                    }),
                    ..cc_meta()
                },
                "[test] teardown boundary",
            )
            .await;
        }

        /// Distinct sequence ordering, so "newer than the latest start" has a
        /// well-defined answer.
        async fn tick() {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }

        /// The reported shape. No flag was ever set (the session did not exist
        /// when the boundary was emitted), yet the turn is already covered, so
        /// this session must NOT stack a second terminator on it.
        #[tokio::test]
        async fn a_boundary_that_predates_the_session_still_suppresses_its_terminal() {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread_id = Uuid::new_v4();

            let anchor = seed_start(&bus, thread_id).await;
            tick().await;
            seed_teardown_boundary(&bus, thread_id, anchor).await;

            let never_set = AtomicBool::new(false);
            assert!(
                super::super::external_terminal_already_emitted(
                    &pool,
                    &never_set,
                    thread_id,
                    Some(anchor),
                    true,
                    "test",
                )
                .await,
                "the teardown boundary covers this turn, so a session that \
                 registered after it must adopt that boundary rather than emit \
                 its own"
            );

            pool.close().await;
            teardown_test_db(&db_name).await;
        }

        /// The gate that keeps the normal path free: outside a shutdown the DB
        /// is never consulted, so an ordinary turn pays no query and an old
        /// abort cannot suppress a live turn's terminal.
        #[tokio::test]
        async fn outside_a_shutdown_only_the_flag_answers() {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread_id = Uuid::new_v4();

            let anchor = seed_start(&bus, thread_id).await;
            tick().await;
            seed_teardown_boundary(&bus, thread_id, anchor).await;

            let never_set = AtomicBool::new(false);
            assert!(
                !super::super::external_terminal_already_emitted(
                    &pool,
                    &never_set,
                    thread_id,
                    Some(anchor),
                    false,
                    "test",
                )
                .await,
                "with no shutdown in progress the flag is the whole answer, \
                 exactly as before this arm existed"
            );

            let flagged = AtomicBool::new(true);
            assert!(
                super::super::external_terminal_already_emitted(
                    &pool,
                    &flagged,
                    thread_id,
                    Some(anchor),
                    false,
                    "test",
                )
                .await,
                "the in-memory flag still answers on its own, shutdown or not"
            );

            pool.close().await;
            teardown_test_db(&db_name).await;
        }

        /// The counter-case that rules out an anchor-ONLY gate. A live
        /// coding-agent session keeps ONE anchor across follow-up turns, so the
        /// window half is load-bearing: once a new start event out-sequences the
        /// boundary, the turn it opened is owed its own terminal even though the
        /// anchor is unchanged.
        ///
        /// Observed live, thread 950f5ebb / request 603fa6b2:
        /// `SessionStarted`, `MessageReceived`, `ResponseGenerated`,
        /// `MessageReceived`, `ResponseGenerated`. An anchor-only gate would
        /// have swallowed the second `ResponseGenerated` and stuck the thread at
        /// `running` forever.
        #[tokio::test]
        async fn a_new_start_event_re_arms_the_terminal_even_during_shutdown() {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread_id = Uuid::new_v4();

            let anchor = seed_start(&bus, thread_id).await;
            tick().await;
            seed_teardown_boundary(&bus, thread_id, anchor).await;
            tick().await;
            // The next turn opens. The boundary above now belongs to the
            // previous one and says nothing about this one. The session keeps
            // its original anchor, which is exactly why the anchor alone cannot
            // decide this.
            seed_start(&bus, thread_id).await;

            let never_set = AtomicBool::new(false);
            assert!(
                !super::super::external_terminal_already_emitted(
                    &pool,
                    &never_set,
                    thread_id,
                    Some(anchor),
                    true,
                    "test",
                )
                .await,
                "a turn opened after the boundary is owed its own terminator: \
                 suppressing it would strand the thread mid-turn"
            );

            pool.close().await;
            teardown_test_db(&db_name).await;
        }

        /// The counter-case that rules out a WINDOW-only gate, and the reason
        /// the anchor half exists.
        ///
        /// Two ordinary turn shapes carry none of `THREAD_START_EVENTS_SQL`: a
        /// parent woken by `ChildThreadCompleted`, and an `answered_after_idle`
        /// continuation, whose `ContinuationStarted` is deliberately withheld by
        /// `continue_should_open_resume_exchange`. For those, a previous turn's
        /// abort never leaves the window, so a window-only gate reads a stale
        /// boundary as covering the live turn and the turn gets NO terminator at
        /// all: the same loss this whole change exists to prevent, in a new
        /// disguise.
        #[tokio::test]
        async fn a_stale_boundary_from_a_previous_turn_does_not_suppress_this_one() {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread_id = Uuid::new_v4();

            // Turn 1, ended by an abort. Nothing after it is a start event.
            let old_anchor = seed_start(&bus, thread_id).await;
            tick().await;
            seed_teardown_boundary(&bus, thread_id, old_anchor).await;
            tick().await;

            // Turn 2 opens with no start event of its own, the way an
            // `answered_after_idle` resume does. Its anchor is the
            // `CodingAgentPromptSent` the continuation emitted.
            let new_anchor = bus
                .emit(BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::CodingAgentPromptSent {
                        text: "continue".into(),
                        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                        origin: None,
                    },
                    meta: cc_meta(),
                })
                .await
                .expect("emit succeeds")
                .expect("event persisted")
                .event_id;

            assert!(
                crate::engine::agent_recovery::boundary_abort_already_emitted(&pool, thread_id)
                    .await,
                "precondition: the window-only predicate still counts the stale \
                 abort, because no start event ever superseded it"
            );

            let never_set = AtomicBool::new(false);
            assert!(
                !super::super::external_terminal_already_emitted(
                    &pool,
                    &never_set,
                    thread_id,
                    Some(new_anchor),
                    true,
                    "test",
                )
                .await,
                "the stale boundary names the PREVIOUS turn, so this turn must \
                 still emit its own terminator"
            );

            pool.close().await;
            teardown_test_db(&db_name).await;
        }

        /// A turn with no anchor cannot prove a boundary covers it, so it emits.
        /// The safe direction: a duplicate panel is cosmetic, a missing
        /// terminator strands the turn.
        #[tokio::test]
        async fn a_turn_with_no_anchor_still_emits_its_terminal() {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread_id = Uuid::new_v4();

            let anchor = seed_start(&bus, thread_id).await;
            tick().await;
            seed_teardown_boundary(&bus, thread_id, anchor).await;

            let never_set = AtomicBool::new(false);
            assert!(
                !super::super::external_terminal_already_emitted(
                    &pool, &never_set, thread_id, None, true, "test",
                )
                .await,
                "with no anchor there is nothing to match the boundary against"
            );

            pool.close().await;
            teardown_test_db(&db_name).await;
        }
    }

    #[test]
    fn terminal_event_matches_kind() {
        use crate::engine::thread_events::{AbortCause, CancelCause};
        let aborted = LucidosEngine::make_terminal_event(
            TerminalKind::Aborted(AbortCause::EngineShutdown),
            "partial output".into(),
            Some("claude-opus-4-6".into()),
            Some("high".into()),
        );
        match &aborted {
            crate::engine::thread_events::ThreadEvent::ResponseAborted {
                model,
                reasoning_effort,
                cause,
                ..
            } => {
                assert_eq!(model.as_deref(), Some("claude-opus-4-6"));
                assert_eq!(reasoning_effort.as_deref(), Some("high"));
                assert_eq!(*cause, AbortCause::EngineShutdown);
            }
            _ => panic!("Expected ResponseAborted"),
        }
        assert!(matches!(
            LucidosEngine::make_terminal_event(
                TerminalKind::Canceled(CancelCause::UserStop),
                "x".into(),
                None,
                None
            ),
            crate::engine::thread_events::ThreadEvent::ResponseCanceled { .. }
        ));
        assert!(matches!(
            LucidosEngine::make_terminal_event(TerminalKind::Generated, "x".into(), None, None),
            crate::engine::thread_events::ThreadEvent::ResponseGenerated { .. }
        ));
    }

    #[tokio::test]
    async fn test_startup_semaphore_limits_concurrency() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let sem = Arc::new(tokio::sync::Semaphore::new(2));
        let active = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..6 {
            let sem = sem.clone();
            let active = active.clone();
            let max_seen = max_seen.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_seen.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert!(
            max_seen.load(Ordering::SeqCst) <= 2,
            "Semaphore should limit to 2 concurrent startups"
        );
    }
}
