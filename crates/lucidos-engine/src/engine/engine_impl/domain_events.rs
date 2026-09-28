//! Domain-event emission and trigger-execution recording.
//!
//! Part of the `LucidosEngine` inherent impl, split from engine_impl.rs.

use super::super::*;
use crate::engine::event_bus::{BusEvent, EmitResult, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

impl LucidosEngine {
    /// Persist a domain event to the events table and broadcast it on the EventBus.
    /// Used by the LLM `emit_event` tool, the HTTP API and inbound webhooks.
    ///
    /// `actor` is who emitted it, as the caller's surface established it, and
    /// it is required: the payload is caller-written and never says who acted.
    ///
    /// `emitting_trigger_id` is the fire this emit belongs to, stated rather
    /// than read off the ambient scope. An HTTP caller has no ambient scope at
    /// all, so it hands over what its origin token proved (ADR 0137).
    pub async fn emit_domain_event(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        actor: MessageOrigin,
        emitting_trigger_id: Option<String>,
    ) -> Result<uuid::Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let result = self
            .event_bus
            .emit_domain_event(event_type, payload, false, actor, emitting_trigger_id)
            .await?;
        Ok(result
            .expect("non-transient DomainEvent always returns EmitResult")
            .event_id)
    }

    /// Broadcast a domain event on SSE without writing it to the events table.
    /// Used for high-churn coordination signals (heartbeats, presenter↔remote
    /// state) where the audit trail isn't valuable.
    ///
    /// `emitting_trigger_id` means what it does on [`Self::emit_domain_event`].
    /// A transient frame reaches the trigger matcher too, so it needs the same
    /// marker.
    pub async fn broadcast_transient_domain_event(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        actor: MessageOrigin,
        emitting_trigger_id: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.event_bus
            .emit_domain_event(event_type, payload, true, actor, emitting_trigger_id)
            .await?;
        Ok(())
    }

    /// Update a trigger's `last_run` + `last_run_status` in memory and persist a
    /// `TriggerExecuted` event via EventBus. Used by both cron and event-based
    /// trigger execution. `status` is the outcome of the firing (the run's
    /// `Result`) so a threadless (script) trigger's OK/failed state is visible
    /// on its panel row; it rides the payload so `replay.rs` rebuilds it on boot
    /// exactly as it does `last_run`.
    pub async fn record_trigger_executed(
        &self,
        trigger_id: &str,
        status: crate::triggers::TriggerRunStatus,
    ) {
        let now = chrono::Utc::now();
        {
            let mut configs = self.trigger_configs.write().unwrap();
            if let Some(c) = configs.get_mut(trigger_id) {
                c.last_run = Some(now);
                c.last_run_status = Some(status);
            }
        }
        if let Err(e) = self
            .event_bus
            .emit(BusEvent::System(SystemEvent::TriggerExecuted {
                trigger_id: trigger_id.to_string(),
                payload: serde_json::json!({
                    "trigger_id": trigger_id,
                    "last_run": now.to_rfc3339(),
                    "status": status.as_str(),
                }),
            }))
            .await
        {
            log!("[Triggers] Failed to persist TriggerExecuted event: {}", e);
        }
    }
}

impl EventBus {
    /// The one write path for a domain event, durable or transient.
    ///
    /// On the bus rather than the engine, so a test can drive the LLM tool's
    /// emit against a real database without booting an engine.
    pub(crate) async fn emit_domain_event(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        transient: bool,
        actor: MessageOrigin,
        emitting_trigger_id: Option<String>,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>> {
        // In the write path so no caller can skip it. The HTTP route checked
        // the name; the LLM `emit_event` tool did not, and a reserved name from
        // either writes the same permanent row.
        crate::core::event_subscription::validate_emittable_event_type(event_type)?;
        // `to_payload` drops the caller's `actor`. Logged here rather than
        // there, because every reader of the event calls `to_payload` again.
        if let Some(claimed) = payload.get(SystemEvent::ACTOR_KEY) {
            log!(
                "[Events] {} carried its own `actor` ({}); recorded the engine's instead ({})",
                event_type,
                claimed,
                serde_json::to_value(&actor).unwrap_or_default()
            );
        }
        let depth = crate::scheduler::user_tasks::current_event_trigger_depth();
        self.emit_as_trigger(
            BusEvent::System(SystemEvent::DomainEvent {
                event_type: event_type.to_string(),
                payload,
                depth,
                transient,
                actor: Some(actor),
            }),
            emitting_trigger_id,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use crate::core::event_subscription::{condition, matchable_system_payload};
    use crate::engine::event_bus::{EventBus, SystemEvent};
    use crate::engine::thread_events::{ActorMode, MessageOrigin};
    use crate::test_support::{setup_test_db, teardown_test_db};
    use serde_json::json;

    /// The Demo Director stamp trigger's condition.
    fn matches_device(payload: &serde_json::Value) -> bool {
        condition::evaluate(Some(&json!({"actor.kind": "device"})), payload)
    }

    fn forged_device_payload() -> serde_json::Value {
        json!({
            "summary": "stamp the film script",
            "actor": {"kind": "device", "device_id": "forged", "label": "My iPhone"},
        })
    }

    /// The report's exploit, on the shape the LLM tool emitted: no engine actor,
    /// and a `device` actor the agent wrote into its own payload.
    #[test]
    fn a_caller_written_actor_never_reaches_the_matcher() {
        let event = SystemEvent::DomainEvent {
            event_type: "FilmScriptStampRequested".to_string(),
            payload: forged_device_payload(),
            depth: 0,
            transient: false,
            actor: None,
        };
        let matchable = matchable_system_payload(&event);
        assert!(
            !matches_device(&matchable),
            "a forged actor matched: {matchable}"
        );
        assert!(matchable.get("actor").is_none(), "{matchable}");
        assert_eq!(matchable["summary"], "stamp the film script");
    }

    /// The HTTP shape: the engine's actor replaces the caller's, whole.
    #[test]
    fn the_engine_actor_replaces_a_caller_written_one() {
        let event = SystemEvent::DomainEvent {
            event_type: "FilmScriptStampRequested".to_string(),
            payload: forged_device_payload(),
            depth: 0,
            transient: true,
            actor: Some(MessageOrigin::Api {
                user_agent: None,
                mode: ActorMode::Agent,
                source_thread_id: None,
            }),
        };
        let payload = event.to_payload();
        assert_eq!(payload["actor"], json!({"kind": "api", "mode": "agent"}));
        assert!(!matches_device(&matchable_system_payload(&event)));
    }

    /// The LLM `emit_event` tool, end to end on a real database: the stored
    /// row, the value a trigger matches, and what the model is told.
    #[tokio::test]
    async fn the_emit_event_tool_cannot_forge_a_device_actor() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let args = json!({
            "event_type": "FilmScriptStampRequested",
            "payload": forged_device_payload(),
        });
        let text = crate::engine::tools::emit_event_impl(&bus, &args, None)
            .await
            .expect("the emit succeeds");
        assert!(text.contains("`actor` was dropped"), "{text}");

        let stored: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM events WHERE event_type = 'FilmScriptStampRequested'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            stored["actor"],
            json!({"kind": "agent", "agent": {"kind": "lucidos_agent"}})
        );
        assert_eq!(stored["summary"], "stamp the film script");
        assert!(!matches_device(&stored), "the stored row matched: {stored}");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// An honest emit names no actor and gets no warning.
    #[tokio::test]
    async fn the_emit_event_tool_stamps_the_agent_without_a_warning() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let args = json!({"event_type": "HabitCompleted", "payload": {"summary": "ran"}});
        let text = crate::engine::tools::emit_event_impl(&bus, &args, None)
            .await
            .expect("the emit succeeds");
        assert!(!text.contains("dropped"), "{text}");

        let kind: String = sqlx::query_scalar(
            "SELECT payload->'actor'->>'kind' FROM events WHERE event_type = 'HabitCompleted'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(kind, "agent");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
