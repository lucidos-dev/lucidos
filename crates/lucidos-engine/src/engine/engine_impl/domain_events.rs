//! Domain-event emission and trigger-execution recording.
//!
//! Part of the `LucidosEngine` inherent impl, split from engine_impl.rs.

use super::super::*;

impl LucidosEngine {
    /// Persist a domain event to the events table and broadcast it on the EventBus.
    /// Used by the LLM `emit_event` tool and the HTTP API for non-transient emits.
    /// `actor` is the originating `MessageOrigin` (HTTP handler resolves it via
    /// `user_actor_resolved`); engine-internal callers (LLM tool, scheduler)
    /// pass `None`.
    pub async fn emit_domain_event(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> Result<uuid::Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let result = self
            .emit_domain_event_inner(event_type, payload, false, actor)
            .await?;
        Ok(result
            .expect("non-transient DomainEvent always returns EmitResult")
            .event_id)
    }

    /// Broadcast a domain event on SSE without writing it to the events table.
    /// Used for high-churn coordination signals (heartbeats, presenter↔remote
    /// state) where the audit trail isn't valuable.
    pub async fn broadcast_transient_domain_event(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.emit_domain_event_inner(event_type, payload, true, actor)
            .await?;
        Ok(())
    }

    async fn emit_domain_event_inner(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        transient: bool,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> Result<
        Option<crate::engine::event_bus::EmitResult>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let depth = crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH
            .try_with(|d| *d)
            .unwrap_or(0);
        self.event_bus
            .emit(crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::DomainEvent {
                    event_type: event_type.to_string(),
                    payload,
                    depth,
                    transient,
                    actor,
                },
            ))
            .await
    }

    /// Update a trigger's `last_run` in memory and persist a `TriggerExecuted`
    /// event via EventBus. Used by both cron and event-based trigger execution.
    pub async fn record_trigger_executed(&self, trigger_id: &str) {
        let now = chrono::Utc::now();
        {
            let mut configs = self.trigger_configs.write().unwrap();
            if let Some(c) = configs.get_mut(trigger_id) {
                c.last_run = Some(now);
            }
        }
        if let Err(e) = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::TriggerExecuted {
                    trigger_id: trigger_id.to_string(),
                    payload: serde_json::json!({
                        "trigger_id": trigger_id,
                        "last_run": now.to_rfc3339(),
                    }),
                },
            ))
            .await
        {
            log!("[Triggers] Failed to persist TriggerExecuted event: {}", e);
        }
    }

}
