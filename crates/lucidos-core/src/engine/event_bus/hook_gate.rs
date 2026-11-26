use std::sync::{Arc, OnceLock, RwLock};

use serde_json::Value;
use uuid::Uuid;

use super::{BusEvent, EmitResult, EventBus, SystemEvent, ThreadAppendEffect};
use crate::core::event_hooks::{
    armed_hooks, ArmedHook, EventHookAsker, HookAsk, HookVerdict, PendingEvent, Refusal,
    RefusalCause, ASKED_EVENT_TYPES, FIRST_MESSAGE_EVENT_TYPE, FIRST_MESSAGE_KEY,
};
use crate::core::event_subscription::validate_emittable_event_type;
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::trigger_context::current_trigger_id;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Default)]
pub(super) struct HookGate {
    armed: RwLock<ArmedCache>,
    asker: OnceLock<Arc<dyn EventHookAsker>>,
}

#[derive(Default)]
struct ArmedCache {
    generation: u64,
    hooks: Option<Arc<Vec<ArmedHook>>>,
}

impl HookGate {
    async fn armed(&self, pool: &sqlx::PgPool) -> Result<Arc<Vec<ArmedHook>>, BoxError> {
        let generation = {
            let cache = self.armed.read().unwrap_or_else(|e| e.into_inner());
            if let Some(hooks) = &cache.hooks {
                return Ok(Arc::clone(hooks));
            }
            cache.generation
        };
        let hooks = Arc::new(armed_hooks(pool).await?);
        let mut cache = self.armed.write().unwrap_or_else(|e| e.into_inner());
        if cache.generation == generation {
            cache.hooks = Some(Arc::clone(&hooks));
        }
        Ok(hooks)
    }

    pub(super) fn forget_armed_hooks(&self) {
        let mut cache = self.armed.write().unwrap_or_else(|e| e.into_inner());
        cache.generation += 1;
        cache.hooks = None;
    }
}

#[derive(Debug)]
#[must_use = "an Asked token is spent by EventBus::emit_asked"]
pub struct Asked {
    event_type: &'static str,
    thread_id: Uuid,
    returned: Vec<SystemEvent>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookRefusal {
    pub slug: Option<String>,
    pub cause: RefusalCause,
    pub reason: String,
}

impl std::fmt::Display for HookRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.slug {
            Some(slug) => write!(f, "The event hook '{slug}' stopped this: {}", self.reason),
            None => write!(f, "The event hooks stopped this: {}", self.reason),
        }
    }
}

impl std::error::Error for HookRefusal {}

fn actor_of(event: &ThreadEvent, meta: &EventMeta) -> Option<MessageOrigin> {
    meta.actor.clone().or_else(|| match event {
        ThreadEvent::MessageReceived { origin, .. }
        | ThreadEvent::TriggerStarted { origin, .. }
        | ThreadEvent::ChangeProposed { origin, .. } => origin.clone(),
        _ => None,
    })
}

impl EventBus {
    pub async fn install_event_hook_asker(
        &self,
        asker: Arc<dyn EventHookAsker>,
    ) -> Result<(), BoxError> {
        if self.hook_gate.asker.set(asker).is_err() {
            return Err("an event hook asker is already installed".into());
        }
        self.hook_gate.armed(&self.pool).await.map(drop)
    }

    pub async fn hooks_armed_on(&self, event_type: &str) -> Result<bool, BoxError> {
        let armed = self.hook_gate.armed(&self.pool).await?;
        Ok(armed.iter().any(|hook| hook.event_type == event_type))
    }

    pub async fn ask_hooks(
        &self,
        thread_id: Uuid,
        event: &ThreadEvent,
        meta: &EventMeta,
    ) -> Result<Asked, HookRefusal> {
        let event_type = event.event_type();
        let mut asked = Asked {
            event_type,
            thread_id,
            returned: Vec::new(),
        };
        let actor = actor_of(event, meta);
        if !ASKED_EVENT_TYPES.contains(&event_type)
            || matches!(actor, Some(MessageOrigin::EventHook { .. }))
        {
            return Ok(asked);
        }
        let refuse = |slug: Option<&str>, cause, reason: String| Refused {
            slug: slug.map(str::to_string),
            refusal: Refusal {
                cause,
                reason,
                detail: None,
            },
        };
        let armed = match self.hook_gate.armed(&self.pool).await {
            Ok(armed) => armed,
            Err(e) => {
                let refused = refuse(
                    None,
                    RefusalCause::Unavailable,
                    format!("the armed event hooks could not be read: {e}"),
                );
                return Err(self.record(refused, event_type, thread_id, actor).await);
            }
        };
        let hooks: Vec<&ArmedHook> = armed
            .iter()
            .filter(|hook| hook.event_type == event_type)
            .collect();
        let Some(first) = hooks.first() else {
            return Ok(asked);
        };
        let mut pending = PendingEvent::of_thread(thread_id, event, meta);
        if event_type == FIRST_MESSAGE_EVENT_TYPE {
            match self.thread_has_started(thread_id).await {
                Ok(true) => return Ok(asked),
                Ok(false) => {
                    if let Value::Object(payload) = &mut pending.payload {
                        payload.insert(FIRST_MESSAGE_KEY.to_string(), Value::Bool(true));
                    }
                }
                Err(e) => {
                    let refused = refuse(
                        Some(&first.slug),
                        RefusalCause::Unavailable,
                        format!(
                            "whether this is the thread's first message could not be read: {e}"
                        ),
                    );
                    return Err(self.record(refused, event_type, thread_id, actor).await);
                }
            }
        }
        let Some(asker) = self.hook_gate.asker.get() else {
            let refused = refuse(
                Some(&first.slug),
                RefusalCause::Unavailable,
                "no event hook runner is installed".to_string(),
            );
            return Err(self.record(refused, event_type, thread_id, actor).await);
        };
        let chain_depth = super::thread_emit_depth(thread_id);
        for hook in hooks {
            let ask = HookAsk {
                hook,
                pending: &pending,
                chain_depth,
            };
            let events = match asker.ask(ask).await {
                HookVerdict::Skipped => continue,
                HookVerdict::Allowed(events) => events,
                HookVerdict::Refused(refusal) => {
                    let refused = Refused {
                        slug: Some(hook.slug.clone()),
                        refusal,
                    };
                    return Err(self.record(refused, event_type, thread_id, actor).await);
                }
            };
            for returned in events {
                if let Err(e) = validate_emittable_event_type(&returned.event_type) {
                    let refused = refuse(
                        Some(&hook.slug),
                        RefusalCause::InvalidAnswer,
                        format!(
                            "The hook '{}' gave no answer: a hook returns domain events only: {e}",
                            hook.slug
                        ),
                    );
                    return Err(self.record(refused, event_type, thread_id, actor).await);
                }
                asked.returned.push(SystemEvent::DomainEvent {
                    event_type: returned.event_type,
                    payload: Value::Object(returned.payload),
                    depth: chain_depth,
                    transient: false,
                    actor: Some(MessageOrigin::EventHook {
                        slug: hook.slug.clone(),
                    }),
                });
            }
        }
        Ok(asked)
    }

    pub async fn emit_asked(
        &self,
        asked: Asked,
        thread_id: Uuid,
        event: ThreadEvent,
        meta: EventMeta,
    ) -> Result<Option<EmitResult>, BoxError> {
        let emitting_trigger_id = current_trigger_id();
        self.emit_asked_as_trigger(asked, thread_id, event, meta, emitting_trigger_id)
            .await
    }

    pub async fn emit_asked_as_trigger(
        &self,
        asked: Asked,
        thread_id: Uuid,
        event: ThreadEvent,
        meta: EventMeta,
        emitting_trigger_id: Option<String>,
    ) -> Result<Option<EmitResult>, BoxError> {
        if asked.event_type != event.event_type() || asked.thread_id != thread_id {
            return Err(format!(
                "an Asked token for {} on {} cannot record {} on {thread_id}",
                asked.event_type,
                asked.thread_id,
                event.event_type()
            )
            .into());
        }
        let effect = (!asked.returned.is_empty())
            .then_some(ThreadAppendEffect::HookReturned(asked.returned));
        let appended = self
            .append_thread_events_with_question(
                thread_id,
                vec![(event, meta)],
                super::thread_emit_depth(thread_id),
                emitting_trigger_id,
                None,
                effect,
            )
            .await?;
        Ok(appended.and_then(|results| results.into_iter().next()))
    }

    async fn thread_has_started(&self, thread_id: Uuid) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM events WHERE thread_id = $1 \
             AND event_type IN ('MessageReceived', 'TriggerStarted'))",
        )
        .bind(thread_id)
        .fetch_one(&self.pool)
        .await
    }

    async fn record(
        &self,
        refused: Refused,
        event_type: &str,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> HookRefusal {
        let Refused { slug, refusal } = refused;
        if let Some(slug) = &slug {
            let event = SystemEvent::EventHookRefused {
                slug: slug.clone(),
                event_type: event_type.to_string(),
                thread_id: Some(thread_id),
                cause: refusal.cause,
                reason: refusal.reason.clone(),
                detail: refusal.detail,
                actor,
            };
            if let Err(e) = self.emit(BusEvent::System(event)).await {
                crate::log_legacy!("[EventHooks] EventHookRefused for '{slug}' not recorded: {e}");
            }
        } else {
            crate::log_legacy!(
                "[EventHooks] {event_type} on {thread_id} refused: {}",
                refusal.reason
            );
        }
        HookRefusal {
            slug,
            cause: refusal.cause,
            reason: refusal.reason,
        }
    }
}

struct Refused {
    slug: Option<String>,
    refusal: Refusal,
}

#[cfg(test)]
#[path = "hook_gate_tests.rs"]
mod tests;
