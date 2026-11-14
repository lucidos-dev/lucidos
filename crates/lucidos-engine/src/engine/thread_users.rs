//! The users of a *root thread* and its sub-threads (ADR 0623): its *driver*
//! and *watchers*, who are users. The fold reads the role events on the family
//! root with the current user snapshot. So moving a device or an email to
//! another user re-attributes every role it ever took.
//!
//! **A role is never authority.** Anyone can answer any gate, and no handler
//! may refuse an action because of a role. Roles only decide who a push about
//! the root thread reaches, and what the factory app shows.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::agent_pause::{self, MessageAudience};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    AgentPauseCause, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::gateway_users::{UserRef, UserSnapshot};
use crate::scheduler::notifications::PushGroup;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The mentions a chat send carries, checked against the gateway's users.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UserAddressing {
    /// User ids, each once, in the order sent.
    pub mentions: Vec<String>,
    pub agent_mentioned: bool,
}

/// Route one user's message: flip the pause as the route says, and answer
/// who the message is for.
pub(crate) async fn route_user_only_message(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: Uuid,
    addressing: &UserAddressing,
    actor: Option<MessageOrigin>,
) -> Result<MessageAudience, BoxError> {
    let paused = agent_pause::agent_paused(pool, thread_id).await?;
    let question_open = paused
        && crate::engine::agent_question::lookup_active_question_tool_use_id(pool, thread_id)
            .await
            .is_some();
    let route = agent_pause::user_only_message_route(
        !addressing.mentions.is_empty(),
        addressing.agent_mentioned,
        paused,
        question_open,
    );
    if let Some(change) = route.pause_change {
        agent_pause::record_pause_change(bus, thread_id, change, AgentPauseCause::Mention, actor)
            .await?;
    }
    Ok(route.audience)
}

/// One user-only message, as the send recorded it.
pub(crate) struct UserOnlyMessage {
    pub text: String,
    pub image_hashes: Vec<String>,
    pub device_id: Option<String>,
    pub mentions: Vec<String>,
    pub event_id: Option<Uuid>,
}

/// Record a user-only message. It starts no turn.
pub(crate) async fn post_user_only_message(
    bus: &EventBus,
    thread_id: Uuid,
    message: UserOnlyMessage,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserOnlyMessagePosted {
            text: message.text,
            user_image_hashes: message.image_hashes,
            device_id: message.device_id,
            mentioned_user_ids: message.mentions,
        },
        meta: EventMeta {
            event_id: message.event_id,
            channel: Some(EventChannel::Chat),
            actor,
            ..EventMeta::NONE
        },
    })
    .await
    .map(|_| ())
}

/// Persist notice intent before recording the message. Recovery joins the
/// recorded message, so an abandoned send can never notify anyone.
pub(crate) async fn queue_user_notice(
    engine: &crate::api::SharedEngine,
    thread_id: Uuid,
    event_id: Uuid,
    text: &str,
    addressing: &UserAddressing,
    user_only: bool,
    sender: &Option<MessageOrigin>,
) -> Result<(), BoxError> {
    let notice = UserOnlyMessageNotice {
        thread_id,
        event_id: Some(event_id),
        text: text.to_string(),
        mentions: addressing.mentions.clone(),
        user_only,
        sender: sender.clone(),
    };
    sqlx::query("INSERT INTO user_only_message_notices (event_id, notice) VALUES ($1, $2) ON CONFLICT (event_id) DO UPDATE SET notice = EXCLUDED.notice WHERE NOT EXISTS (SELECT 1 FROM events WHERE id = EXCLUDED.event_id)")
        .bind(event_id)
        .bind(sqlx::types::Json(notice))
        .execute(engine.pool()).await?;
    Ok(())
}

/// Clear abandoned notice intent without removing a recorded message's delivery.
pub(crate) async fn clear_unrecorded_user_notice(
    pool: &sqlx::PgPool,
    event_id: Uuid,
) -> Result<(), BoxError> {
    sqlx::query("DELETE FROM user_only_message_notices WHERE event_id=$1 AND NOT EXISTS (SELECT 1 FROM events WHERE id=$1)")
        .bind(event_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// The event types the fold reads, in the order they were recorded.
const ROLE_EVENT_TYPES: &[&str] = &[
    "WorkItemDriverTaken",
    "WorkItemDriverReleased",
    "WorkItemWatched",
    "WorkItemUnwatched",
];

/// Who steers a root thread right now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ThreadUsers {
    pub driver: Option<UserRef>,
    pub watchers: Vec<UserRef>,
}

/// Who a push about a root thread reaches.
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
    /// The user a mention made a watcher. `None` means the actor's user.
    pub user_id: Option<String>,
}

/// The roles a requester and a run of role events give. A user-started
/// root thread starts with its requester as driver; any other origin starts
/// with none. An event names its user, else its actor's user. One that
/// names nobody changes nothing.
pub(crate) fn fold(
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

/// The audience of a root thread's push. It is the whole team in four cases:
/// the gateway knows fewer than two users, the root thread has no driver, a
/// gate escalated, or nobody it names holds a device. So a gate always
/// reaches someone, and a personal workspace pushes as it always did.
pub(crate) fn audience(roles: &ThreadUsers, escalated: bool, snapshot: &UserSnapshot) -> Audience {
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

/// The users of the root thread `root`.
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

/// The users of each root thread in `roots`, in two queries. The requester
/// is who sent the root's first message, to its agent or to users, or the
/// origin of its `TriggerStarted`: a trigger-born thread has no message of its own.
async fn roles_of_many(
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

/// Whether a gate under the root thread `root` escalated and is still open.
async fn has_open_escalated_gate(pool: &PgPool, root: Uuid) -> Result<bool, sqlx::Error> {
    Ok(!roots_with_open_escalated_gate(pool, &[root])
        .await?
        .is_empty())
}

/// The root threads among `roots` with a gate that escalated and is still open.
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

/// The most root threads one listing returns, newest activity first.
pub const LIST_LIMIT: i64 = 200;

/// One open root thread, as the factory app lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThreadUsersSummary {
    pub root_thread_id: Uuid,
    pub title: Option<String>,
    pub last_activity: chrono::DateTime<chrono::Utc>,
    #[serde(flatten)]
    pub roles: ThreadUsers,
    /// A thread in its family waits on a question.
    pub gate_open: bool,
    /// One of its open gates waited past the team's timeout.
    pub escalated: bool,
    /// The Team view: an open gate that nobody owns, because the root thread
    /// has no driver or the gate escalated.
    pub needs_team: bool,
    /// The user `list` was asked for drives or watches it.
    pub yours: bool,
}

/// The open root threads: sent and not archived, the home thread aside.
/// An unsent compose draft is not listed.
/// `yours` marks the ones `user` drives or watches.
pub async fn list(
    pool: &PgPool,
    snapshot: &UserSnapshot,
    user: Option<&UserRef>,
) -> Result<Vec<ThreadUsersSummary>, sqlx::Error> {
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
            let yours = user.is_some_and(|p| {
                roles
                    .driver
                    .iter()
                    .chain(&roles.watchers)
                    .any(|o| o.id == p.id)
            });
            ThreadUsersSummary {
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

/// Who a push about `thread_id`, or any thread in its family, reaches.
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

/// Who a notification reaches, from the thread it opens: its tap's thread
/// target, else the thread of the event it links. One that opens no thread
/// reaches the whole team, and so does every notification on a gateway with
/// fewer than two users, at no cost.
pub async fn audience_of_notification(
    pool: &PgPool,
    tap: &crate::scheduler::notifications::Tap,
    link_event_id: Option<Uuid>,
    snapshot: &UserSnapshot,
) -> Result<Audience, sqlx::Error> {
    use crate::scheduler::notifications::{NavigateTarget, Tap};
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

/// Record a role event on a root thread's family root.
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

/// A driver change one user made on a root thread another user drove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Takeover {
    pub from: UserRef,
    pub to: UserRef,
}

/// Answering a gate makes its answerer the driver: a device's holder, or the
/// user a trigger fire's actor email names. Nobody else becomes one, and
/// a driver answering again records nothing. Returns the takeover when
/// someone else drove before.
pub async fn take_on_answer(
    pool: &PgPool,
    bus: &EventBus,
    gate_thread_id: Uuid,
    answerer: &MessageOrigin,
    snapshot: &UserSnapshot,
) -> Result<Option<Takeover>, BoxError> {
    let Some(who) = snapshot.user_of_origin(answerer) else {
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
    pub driver: UserRef,
}

/// Open gates asked more than `hours` ago whose root thread has a driver and
/// that never escalated.
pub(crate) async fn overdue_gates(
    pool: &PgPool,
    hours: f64,
    snapshot: &UserSnapshot,
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
    let snapshot = crate::gateway_users::current();
    let gates = match overdue_gates(engine.pool(), hours, &snapshot).await {
        Ok(gates) => gates,
        Err(e) => {
            log!("[ThreadUsers] could not list overdue gates: {}", e);
            return;
        }
    };
    for gate in gates {
        if let Err(e) = escalate(engine, &gate).await {
            log!(
                "[ThreadUsers] could not escalate gate {} on {}: {}",
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
        None,
        None,
    )
    .await
}

/// What a root thread tells its users besides its gates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserNotice {
    /// `0` answered a gate on a root thread the recipient drove, so now drives it.
    GateAnswered(Takeover),
    /// `0` tapped Take on a root thread the recipient drove.
    TakenOver(Takeover),
    /// The root thread was archived, by `by` when a user did it.
    Completed { by: Option<UserRef> },
}

impl UserNotice {
    /// Who hears it: the displaced driver, or for a finished root thread its
    /// driver and watchers, except the user who finished it.
    pub fn recipients(&self, roles: &ThreadUsers) -> Vec<UserRef> {
        match self {
            Self::GateAnswered(t) | Self::TakenOver(t) => vec![t.from.clone()],
            Self::Completed { by } => roles
                .driver
                .iter()
                .chain(&roles.watchers)
                .filter(|o| by.as_ref().is_none_or(|b| b.id != o.id))
                .fold(Vec::new(), |mut users, o| {
                    if !users.iter().any(|p: &UserRef| p.id == o.id) {
                        users.push(o.clone());
                    }
                    users
                }),
        }
    }

    fn text(&self, title: &str) -> (String, String) {
        match self {
            Self::GateAnswered(t) => (
                format!("{} answered your gate", t.to.name),
                format!(
                    "{} answered a question on \"{title}\" and now drives it.",
                    t.to.name
                ),
            ),
            Self::TakenOver(t) => (
                format!("{} took over", t.to.name),
                format!(
                    "{} now drives \"{title}\". You can still answer its gates.",
                    t.to.name
                ),
            ),
            Self::Completed { by } => (
                "Thread done".to_string(),
                match by {
                    Some(user) => format!("{} archived \"{title}\".", user.name),
                    None => format!("\"{title}\" was archived."),
                },
            ),
        }
    }
}

/// Tell the users a notice names about the root thread `root`, on their
/// devices only. Nothing on a gateway with fewer than two users, and nothing
/// when they hold no device.
pub async fn notify(
    engine: &crate::api::SharedEngine,
    root: Uuid,
    notice: UserNotice,
    snapshot: &UserSnapshot,
) -> Result<(), BoxError> {
    if snapshot.user_count() < 2 {
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
    let thread_title: Option<String> =
        sqlx::query_scalar("SELECT title FROM thread_summaries WHERE thread_id = $1")
            .bind(root)
            .fetch_optional(engine.pool())
            .await?
            .flatten();
    let (title, message) = notice.text(thread_title.as_deref().unwrap_or("a thread"));
    announce(
        engine,
        &title,
        &message,
        root,
        None,
        Some(devices.into_iter().collect()),
        None,
        None,
    )
    .await
}

/// A user's message that mentions users, or that stays between users.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserOnlyMessageNotice {
    pub thread_id: Uuid,
    /// The recorded message, so a tap lands on it.
    pub event_id: Option<Uuid>,
    pub text: String,
    /// User ids, as the send checked them.
    pub mentions: Vec<String>,
    /// A *user-only message*, rather than one that woke the agent.
    pub user_only: bool,
    pub sender: Option<MessageOrigin>,
}

/// The user ids a user's message reaches: the users it mentions, and
/// for a user-only message also the root's driver and watchers. Never the
/// sender's own user, and each user once.
pub(crate) fn user_only_message_recipients(
    roles: &ThreadUsers,
    mentions: &[String],
    user_only: bool,
    sender: Option<&UserRef>,
) -> Vec<String> {
    let users = roles
        .driver
        .iter()
        .chain(&roles.watchers)
        .filter(|_| user_only)
        .map(|o| &o.id);
    let mut recipients: Vec<String> = Vec::new();
    for id in mentions.iter().chain(users) {
        if sender.is_none_or(|s| &s.id != id) && !recipients.contains(id) {
            recipients.push(id.clone());
        }
    }
    recipients
}

/// After a user's message: its mentioned users watch the root thread, and
/// one notification reaches who the message is for, on their devices only.
/// Recovery honors admitted messages even after team membership changes.
pub async fn on_user_only_message(
    engine: &crate::api::SharedEngine,
    notice: UserOnlyMessageNotice,
    snapshot: &UserSnapshot,
) -> Result<(), BoxError> {
    let pool = engine.pool();
    if let Some(id) = notice.event_id {
        let frozen: Option<sqlx::types::Json<MessagePlan>> = sqlx::query_scalar(
            "SELECT deliveries FROM user_only_message_notices WHERE event_id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?
        .flatten();
        if let Some(frozen) = frozen {
            return execute_message_plan(engine, &notice, frozen.0).await;
        }
    }
    let root = crate::engine::thread_family::family_root(pool, notice.thread_id).await?;
    let roles = roles(pool, root, snapshot).await?;
    let watches = notice
        .mentions
        .iter()
        .filter(|id| {
            snapshot.user_of_id(id).is_some()
                && !roles
                    .driver
                    .iter()
                    .chain(&roles.watchers)
                    .any(|role| &role.id == *id)
        })
        .cloned()
        .collect();
    let sender = notice
        .sender
        .as_ref()
        .and_then(|s| snapshot.user_of_origin(s));
    let recipients =
        user_only_message_recipients(&roles, &notice.mentions, notice.user_only, sender.as_ref());
    let thread_title: Option<String> =
        sqlx::query_scalar("SELECT title FROM thread_summaries WHERE thread_id = $1")
            .bind(notice.thread_id)
            .fetch_optional(pool)
            .await?
            .flatten();
    let who = sender.map_or_else(|| "Someone".to_string(), |s| s.name);
    let title = match thread_title {
        Some(thread) => format!("{who} in \"{thread}\""),
        None => who,
    };
    let mut deliveries = Vec::with_capacity(recipients.len());
    for recipient_id in recipients {
        let devices: BTreeSet<String> = snapshot
            .devices_of(&recipient_id)
            .into_iter()
            .map(str::to_string)
            .collect();
        if devices.is_empty() {
            continue;
        }
        let to_driver = notice.user_only
            && roles
                .driver
                .as_ref()
                .is_some_and(|driver| driver.id == recipient_id);
        deliveries.push(MessageDelivery {
            recipient_id,
            title: title.clone(),
            devices: devices.into_iter().collect(),
            to_driver,
        });
    }
    let plan = MessagePlan {
        root,
        watches,
        deliveries,
    };
    if let Some(id) = notice.event_id {
        sqlx::query("UPDATE user_only_message_notices SET deliveries = $2 WHERE event_id = $1")
            .bind(id)
            .bind(sqlx::types::Json(&plan))
            .execute(pool)
            .await?;
    }
    execute_message_plan(engine, &notice, plan).await
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MessagePlan {
    root: Uuid,
    watches: Vec<String>,
    deliveries: Vec<MessageDelivery>,
}

async fn execute_message_plan(
    engine: &crate::api::SharedEngine,
    notice: &UserOnlyMessageNotice,
    plan: MessagePlan,
) -> Result<(), BoxError> {
    apply_message_watches(
        engine.pool(),
        &engine.event_bus,
        notice,
        plan.root,
        &plan.watches,
    )
    .await?;
    deliver_message(engine, notice, plan.deliveries).await
}

async fn apply_message_watches(
    pool: &PgPool,
    bus: &EventBus,
    notice: &UserOnlyMessageNotice,
    root: Uuid,
    watches: &[String],
) -> Result<(), BoxError> {
    for user_id in watches {
        let event_id = notice
            .event_id
            .map(|id| Uuid::new_v5(&id, format!("watch:{user_id}").as_bytes()));
        if let Some(id) = event_id {
            let recorded: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM events WHERE id = $1)")
                    .bind(id)
                    .fetch_one(pool)
                    .await?;
            if recorded {
                continue;
            }
        }
        bus.emit(BusEvent::Thread {
            thread_id: root,
            event: ThreadEvent::WorkItemWatched {
                user_id: Some(user_id.clone()),
            },
            meta: EventMeta {
                event_id,
                ..EventMeta::with_actor(notice.sender.clone())
            },
        })
        .await?;
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MessageDelivery {
    recipient_id: String,
    title: String,
    devices: Vec<String>,
    to_driver: bool,
}

async fn deliver_message(
    engine: &crate::api::SharedEngine,
    notice: &UserOnlyMessageNotice,
    deliveries: Vec<MessageDelivery>,
) -> Result<(), BoxError> {
    let mut failure = None;
    for delivery in deliveries {
        if let Err(error) = announce(
            engine,
            &delivery.title,
            &notice.text,
            notice.thread_id,
            notice.event_id,
            Some(delivery.devices),
            notice.user_only.then_some(PushGroup::UserOnlyMessages {
                thread_id: notice.thread_id,
                to_driver: delivery.to_driver,
            }),
            notice.event_id.map(|id| {
                Uuid::new_v5(
                    &id,
                    format!("recipient:{}", delivery.recipient_id).as_bytes(),
                )
            }),
        )
        .await
        {
            failure = Some(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

/// Write a notification that opens `thread_id`, at `event_id` when given,
/// and push it. `audience` `None` lets the engine work it out from the root
/// thread.
#[allow(clippy::too_many_arguments)]
async fn announce(
    engine: &crate::api::SharedEngine,
    title: &str,
    message: &str,
    thread_id: Uuid,
    event_id: Option<Uuid>,
    audience: Option<Vec<String>>,
    push_group: Option<PushGroup>,
    durable_id: Option<Uuid>,
) -> Result<(), BoxError> {
    use crate::engine::event_bus::SystemEvent;
    use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};

    let id = durable_id.unwrap_or_else(Uuid::new_v4);
    let tap = Tap::Navigate {
        to: Box::new(NavigateUi {
            target: NavigateTarget::Thread,
            id: Some(thread_id.to_string()),
            event_id: event_id.map(|e| e.to_string()),
            ..Default::default()
        }),
    };
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM notifications WHERE id = $1)")
            .bind(id)
            .fetch_one(engine.pool())
            .await?;
    if !exists {
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
                push_group,
            }))
            .await?;
    }
    if durable_id.is_some() {
        sqlx::query("INSERT INTO user_only_message_dispatches (notification_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(id).execute(engine.pool()).await?;
        let completed: bool = sqlx::query_scalar(
            "SELECT completed FROM user_only_message_dispatches WHERE notification_id = $1",
        )
        .bind(id)
        .fetch_one(engine.pool())
        .await?;
        if completed {
            return Ok(());
        }
        crate::scheduler::push::send_user_only_push(
            engine, title, message, id, thread_id, event_id, tap,
        )
        .await?;
        sqlx::query(
            "UPDATE user_only_message_dispatches SET completed = true WHERE notification_id = $1",
        )
        .bind(id)
        .execute(engine.pool())
        .await?;
        return Ok(());
    }
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
#[path = "thread_users_tests.rs"]
mod tests;
