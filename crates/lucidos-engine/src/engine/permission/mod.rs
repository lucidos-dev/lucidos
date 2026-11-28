pub mod batch;
pub use crate::engine::thread_events::permission::device_session;
pub mod legacy;
pub mod menu;
pub mod prepare;
pub mod remote_start;
pub mod standing_deny;
pub mod tool_grants;
pub mod words;

pub use crate::engine::thread_events::permission::Action;

use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{AnswerKind, EventMeta, MessageOrigin, ThreadEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const SPENT_UNIQUE_INDEX: &str = "events_permission_spent_unique";

pub(crate) use crate::engine::thread_events::permission::engine_card_sql;
pub use crate::engine::thread_events::permission::{
    GrantBreadth, Holder, Lifetime, PermissionChoice, PermissionRequest, PickedBy,
};

pub(crate) use crate::engine::thread_events::permission::request::parsed_options;
pub use crate::engine::thread_events::permission::request::{
    inline_code, raise_request, raise_turn_holding_request, PermissionCardText, RequestCard,
    RequestOption, REASON_HEADING,
};
pub use crate::engine::thread_events::permission::{PreparedCard, PreparedOption};

pub async fn card_request(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<PermissionRequest>, BoxError> {
    let request: Option<serde_json::Value> = sqlx::query_scalar(&format!(
        "SELECT payload->'permission_request' FROM events \
         WHERE aggregate_id = $1 AND event_type IN ({}) \
           AND payload->>'tool_use_id' = $2 AND payload ? 'permission_request' \
         ORDER BY sequence DESC LIMIT 1",
        crate::engine::question_park_sql::CARD_EVENT_TYPES_SQL,
    ))
    .bind(thread_id.to_string())
    .bind(tool_use_id)
    .fetch_optional(pool)
    .await?;
    Ok(request.map(serde_json::from_value).transpose()?)
}

pub async fn tappable_request(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    action: Action,
) -> Result<Option<String>, BoxError> {
    use crate::engine::question_park_sql::{open_posted_card_sql, waiting_queued_card_sql};
    Ok(sqlx::query_scalar(&format!(
        "SELECT q.payload->>'tool_use_id' FROM events q \
         WHERE (({open}) OR (q.aggregate_id = $1 AND {queued})) \
           AND q.payload->'permission_request'->>'action' = $2 \
         ORDER BY q.sequence DESC LIMIT 1",
        open = open_posted_card_sql("q", "$1"),
        queued = waiting_queued_card_sql("q"),
    ))
    .bind(thread_id.to_string())
    .bind(action_wire(action)?)
    .fetch_optional(pool)
    .await?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Granted(Uuid),
    Refused,
    RefusedStanding,
    Ignored,
}

/// INVARIANT: Act on an answer the card already recorded. Only a device's pick of one of
/// the request's own options counts.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_answer(
    bus: &EventBus,
    thread_id: Uuid,
    tool_use_id: &str,
    request: &PermissionRequest,
    answer: &AnswerKind,
    actor: Option<&MessageOrigin>,
    picked_by: PickedBy,
) -> Result<Resolution, BoxError> {
    let AnswerKind::Selected { option_id } = answer else {
        return Ok(Resolution::Ignored);
    };
    if !matches!(actor, Some(MessageOrigin::Device { .. })) {
        crate::log_legacy!(
            "[Permission] Card {tool_use_id} on thread {thread_id} was answered by no device, \
             so it grants nothing"
        );
        return Ok(Resolution::Ignored);
    }
    let choice = request.choices.get(option_id);
    if matches!(picked_by, PickedBy::Batch { .. }) && !choice.is_some_and(|c| c.batch_may_pick()) {
        crate::log_legacy!(
            "[Permission] A batch picked option {option_id} of card {tool_use_id} on thread \
             {thread_id}, which only the owner's own tap may pick, so it grants nothing"
        );
        return Ok(Resolution::Ignored);
    }
    if request.lines.is_some() {
        return Ok(Resolution::Ignored);
    }
    match choice {
        Some(PermissionChoice::Grant {
            lifetime,
            holder,
            breadth,
        }) => {
            let permission_id = Uuid::new_v4();
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::PermissionGranted {
                    permission_id,
                    action: request.action,
                    breadth: match breadth {
                        GrantBreadth::Asked => request.breadth.clone(),
                        GrantBreadth::Any => {
                            if request.action == Action::GrantDeviceSession {
                                return Ok(Resolution::Ignored);
                            }
                            None
                        }
                        GrantBreadth::DeviceSession { .. } => {
                            if request.action != Action::GrantDeviceSession
                                || !breadth.matches_device_target(request.breadth.as_deref())
                            {
                                return Ok(Resolution::Ignored);
                            }
                            Some(
                                breadth
                                    .device_scope_key()
                                    .ok_or("invalid device session scope")?,
                            )
                        }
                    },
                    lifetime: *lifetime,
                    holder: holder.clone(),
                    request_tool_use_id: tool_use_id.to_string(),
                    picked_by,
                    pattern: None,
                },
                meta: EventMeta::with_actor(actor.cloned()),
            })
            .await?;
            Ok(Resolution::Granted(permission_id))
        }
        Some(PermissionChoice::Refuse) => Ok(Resolution::Refused),
        Some(PermissionChoice::RefuseStanding) => Ok(Resolution::RefusedStanding),
        Some(PermissionChoice::Revoke | PermissionChoice::RetiredUnpause) | None => {
            Ok(Resolution::Ignored)
        }
    }
}

pub(crate) use crate::engine::thread_events::permission::LIVE_SQL;
use crate::engine::thread_events::permission::ROW_COLUMNS;
pub use crate::engine::thread_events::permission::{
    live_permission, permission_row, PermissionRow, PermissionUser,
};

/// INVARIANT: Use `permission` for one action. A once permission records its spend, and
/// `false` means the spend did not land: a lost race or a failed write. An
/// unknown never allows the action.
pub async fn spend(bus: &EventBus, permission: &PermissionRow) -> bool {
    if permission.lifetime != Lifetime::Once.wire() {
        return true;
    }
    let Some(thread_id) = permission.granted_in_thread_id else {
        crate::log_legacy!(
            "[Permission] Permission {} has no thread to record its spend in",
            permission.permission_id
        );
        return false;
    };
    match bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::PermissionSpent {
                permission_id: permission.permission_id,
            },
            meta: EventMeta::NONE,
        })
        .await
    {
        Ok(_) => true,
        Err(e) => {
            let why = if crate::engine::event_bus::violates_unique_index(&*e, SPENT_UNIQUE_INDEX) {
                "another action spent it first".to_string()
            } else {
                e.to_string()
            };
            crate::log_legacy!(
                "[Permission] Could not spend permission {}: {why}",
                permission.permission_id
            );
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeOutcome {
    Revoked,
    AlreadyEnded,
    NotFound,
}

pub async fn revoke(
    bus: &EventBus,
    permission_id: Uuid,
    actor: Option<MessageOrigin>,
) -> Result<RevokeOutcome, BoxError> {
    let Some(row) = permission_row(bus.pool(), permission_id).await? else {
        return Ok(RevokeOutcome::NotFound);
    };
    if row.ended_at.is_some() {
        return Ok(RevokeOutcome::AlreadyEnded);
    }
    let lane = serde_json::from_value(serde_json::Value::String(row.action.clone()))
        .ok()
        .and_then(tool_grants::lane_of);
    match (
        lane,
        row.pattern,
        row.holder_kind.as_str(),
        row.granted_in_thread_id,
    ) {
        (Some(lane), Some(pattern), "workspace", _) => {
            let by = tool_grants::ChangedBy {
                source: crate::core::grants::GrantSource::Revoke,
                thread_id: None,
                actor,
            };
            let lines = [pattern];
            tool_grants::change_standing(
                bus,
                lane,
                tool_grants::StandingChange::Remove(&lines),
                by,
            )
            .await?;
        }
        (_, _, _, granted_in) => {
            let thread_id = match granted_in {
                Some(thread_id) => thread_id,
                None => crate::engine::home_thread::ensure_home_thread(bus, bus.pool()).await?,
            };
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::PermissionRevoked { permission_id },
                meta: EventMeta::with_actor(actor),
            })
            .await?;
        }
    }
    Ok(RevokeOutcome::Revoked)
}

pub async fn list(pool: &sqlx::PgPool, live_only: bool) -> Result<Vec<PermissionRow>, sqlx::Error> {
    let rows: Vec<PermissionRow> = sqlx::query_as(&format!(
        "SELECT {ROW_COLUMNS} FROM permissions p \
         WHERE NOT $1 OR (p.pattern IS NOT NULL AND p.ended_at IS NULL) OR {live} \
         ORDER BY granted_at DESC",
        live = *LIVE_SQL,
    ))
    .bind(live_only)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(PermissionRow::with_unmapped_flag)
        .collect())
}

pub(crate) use crate::engine::thread_events::permission::action_wire;
pub use crate::engine::thread_events::permission::thread_user;

const TITLE_MAX_CHARS: usize = 120;

/// INVARIANT: A title on one line, capped, with its markdown escaped, so it cannot end
/// a card's bold sentence or start one of its own. Another thread may have
/// written it.
pub fn one_line(title: &str) -> String {
    let flat = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let capped = match flat.char_indices().nth(TITLE_MAX_CHARS) {
        Some((end, _)) => format!("{}\u{2026}", &flat[..end]),
        None => flat,
    };
    capped
        .chars()
        .flat_map(|c| {
            let escape = "\\`*_{}[]<>()#+-.!|~".contains(c);
            escape.then_some('\\').into_iter().chain(std::iter::once(c))
        })
        .collect()
}

#[cfg(test)]
#[path = "permission_tests.rs"]
mod tests;
