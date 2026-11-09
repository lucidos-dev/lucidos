//! A *work item*: a root thread and its sub-threads (ADR 0623). Its *driver*
//! and *watchers* are operators. The fold reads the role events on the family
//! root with the current operator snapshot. So moving a device or an email to
//! another operator re-attributes every role it ever took.
//!
//! **A role is never authority.** Anyone can answer any gate, and no handler
//! may refuse an action because of a role. Roles only decide who a push about
//! the work item reaches, and what the factory app shows.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::gateway_operators::{OperatorRef, OperatorSnapshot};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The event types the fold reads, in the order they were recorded.
const ROLE_EVENT_TYPES: &[&str] = &[
    "WorkItemDriverTaken",
    "WorkItemDriverReleased",
    "WorkItemWatched",
    "WorkItemUnwatched",
];

/// Who steers a work item right now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WorkItemRoles {
    pub driver: Option<OperatorRef>,
    pub watchers: Vec<OperatorRef>,
}

/// Who a push about a work item reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience {
    /// Every device, as a push reaches today.
    Team,
    /// These devices only: the driver's and the watchers'.
    Devices(BTreeSet<String>),
}

impl Audience {
    /// The device ids a client checks itself against. `None` means everyone.
    pub fn device_ids(&self) -> Option<Vec<String>> {
        match self {
            Self::Team => None,
            Self::Devices(ids) => Some(ids.iter().cloned().collect()),
        }
    }

    /// The audience a notification row recorded. `None` is the whole team.
    pub fn from_device_ids(ids: Option<Vec<String>>) -> Self {
        match ids {
            None => Self::Team,
            Some(ids) => Self::Devices(ids.into_iter().collect()),
        }
    }

    /// Whether a push may reach this device. A subscription with no device id
    /// is reachable only by the team.
    pub fn reaches(&self, device_id: Option<&str>) -> bool {
        match self {
            Self::Team => true,
            Self::Devices(ids) => device_id.is_some_and(|id| ids.contains(id)),
        }
    }
}

/// One recorded role event, as the fold reads it.
#[derive(Debug, Clone)]
pub(crate) struct RoleEvent {
    pub event_type: String,
    pub actor: Option<MessageOrigin>,
}

/// The roles a requester and a run of role events give. A person-started
/// work item starts with its requester as driver; any other origin starts
/// with none. An actor no operator holds changes nothing.
pub(crate) fn fold(
    requester: Option<&MessageOrigin>,
    events: &[RoleEvent],
    snapshot: &OperatorSnapshot,
) -> WorkItemRoles {
    let mut roles = WorkItemRoles {
        driver: requester
            .filter(|origin| matches!(origin, MessageOrigin::Device { .. }))
            .and_then(|origin| snapshot.operator_of_origin(origin)),
        watchers: Vec::new(),
    };
    for event in events {
        let Some(who) = event
            .actor
            .as_ref()
            .and_then(|a| snapshot.operator_of_origin(a))
        else {
            continue;
        };
        match event.event_type.as_str() {
            "WorkItemDriverTaken" => roles.driver = Some(who),
            // A release gives up only what its actor holds.
            "WorkItemDriverReleased" => {
                if roles.driver.as_ref().is_some_and(|d| d.id == who.id) {
                    roles.driver = None;
                }
            }
            "WorkItemWatched" => {
                if !roles.watchers.iter().any(|w| w.id == who.id) {
                    roles.watchers.push(who);
                }
            }
            "WorkItemUnwatched" => roles.watchers.retain(|w| w.id != who.id),
            _ => {}
        }
    }
    roles
}

/// The audience of a work item's push. It is the whole team in four cases:
/// the gateway knows fewer than two people, the work item has no driver, a
/// gate escalated, or nobody it names holds a device. So a gate always
/// reaches someone, and a personal workspace pushes as it always did.
pub(crate) fn audience(
    roles: &WorkItemRoles,
    escalated: bool,
    snapshot: &OperatorSnapshot,
) -> Audience {
    if snapshot.person_count() < 2 {
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

/// The roles of the work item `root` heads.
pub async fn roles(
    pool: &PgPool,
    root: Uuid,
    snapshot: &OperatorSnapshot,
) -> Result<WorkItemRoles, sqlx::Error> {
    Ok(roles_of_many(pool, &[root], snapshot)
        .await?
        .remove(&root)
        .unwrap_or_default())
}

/// The roles of each work item `roots` names, in two queries. The requester
/// is the origin of the root's first message, or of its `TriggerStarted`: a
/// trigger-born thread has no `MessageReceived` of its own.
async fn roles_of_many(
    pool: &PgPool,
    roots: &[Uuid],
    snapshot: &OperatorSnapshot,
) -> Result<HashMap<Uuid, WorkItemRoles>, sqlx::Error> {
    let requesters: Vec<(Uuid, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT DISTINCT ON (thread_id) thread_id, COALESCE(payload->'origin', payload->'actor') \
         FROM events WHERE thread_id = ANY($1) \
           AND event_type IN ('MessageReceived', 'TriggerStarted') \
         ORDER BY thread_id, sequence",
    )
    .bind(roots)
    .fetch_all(pool)
    .await?;
    let requesters: HashMap<Uuid, MessageOrigin> = requesters
        .into_iter()
        .filter_map(|(root, v)| Some((root, serde_json::from_value(v?).ok()?)))
        .collect();

    let rows: Vec<(Uuid, String, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT thread_id, event_type, payload->'actor' FROM events \
         WHERE thread_id = ANY($1) AND event_type = ANY($2) ORDER BY sequence",
    )
    .bind(roots)
    .bind(ROLE_EVENT_TYPES)
    .fetch_all(pool)
    .await?;
    let mut events: HashMap<Uuid, Vec<RoleEvent>> = HashMap::new();
    for (root, event_type, actor) in rows {
        events.entry(root).or_default().push(RoleEvent {
            event_type,
            actor: actor.and_then(|v| serde_json::from_value(v).ok()),
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

/// Whether a gate of the work item `root` heads escalated and is still open.
async fn has_open_escalated_gate(pool: &PgPool, root: Uuid) -> Result<bool, sqlx::Error> {
    Ok(!roots_with_open_escalated_gate(pool, &[root])
        .await?
        .is_empty())
}

/// The work items among `roots` with a gate that escalated and is still open.
async fn roots_with_open_escalated_gate(
    pool: &PgPool,
    roots: &[Uuid],
) -> Result<HashSet<Uuid>, sqlx::Error> {
    let escalated: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT e.thread_id FROM events e \
         WHERE e.thread_id = ANY($1) AND e.event_type = 'WorkItemGateEscalated' \
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

/// The most work items one listing returns, newest activity first.
pub const LIST_LIMIT: i64 = 200;

/// One open work item, as the factory app lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkItemSummary {
    pub root_thread_id: Uuid,
    pub title: Option<String>,
    pub last_activity: chrono::DateTime<chrono::Utc>,
    #[serde(flatten)]
    pub roles: WorkItemRoles,
    /// A thread of the work item waits on a question.
    pub gate_open: bool,
    /// One of its open gates waited past the team's timeout.
    pub escalated: bool,
    /// The Team view: an open gate that nobody owns, because the work item
    /// has no driver or the gate escalated.
    pub needs_team: bool,
    /// The person `list` was asked for drives or watches it.
    pub yours: bool,
}

/// The open work items: sent root threads not archived, the home thread
/// aside. An unsent compose draft is no work item.
/// `yours` marks the ones `person` drives or watches.
pub async fn list(
    pool: &PgPool,
    snapshot: &OperatorSnapshot,
    person: Option<&OperatorRef>,
) -> Result<Vec<WorkItemSummary>, sqlx::Error> {
    let open: Vec<(Uuid, Option<String>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT thread_id, title, last_activity FROM thread_summaries \
         WHERE parent_thread_id IS NULL AND successor_of IS NULL \
           AND state = 'active' AND archive_state <> 'archived' AND NOT is_home \
         ORDER BY last_activity DESC LIMIT $1",
    )
    .bind(LIST_LIMIT)
    .fetch_all(pool)
    .await?;
    let roots: Vec<Uuid> = open.iter().map(|(root, ..)| *root).collect();
    let mut roles = roles_of_many(pool, &roots, snapshot).await?;
    let escalated = roots_with_open_escalated_gate(pool, &roots).await?;

    let waiting: Vec<Uuid> = sqlx::query_scalar(
        "SELECT thread_id FROM thread_summaries \
         WHERE status = 'waiting_for_user_answer' AND archive_state <> 'archived'",
    )
    .fetch_all(pool)
    .await?;
    let mut gated = HashSet::new();
    for thread in waiting {
        gated.insert(crate::engine::thread_family::family_root(pool, thread).await?);
    }

    Ok(open
        .into_iter()
        .map(|(root, title, last_activity)| {
            let roles = roles.remove(&root).unwrap_or_default();
            let gate_open = gated.contains(&root);
            let escalated = gate_open && escalated.contains(&root);
            let yours = person.is_some_and(|p| {
                roles
                    .driver
                    .iter()
                    .chain(&roles.watchers)
                    .any(|o| o.id == p.id)
            });
            WorkItemSummary {
                root_thread_id: root,
                title,
                last_activity,
                needs_team: gate_open && (roles.driver.is_none() || escalated),
                roles,
                gate_open,
                escalated,
                yours,
            }
        })
        .collect())
}

/// Who a push about `thread_id`, or any thread of its work item, reaches.
pub async fn audience_of_thread(
    pool: &PgPool,
    thread_id: Uuid,
    snapshot: &OperatorSnapshot,
) -> Result<Audience, sqlx::Error> {
    let root = crate::engine::thread_family::family_root(pool, thread_id).await?;
    let roles = roles(pool, root, snapshot).await?;
    let escalated = has_open_escalated_gate(pool, root).await?;
    Ok(audience(&roles, escalated, snapshot))
}

/// Who a notification reaches, from the thread it opens: its tap's thread
/// target, else the thread of the event it links. One that opens no thread
/// reaches the whole team, and so does every notification on a gateway with
/// fewer than two people, at no cost.
pub async fn audience_of_notification(
    pool: &PgPool,
    tap: &crate::scheduler::notifications::Tap,
    link_event_id: Option<Uuid>,
    snapshot: &OperatorSnapshot,
) -> Result<Audience, sqlx::Error> {
    use crate::scheduler::notifications::{NavigateTarget, Tap};
    if snapshot.person_count() < 2 {
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

/// Record a role event on a work item's family root.
pub async fn record(
    bus: &EventBus,
    root: Uuid,
    event: ThreadEvent,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    bus.emit(BusEvent::Thread {
        thread_id: root,
        event,
        meta: EventMeta::with_actor(actor),
    })
    .await
    .map(|_| ())
}

/// A driver change one person made on a work item another person drove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Takeover {
    pub from: OperatorRef,
    pub to: OperatorRef,
}

/// Answering a gate makes its answerer the driver: a device's holder, or the
/// operator a trigger fire's actor email names. Nobody else becomes one, and
/// a driver answering again records nothing. Returns the takeover when
/// someone else drove before.
pub async fn take_on_answer(
    pool: &PgPool,
    bus: &EventBus,
    gate_thread_id: Uuid,
    answerer: &MessageOrigin,
    snapshot: &OperatorSnapshot,
) -> Result<Option<Takeover>, BoxError> {
    let Some(who) = snapshot.operator_of_origin(answerer) else {
        return Ok(None);
    };
    let root = crate::engine::thread_family::family_root(pool, gate_thread_id).await?;
    let current = roles(pool, root, snapshot).await?;
    if current.driver.as_ref().is_some_and(|d| d.id == who.id) {
        return Ok(None);
    }
    record(
        bus,
        root,
        ThreadEvent::WorkItemDriverTaken,
        Some(answerer.clone()),
    )
    .await?;
    Ok(current.driver.map(|from| Takeover { from, to: who }))
}

/// A gate that waited on its driver past the timeout and has not escalated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OverdueGate {
    pub root: Uuid,
    pub gate_thread_id: Uuid,
    pub tool_use_id: String,
    pub question_event_id: Uuid,
    pub driver: OperatorRef,
}

/// Open gates asked more than `hours` ago whose work item has a driver and
/// that never escalated.
pub(crate) async fn overdue_gates(
    pool: &PgPool,
    hours: f64,
    snapshot: &OperatorSnapshot,
) -> Result<Vec<OverdueGate>, sqlx::Error> {
    let open: Vec<(Uuid, String, Uuid)> = sqlx::query_as(
        "SELECT q.thread_id, q.payload->>'tool_use_id', q.id FROM events q \
         WHERE q.event_type = 'UserQuestionAsked' \
           AND q.created < now() - make_interval(secs => $1) \
           AND NOT EXISTS (SELECT 1 FROM events a \
               WHERE a.thread_id = q.thread_id AND a.event_type = 'UserQuestionAnswered' \
                 AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id') \
           AND EXISTS (SELECT 1 FROM thread_summaries s \
               WHERE s.thread_id = q.thread_id AND s.status = 'waiting_for_user_answer') \
         ORDER BY q.created",
    )
    .bind(hours * 3600.0)
    .fetch_all(pool)
    .await?;

    let mut overdue = Vec::new();
    for (gate_thread_id, tool_use_id, question_event_id) in open {
        let root = crate::engine::thread_family::family_root(pool, gate_thread_id).await?;
        let escalated: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM events WHERE thread_id = $1 \
                 AND event_type = 'WorkItemGateEscalated' \
                 AND payload->>'gate_thread_id' = $2 AND payload->>'tool_use_id' = $3)",
        )
        .bind(root)
        .bind(gate_thread_id.to_string())
        .bind(&tool_use_id)
        .fetch_one(pool)
        .await?;
        if escalated {
            continue;
        }
        if let Some(driver) = roles(pool, root, snapshot).await?.driver {
            overdue.push(OverdueGate {
                root,
                gate_thread_id,
                tool_use_id,
                question_event_id,
                driver,
            });
        }
    }
    Ok(overdue)
}

/// How often the scheduler looks for overdue gates: every five minutes.
pub const GATE_ESCALATION_CRON: &str = "0 */5 * * * *";

/// The scheduler's sweep: escalate every overdue gate once, with a team
/// notification that opens the question. Off while `driver_timeout_hours`
/// is 0.
pub async fn escalate_overdue_gates(engine: &crate::api::SharedEngine) {
    use crate::core::preference_catalog as prefs;
    let hours = prefs::DRIVER_TIMEOUT_HOURS.read(engine.pool()).await;
    if hours <= 0.0 {
        return;
    }
    let snapshot = crate::gateway_operators::current();
    let gates = match overdue_gates(engine.pool(), hours, &snapshot).await {
        Ok(gates) => gates,
        Err(e) => {
            log!("[WorkItem] could not list overdue gates: {}", e);
            return;
        }
    };
    for gate in gates {
        if let Err(e) = escalate(engine, &gate).await {
            log!(
                "[WorkItem] could not escalate gate {} on {}: {}",
                gate.tool_use_id,
                gate.gate_thread_id,
                e
            );
        }
    }
}

/// Record the escalation, then notify. The audience reads the escalation
/// back, so it is the whole team.
async fn escalate(engine: &crate::api::SharedEngine, gate: &OverdueGate) -> Result<(), BoxError> {
    record(
        &engine.event_bus,
        gate.root,
        ThreadEvent::WorkItemGateEscalated {
            gate_thread_id: gate.gate_thread_id,
            tool_use_id: gate.tool_use_id.clone(),
        },
        None,
    )
    .await?;

    let message = format!(
        "A question has waited on {} past the team's timeout. Anyone can answer it.",
        gate.driver.name
    );
    announce(
        engine,
        "A gate needs the team",
        &message,
        gate.gate_thread_id,
        Some(gate.question_event_id),
        None,
    )
    .await
}

/// What a work item tells its people besides its gates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkItemNotice {
    /// `0` answered a gate on a work item the recipient drove, so now drives it.
    GateAnswered(Takeover),
    /// `0` tapped Take on a work item the recipient drove.
    TakenOver(Takeover),
    /// The root thread was archived, by `by` when a person did it.
    Completed { by: Option<OperatorRef> },
}

impl WorkItemNotice {
    /// Who hears it: the displaced driver, or for a finished work item its
    /// driver and watchers, except the person who finished it.
    pub fn recipients(&self, roles: &WorkItemRoles) -> Vec<OperatorRef> {
        match self {
            Self::GateAnswered(t) | Self::TakenOver(t) => vec![t.from.clone()],
            Self::Completed { by } => roles
                .driver
                .iter()
                .chain(&roles.watchers)
                .filter(|o| by.as_ref().is_none_or(|b| b.id != o.id))
                .fold(Vec::new(), |mut people, o| {
                    if !people.iter().any(|p: &OperatorRef| p.id == o.id) {
                        people.push(o.clone());
                    }
                    people
                }),
        }
    }

    fn text(&self, work_item: &str) -> (String, String) {
        match self {
            Self::GateAnswered(t) => (
                format!("{} answered your gate", t.to.name),
                format!(
                    "{} answered a question on \"{work_item}\" and now drives it.",
                    t.to.name
                ),
            ),
            Self::TakenOver(t) => (
                format!("{} took over", t.to.name),
                format!(
                    "{} now drives \"{work_item}\". You can still answer its gates.",
                    t.to.name
                ),
            ),
            Self::Completed { by } => (
                "Work item done".to_string(),
                match by {
                    Some(person) => format!("{} archived \"{work_item}\".", person.name),
                    None => format!("\"{work_item}\" was archived."),
                },
            ),
        }
    }
}

/// Tell the people a notice names about the work item `root` heads, on their
/// devices only. Nothing on a gateway with fewer than two people, and nothing
/// when they hold no device.
pub async fn notify(
    engine: &crate::api::SharedEngine,
    root: Uuid,
    notice: WorkItemNotice,
    snapshot: &OperatorSnapshot,
) -> Result<(), BoxError> {
    if snapshot.person_count() < 2 {
        return Ok(());
    }
    let roles = roles(engine.pool(), root, snapshot).await?;
    let devices: BTreeSet<String> = notice
        .recipients(&roles)
        .iter()
        .flat_map(|o| snapshot.devices_of(&o.id))
        .map(str::to_string)
        .collect();
    if devices.is_empty() {
        return Ok(());
    }
    let work_item: Option<String> =
        sqlx::query_scalar("SELECT title FROM thread_summaries WHERE thread_id = $1")
            .bind(root)
            .fetch_optional(engine.pool())
            .await?
            .flatten();
    let (title, message) = notice.text(work_item.as_deref().unwrap_or("a work item"));
    announce(
        engine,
        &title,
        &message,
        root,
        None,
        Some(devices.into_iter().collect()),
    )
    .await
}

/// Write a notification that opens `thread_id`, at `event_id` when given,
/// and push it. `audience` `None` lets the engine work it out from the work
/// item.
async fn announce(
    engine: &crate::api::SharedEngine,
    title: &str,
    message: &str,
    thread_id: Uuid,
    event_id: Option<Uuid>,
    audience: Option<Vec<String>>,
) -> Result<(), BoxError> {
    use crate::engine::event_bus::SystemEvent;
    use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};

    let id = Uuid::new_v4();
    let tap = Tap::Navigate {
        to: Box::new(NavigateUi {
            target: NavigateTarget::Thread,
            id: Some(thread_id.to_string()),
            event_id: event_id.map(|e| e.to_string()),
            ..Default::default()
        }),
    };
    engine
        .event_bus
        .emit(BusEvent::System(SystemEvent::NotificationCreated {
            id: id.to_string(),
            title: title.to_string(),
            message: message.to_string(),
            task_id: None,
            app_id: None,
            thread_id: Some(thread_id.to_string()),
            event_id: event_id.map(|e| e.to_string()),
            tap: tap.clone(),
            actor: None,
            audience,
        }))
        .await?;
    crate::scheduler::push::send_push_to_all_with_app(
        engine,
        title,
        message,
        Some(id),
        None,
        Some(thread_id),
        event_id,
        tap,
    );
    Ok(())
}

#[cfg(test)]
#[path = "work_item_tests.rs"]
mod tests;
