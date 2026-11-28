use super::lifecycle::{stop_terminal_kind, TerminalKind};
use super::text_buffer::CodingAgentTextBuffer;
use crate::engine::agent_session::run_session::completion::CompletionExt;
use crate::engine::chat::process::ChatProcessExt;
use crate::engine::thread_events::{EngineReason, EventChannel, MessageOrigin};
use crate::engine::LucidosEngine;
use uuid::Uuid;

#[easy_ext::ext(RuntimeHelpersExt)]
pub(crate) impl LucidosEngine {
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

    fn session_is_shutting_down(&self, per_session: bool) -> bool {
        per_session || self.is_shutting_down()
    }

    /// INVARIANT: [`Self::session_is_shutting_down`] for a caller that does not already
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

    #[test]
    fn the_panic_cleanup_reaps_the_panicked_runs_entry() {
        let thread_id = Uuid::new_v4();
        let (session, rx) = crate::engine::AgentSession::for_test();
        drop(rx);
        let mut sessions = std::collections::HashMap::from([(thread_id, session)]);

        reap_dead_session_entry(&mut sessions, thread_id);
        assert!(sessions.is_empty());
    }

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

    #[test]
    fn stamp_host_actor_stamps_the_supplied_actor_when_aborted_and_no_actor() {
        use crate::engine::thread_events::{EventMeta, MessageOrigin};
        let mut meta = EventMeta::NONE;
        LucidosEngine::stamp_host_actor_if_aborted(&mut meta, true, MessageOrigin::system());
        assert!(matches!(meta.actor, Some(MessageOrigin::System)));
    }

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

    #[test]
    fn stamp_host_actor_no_op_when_not_aborted() {
        use crate::engine::thread_events::{EventMeta, MessageOrigin};
        let mut meta = EventMeta::NONE;
        LucidosEngine::stamp_host_actor_if_aborted(&mut meta, false, MessageOrigin::system());
        assert!(meta.actor.is_none());
    }

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

        async fn tick() {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }

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

        #[tokio::test]
        async fn a_new_start_event_re_arms_the_terminal_even_during_shutdown() {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread_id = Uuid::new_v4();

            let anchor = seed_start(&bus, thread_id).await;
            tick().await;
            seed_teardown_boundary(&bus, thread_id, anchor).await;
            tick().await;
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

        #[tokio::test]
        async fn a_stale_boundary_from_a_previous_turn_does_not_suppress_this_one() {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread_id = Uuid::new_v4();

            let old_anchor = seed_start(&bus, thread_id).await;
            tick().await;
            seed_teardown_boundary(&bus, thread_id, old_anchor).await;
            tick().await;

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
