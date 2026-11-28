use uuid::Uuid;

use super::{PermissionRequest, RequestCard};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::permission::request::CARD_ID_PREFIX;
use crate::engine::thread_events::{EventMeta, ThreadEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub const ENGINE_CARD_REFUSAL: &str =
    "Lucidos already raised that card for the owner. End your turn and let the owner answer it.";

pub async fn prepare_request(
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

#[derive(Debug)]
pub enum PreparedAsk {
    Ordinary,
    Request {
        request: PermissionRequest,
        questions: serde_json::Value,
    },
}

pub fn names_permission_request(questions: &serde_json::Value) -> bool {
    question_texts(questions)
        .iter()
        .any(|t| t.starts_with(CARD_ID_PREFIX))
}

fn question_texts(questions: &serde_json::Value) -> Vec<String> {
    questions
        .as_array()
        .map(|qs| {
            qs.iter()
                .filter_map(crate::engine::question_parsing::question_text)
                .collect()
        })
        .unwrap_or_default()
}

/// INVARIANT: Is this ask a prepared permission request? An agent asks one by putting
/// the id `lucidos ask-permission` printed as its only question. The request
/// must be prepared on this same thread, after the last snapshot restore.
///
/// `Err` carries the refusal the agent reads, and no card is raised.
pub async fn prepared_ask(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    questions: &serde_json::Value,
) -> Result<PreparedAsk, String> {
    let texts = question_texts(questions);
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
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM events uqa WHERE ({open} OR (uqa.aggregate_id = $1 \
               AND {queued})) \
             AND uqa.payload->>'tool_use_id' = $2 \
             AND uqa.payload->'permission_request' IS NOT NULL)",
            open = crate::engine::question_park_sql::open_posted_card_sql("uqa", "$1"),
            queued = crate::engine::question_park_sql::waiting_queued_card_sql("uqa"),
        );
        let raised: bool = sqlx::query_scalar(&sql)
            .bind(thread_id.to_string())
            .bind(request_id)
            .fetch_one(pool)
            .await
            .map_err(|e| format!("Could not read the permission card: {e}"))?;
        if raised {
            return Err(ENGINE_CARD_REFUSAL.into());
        }
        return Err(format!(
            "No permission request {request_id} exists on this thread. Run \
             `lucidos ask-permission` or `lucidos permissions batch` from this thread first, \
             then ask with the id it prints."
        ));
    };
    let unreadable = |e: serde_json::Error| format!("The permission request is unreadable: {e}");
    let request: PermissionRequest = serde_json::from_value(request).map_err(unreadable)?;
    let card: super::PreparedCard = serde_json::from_value(card).map_err(unreadable)?;
    let ids: std::collections::BTreeSet<String> = super::parsed_options(&card)
        .into_iter()
        .map(|o| o.id)
        .collect();
    if !ids.iter().eq(request.choices.keys()) {
        return Err("The permission request's options no longer match its card.".to_string());
    }
    Ok(PreparedAsk::Request {
        questions: card.questions(),
        request,
    })
}
