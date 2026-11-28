use std::collections::{BTreeSet, HashSet};

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

pub use crate::engine::thread_events::queue::UserAddressing;

pub async fn route_user_only_message(
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

pub struct UserOnlyMessage {
    pub text: String,
    pub image_hashes: Vec<String>,
    pub device_id: Option<String>,
    pub mentions: Vec<String>,
    pub event_id: Option<Uuid>,
}

pub async fn post_user_only_message(
    bus: &EventBus,
    thread_id: Uuid,
    message: UserOnlyMessage,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    match bus
        .emit(BusEvent::Thread {
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
    {
        Ok(_) => Ok(()),
        // INVARIANT: A user Stop or take-back won the delivery claim. Nothing is posted
        // and the accepted input finishes normally instead of retrying.
        Err(error) if crate::engine::consumption_claim::is_removed_refusal(&*error) => Ok(()),
        Err(error) => Err(error),
    }
}

/// INVARIANT: Persist notice intent before recording the message. Recovery joins the
/// recorded message, so an abandoned send can never notify anyone.
pub(crate) async fn queue_user_notice(
    engine: &crate::engine::types::SharedEngine,
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

#[cfg(test)]
use crate::core::store::thread_audience::{audience, fold, RoleEvent};
pub use crate::core::store::thread_audience::{roles, Audience, ThreadUsers};
use crate::core::store::thread_audience::{roles_of_many, roots_with_open_escalated_gate};

pub const LIST_LIMIT: i64 = 200;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThreadUsersSummary {
    pub root_thread_id: Uuid,
    pub title: Option<String>,
    pub last_activity: chrono::DateTime<chrono::Utc>,
    #[serde(flatten)]
    pub roles: ThreadUsers,
    pub gate_open: bool,
    pub escalated: bool,
    pub needs_team: bool,
    pub yours: bool,
}

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
         WHERE status = 'waiting_for_user_answer' AND hold IS NULL \
           AND archive_state <> 'archived'",
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

pub use crate::core::store::thread_audience::{audience_of_notification, audience_of_thread};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Takeover {
    pub from: UserRef,
    pub to: UserRef,
}

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
        ThreadEvent::RootThreadDriverTaken,
        Some(answerer.clone()),
    )
    .await?;
    Ok(current.driver.map(|from| Takeover { from, to: who }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OverdueGate {
    pub root: Uuid,
    pub gate_thread_id: Uuid,
    pub tool_use_id: String,
    pub question_event_id: Uuid,
    pub driver: UserRef,
}

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
               WHERE s.thread_id = q.thread_id AND s.status = 'waiting_for_user_answer' \
                 AND s.hold IS NULL) \
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
                 AND event_type IN ('RootThreadGateEscalated', 'WorkItemGateEscalated') \
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

pub const GATE_ESCALATION_CRON: &str = "0 */5 * * * *";

pub async fn escalate_overdue_gates(engine: &crate::engine::types::SharedEngine) {
    use crate::core::preference_catalog as prefs;
    let hours = prefs::DRIVER_TIMEOUT_HOURS.read(engine.pool()).await;
    if hours <= 0.0 {
        return;
    }
    let snapshot = crate::gateway_users::current();
    let gates = match overdue_gates(engine.pool(), hours, &snapshot).await {
        Ok(gates) => gates,
        Err(e) => {
            log_legacy!("[ThreadUsers] could not list overdue gates: {}", e);
            return;
        }
    };
    for gate in gates {
        if let Err(e) = escalate(engine, &gate).await {
            log_legacy!(
                "[ThreadUsers] could not escalate gate {} on {}: {}",
                gate.tool_use_id,
                gate.gate_thread_id,
                e
            );
        }
    }
}

/// INVARIANT: Record the escalation, then notify. The audience reads the escalation
/// back, so it is the whole team.
async fn escalate(
    engine: &crate::engine::types::SharedEngine,
    gate: &OverdueGate,
) -> Result<(), BoxError> {
    record(
        &engine.event_bus,
        gate.root,
        ThreadEvent::RootThreadGateEscalated {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserNotice {
    GateAnswered(Takeover),
    TakenOver(Takeover),
    Completed { by: Option<UserRef> },
}

impl UserNotice {
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

pub async fn notify(
    engine: &crate::engine::types::SharedEngine,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserOnlyMessageNotice {
    pub thread_id: Uuid,
    pub event_id: Option<Uuid>,
    pub text: String,
    pub mentions: Vec<String>,
    pub user_only: bool,
    pub sender: Option<MessageOrigin>,
}

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

/// INVARIANT: After a user's message: its mentioned users watch the root thread, and
/// one notification reaches who the message is for, on their devices only.
/// Recovery honors admitted messages even after team membership changes.
pub async fn on_user_only_message(
    engine: &crate::engine::types::SharedEngine,
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
    engine: &crate::engine::types::SharedEngine,
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
            event: ThreadEvent::RootThreadWatched {
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
    engine: &crate::engine::types::SharedEngine,
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

#[allow(clippy::too_many_arguments)]
async fn announce(
    engine: &crate::engine::types::SharedEngine,
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
