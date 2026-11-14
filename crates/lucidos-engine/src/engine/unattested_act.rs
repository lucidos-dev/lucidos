//! The owner approval card an *unattested caller*'s owner-only act parks on.
//!
//! The engine words the card, and only a stamped device's Allow once counts.
//! `api::owner_door` parks a request on one and replays it on Allow.
//! `agent_session::spawn_gate` parks a coding-agent run on one and starts the
//! run on Allow. Either way the act runs once: `UnattestedActSpent` lands
//! before it, and a unique index refuses a second.

use uuid::Uuid;

use crate::api::owner_approval::{one_line, ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    CancelCause, EventChannel, EventMeta, MessageOrigin, ThreadEvent, UnattestedAct,
    UnattestedRequest,
};

/// Every card id starts with this, so a log line names what raised it.
const CARD_ID_PREFIX: &str = "unattested-act:";

/// The unique index that makes a spend single-use.
const SPENT_UNIQUE_INDEX: &str = "events_unattested_act_spent_unique";

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
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let tool_use_id = format!("{CARD_ID_PREFIX}{}", Uuid::new_v4());
    let options = crate::engine::agent_session::parse_ask_user_question_inputs(
        &serde_json::json!({ "questions": [{
            "question": question,
            "header": "Approval",
            "multiSelect": false,
            "options": [
                { "label": ALLOW_ONCE_LABEL, "description": "Do this one act, one time." },
                { "label": DONT_ALLOW_LABEL, "description": "Nothing happens." },
            ],
        }] }),
    )
    .remove(0)
    .options;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.clone(),
            cc_session_id: String::new(),
            question,
            options,
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            unattested_act: Some(unattested),
            permission_request: None,
        },
        meta: EventMeta {
            channel: Some(channel),
            ..EventMeta::NONE
        },
    })
    .await?;
    Ok(tool_use_id)
}

/// The act a card on `thread_id` parks, or `None` for any other card.
pub(crate) async fn card_act(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<UnattestedAct>, Box<dyn std::error::Error + Send + Sync>> {
    let act: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'unattested_act' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'UserQuestionAsked' \
           AND payload->>'tool_use_id' = $2 AND payload ? 'unattested_act' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .bind(tool_use_id)
    .fetch_optional(pool)
    .await?;
    Ok(act.map(serde_json::from_value).transpose()?)
}

/// The newest card on `thread_id` that a device allowed once and nothing has
/// spent yet, with its act. `tool_use_id` narrows it to one card, and
/// `start_only` to a coding-agent start.
pub(crate) async fn allowed_card(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: Option<&str>,
    start_only: bool,
) -> Result<Option<(String, UnattestedAct)>, Box<dyn std::error::Error + Send + Sync>> {
    let row: Option<(String, serde_json::Value)> = sqlx::query_as(concat!(
        "SELECT q.payload->>'tool_use_id', q.payload->'unattested_act' \
         FROM events q \
         JOIN events a ON a.aggregate_id = q.aggregate_id \
              AND a.event_type = 'UserQuestionAnswered' \
              AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id' \
         WHERE q.aggregate_id = $1 AND q.event_type = 'UserQuestionAsked' \
           AND q.payload ? 'unattested_act' \
           AND ($2::text IS NULL OR q.payload->>'tool_use_id' = $2) \
           AND (NOT $3 OR q.payload->'unattested_act'->'request'->>'kind' \
                = 'start_coding_agent') \
           AND a.payload->'answer'->>'kind' = 'Selected' \
           AND a.payload->'actor'->>'kind' = 'device' \
           AND EXISTS ( \
                 SELECT 1 FROM jsonb_array_elements(q.payload->'options') o \
                 WHERE o->>'id' = a.payload->'answer'->>'option_id' \
                   AND o->>'label' = $4) \
           AND q.sequence > ",
        crate::core::snapshot::boundary::restore_floor_sql!(),
        " \
           AND NOT EXISTS ( \
                 SELECT 1 FROM events s WHERE s.aggregate_id = $1 \
                   AND s.event_type = 'UnattestedActSpent' \
                   AND s.payload->>'tool_use_id' = q.payload->>'tool_use_id') \
         ORDER BY a.sequence DESC LIMIT 1"
    ))
    .bind(thread_id.to_string())
    .bind(tool_use_id)
    .bind(start_only)
    .bind(ALLOW_ONCE_LABEL)
    .fetch_optional(pool)
    .await?;
    row.map(|(id, act)| Ok((id, serde_json::from_value(act)?)))
        .transpose()
}

/// Is a coding-agent start card on `thread_id` still waiting for its answer?
pub(crate) async fn start_card_waiting(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events q \
           WHERE q.aggregate_id = $1 AND q.event_type = 'UserQuestionAsked' \
             AND q.payload->'unattested_act'->'request'->>'kind' = 'start_coding_agent' \
             AND NOT EXISTS ( \
                   SELECT 1 FROM events a WHERE a.aggregate_id = $1 \
                     AND a.event_type = 'UserQuestionAnswered' \
                     AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id'))",
    )
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
}

/// Spend an allowed card. `true` means `UnattestedActSpent` landed and the act
/// may run. A lost race or a failed write is `false`: an unknown must not run
/// an owner-only act.
pub(crate) async fn spend(bus: &EventBus, thread_id: Uuid, tool_use_id: &str) -> bool {
    match bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::UnattestedActSpent {
                tool_use_id: tool_use_id.to_string(),
            },
            meta: EventMeta::NONE,
        })
        .await
    {
        Ok(_) => true,
        Err(e) => {
            let why = if crate::engine::event_bus::violates_unique_index(&*e, SPENT_UNIQUE_INDEX) {
                "another run spent it first".to_string()
            } else {
                e.to_string()
            };
            crate::log!(
                "[UnattestedAct] Could not spend card {tool_use_id} on thread {thread_id}: {why}"
            );
            false
        }
    }
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
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let refused: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events q, jsonb_array_elements(q.payload->'options') o \
           WHERE q.aggregate_id = $1 AND q.event_type = 'UserQuestionAsked' \
             AND q.payload->>'tool_use_id' = $2 \
             AND q.payload->'unattested_act'->'request'->>'kind' = 'start_coding_agent' \
             AND o->>'id' = $3 AND o->>'label' = $4)",
    )
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
        Ok(Some((tool_use_id, _))) => spend(bus, thread_id, &tool_use_id).await,
        Ok(None) => false,
        Err(e) => {
            crate::log!("[UnattestedAct] Could not read the cards of thread {thread_id}: {e}");
            false
        }
    }
}

#[cfg(test)]
#[path = "unattested_act_tests.rs"]
mod tests;
