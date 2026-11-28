//! Pre-arming report for addressed card waits. The source rows determine
//! whether a card is still open, and the current tree determines its audience.

use futures::TryStreamExt;
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use super::{live_wait_from_payload, LiveWait};
use crate::core::event_subscription::matchable_system_payload;
use crate::engine::event_bus::{SubThreadCardKind, SystemEvent};
use crate::engine::question_park_sql::{
    open_posted_card_sql, unresolved_permission_prompt_sql, PERMISSION_PROMPT_EVENT_TYPES,
};
use crate::engine::LucidosEngine;

const PAGE_SIZE: usize = 8;

#[derive(Debug, Serialize)]
pub struct PendingCardPage {
    pub wait_id: Uuid,
    pub cards: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_after: Option<i64>,
}

impl PendingCardPage {
    pub fn tool_text(&self) -> String {
        if self.cards.is_empty() {
            return "No matching sub-thread cards are still open in this report.".into();
        }
        let mut text = format!(
            "Matching sub-thread cards that were already open when this wait armed. Read or answer each card in its source thread.\n{}",
            serde_json::to_string(&self.cards).expect("JSON values serialize"),
        );
        if let Some(after) = self.next_after {
            text.push_str(&format!(
                "\nMore cards are available. Call list_event_waits with {{\"wait_id\":\"{}\",\"after\":{after}}} for the next page.",
                self.wait_id
            ));
        }
        text
    }
}

pub(super) async fn pending_cards(
    pool: &PgPool,
    wait: &LiveWait,
    after: i64,
) -> Result<PendingCardPage, sqlx::Error> {
    let event = SystemEvent::SUB_THREAD_CARD_OPENED;
    let aggregate = SystemEvent::SUB_THREAD_CARD_AGGREGATE;
    let permission_prompt_names = PERMISSION_PROMPT_EVENT_TYPES.map(|(request, _)| request);
    let query = format!(
        "WITH RECURSIVE subtree(thread_id, first_hop) AS (
             SELECT thread_id, thread_id FROM thread_summaries WHERE parent_thread_id = $1
             UNION
             SELECT child.thread_id, s.first_hop
             FROM subtree s JOIN thread_summaries child ON child.parent_thread_id = s.thread_id
             WHERE child.thread_id <> $1
         )
         SELECT s.thread_id, s.first_hop, c.id, c.sequence,
                (c.event_type <> 'UserQuestionAsked' OR c.payload ? 'permission_request')
         FROM subtree s JOIN events c ON c.aggregate = 'thread' AND c.aggregate_id = s.thread_id::text
         LEFT JOIN events relay ON relay.aggregate = '{aggregate}'
           AND relay.aggregate_id = $1::uuid::text AND relay.event_type = '{event}'
           AND relay.payload#>>'{{data,source_event_id}}' = c.id::text
         WHERE s.thread_id <> $1 AND c.sequence > $2 AND c.sequence <= $3
           AND COALESCE(relay.sequence, c.sequence) <= $3
           AND c.event_type = ANY($4)
           AND ({posted} OR {prompts})
           AND NOT EXISTS (
               SELECT 1 FROM events first_post
               WHERE first_post.aggregate = 'thread' AND first_post.aggregate_id = c.aggregate_id
                 AND first_post.sequence < c.sequence AND first_post.event_type = c.event_type
                 AND first_post.payload->>(CASE WHEN c.event_type = 'UserQuestionAsked'
                                                THEN 'tool_use_id' ELSE 'request_id' END)
                     = c.payload->>(CASE WHEN c.event_type = 'UserQuestionAsked'
                                         THEN 'tool_use_id' ELSE 'request_id' END)
           )
         ORDER BY c.sequence",
        posted = open_posted_card_sql("c", "s.thread_id::text"),
        prompts = unresolved_permission_prompt_sql("c"),
    );
    let names = [
        "UserQuestionAsked",
        permission_prompt_names[0],
        permission_prompt_names[1],
        permission_prompt_names[2],
    ];
    let mut rows = sqlx::query_as::<_, (Uuid, Uuid, Uuid, i64, bool)>(&query)
        .bind(wait.thread_id)
        .bind(after)
        .bind(wait.watermark)
        .bind(&names[..])
        .fetch(pool);
    let mut cards = Vec::new();
    let mut last = after;
    let mut next_after = None;
    while let Some((source, child, source_event_id, seq, permission)) = rows.try_next().await? {
        let payload = matchable_system_payload(&SystemEvent::SubThreadCardOpened {
            ancestor_thread_id: wait.thread_id,
            child_thread_id: child,
            source_thread_id: source,
            source_event_id,
            card_kind: if permission {
                SubThreadCardKind::PermissionRequest
            } else {
                SubThreadCardKind::Question
            },
        });
        if wait.matched_index(event, &payload).is_some() {
            if cards.len() == PAGE_SIZE {
                next_after = Some(last);
                break;
            }
            cards.push(payload);
            last = seq;
        }
    }

    Ok(PendingCardPage {
        wait_id: wait.wait_id,
        cards,
        next_after,
    })
}

impl LucidosEngine {
    /// The wait may have spent since the first page. Its persisted start still
    /// supplies the original conditions and watermark for subsequent pages.
    pub async fn pending_event_wait_cards(
        &self,
        thread_id: Uuid,
        wait_id: Uuid,
        after: i64,
    ) -> Result<Option<PendingCardPage>, sqlx::Error> {
        let row: Option<(Value, i64, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
            "SELECT payload, sequence, created FROM events
             WHERE aggregate = 'thread' AND aggregate_id = $1
               AND event_type = 'EventWaitStarted' AND payload->>'wait_id' = $2
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(thread_id.to_string())
        .bind(wait_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let Some((payload, sequence, created)) = row else {
            return Ok(None);
        };
        let wait = live_wait_from_payload(thread_id, &payload, sequence, created)
            .ok_or_else(|| sqlx::Error::Protocol("Could not read the saved subscription".into()))?;
        if !wait.watches(SystemEvent::SUB_THREAD_CARD_OPENED) {
            return Ok(None);
        }
        pending_cards(&self.pool, &wait, after).await.map(Some)
    }
}
