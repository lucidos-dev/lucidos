use super::lifecycle::{stop_terminal_kind, TerminalKind};
use super::text_buffer::CodingAgentTextBuffer;
use crate::engine::agent_session::run_session::completion::CompletionExt;
use crate::engine::chat::process::ChatProcessExt;
use crate::engine::thread_events::{EngineReason, EventChannel, MessageOrigin};
use crate::engine::LucidosEngine;
use uuid::Uuid;

#[easy_ext::ext(RuntimeHelpersExt)]
pub(crate) impl LucidosEngine {
    /// Signal the agent runtime to terminate, then flush any un-persisted text
    /// as a final stream event. The runtime task watches `cancel` and kills its
    /// child; we don't own the process here.
    async fn kill_cc_and_flush(
        cancel: &tokio_util::sync::CancellationToken,
        claude_text_buf: &mut CodingAgentTextBuffer,
        event_bus: &crate::engine::event_bus::EventBus,
        thread_id: Uuid,
        meta: &crate::engine::thread_events::EventMeta,
        coding_agent: crate::runtime::CodingAgent,
    ) {
        cancel.cancel();
        claude_text_buf
            .flush(
                event_bus,
                thread_id,
                coding_agent,
                meta,
                "[AgentSession] CodingAgentTextStreamed flush on cancel",
            )
            .await;
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
    fn session_is_shutting_down(&self, per_session: bool) -> bool {
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
    async fn thread_session_is_shutting_down(&self, thread_id: Uuid) -> bool {
        let per_session = {
            let guard = self.agent_sessions.lock().await;
            guard
                .get(&thread_id)
                .is_some_and(|s| s.shutting_down.load(std::sync::atomic::Ordering::Relaxed))
        };
        self.session_is_shutting_down(per_session)
    }

    fn stamp_host_actor_if_aborted(
        meta: &mut crate::engine::thread_events::EventMeta,
        is_aborted: bool,
        actor: crate::engine::thread_events::MessageOrigin,
    ) {
        if is_aborted && meta.actor.is_none() {
            meta.actor = Some(actor);
        }
    }

    /// Build the terminal event for a CC turn from its classified kind.
    fn make_terminal_event(
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
                crate::engine::thread_events::ThreadEvent::ResponseFailed {
                    spend_cap: crate::llm::spend_cap::SpendCapNotice::from_agent_text(
                        "Coding agent",
                        &error,
                    ),
                    error,
                    agent_exit: None,
                    provider_error: None,
                }
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
    /// At engine teardown, the whole body is skipped for a session parked
    /// on an unanswered `AskUserQuestion` (see
    /// [`crate::engine::question_park_sql::preserve_question_park_at_shutdown`]):
    /// such a session must land NOTHING after its `UserQuestionAsked`.
    #[allow(clippy::too_many_arguments)]
    async fn emit_stop_terminal(
        &self,
        arm: &'static str,
        thread_id: Uuid,
        is_waiting: bool,
        is_shutdown: bool,
        suppress_user_terminal: bool,
        redirect_cause: Option<crate::engine::thread_events::CancelCause>,
        agent_cancel: &tokio_util::sync::CancellationToken,
        claude_text_buf: &mut CodingAgentTextBuffer,
        meta: &crate::engine::thread_events::EventMeta,
        external_terminal_emitted: &std::sync::atomic::AtomicBool,
        coding_agent: crate::runtime::CodingAgent,
    ) {
        let is_shutdown = self.session_is_shutting_down(is_shutdown);
        if crate::engine::question_park_sql::preserve_question_park_at_shutdown(
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
            &self.event_bus,
            thread_id,
            meta,
            coding_agent,
        )
        .await;
        let Some(mut kind) = stop_terminal_kind(is_shutdown, is_waiting, suppress_user_terminal)
        else {
            crate::log_legacy!(
                "[AgentSession] {} arm: session {} stopped without a terminal event \
                 (idle, user-action, or idle shutdown)",
                arm,
                thread_id
            );
            return;
        };
        // Refine a real-Cancel into a redirect cancel when this stop came from a
        // redirect that escalated (the agent ignored the graceful interrupt).
        // It renders neutrally, not as "Canceled ✕".
        if let (Some(redirect), TerminalKind::Canceled(cause)) = (redirect_cause, &mut kind) {
            *cause = redirect;
        }
        let is_aborted = matches!(kind, TerminalKind::Aborted(_));
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
        let (model, reasoning_effort) = self
            .terminal_model_settings(thread_id, external_terminal_emitted)
            .await;
        let terminal_event = Self::make_terminal_event(
            kind,
            claude_text_buf.as_str().to_string(),
            model,
            reasoning_effort,
        );
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
    /// The engine writes every automated prompt, so `reason` is required and
    /// the event always carries `MessageOrigin::Engine`. An origin-less prompt
    /// that opens a thread renders Origin "Unknown" in the route popover.
    async fn emit_automated_prompt(
        &self,
        thread_id: Uuid,
        prompt: &str,
        reason: EngineReason,
    ) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let coding_agent = self.thread_coding_agent(thread_id).await;
        let result = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::CodingAgentPromptSent {
                    input_event_id: None,
                    text: prompt.to_string(),
                    coding_agent,
                    origin: Some(MessageOrigin::engine(reason)),
                },
                meta: crate::engine::thread_events::EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    ..crate::engine::thread_events::EventMeta::NONE
                },
            })
            .await?;
        Ok(result.expect("CodingAgentPromptSent is persisted").event_id)
    }

    /// Send `prompt` to the coding-agent thread `thread_id` as an engine turn,
    /// through the router like any message. The first turn opens the thread,
    /// named `title`, in the inbox. A lane thread and a fix thread open this
    /// way, so they keep the ordinary steps, questions and recovery.
    /// `on_failure` hears the error when the turn fails.
    fn spawn_engine_coding_turn_or_else(
        self: &std::sync::Arc<Self>,
        thread_id: Uuid,
        prompt: String,
        title: Option<String>,
        reason: EngineReason,
        on_failure: impl FnOnce(&str) + Send + 'static,
    ) {
        self.spawn_engine_coding_turn_with(
            thread_id,
            prompt,
            title,
            MessageOrigin::engine(reason),
            None,
            on_failure,
        );
    }

    /// [`Self::spawn_engine_coding_turn_or_else`] on a named `implementer`,
    /// not the workspace's default, sent as `origin`'s message.
    fn spawn_engine_coding_turn_as(
        self: &std::sync::Arc<Self>,
        thread_id: Uuid,
        prompt: String,
        title: Option<String>,
        origin: MessageOrigin,
        implementer: crate::engine::successor::Implementer,
        on_failure: impl FnOnce(&str) + Send + 'static,
    ) {
        self.spawn_engine_coding_turn_with(
            thread_id,
            prompt,
            title,
            origin,
            Some(implementer),
            on_failure,
        );
    }

    fn spawn_engine_coding_turn_with(
        self: &std::sync::Arc<Self>,
        thread_id: Uuid,
        prompt: String,
        title: Option<String>,
        origin: MessageOrigin,
        implementer: Option<crate::engine::successor::Implementer>,
        on_failure: impl FnOnce(&str) + Send + 'static,
    ) {
        let engine = self.clone();
        Self::spawn_cc_task_guarded(engine.clone(), thread_id, async move {
            let (cc_model, reasoning_effort, coding_agent) = match &implementer {
                Some(implementer) => (
                    implementer.model.as_deref(),
                    implementer.reasoning_effort.as_deref(),
                    Some(implementer.coding_agent),
                ),
                None => (None, None, None),
            };
            let result = engine
                .process_message_with_steps(
                    &prompt,
                    None,
                    None,
                    None,
                    reasoning_effort,
                    None,
                    None,
                    None,
                    Some(true),
                    None,
                    Some(thread_id),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    origin.mode(),
                    cc_model,
                    coding_agent,
                    None,
                    title.as_deref(),
                    Some(origin),
                    None,
                    crate::engine::FollowUpUrgency::Normal,
                    None,
                )
                .await;
            match result {
                Ok(res) if !res.orphaned_injections.is_empty() => {
                    crate::api::chat::process_orphan_chain(
                        engine.clone(),
                        res.thread_id,
                        res.orphaned_injections,
                    )
                    .await;
                }
                Ok(_) => {}
                Err(e) => {
                    crate::log_legacy!("[AgentSession] engine turn on {} failed: {}", thread_id, e);
                    on_failure(&e.to_string());
                    crate::engine::claude_code::emit_background_task_failure(
                        &engine,
                        thread_id,
                        &e,
                        "[AgentSession] engine turn failure",
                    )
                    .await;
                }
            }
        });
    }
}

pub(super) async fn external_terminal_already_emitted(
    pool: &sqlx::PgPool,
    flag: &std::sync::atomic::AtomicBool,
    thread_id: Uuid,
    request_event_id: Option<Uuid>,
    is_shutdown: bool,
    site: &'static str,
) -> bool {
    if flag.load(std::sync::atomic::Ordering::Acquire) {
        crate::log_legacy!(
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
        crate::log_legacy!(
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
    use crate::engine::coding_task::reap_dead_session_entry;

    /// The panicked run dropped its receiver, so its entry reads dead and goes.
    #[test]
    fn the_panic_cleanup_reaps_the_panicked_runs_entry() {
        let thread_id = Uuid::new_v4();
        let (session, rx) = crate::engine::AgentSession::for_test();
        drop(rx);
        let mut sessions = std::collections::HashMap::from([(thread_id, session)]);

        reap_dead_session_entry(&mut sessions, thread_id);
        assert!(sessions.is_empty());
    }

    /// A replacement registered before the panic surfaced is live and stays.
    #[test]
    fn the_panic_cleanup_leaves_a_live_replacement_alone() {
        let thread_id = Uuid::new_v4();
        let (replacement, _rx) = crate::engine::AgentSession::for_test();
        let mut sessions = std::collections::HashMap::from([(thread_id, replacement)]);

        reap_dead_session_entry(&mut sessions, thread_id);
        assert!(
            sessions.contains_key(&thread_id),
            "removing a live replacement hides its subprocess from every watchdog and Stop"
        );
    }

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
    /// (`crate::engine::switch_recovery_sql::SWITCH_TEARDOWN_ABORT_SQL`), so it is what buys the
    /// session its `paused` verdict and its auto-resume. Hardcoding `System`
    /// here is what cost a chat thread both on 2026-08-07.
    #[test]
    fn stamp_host_actor_stamps_the_teardown_device_when_aborted() {
        use crate::engine::thread_events::{AbortCause, EventMeta, MessageOrigin};
        let device = MessageOrigin::Device {
            device_id: "d-1".into(),
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
        };
        let mut meta = EventMeta {
            actor: Some(device.clone()),
            ..EventMeta::NONE
        };
        LucidosEngine::stamp_host_actor_if_aborted(&mut meta, true, MessageOrigin::system());
        assert_eq!(meta.actor, Some(device));
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
                    provider: None,
                    voice_session_id: None,
                    wake_mode: None,
                    text: "so go?".into(),
                    user_image_hashes: vec![],
                    device_id: None,
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
                        input_event_id: None,
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

    mod stop_terminal {
        use super::super::{CodingAgentTextBuffer, RuntimeHelpersExt};
        use crate::engine::thread_events::{EventMeta, MessageOrigin};
        use crate::engine::LucidosEngine;
        use crate::test_support::{setup_test_db, teardown_test_db};
        use std::sync::atomic::AtomicBool;
        use uuid::Uuid;

        async fn stop(
            engine: &LucidosEngine,
            thread_id: Uuid,
            is_shutdown: bool,
            flag: &AtomicBool,
        ) {
            let cancel = tokio_util::sync::CancellationToken::new();
            let mut buffer = CodingAgentTextBuffer::default();
            engine
                .emit_stop_terminal(
                    "stop",
                    thread_id,
                    false,
                    is_shutdown,
                    false,
                    None,
                    &cancel,
                    &mut buffer,
                    &EventMeta::NONE,
                    flag,
                    crate::runtime::CodingAgent::ClaudeCode,
                )
                .await;
        }

        async fn count(engine: &LucidosEngine, thread_id: Uuid, predicate: &str) -> i64 {
            sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND {predicate}"
            ))
            .bind(thread_id)
            .fetch_one(engine.pool())
            .await
            .unwrap()
        }

        /// A session registered after the teardown flag pass carries a false
        /// per-session flag. It must still abort, stamped with the device.
        #[tokio::test]
        async fn a_late_session_aborts_with_the_teardown_device_as_actor() {
            if !crate::test_support::in_engine_child(
                "engine::agent_session::runtime_helpers::tests::stop_terminal::a_late_session_aborts_with_the_teardown_device_as_actor",
            )
            .await
            {
                return;
            }
            let (_pool, db) = setup_test_db().await;
            let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
            let device = MessageOrigin::Device {
                device_id: "d-1".into(),
            };
            assert!(engine.stash_restart_actor(Some(device)));
            engine.begin_teardown();
            let thread_id = Uuid::new_v4();

            stop(&engine, thread_id, false, &AtomicBool::new(false)).await;

            assert_eq!(
                count(
                    &engine,
                    thread_id,
                    crate::engine::switch_recovery_sql::SWITCH_TEARDOWN_ABORT_SQL
                )
                .await,
                1,
                "one device-attributed EngineShutdown abort"
            );
            assert_eq!(
                count(&engine, thread_id, "event_type = 'ResponseCanceled'").await,
                0
            );
            engine.pool().close().await;
            teardown_test_db(&db).await;
        }

        /// An eviction pre-emit already ended the turn as an abort. A user
        /// Stop arriving after it must not add a cancel that masks the abort.
        #[tokio::test]
        async fn a_user_stop_after_an_external_terminal_emits_nothing() {
            if !crate::test_support::in_engine_child(
                "engine::agent_session::runtime_helpers::tests::stop_terminal::a_user_stop_after_an_external_terminal_emits_nothing",
            )
            .await
            {
                return;
            }
            let (_pool, db) = setup_test_db().await;
            let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
            let thread_id = Uuid::new_v4();

            stop(&engine, thread_id, false, &AtomicBool::new(true)).await;

            assert_eq!(
                count(
                    &engine,
                    thread_id,
                    "event_type IN ('ResponseCanceled', 'ResponseAborted')"
                )
                .await,
                0
            );
            engine.pool().close().await;
            teardown_test_db(&db).await;
        }
    }

    mod guard_tests {
        /// Source-scan tripwire: **`is_shutdown` always holds the effective
        /// answer**, never the raw per-session flag.
        ///
        /// A behavioural test cannot reach this shape of a past switch report:
        /// every consumer sits inside `run_session`, which needs a live engine and
        /// a subprocess. One function bound `is_shutdown` from the bare atomic. It
        /// asked the widened question once and the narrow one three lines later,
        /// and the narrow read chose the terminal event.
        ///
        /// So the rule is about the name. In the three production files that
        /// decide a terminal, every `let is_shutdown = …` comes from
        /// `session_is_shutting_down`. Widening twice is harmless, since the OR is
        /// idempotent, and a reader never traces a call chain to learn its meaning.
        ///
        /// Scoped to those three deliberately. Test files bind the name to a plain
        /// literal for the truth tables (`cancel_lifecycle_tests.rs`), and
        /// `run_session/entry_guard.rs` reads the per-session flag on purpose.
        /// Neither is a terminal decision.
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

        /// Source-scan tripwire: **a terminal event reports the model and effort
        /// the session entry holds.** Only the entry knows the *effort in force*,
        /// so no terminal may be built from the loop's
        /// spawn-time `normalized_model` and `cc_reasoning_effort` locals.
        #[test]
        fn terminal_events_never_take_the_spawn_time_model() {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join("engine")
                .join("agent_session");
            let files = [
                dir.join("runtime_helpers.rs"),
                dir.join("run_session").join("run.rs"),
                dir.join("run_session").join("completion.rs"),
            ];
            let builders = [
                "make_terminal_event(",
                "emit_stop_terminal(",
                "emit_response_aborted(",
            ];
            let mut violations: Vec<String> = Vec::new();
            let mut checked = 0usize;
            for path in &files {
                let content = std::fs::read_to_string(path).expect("read source");
                let production = content.split("#[cfg(test)]").next().unwrap_or(&content);
                for builder in builders {
                    for (at, _) in production.match_indices(builder) {
                        if production[..at].ends_with("fn ") {
                            continue;
                        }
                        // The call runs to the end of its statement.
                        let call = &production[at..];
                        let call = &call[..call.find(';').unwrap_or(call.len())];
                        checked += 1;
                        if call.contains("normalized_model") || call.contains("cc_reasoning_effort")
                        {
                            violations.push(format!(
                                "{}: {}",
                                path.file_name().unwrap().to_string_lossy(),
                                call.lines().next().unwrap_or_default().trim()
                            ));
                        }
                    }
                }
            }
            assert!(
                checked >= 5,
                "the scan found only {checked} terminal builder calls, so it has \
             stopped matching the source it is supposed to guard"
            );
            assert!(
                violations.is_empty(),
                "a terminal event must report the session's current model, not the \
             spawn-time locals:\n{}",
                violations.join("\n")
            );
        }
    }
}
