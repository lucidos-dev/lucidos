//! The interface to *event hooks* (ADR 0746). The `lucidos-event-hooks` leaf
//! crate implements [`EventHooks`], and the engine executable wires it in
//! (ADR 0738).

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

use crate::engine::event_bus::SystemEvent;
use crate::engine::thread_events::MessageOrigin;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The `data/` tree that holds one folder per hook.
pub const HOOKS_DIR: &str = "hooks";

/// The file in a hook's folder that declares it.
pub const HOOK_DEFINITION_FILE: &str = "hook.toml";

/// A `hook.toml`, as its author wrote it. An unknown key fails the parse, so a
/// misspelled one never passes in silence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookDefinition {
    pub name: String,
    /// The hooked event's wire name: a thread event or a persisted system event.
    pub event_type: String,
    /// A payload filter in the event subscription `condition` syntax.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Value>,
    /// The script's path inside the hook's folder.
    pub script: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// The bus carrier the hooked event travels on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookedEventKind {
    Thread,
    System,
}

/// One folder under `data/hooks/`, as the hook list shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HookListing {
    Valid {
        slug: String,
        event_kind: HookedEventKind,
        definition: HookDefinition,
        /// The digest that binds an arming to this content: the definition
        /// file whose bytes parsed into `definition`, and the script it names.
        content_hash: String,
    },
    Invalid {
        slug: String,
        /// Set when the file parsed but a check failed.
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

    /// The digest of a valid hook's content. An invalid one has none.
    pub fn content_hash(&self) -> Option<&str> {
        match self {
            Self::Valid { content_hash, .. } => Some(content_hash),
            Self::Invalid { .. } => None,
        }
    }
}

/// What the engine asks of the event hook feature.
pub trait EventHooks: Send + Sync {
    /// Every entry under the workspace's `data/hooks/`, invalid ones included,
    /// sorted by slug. `Err` when the tree itself cannot be read.
    fn list_definitions(&self, workspace: &Path) -> Result<Vec<HookListing>, String>;

    /// The event types an *asking site* asks about. Only a hook on one of
    /// these can be armed.
    fn asked_event_types(&self) -> &'static [&'static str] {
        ASKED_EVENT_TYPES
    }
}

/// The event types some *asking site* asks the hooks about. None asks yet, so
/// no hook can be armed.
pub const ASKED_EVENT_TYPES: &[&str] = &[];

/// The `events.aggregate` of the arming events, keyed by hook slug.
pub const HOOK_AGGREGATE: &str = "event_hook";

/// The arming events. No hook may name one, so no hook can stop the owner
/// arming, disarming or removing a hook.
pub const ARMING_EVENT_TYPES: [&str; 3] =
    ["EventHookArmed", "EventHookDisarmed", "EventHookRemoved"];

/// A hook's arming, from its latest `EventHookArmed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmedHook {
    pub slug: String,
    pub event_type: String,
    pub content_hash: String,
    pub armed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<MessageOrigin>,
}

/// Every hook whose latest arming event is `EventHookArmed`, sorted by slug.
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

/// Where one hook stands with the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmState {
    Disarmed,
    /// Armed, and its content still hashes as armed.
    Armed,
    /// Armed, but its definition or script changed, broke or left. Its events
    /// are refused until the owner arms it again or disarms it.
    Changed,
}

impl ArmState {
    /// `listing` is the hook's folder as listed, `None` when it is gone.
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
