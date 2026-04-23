use super::lifecycle::TerminalKind;
use crate::engine::thread_events::{EventChannel, MessageOrigin, SessionEndReason};
use crate::engine::CognosEngine;
use uuid::Uuid;

impl CognosEngine {
    pub(crate) fn clear_cc_debounce(&self, thread_id: Uuid) {
        if let Ok(mut spawns) = self.last_cc_spawn.lock() {
            spawns.remove(&thread_id);
        }
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
                                agent: crate::runtime::AgentKind::ClaudeCode,
                            },
                            meta: meta.clone(),
                        },
                        "[ClaudeCode] CodingAgentTextStreamed flush on cancel",
                    )
                    .await;
            }
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
            TerminalKind::Canceled => {
                crate::engine::thread_events::ThreadEvent::ResponseCanceled {
                    text,
                    images: vec![],
                    model,
                    reasoning_effort,
                }
            }
            TerminalKind::Aborted => crate::engine::thread_events::ThreadEvent::ResponseAborted {
                text,
                images: vec![],
                model,
                reasoning_effort,
            },
        }
    }

    /// Emit a `CodingAgentPromptSent` event for automated CC sessions (hardening,
    /// merge conflict, recovery) and return the event ID for use as `origin_id`.
    /// This ensures all events emitted by the CC session have a valid
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
        let result = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::CodingAgentPromptSent {
                    text: prompt.to_string(),
                    agent: crate::runtime::AgentKind::ClaudeCode,
                    origin,
                },
                meta: crate::engine::thread_events::EventMeta {
                    channel: Some(EventChannel::CodingAgent),
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
        engine: std::sync::Arc<CognosEngine>,
        thread_id: Uuid,
        future: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let handle = tokio::spawn(future);
        Self::monitor_cc_task(engine, thread_id, handle);
    }

    /// Monitor a spawned CC task's JoinHandle. If it panics, emit `ResponseFailed`
    /// and `SessionEnded` and clean up the session entry. Shared by both
    /// `spawn_cc_task_guarded` and `spawn_agent_thread`.
    pub(crate) fn monitor_cc_task(
        engine: std::sync::Arc<CognosEngine>,
        thread_id: Uuid,
        handle: tokio::task::JoinHandle<()>,
    ) {
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
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_event_matches_kind() {
        let aborted = CognosEngine::make_terminal_event(
            TerminalKind::Aborted,
            "partial output".into(),
            Some("claude-opus-4-6".into()),
            Some("high".into()),
        );
        match &aborted {
            crate::engine::thread_events::ThreadEvent::ResponseAborted {
                model,
                reasoning_effort,
                ..
            } => {
                assert_eq!(model.as_deref(), Some("claude-opus-4-6"));
                assert_eq!(reasoning_effort.as_deref(), Some("high"));
            }
            _ => panic!("Expected ResponseAborted"),
        }
        assert!(matches!(
            CognosEngine::make_terminal_event(TerminalKind::Canceled, "x".into(), None, None),
            crate::engine::thread_events::ThreadEvent::ResponseCanceled { .. }
        ));
        assert!(matches!(
            CognosEngine::make_terminal_event(TerminalKind::Generated, "x".into(), None, None),
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
