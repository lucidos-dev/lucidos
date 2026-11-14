//! The permission request an *unattested caller*'s owner-only act parks on.
//!
//! The engine words the card. It offers Allow once and Don't allow, and only
//! a tap from the owner's device grants. The grant's breadth is a fresh id, so
//! it covers that one act. `api::owner_door` parks a request and replays it on
//! the grant. `agent_session::spawn_gate` parks a coding-agent run and starts
//! it on the grant. Either way the act runs once: `PermissionSpent` lands
//! before it, and a unique index refuses a second.

use uuid::Uuid;

use crate::api::permission_requests::one_line;
use crate::engine::event_bus::EventBus;
use crate::engine::permission::menu::{self, DONT_ALLOW_LABEL};
use crate::engine::permission::{self, Action, PermissionRow, RequestCard};
use crate::engine::thread_events::{
    CancelCause, EventChannel, EventMeta, MessageOrigin, ThreadEvent, UnattestedAct,
    UnattestedRequest,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Where a card's parked act sits in its `UserQuestionAsked` payload. A card
/// raised before permission requests kept it at the top level. Such a card
/// still parks a start, and its Don't allow still closes it.
const ACT_SQL: &str = "COALESCE(q.payload->'permission_request'->'unattested_act', \
                       q.payload->'unattested_act')";

/// The card's question. `act` is the engine's own wording of the act. The
/// caller's claims and the request path are escaped, since the caller wrote
/// them.
pub(crate) fn card_question(act: &str, unattested: &UnattestedAct) -> String {
    let claims = match &unattested.caller {
        MessageOrigin::UnattestedCaller {
            claimed_device_id,
            claimed_local_token,
            claimed_workspace,
            ..
        } => {
            let mut claims = Vec::new();
            if let Some(device) = claimed_device_id {
                claims.push(format!(
                    "named the device \u{201c}{}\u{201d} without the gateway's stamp",
                    one_line(device)
                ));
            }
            if let Some(workspace) = claimed_workspace {
                claims.push(format!(
                    "named the workspace \u{201c}{}\u{201d} without the gateway's stamp",
                    one_line(workspace)
                ));
            }
            if *claimed_local_token {
                claims.push("presented the machine-local token".to_string());
            }
            claims
        }
        MessageOrigin::Workspace { workspace, .. } => vec![format!(
            "came from the workspace \u{201c}{}\u{201d}, which could not verify who started \
             the thread that called",
            one_line(workspace)
        )],
        _ => Vec::new(),
    };
    let claimed = match claims.as_slice() {
        [] => String::new(),
        _ => format!(" It {}.", claims.join(" and ")),
    };
    let request = match &unattested.request {
        UnattestedRequest::Http { method, path, .. } => {
            format!("\n\nThe request: {} /api/v1{}", method, one_line(path))
        }
        UnattestedRequest::StartCodingAgent => String::new(),
    };
    format!(
        "**Let a caller Lucidos cannot verify do this, once?**\n\n\
         It asks to **{act}**. It came with no signed identity, so any process on \
         this machine could have sent it.{claimed}\n\n\
         Allow once does it one time. Allow it only if you asked for it yourself.\
         {request}"
    )
}

/// Raise the card on `thread_id` and return its id.
pub(crate) async fn raise_card(
    bus: &EventBus,
    thread_id: Uuid,
    question: String,
    unattested: UnattestedAct,
    channel: EventChannel,
) -> Result<String, BoxError> {
    let action = Action::ActForUnattestedCaller;
    let card = RequestCard {
        question,
        header: "Permission",
        action,
        breadth: Some(Uuid::new_v4().to_string()),
        options: menu::options(action, thread_id, "Nothing happens."),
        batch: None,
        unattested_act: Some(Box::new(unattested)),
    };
    permission::raise_request(bus, thread_id, card, channel).await
}

/// The act a card on `thread_id` parks, or `None` for any other card.
pub(crate) async fn card_act(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<UnattestedAct>, BoxError> {
    let act: Option<serde_json::Value> = sqlx::query_scalar(&format!(
        "SELECT {ACT_SQL} FROM events q \
         WHERE q.aggregate_id = $1 AND q.event_type = 'UserQuestionAsked' \
           AND q.payload->>'tool_use_id' = $2 AND {ACT_SQL} IS NOT NULL \
         ORDER BY q.sequence DESC LIMIT 1"
    ))
    .bind(thread_id.to_string())
    .bind(tool_use_id)
    .fetch_optional(pool)
    .await?;
    Ok(act.map(serde_json::from_value).transpose()?)
}

/// A live permission granted from an unattested caller's card, and its act.
#[derive(sqlx::FromRow)]
struct AllowedRow {
    #[sqlx(flatten)]
    permission: PermissionRow,
    act: serde_json::Value,
}

/// The newest live permission a card on `thread_id` granted, with the act it
/// lets run. `tool_use_id` narrows it to one card, and `start_only` to a
/// coding-agent start. A card raised before permission requests grants none.
pub(crate) async fn allowed_card(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: Option<&str>,
    start_only: bool,
) -> Result<Option<(PermissionRow, UnattestedAct)>, BoxError> {
    let row: Option<AllowedRow> = sqlx::query_as(&format!(
        "SELECT p.*, c.payload->'permission_request'->'unattested_act' AS act \
         FROM permissions p \
         JOIN events c ON c.thread_id = p.granted_in_thread_id \
              AND c.event_type = 'UserQuestionAsked' \
              AND c.payload->>'tool_use_id' = p.request_tool_use_id \
         WHERE {live} AND p.granted_in_thread_id = $1 AND p.action = $2 \
           AND ($3::text IS NULL OR p.request_tool_use_id = $3) \
           AND (NOT $4 OR c.payload->'permission_request'->'unattested_act' \
                          ->'request'->>'kind' = 'start_coding_agent') \
         ORDER BY p.granted_at DESC LIMIT 1",
        live = *permission::LIVE_SQL,
    ))
    .bind(thread_id)
    .bind(permission::action_wire(Action::ActForUnattestedCaller)?)
    .bind(tool_use_id)
    .bind(start_only)
    .fetch_optional(pool)
    .await?;
    row.map(|row| Ok((row.permission, serde_json::from_value(row.act)?)))
        .transpose()
}

/// The act the owner allowed on card `tool_use_id`, ready to run. A request
/// has spent its permission already. A start spends it when the resumed run
/// reaches the spawn gate. `None` when nothing is allowed or the spend failed.
pub(crate) async fn take_allowed_act(
    bus: &EventBus,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Option<UnattestedAct> {
    let (permission, act) =
        match allowed_card(bus.pool(), thread_id, Some(tool_use_id), false).await {
            Ok(Some(allowed)) => allowed,
            Ok(None) => return None,
            Err(e) => {
                crate::log!("[UnattestedAct] Could not read card {tool_use_id}: {e}");
                return None;
            }
        };
    match act.request {
        UnattestedRequest::StartCodingAgent => Some(act),
        UnattestedRequest::Http { .. } => permission::spend(bus, &permission).await.then_some(act),
    }
}

/// Is a coding-agent start card on `thread_id` still waiting for its answer?
pub(crate) async fn start_card_waiting(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM events q \
           WHERE q.aggregate_id = $1 AND q.event_type = 'UserQuestionAsked' \
             AND {ACT_SQL}->'request'->>'kind' = 'start_coding_agent' \
             AND NOT EXISTS ( \
                   SELECT 1 FROM events a WHERE a.aggregate_id = $1 \
                     AND a.event_type = 'UserQuestionAnswered' \
                     AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id'))"
    ))
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
}

/// Is `origin_id` the `ContinuationRequested` the owner's Allow of a start
/// emitted? Only that run may spend the Allow, so a later message from any
/// caller cannot ride it. An unreadable answer is no.
pub(crate) async fn origin_is_an_allowed_start(pool: &sqlx::PgPool, origin_id: Uuid) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events \
           WHERE id = $1 AND event_type = 'ContinuationRequested' \
             AND payload->>'reason' = $2)",
    )
    .bind(origin_id)
    .bind(crate::engine::agent_recovery::OWNER_ALLOWED_START_REASON)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!("[UnattestedAct] Could not read the origin of run {origin_id}: {e}");
        false
    })
}

/// Close the turns a coding-agent start card parked, when its answer was Don't
/// allow. Each start since the thread's last terminator ends `refused`, so no
/// later Allow carries it. Any other card or answer is left alone.
pub(crate) async fn close_refused_start(
    bus: &EventBus,
    thread_id: Uuid,
    tool_use_id: &str,
    option_id: &str,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    let refused: bool = sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM events q, jsonb_array_elements(q.payload->'options') o \
           WHERE q.aggregate_id = $1 AND q.event_type = 'UserQuestionAsked' \
             AND q.payload->>'tool_use_id' = $2 \
             AND {ACT_SQL}->'request'->>'kind' = 'start_coding_agent' \
             AND o->>'id' = $3 AND o->>'label' = $4)"
    ))
    .bind(thread_id.to_string())
    .bind(tool_use_id)
    .bind(option_id)
    .bind(DONT_ALLOW_LABEL)
    .fetch_one(bus.pool())
    .await?;
    if !refused {
        return Ok(());
    }
    let parked: Vec<Uuid> = sqlx::query_scalar(
        "SELECT e.id FROM events e \
          WHERE e.aggregate_id = $1 \
            AND e.event_type IN ('MessageReceived', 'ContinuationRequested') \
            AND e.sequence > COALESCE((SELECT MAX(t.sequence) FROM events t \
                  WHERE t.aggregate_id = $1 AND t.event_type = ANY($2)), 0) \
          ORDER BY e.sequence",
    )
    .bind(thread_id.to_string())
    .bind(ThreadEvent::TERMINATOR_EVENT_TYPES)
    .fetch_all(bus.pool())
    .await?;
    for request_event_id in parked {
        crate::engine::thread_events::emit_response_canceled(
            bus,
            bus.pool(),
            thread_id,
            CancelCause::Refused,
            String::new(),
            Vec::new(),
            None,
            None,
            EventMeta {
                request_event_id: Some(request_event_id),
                channel: Some(EventChannel::ClaudeCode),
                actor: actor.clone(),
                ..EventMeta::NONE
            },
            "[UnattestedAct] ResponseCanceled (the owner refused the start)",
        )
        .await;
    }
    Ok(())
}

/// Spend the newest allowed coding-agent start on `thread_id`, if one waits.
pub(crate) async fn spend_allowed_start(bus: &EventBus, thread_id: Uuid) -> bool {
    match allowed_card(bus.pool(), thread_id, None, true).await {
        Ok(Some((permission, _))) => permission::spend(bus, &permission).await,
        Ok(None) => false,
        Err(e) => {
            crate::log!("[UnattestedAct] Could not read the cards of thread {thread_id}: {e}");
            false
        }
    }
}

#[cfg(test)]
#[path = "unattested_act_tests.rs"]
pub(crate) mod tests;
