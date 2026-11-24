//! Read the handoff that the next coding-agent session will pick up.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::store::thread_hold::{thread_hold, TurnHold};
use crate::core::store::thread_stop::stopped_before;

use super::handoff_sql::ends_handoff_sql;
use super::thread_events::{CeilingCrossing, CodingAgent};
use super::thread_events::{HandoffCause, HandoffOpening, HandoffRequest, SessionStart};

/// The *implementer* a coding-agent thread runs on. An unset model or effort
/// means the backend's own default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Implementer {
    pub coding_agent: CodingAgent,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

/// The first message of a *successor session*: its `opening`, then the user's
/// `request`, then the `input` that started the session when it was neither.
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

/// A `SessionHandedOff` that no `SessionStarted` has followed yet: the next
/// session of the thread is its *successor session*.
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
    /// Absent on a handoff recorded before the field: its `message` carries
    /// any request itself.
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
    /// Whether the engine starts its session by itself, by
    /// [`SessionStart::is_unprompted`].
    pub fn starts_unprompted(&self) -> bool {
        self.starts.is_unprompted()
    }

    /// The first message of its session, given the `input` that started it.
    pub fn first_message(&self, input: &str) -> String {
        successor_first_message(
            &self.message,
            self.request.as_ref().map(|r| r.message.as_str()),
            input,
        )
    }

    /// The continuity its session records on `SessionStarted`.
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

/// The handoff `thread_id`'s next session starts from, if one is pending.
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

/// Whether `thread_id`'s successor session waits to start. A failed read
/// counts as no: the turn end then sends the agent back as usual.
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

/// The handoff whose successor session the engine starts on `thread_id` by
/// itself, read once no run holds the thread. A closed lane thread starts no
/// engine handoff, since its batch already ended (ADR 0675). A handoff the
/// owner's Stop held waits for their next input. A user's request still
/// starts, because the user asked for it.
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

/// A successor session that would start on its own unless held.
pub struct SuccessorWake {
    pub handoff: PendingHandoff,
    pub hold: Option<TurnHold>,
}

/// Share the successor's admission verdict with liveness.
pub async fn successor_wake(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<SuccessorWake>, sqlx::Error> {
    let mut connection = pool.acquire().await?;
    let Some(wake) = successor_wake_on(&mut connection, thread_id).await? else {
        return Ok(None);
    };
    // Apply can lift the Stop while the handoff still waits for owner input.
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
    // A Stop that stood when the handoff landed leaves it for the next input.
    // A user's handoff request runs as theirs.
    if handoff.request.is_none() && stopped_before(connection, thread_id, handed_off).await? {
        return Ok(Some(SuccessorWake {
            handoff,
            hold: Some(TurnHold::Stop),
        }));
    }
    // An open card holds the successor until it closes (ADR 0803).
    if let Some(card) = crate::engine::question_park_sql::open_card(connection, thread_id).await? {
        return Ok(Some(SuccessorWake {
            handoff,
            hold: Some(TurnHold::OpenCard { card_id: card.id }),
        }));
    }
    // A standing hold holds the successor unless the owner started the turn
    // that handed off after it.
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
