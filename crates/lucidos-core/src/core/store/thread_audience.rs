use std::collections::{BTreeSet, HashMap, HashSet};

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::thread_events::notification::{NavigateTarget, Tap};
use crate::engine::thread_events::MessageOrigin;
use crate::gateway_users::{UserRef, UserSnapshot};

const ROLE_EVENT_TYPES: &[&str] = &[
    "RootThreadDriverTaken",
    "WorkItemDriverTaken",
    "RootThreadDriverReleased",
    "WorkItemDriverReleased",
    "RootThreadWatched",
    "WorkItemWatched",
    "RootThreadUnwatched",
    "WorkItemUnwatched",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ThreadUsers {
    pub driver: Option<UserRef>,
    pub watchers: Vec<UserRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience {
    Team,
    Devices(BTreeSet<String>),
}

impl Audience {
    pub fn device_ids(&self) -> Option<Vec<String>> {
        match self {
            Self::Team => None,
            Self::Devices(ids) => Some(ids.iter().cloned().collect()),
        }
    }

    pub fn from_device_ids(ids: Option<Vec<String>>) -> Self {
        match ids {
            None => Self::Team,
            Some(ids) => Self::Devices(ids.into_iter().collect()),
        }
    }

    pub fn reaches(&self, device_id: Option<&str>) -> bool {
        match self {
            Self::Team => true,
            Self::Devices(ids) => device_id.is_some_and(|id| ids.contains(id)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoleEvent {
    pub event_type: String,
    pub actor: Option<MessageOrigin>,
    pub user_id: Option<String>,
}

pub fn fold(
    requester: Option<&MessageOrigin>,
    events: &[RoleEvent],
    snapshot: &UserSnapshot,
) -> ThreadUsers {
    let mut roles = ThreadUsers {
        driver: requester
            .filter(|origin| matches!(origin, MessageOrigin::Device { .. }))
            .and_then(|origin| snapshot.user_of_origin(origin)),
        watchers: Vec::new(),
    };
    for event in events {
        let who = match &event.user_id {
            Some(id) => snapshot.user_of_id(id),
            None => event
                .actor
                .as_ref()
                .and_then(|a| snapshot.user_of_origin(a)),
        };
        let Some(who) = who else {
            continue;
        };
        match event.event_type.as_str() {
            "RootThreadDriverTaken" | "WorkItemDriverTaken" => roles.driver = Some(who),
            "RootThreadDriverReleased" | "WorkItemDriverReleased" => {
                if roles.driver.as_ref().is_some_and(|d| d.id == who.id) {
                    roles.driver = None;
                }
            }
            "RootThreadWatched" | "WorkItemWatched" => {
                if !roles.watchers.iter().any(|w| w.id == who.id) {
                    roles.watchers.push(who);
                }
            }
            "RootThreadUnwatched" | "WorkItemUnwatched" => {
                roles.watchers.retain(|w| w.id != who.id)
            }
            _ => {}
        }
    }
    roles
}

pub fn audience(roles: &ThreadUsers, escalated: bool, snapshot: &UserSnapshot) -> Audience {
    if snapshot.user_count() < 2 {
        return Audience::Team;
    }
    let Some(driver) = roles.driver.as_ref().filter(|_| !escalated) else {
        return Audience::Team;
    };
    let devices: BTreeSet<String> = std::iter::once(driver)
        .chain(&roles.watchers)
        .flat_map(|o| snapshot.devices_of(&o.id))
        .map(str::to_string)
        .collect();
    if devices.is_empty() {
        Audience::Team
    } else {
        Audience::Devices(devices)
    }
}

pub async fn roles(
    pool: &PgPool,
    root: Uuid,
    snapshot: &UserSnapshot,
) -> Result<ThreadUsers, sqlx::Error> {
    Ok(roles_of_many(pool, &[root], snapshot)
        .await?
        .remove(&root)
        .unwrap_or_default())
}

pub async fn roles_of_many(
    pool: &PgPool,
    roots: &[Uuid],
    snapshot: &UserSnapshot,
) -> Result<HashMap<Uuid, ThreadUsers>, sqlx::Error> {
    let requesters: Vec<(Uuid, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT DISTINCT ON (thread_id) thread_id, COALESCE(payload->'origin', payload->'actor') \
         FROM events WHERE thread_id = ANY($1) \
           AND event_type IN ('MessageReceived', 'UserOnlyMessagePosted', 'TriggerStarted') \
         ORDER BY thread_id, sequence",
    )
    .bind(roots)
    .fetch_all(pool)
    .await?;
    let requesters: HashMap<Uuid, MessageOrigin> = requesters
        .into_iter()
        .filter_map(|(root, v)| Some((root, serde_json::from_value(v?).ok()?)))
        .collect();

    let rows: Vec<(Uuid, String, Option<serde_json::Value>, Option<String>)> = sqlx::query_as(
        "SELECT thread_id, event_type, payload->'actor', payload->>'user_id' FROM events \
         WHERE thread_id = ANY($1) AND event_type = ANY($2) ORDER BY sequence",
    )
    .bind(roots)
    .bind(ROLE_EVENT_TYPES)
    .fetch_all(pool)
    .await?;
    let mut events: HashMap<Uuid, Vec<RoleEvent>> = HashMap::new();
    for (root, event_type, actor, user_id) in rows {
        events.entry(root).or_default().push(RoleEvent {
            event_type,
            actor: actor.and_then(|v| serde_json::from_value(v).ok()),
            user_id,
        });
    }
    Ok(roots
        .iter()
        .map(|root| {
            let events = events.get(root).map(Vec::as_slice).unwrap_or_default();
            (*root, fold(requesters.get(root), events, snapshot))
        })
        .collect())
}

async fn has_open_escalated_gate(pool: &PgPool, root: Uuid) -> Result<bool, sqlx::Error> {
    Ok(!roots_with_open_escalated_gate(pool, &[root])
        .await?
        .is_empty())
}

pub async fn roots_with_open_escalated_gate(
    pool: &PgPool,
    roots: &[Uuid],
) -> Result<HashSet<Uuid>, sqlx::Error> {
    let escalated: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT e.thread_id FROM events e \
         WHERE e.thread_id = ANY($1) AND e.event_type IN ('RootThreadGateEscalated', 'WorkItemGateEscalated') \
           AND NOT EXISTS ( \
               SELECT 1 FROM events a \
               WHERE a.thread_id = (e.payload->>'gate_thread_id')::uuid \
                 AND a.event_type = 'UserQuestionAnswered' \
                 AND a.payload->>'tool_use_id' = e.payload->>'tool_use_id')",
    )
    .bind(roots)
    .fetch_all(pool)
    .await?;
    Ok(escalated.into_iter().collect())
}

pub async fn audience_of_thread(
    pool: &PgPool,
    thread_id: Uuid,
    snapshot: &UserSnapshot,
) -> Result<Audience, sqlx::Error> {
    let root = crate::engine::thread_family::family_root(pool, thread_id).await?;
    let roles = roles(pool, root, snapshot).await?;
    let escalated = has_open_escalated_gate(pool, root).await?;
    Ok(audience(&roles, escalated, snapshot))
}

pub async fn audience_of_notification(
    pool: &PgPool,
    tap: &Tap,
    link_event_id: Option<Uuid>,
    snapshot: &UserSnapshot,
) -> Result<Audience, sqlx::Error> {
    if snapshot.user_count() < 2 {
        return Ok(Audience::Team);
    }
    let tapped = match tap {
        Tap::Navigate { to } if to.target == NavigateTarget::Thread => {
            to.id.as_deref().and_then(|id| Uuid::parse_str(id).ok())
        }
        _ => None,
    };
    let thread = match (tapped, link_event_id) {
        (Some(thread), _) => Some(thread),
        (None, Some(event_id)) => {
            sqlx::query_scalar::<_, Option<Uuid>>("SELECT thread_id FROM events WHERE id = $1")
                .bind(event_id)
                .fetch_optional(pool)
                .await?
                .flatten()
        }
        (None, None) => None,
    };
    match thread {
        Some(thread) => audience_of_thread(pool, thread, snapshot).await,
        None => Ok(Audience::Team),
    }
}
