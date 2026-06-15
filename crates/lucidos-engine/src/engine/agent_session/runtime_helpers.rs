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
        let stored: Option<String> = sqlx::query_scalar(
            "SELECT coding_agent FROM thread_summaries WHERE thread_id = $1",
        )
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
                            event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed {
                                text: delta.to_string(),
                                coding_agent,
                            },
                            meta: meta.clone(),
                        },
                        "[ClaudeCode] CodingAgentTextStreamed flush on cancel",
                    )
                    .await;
            }
        }
    }

    /// Stamp `actor: System` on `meta` when an aborted-by-host-system terminal
    /// fires (safety-net, shutdown cancel) and no actor has been set already.
    /// Lets the AbortPanel render '⚙ System' for process-killed aborts —
    /// distinct from engine-deliberate work like hardening retrigger.
    pub(super) fn stamp_system_actor_if_aborted(
        meta: &mut crate::engine::thread_events::EventMeta,
        is_aborted: bool,
    ) {
        if is_aborted && meta.actor.is_none() {
            meta.actor = Some(crate::engine::thread_events::MessageOrigin::system());
        }
    }

    /// True iff an engine-internal path already pre-emitted the boundary
    /// `ResponseAborted` for this session — `run_session`'s terminal arms
    /// (Result classify, cancel, chat_cancel, safety net) skip their own emit
    /// when set, so the user sees one panel instead of two. Set by
    /// `abort_in_flight_for_restart` (`/api/v1/restart`) and
    /// `emit_stuck_thread_eviction_abort` (`register_thread_queued` 60s
    /// timeout).
    pub(super) fn external_terminal_already_emitted(
        flag: &std::sync::atomic::AtomicBool,
        thread_id: Uuid,
        site: &'static str,
    ) -> bool {
        let skip = flag.load(std::sync::atomic::Ordering::Acquire);
        if skip {
            crate::log!(
                "[ClaudeCode] Skipping terminal emit ({}) for thread {} — external pre-emit already landed",
                site,
                thread_id
            );
        }
        skip
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn emit_stop_terminal(
        &self,
        arm: &'static str,
        thread_id: Uuid,
        is_waiting: bool,
        is_shutdown: bool,
        suppress_user_terminal: bool,
        agent_cancel: &tokio_util::sync::CancellationToken,
        claude_text_buf: &str,
        last_text_persisted_len: usize,
        meta: &crate::engine::thread_events::EventMeta,
        external_terminal_emitted: &std::sync::atomic::AtomicBool,
        normalized_model: &Option<String>,
        cc_reasoning_effort: &Option<String>,
        coding_agent: crate::runtime::CodingAgent,
    ) {
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
        let Some(kind) = stop_terminal_kind(is_shutdown, is_waiting, suppress_user_terminal)
        else {
            crate::log!(
                "[ClaudeCode] {} arm: session {} stopped without a terminal event \
                 (idle, user-action, or idle shutdown)",
                arm,
                thread_id
            );
            return;
        };
        let is_aborted = matches!(kind, TerminalKind::Aborted(_));
        // Dedup BOTH Aborted and Canceled — eviction-path pre-emits set the
        // flag with actor=System, so a follow-up Canceled here would mask the
        // engine-initiated abort as a user cancel.
        if Self::external_terminal_already_emitted(external_terminal_emitted, thread_id, arm) {
            return;
        }
        let terminal_event = Self::make_terminal_event(
            kind,
            claude_text_buf.to_string(),
            normalized_model.clone(),
            cc_reasoning_effort.clone(),
        );
        // Aborted-during-shutdown means the host system killed the process;
        // stamp `actor: System` so the AbortPanel reads ⚙ System. User-driven
        // Canceled inherits the existing meta.
        let mut emit_meta = meta.clone();
        Self::stamp_system_actor_if_aborted(&mut emit_meta, is_aborted);
        let log_label = format!("[ClaudeCode] terminal event ({})", arm);
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

    /// Emit a `CodingAgentPromptSent` event for automated Claude Code sessions (hardening,
    /// merge conflict, recovery) and return the event ID for use as `origin_id`.
    /// This ensures all events emitted by the Claude Code session have a valid
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
                        "[ClaudeCode] CC task panicked for thread {}: {}",
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
                            "[ClaudeCode] ResponseFailed after panic",
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
                            "[ClaudeCode] SessionEnded after panic",
                        )
                        .await;
                    engine.agent_sessions.lock().await.remove(&thread_id);
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `stamp_system_actor_if_aborted` stamps `MessageOrigin::System` (NOT
    /// `Engine{OrphanRecovery}`) on aborted terminals — the host system killed
    /// the process; the engine just emits the marker. Engine actor stays for
    /// engine-deliberate work like hardening retrigger or scheduler.
    #[test]
    fn stamp_system_actor_stamps_system_when_aborted_and_no_actor() {
        use crate::engine::thread_events::{EventMeta, MessageOrigin};
        let mut meta = EventMeta::NONE;
        LucidosEngine::stamp_system_actor_if_aborted(&mut meta, true);
        assert!(matches!(meta.actor, Some(MessageOrigin::System)));
    }

    /// Non-aborted terminals (Generated, Canceled) carry the inbound meta
    /// untouched — Generated is a normal turn end, Canceled is user-driven.
    /// Stamping system on those would mis-attribute the AbortPanel.
    #[test]
    fn stamp_system_actor_no_op_when_not_aborted() {
        use crate::engine::thread_events::EventMeta;
        let mut meta = EventMeta::NONE;
        LucidosEngine::stamp_system_actor_if_aborted(&mut meta, false);
        assert!(meta.actor.is_none());
    }

    /// If a more specific actor is already set (e.g. device for /api/v1/restart
    /// pre-emit), don't overwrite it. The pre-emit's device attribution must
    /// survive so the AbortPanel reads "You — Restarted" not "System".
    #[test]
    fn stamp_system_actor_does_not_overwrite_existing() {
        use crate::engine::thread_events::{EventMeta, MessageOrigin};
        let device = MessageOrigin::Device {
            device_id: "d-1".into(),
            label: "iOS Safari PWA".into(),
        };
        let mut meta = EventMeta {
            actor: Some(device.clone()),
            ..EventMeta::NONE
        };
        LucidosEngine::stamp_system_actor_if_aborted(&mut meta, true);
        assert_eq!(meta.actor, Some(device));
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
