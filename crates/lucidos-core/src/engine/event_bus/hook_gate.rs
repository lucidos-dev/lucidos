use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;
use uuid::Uuid;

use super::{BusEvent, EmitResult, EventBus, SystemEvent, ThreadAppendEffect};
use crate::core::event_hooks::{
    armed_hooks, hook_creation_key, ArmedHook, EventHookAsker, HookArgs, HookAsk, HookChoice,
    HookVerdict, PendingEvent, Refusal, RefusalCause, ASKED_EVENT_TYPES, FIRST_MESSAGE_EVENT_TYPE,
    FIRST_MESSAGE_KEY, HOOK_CREATION_VALUE_LIMIT,
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
    pub input: Option<Box<HookInputRequest>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HookInputRequest {
    pub hook: String,
    pub argument: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation: Option<HookCreationOffer>,
    pub choices_timeout_secs: u64,
    pub choices: Vec<HookChoice>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HookCreationOffer {
    pub label: String,
    pub key: String,
    pub max_bytes: usize,
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

    pub async fn first_message_hook_choices(
        &self,
        slug: &str,
        argument: &str,
        query: &str,
    ) -> Result<Vec<HookChoice>, String> {
        let armed = self
            .hook_gate
            .armed(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        let hook = armed
            .iter()
            .find(|hook| hook.slug == slug && hook.event_type == FIRST_MESSAGE_EVENT_TYPE)
            .ok_or("This hook is not armed for a first message.")?;
        let asker = self
            .hook_gate
            .asker
            .get()
            .ok_or("No event hook choices runner is installed.")?;
        asker.choices(hook, argument, query).await
    }

    pub async fn observe_change_hooks(
        &self,
        pending: &PendingEvent,
        thread_id: Uuid,
        built_in_refused: bool,
    ) {
        let armed = match self.hook_gate.armed(&self.pool).await {
            Ok(armed) => armed,
            Err(e) => {
                crate::log_legacy!(
                    "[ChangeHookShadow] {} {thread_id}: could not read armed hooks: {e}",
                    pending.event_type
                );
                return;
            }
        };
        for hook in armed
            .iter()
            .filter(|hook| hook.event_type == pending.event_type)
        {
            let Some(asker) = self.hook_gate.asker.get() else {
                crate::log_legacy!(
                    "[ChangeHookShadow] {} {thread_id}: runner unavailable",
                    pending.event_type
                );
                return;
            };
            let outcome = tokio::time::timeout(
                Duration::from_secs(crate::core::event_hooks::TIMEOUT_CEILING_SECS),
                asker.ask(HookAsk {
                    hook,
                    pending,
                    arguments: &HookArgs::new(),
                    chain_depth: 1,
                    can_answer: false,
                }),
            )
            .await;
            let hook_refused = match outcome {
                Ok(HookVerdict::Allowed(events)) if events.is_empty() => Some(false),
                Ok(HookVerdict::Refused(refusal)) if refusal.cause == RefusalCause::Refused => {
                    Some(true)
                }
                Ok(HookVerdict::Skipped) => continue,
                _ => None,
            };
            crate::log_legacy!(
                "[ChangeHookShadow] {} {thread_id} {} {} built_in_refused={built_in_refused} hook_refused={hook_refused:?} matched={}",
                pending.event_type,
                hook.slug,
                hook.content_hash,
                hook_refused == Some(built_in_refused),
            );
        }
    }

    pub async fn ask_hooks(
        &self,
        thread_id: Uuid,
        event: &ThreadEvent,
        meta: &EventMeta,
    ) -> Result<Asked, HookRefusal> {
        self.ask_hooks_with_args(thread_id, event, meta, &HookArgs::new())
            .await
    }

    pub async fn ask_hooks_with_args(
        &self,
        thread_id: Uuid,
        event: &ThreadEvent,
        meta: &EventMeta,
        arguments: &HookArgs,
    ) -> Result<Asked, HookRefusal> {
        self.ask_hooks_for(thread_id, event, meta, arguments, None)
            .await
    }

    pub async fn ask_hooks_for_input(
        &self,
        thread_id: Uuid,
        event: &ThreadEvent,
        meta: &EventMeta,
        arguments: &HookArgs,
        budget: Duration,
    ) -> Result<Asked, HookRefusal> {
        self.ask_hooks_for(
            thread_id,
            event,
            meta,
            arguments,
            Some(Instant::now() + budget),
        )
        .await
    }

    async fn ask_hooks_for(
        &self,
        thread_id: Uuid,
        event: &ThreadEvent,
        meta: &EventMeta,
        arguments: &HookArgs,
        deadline: Option<Instant>,
    ) -> Result<Asked, HookRefusal> {
        let result = self
            .judge_hooks(thread_id, event, meta, arguments, deadline)
            .await;
        if let (
            Err(refusal),
            ThreadEvent::MessageReceived {
                parent_thread_id: Some(parent),
                wake_mode,
                text,
                ..
            },
        ) = (&result, event)
        {
            if refusal.input.is_none() {
                self.report_refused_child(
                    *parent,
                    thread_id,
                    *wake_mode,
                    text,
                    refusal.to_string(),
                )
                .await;
            }
        }
        result
    }

    async fn judge_hooks(
        &self,
        thread_id: Uuid,
        event: &ThreadEvent,
        meta: &EventMeta,
        arguments: &HookArgs,
        deadline: Option<Instant>,
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
            if matches!(
                actor,
                Some(MessageOrigin::Workspace {
                    remote_start: Some(_),
                    ..
                })
            ) {
                let refused = refuse(
                    Some(&first.slug),
                    RefusalCause::Refused,
                    "Start this thread from the receiving workspace to run its event hooks.".into(),
                );
                return Err(self.record(refused, event_type, thread_id, actor).await);
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
                arguments,
                chain_depth,
                can_answer: deadline.is_some(),
            };
            let verdict = match deadline {
                Some(deadline) => match tokio::time::timeout_at(deadline, asker.ask(ask)).await {
                    Ok(verdict) => verdict,
                    Err(_) => {
                        let refused = refuse(
                            Some(&hook.slug),
                            RefusalCause::TimedOut,
                            "The first-message hooks ran out of time to answer the send request."
                                .into(),
                        );
                        return Err(self.record(refused, event_type, thread_id, actor).await);
                    }
                },
                None => asker.ask(ask).await,
            };
            let events = match verdict {
                HookVerdict::Skipped => continue,
                HookVerdict::Allowed(events) => events,
                HookVerdict::NeedsInput {
                    argument,
                    label,
                    create_label,
                    choices_timeout_secs,
                    choices,
                } => {
                    let reason =
                        format!("The hook '{}' needs the argument '{argument}'.", hook.slug);
                    if deadline.is_some() {
                        let creation = create_label.map(|label| HookCreationOffer {
                            label,
                            key: hook_creation_key(&hook.slug, &argument),
                            max_bytes: HOOK_CREATION_VALUE_LIMIT,
                        });
                        return Err(HookRefusal {
                            slug: Some(hook.slug.clone()),
                            cause: RefusalCause::MissingInput,
                            reason,
                            input: Some(Box::new(HookInputRequest {
                                hook: hook.slug.clone(),
                                argument,
                                label,
                                creation,
                                choices_timeout_secs,
                                choices,
                            })),
                        });
                    }
                    let refused = refuse(Some(&hook.slug), RefusalCause::MissingInput, reason);
                    return Err(self.record(refused, event_type, thread_id, actor).await);
                }
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

    pub async fn hold_hook_input(
        &self,
        message_id: Uuid,
        thread_id: Uuid,
        actor: &MessageOrigin,
        slug: &str,
    ) -> Result<(), BoxError> {
        let mut tx = self.pool.begin().await?;
        super::ordinary_submission::lock_ordinary_message(&mut tx, message_id).await?;
        if super::ordinary_submission::input_already_disposed(&mut tx, message_id).await? {
            return Err("This message already has an outcome.".into());
        }
        let result = sqlx::query(
            "INSERT INTO pending_event_hook_inputs (message_event_id, thread_id, actor, hook_slug)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (message_event_id) DO UPDATE SET hook_slug = EXCLUDED.hook_slug
             WHERE pending_event_hook_inputs.actor = EXCLUDED.actor
               AND pending_event_hook_inputs.thread_id = EXCLUDED.thread_id",
        )
        .bind(message_id)
        .bind(thread_id)
        .bind(serde_json::to_value(actor)?)
        .bind(slug)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err("This message belongs to another input.".into());
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn cancel_hook_input(
        &self,
        message_id: Uuid,
        actor: &MessageOrigin,
    ) -> Result<bool, BoxError> {
        let encoded = serde_json::to_value(actor)?;
        let mut append = self.begin_append().await?;
        super::ordinary_submission::lock_ordinary_message(&mut append.tx, message_id).await?;
        let removed: Option<(Uuid, String)> = sqlx::query_as(
            "DELETE FROM pending_event_hook_inputs WHERE message_event_id = $1 AND actor = $2
             RETURNING thread_id, hook_slug",
        )
        .bind(message_id)
        .bind(&encoded)
        .fetch_optional(&mut *append.tx)
        .await?;
        let Some((thread_id, slug)) = removed else {
            return Ok(sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM events WHERE id = $1 AND event_type = 'EventHookRefused'
                 AND payload->'data'->'actor' = $2)",
            )
            .bind(message_id)
            .bind(encoded)
            .fetch_one(&mut *append.tx)
            .await?);
        };
        self.append_system_event(
            append,
            message_id,
            SystemEvent::EventHookRefused {
                slug,
                event_type: FIRST_MESSAGE_EVENT_TYPE.into(),
                thread_id: Some(thread_id),
                cause: RefusalCause::MissingInput,
                reason: "The sender canceled the event hook's input request.".into(),
                detail: None,
                actor: Some(actor.clone()),
            },
            super::thread_emit_depth(thread_id),
            None,
        )
        .await?;
        Ok(true)
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
            input: None,
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
