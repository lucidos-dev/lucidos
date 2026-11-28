use uuid::Uuid;

use super::thread_events::{EventChannel, SessionEndReason};
use super::LucidosEngine;

impl LucidosEngine {
    pub fn spawn_cc_task_guarded(
        engine: std::sync::Arc<LucidosEngine>,
        thread_id: Uuid,
        future: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let handle = tokio::spawn(future);
        drop(Self::monitor_cc_task(engine, thread_id, handle));
    }

    /// INVARIANT: Monitor the spawned coding task. On panic, emit `ResponseFailed` and
    /// `SessionEnded`, then clean up its session entry. The returned handle
    /// resolves after the task and any cleanup finish. The Thread Queue awaits
    /// it to hold capacity until completion.
    pub fn monitor_cc_task(
        engine: std::sync::Arc<LucidosEngine>,
        thread_id: Uuid,
        handle: tokio::task::JoinHandle<()>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(join_err) = handle.await {
                if join_err.is_panic() {
                    crate::internal_faults::record_pending(
                        crate::internal_faults::FaultKind::TaskPanic,
                    );
                    let panic_msg = match join_err.into_panic().downcast::<String>() {
                        Ok(s) => *s,
                        Err(e) => match e.downcast::<&str>() {
                            Ok(s) => s.to_string(),
                            Err(_) => "unknown panic".to_string(),
                        },
                    };
                    crate::log_legacy!(
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
                                    spend_cap: None,
                                    agent_exit: None,
                                    provider_error: None,
                                },
                                meta: crate::engine::thread_events::EventMeta {
                                    channel: Some(EventChannel::ClaudeCode),
                                    ..crate::engine::thread_events::EventMeta::NONE
                                },
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
                    reap_dead_session_entry(&mut *engine.agent_sessions.lock().await, thread_id);
                }
            }
        })
    }
}

/// INVARIANT: Remove `thread_id`'s entry only when it reads dead.
///
/// The panic cleanup has no identity token for the run that panicked. That run's
/// future is gone, so its entry reads dead. A replacement registered meanwhile
/// is live, and removing it hides its subprocess from every watchdog and Stop.
pub fn reap_dead_session_entry(
    sessions: &mut std::collections::HashMap<Uuid, crate::engine::AgentSession>,
    thread_id: Uuid,
) {
    if sessions.get(&thread_id).is_some_and(|s| !s.is_live()) {
        sessions.remove(&thread_id);
    }
}
