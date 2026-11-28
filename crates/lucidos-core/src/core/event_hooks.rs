use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::event_subscription::{matchable_system_payload, matchable_thread_payload};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub const HOOKS_DIR: &str = "hooks";

pub const HOOK_DEFINITION_FILE: &str = "hook.toml";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookArgument {
    pub label: String,
    pub choices_script: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookChoice {
    pub value: String,
    pub label: String,
}

pub const HOOK_CHOICE_LIMIT: usize = 40;
pub const HOOK_CHOICE_QUERY_LIMIT: usize = 256;
pub const HOOK_CREATION_VALUE_LIMIT: usize = 256;

pub type HookArgs = BTreeMap<String, String>;

pub fn hook_creation_key(slug: &str, argument: &str) -> String {
    format!("new:{slug}:{argument}")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookDefinition {
    pub name: String,
    pub event_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Value>,
    pub script: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arguments: BTreeMap<String, HookArgument>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookedEventKind {
    Thread,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HookListing {
    Valid {
        slug: String,
        event_kind: HookedEventKind,
        definition: HookDefinition,
        content_hash: String,
    },
    Invalid {
        slug: String,
        definition: Option<HookDefinition>,
        problems: Vec<String>,
    },
}

impl HookListing {
    pub fn slug(&self) -> &str {
        match self {
            Self::Valid { slug, .. } | Self::Invalid { slug, .. } => slug,
        }
    }

    pub fn content_hash(&self) -> Option<&str> {
        match self {
            Self::Valid { content_hash, .. } => Some(content_hash),
            Self::Invalid { .. } => None,
        }
    }
}

pub trait EventHooks: Send + Sync {
    fn list_definitions(&self, workspace: &Path) -> Result<Vec<HookListing>, String>;

    fn list_at(&self, _root: &Path) -> Result<Vec<HookListing>, String> {
        Err("The event hook catalog cannot read a plugin.".into())
    }

    fn bundle_plugin_scripts(
        &self,
        _plugin_root: &Path,
        _planned: &mut Vec<super::plugins::PlannedFile>,
    ) -> Result<(), String> {
        Ok(())
    }

    fn asked_event_types(&self) -> &'static [&'static str] {
        ASKED_EVENT_TYPES
    }
}

pub const PROPOSAL_CHECK_EVENT: &str = "ChangeProposalChecked";
pub const APPLY_CHECK_EVENT: &str = "ChangeApplyChecked";

pub const ASKED_EVENT_TYPES: &[&str] = &[
    "ChangeApplyRequested",
    "ChangeProposed",
    PROPOSAL_CHECK_EVENT,
    APPLY_CHECK_EVENT,
    FIRST_MESSAGE_EVENT_TYPE,
    "TriggerStarted",
];

/// INVARIANT: How long a hook may run when its definition names no timeout.
pub const DEFAULT_TIMEOUT_SECS: u64 = 10;

pub const TIMEOUT_CEILING_SECS: u64 = 60;

pub fn effective_timeout(timeout_secs: Option<u64>) -> Duration {
    Duration::from_secs(
        timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, TIMEOUT_CEILING_SECS),
    )
}

pub const FIRST_MESSAGE_EVENT_TYPE: &str = "MessageReceived";

pub const FIRST_MESSAGE_KEY: &str = "first_message";

pub const REFUSED_EVENT_TYPE: &str = "EventHookRefused";

pub const REFUSAL_AGGREGATE: &str = "event_hook_refusal";

#[derive(Debug, Clone, PartialEq)]
pub struct PendingEvent {
    pub event_type: String,
    pub payload: Value,
}

impl PendingEvent {
    pub fn of(event: &BusEvent) -> Self {
        match event {
            BusEvent::Thread {
                thread_id,
                event,
                meta,
            } => Self::of_thread(*thread_id, event, meta),
            BusEvent::System(event) => Self {
                event_type: event.stored_event_type().to_string(),
                payload: matchable_system_payload(event),
            },
        }
    }

    pub fn of_thread(thread_id: Uuid, event: &ThreadEvent, meta: &EventMeta) -> Self {
        Self {
            event_type: event.event_type().to_string(),
            payload: matchable_thread_payload(event, meta, thread_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReturnedEvent {
    pub event_type: String,
    #[serde(default)]
    pub payload: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub cause: RefusalCause,
    pub reason: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefusalCause {
    Refused,
    Crashed,
    TimedOut,
    InvalidAnswer,
    Changed,
    MissingInput,
    Unavailable,
}

pub struct HookAsk<'a> {
    pub hook: &'a ArmedHook,
    pub pending: &'a PendingEvent,
    pub arguments: &'a HookArgs,
    pub chain_depth: u32,
    pub can_answer: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HookVerdict {
    Skipped,
    Allowed(Vec<ReturnedEvent>),
    Refused(Refusal),
    NeedsInput {
        argument: String,
        label: String,
        create_label: Option<String>,
        choices_timeout_secs: u64,
        choices: Vec<HookChoice>,
    },
}

#[async_trait]
pub trait EventHookAsker: Send + Sync {
    async fn ask(&self, ask: HookAsk<'_>) -> HookVerdict;

    async fn choices(
        &self,
        _hook: &ArmedHook,
        _name: &str,
        _query: &str,
    ) -> Result<Vec<HookChoice>, String> {
        Err("No event hook choices runner is installed.".into())
    }
}

pub const HOOK_AGGREGATE: &str = "event_hook";

/// INVARIANT: The arming events. No hook may name one, so no hook can stop the owner
/// arming, disarming or removing a hook.
pub const ARMING_EVENT_TYPES: [&str; 3] =
    ["EventHookArmed", "EventHookDisarmed", "EventHookRemoved"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmedHook {
    pub slug: String,
    pub event_type: String,
    pub content_hash: String,
    pub armed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<MessageOrigin>,
}

pub async fn armed_hooks(pool: &PgPool) -> Result<Vec<ArmedHook>, BoxError> {
    let latest: Vec<(Value, DateTime<Utc>)> = sqlx::query_as(
        "SELECT DISTINCT ON (aggregate_id) payload, created FROM events \
         WHERE aggregate = $1 ORDER BY aggregate_id, sequence DESC",
    )
    .bind(HOOK_AGGREGATE)
    .fetch_all(pool)
    .await?;
    let mut armed = Vec::new();
    for (payload, armed_at) in latest {
        if let SystemEvent::EventHookArmed {
            slug,
            event_type,
            content_hash,
            actor,
        } = SystemEvent::from_stored(payload)?
        {
            armed.push(ArmedHook {
                slug,
                event_type,
                content_hash,
                armed_at,
                actor,
            });
        }
    }
    Ok(armed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmState {
    Disarmed,
    Armed,
    Changed,
}

impl ArmState {
    pub fn of(armed: Option<&ArmedHook>, listing: Option<&HookListing>) -> Self {
        match armed {
            None => Self::Disarmed,
            Some(armed)
                if listing.and_then(HookListing::content_hash)
                    == Some(armed.content_hash.as_str()) =>
            {
                Self::Armed
            }
            Some(_) => Self::Changed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arming_event_names_are_the_variants_names() {
        let events = [
            SystemEvent::EventHookArmed {
                slug: "gate".into(),
                event_type: "MessageReceived".into(),
                content_hash: "abc".into(),
                actor: None,
            },
            SystemEvent::EventHookDisarmed {
                slug: "gate".into(),
                actor: None,
            },
            SystemEvent::EventHookRemoved {
                slug: "gate".into(),
                actor: None,
            },
        ];
        for (event, name) in events.iter().zip(ARMING_EVENT_TYPES) {
            assert_eq!(event.event_type(), name);
            assert_eq!(event.aggregate(), HOOK_AGGREGATE);
            assert_eq!(event.aggregate_id(), "gate");
            assert!(event.is_persisted(), "{name}");
        }
    }

    fn armed(hash: &str) -> ArmedHook {
        ArmedHook {
            slug: "gate".into(),
            event_type: "MessageReceived".into(),
            content_hash: hash.into(),
            armed_at: Utc::now(),
            actor: None,
        }
    }

    fn valid(hash: &str) -> HookListing {
        HookListing::Valid {
            slug: "gate".into(),
            event_kind: HookedEventKind::Thread,
            definition: HookDefinition {
                arguments: Default::default(),
                name: "Gate".into(),
                event_type: "MessageReceived".into(),
                condition: None,
                script: "check.py".into(),
                timeout_secs: None,
            },
            content_hash: hash.into(),
        }
    }

    #[test]
    fn a_hook_stays_armed_only_while_it_hashes_as_armed() {
        let armed = armed("abc");
        assert_eq!(ArmState::of(None, Some(&valid("abc"))), ArmState::Disarmed);
        assert_eq!(
            ArmState::of(Some(&armed), Some(&valid("abc"))),
            ArmState::Armed
        );
        assert_eq!(
            ArmState::of(Some(&armed), Some(&valid("def"))),
            ArmState::Changed
        );
        assert_eq!(ArmState::of(Some(&armed), None), ArmState::Changed);
        let invalid = HookListing::Invalid {
            slug: "gate".into(),
            definition: None,
            problems: vec!["broken".into()],
        };
        assert_eq!(
            ArmState::of(Some(&armed), Some(&invalid)),
            ArmState::Changed
        );
    }
}
