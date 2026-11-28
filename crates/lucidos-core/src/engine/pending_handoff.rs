use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::store::thread_hold::{thread_hold, TurnHold};
use crate::core::store::thread_stop::stopped_before;

use super::handoff_sql::ends_handoff_sql;
use super::thread_events::{CeilingCrossing, CodingAgent};
use super::thread_events::{HandoffCause, HandoffOpening, HandoffRequest, SessionStart};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Implementer {
    pub coding_agent: CodingAgent,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

pub fn successor_first_message(opening: &str, request: Option<&str>, input: &str) -> String {
    let mut message = opening.to_string();
    if let Some(request) = request {
        message.push_str(&format!(
            "\n\n---\nThe user's request for you:\n\n{request}"
        ));
    }
    let started_by_another = input.trim() != opening.trim()
        && request.is_none_or(|request| request.trim() != input.trim());
    if started_by_another {
        message.push_str(&format!(
            "\n\n---\nThe message that started this session:\n\n{input}"
        ));
    }
    message
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PendingHandoff {
    pub opening: HandoffOpening,
    pub message: String,
    #[serde(default)]
    pub checkpoint: Option<String>,
    #[serde(default)]
    pub progress: Option<String>,
    pub coding_agent: CodingAgent,
    #[serde(default)]
    pub coding_agent_model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub request: Option<HandoffRequest>,
    #[serde(default)]
    pub cause: Option<HandoffCause>,
    #[serde(default)]
    pub starts: SessionStart,
    #[serde(default)]
    pub crossing: Option<CeilingCrossing>,
}

impl PendingHandoff {
    pub fn starts_unprompted(&self) -> bool {
        self.starts.is_unprompted()
    }

    pub fn first_message(&self, input: &str) -> String {
        successor_first_message(
            &self.message,
            self.request.as_ref().map(|r| r.message.as_str()),
            input,
        )
    }

    pub fn continuity(&self) -> super::thread_events::SessionContinuity {
        super::thread_events::SessionContinuity::Handoff {
            checkpoint: self.checkpoint.clone(),
            progress: self.progress.clone(),
            crossing: self.crossing,
            opening: self.opening,
            message: self.message.clone(),
        }
    }

    pub fn implementer(&self) -> Implementer {
        Implementer {
            coding_agent: self.coding_agent,
            model: self.coding_agent_model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
        }
    }
}

pub async fn pending_handoff<'e>(
    pool: impl sqlx::PgExecutor<'e>,
    thread_id: Uuid,
) -> Result<Option<PendingHandoff>, sqlx::Error> {
    let payload: Option<serde_json::Value> = sqlx::query_scalar(&format!(
        concat!(
            "SELECT handoff.payload FROM events handoff \
         WHERE handoff.thread_id = $1 AND handoff.event_type = 'SessionHandedOff' \
           AND handoff.sequence > ",
            crate::core::snapshot::boundary::restore_floor_sql!(),
            " \
           AND NOT EXISTS (SELECT 1 FROM events handoff_end \
                 WHERE handoff_end.thread_id = $1 AND {}) \
         ORDER BY handoff.sequence DESC LIMIT 1"
        ),
        ends_handoff_sql("handoff_end", "handoff")
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let Some(payload) = payload else {
        return Ok(None);
    };
    match serde_json::from_value(payload) {
        Ok(handoff) => Ok(Some(handoff)),
        Err(e) => {
            crate::log_legacy!(
                "[Successor] unreadable SessionHandedOff on {}: {}",
                thread_id,
                e
            );
            Ok(None)
        }
    }
}

pub async fn handoff_pending(pool: &PgPool, thread_id: Uuid) -> bool {
    pending_handoff(pool, thread_id)
        .await
        .unwrap_or_else(|e| {
            crate::log_legacy!("[Successor] pending handoff of {}: {}", thread_id, e);
            None
        })
        .is_some()
}

pub const ALREADY_HANDED_OFF: &str = "the thread already continued in a legacy successor thread";

pub async fn successor_to_start(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<PendingHandoff>, sqlx::Error> {
    successor_to_start_on(&mut *pool.acquire().await?, thread_id).await
}

pub(crate) async fn successor_to_start_on(
    connection: &mut sqlx::PgConnection,
    thread_id: Uuid,
) -> Result<Option<PendingHandoff>, sqlx::Error> {
    Ok(successor_wake_on(connection, thread_id)
        .await?
        .filter(|wake| wake.hold.is_none())
        .map(|wake| wake.handoff))
}

pub struct SuccessorWake {
    pub handoff: PendingHandoff,
    pub hold: Option<TurnHold>,
}

pub async fn successor_wake(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<SuccessorWake>, sqlx::Error> {
    let mut connection = pool.acquire().await?;
    let Some(wake) = successor_wake_on(&mut connection, thread_id).await? else {
        return Ok(None);
    };
    // INVARIANT: Apply can lift the Stop while the handoff still waits for owner input.
    if matches!(wake.hold, Some(TurnHold::Stop))
        && !stopped_before(&mut connection, thread_id, i64::MAX).await?
    {
        return Ok(None);
    }
    Ok(Some(wake))
}

async fn successor_wake_on(
    connection: &mut sqlx::PgConnection,
    thread_id: Uuid,
) -> Result<Option<SuccessorWake>, sqlx::Error> {
    let Some(handoff) = pending_handoff(&mut *connection, thread_id)
        .await?
        .filter(PendingHandoff::starts_unprompted)
    else {
        return Ok(None);
    };
    let (blocked, handed_off): (bool, i64) = sqlx::query_as(
        "WITH latest AS (SELECT MAX(sequence) AS seq FROM events \
          WHERE thread_id = $1 AND event_type = 'SessionHandedOff') \
         SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1 \
          AND (status = 'waiting_for_user_answer' OR (status IN ('failed', 'paused') \
            AND EXISTS (SELECT 1 FROM events verdict WHERE verdict.thread_id = $1 \
              AND verdict.event_type IN ('ResponseFailed', 'ResponseAborted') \
              AND verdict.sequence > latest.seq)))), latest.seq FROM latest",
    )
    .bind(thread_id)
    .fetch_one(&mut *connection)
    .await?;
    if blocked {
        return Ok(None);
    }
    if handoff.request.is_none()
        && crate::engine::thread_events::landing_lane::lane_batch_ended(&mut *connection, thread_id)
            .await?
    {
        crate::log_legacy!(
            "[Successor] Thread {} is a closed lane thread and skips its stale handoff",
            thread_id
        );
        return Ok(None);
    }
    if handoff.request.is_none() && stopped_before(connection, thread_id, handed_off).await? {
        return Ok(Some(SuccessorWake {
            handoff,
            hold: Some(TurnHold::Stop),
        }));
    }
    if let Some(card) = crate::engine::question_park_sql::open_card(connection, thread_id).await? {
        return Ok(Some(SuccessorWake {
            handoff,
            hold: Some(TurnHold::OpenCard { card_id: card.id }),
        }));
    }
    let mut hold = None;
    if let Some(thread_hold) = thread_hold(&mut *connection, thread_id).await? {
        let owner_drove: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM events m WHERE m.thread_id = $1 \
             AND m.event_type = 'MessageReceived' \
             AND COALESCE(m.payload->>'mode', 'human') = 'human' \
             AND m.sequence > $2 AND m.sequence < $3)",
        )
        .bind(thread_id)
        .bind(thread_hold.placed_sequence)
        .bind(handed_off)
        .fetch_one(&mut *connection)
        .await?;
        if !owner_drove {
            hold = Some(TurnHold::Hold(Box::new(thread_hold)));
        }
    }
    Ok(Some(SuccessorWake { handoff, hold }))
}
