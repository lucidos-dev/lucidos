//! A *permission request* an agent asks on its own question tool.
//!
//! A question card belongs to the turn that asks it, so the card rides the
//! agent's blocking question tool. `lucidos ask-permission` records a
//! `PermissionRequestPrepared` and prints its id. The agent then asks its
//! question tool with that id as its only question, and [`prepared_ask`] swaps
//! in the engine's card. The tool result carries the owner's pick.

use uuid::Uuid;

use super::{PermissionRequest, RequestCard, CARD_ID_PREFIX};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Record `card` as a request prepared on `thread_id`, and return the id the
/// agent asks with.
pub(crate) async fn prepare_request(
    bus: &EventBus,
    thread_id: Uuid,
    card: RequestCard,
) -> Result<String, BoxError> {
    let request_id = format!("{CARD_ID_PREFIX}{}", Uuid::new_v4());
    let (words, mut request) = card.into_parts();
    request.prepared = Some(request_id.clone());
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::PermissionRequestPrepared {
            request_id: request_id.clone(),
            request,
            card: words,
        },
        meta: EventMeta::NONE,
    })
    .await?;
    Ok(request_id)
}

/// What a question batch an agent asked turns out to be.
#[derive(Debug)]
pub(crate) enum PreparedAsk {
    /// An ordinary question card.
    Ordinary,
    /// An ask naming a prepared request: the engine's card replaces it.
    Request {
        request: PermissionRequest,
        questions: serde_json::Value,
    },
}

/// Is this ask a prepared permission request? An agent asks one by putting
/// the id `lucidos ask-permission` printed as its only question. The request
/// must be prepared on this same thread, after the last snapshot restore.
///
/// `Err` carries the refusal the agent reads, and no card is raised.
pub(crate) async fn prepared_ask(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    questions: &serde_json::Value,
) -> Result<PreparedAsk, String> {
    let texts: Vec<String> = questions
        .as_array()
        .map(|qs| {
            qs.iter()
                .filter_map(crate::engine::agent_session::question_text)
                .collect()
        })
        .unwrap_or_default();
    let Some(request_id) = texts.iter().find(|t| t.starts_with(CARD_ID_PREFIX)) else {
        return Ok(PreparedAsk::Ordinary);
    };
    if texts.len() != 1 {
        return Err(
            "Ask a permission request on its own: one question, whose text is the \
                    request id and nothing else."
                .to_string(),
        );
    }
    let row: Option<(serde_json::Value, serde_json::Value)> = sqlx::query_as(concat!(
        "SELECT payload->'request', payload->'card' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'PermissionRequestPrepared' \
           AND payload->>'request_id' = $2 \
           AND sequence > ",
        crate::core::snapshot::boundary::restore_floor_sql!(),
        " ORDER BY sequence DESC LIMIT 1"
    ))
    .bind(thread_id.to_string())
    .bind(request_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("Could not read the permission request: {e}"))?;
    let Some((request, card)) = row else {
        return Err(format!(
            "No permission request {request_id} exists on this thread. Run \
             `lucidos ask-permission` from this thread first, then ask with the id it prints."
        ));
    };
    let unreadable = |e: serde_json::Error| format!("The permission request is unreadable: {e}");
    let request: PermissionRequest = serde_json::from_value(request).map_err(unreadable)?;
    let card: super::PreparedCard = serde_json::from_value(card).map_err(unreadable)?;
    // The walk parses these same words again, so its ids must be the ones
    // the request keys its choices by.
    let ids: std::collections::BTreeSet<String> =
        card.parsed_options().into_iter().map(|o| o.id).collect();
    if !ids.iter().eq(request.choices.keys()) {
        return Err("The permission request's options no longer match its card.".to_string());
    }
    Ok(PreparedAsk::Request {
        questions: card.questions(),
        request,
    })
}
