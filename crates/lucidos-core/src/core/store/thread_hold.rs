//! The *hold*, and the one reader that answers "may Lucidos start a turn
//! here by itself?".
//!
//! A hold lives only in events: the last of `ThreadHoldPlaced`,
//! `ThreadHoldLifted` and the retired `AutomationPaused` / `AutomationUnpaused`
//! on the thread wins. A retired pause folds as a hold rooted on its own
//! thread, whose id is the pause event's id.

use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::thread_events::MessageOrigin;

/// A hold standing on a thread.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadHold {
    pub reason: String,
    /// Shared by every thread one Hold reached.
    pub hold_id: Uuid,
    /// The thread the Hold was placed on.
    pub root_thread_id: Uuid,
    /// The sequence of the event that placed it on this thread.
    pub placed_sequence: i64,
    pub placed_by: Option<MessageOrigin>,
}

/// The four event types the hold folds from, newest wins.
pub const HOLD_EVENT_TYPES: [&str; 4] = [
    "ThreadHoldPlaced",
    "ThreadHoldLifted",
    "AutomationPaused",
    "AutomationUnpaused",
];

/// The hold as the drawer and the composer strip read it, from
/// `thread_summaries.hold`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ThreadHoldSummary {
    pub reason: String,
    /// The thread the Hold was placed on. Only that thread offers Resume.
    pub root_thread_id: String,
    pub placed_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placed_by: Option<MessageOrigin>,
}

/// The hold standing on `thread_id`, if any.
pub async fn thread_hold<'e>(
    pool: impl sqlx::PgExecutor<'e>,
    thread_id: Uuid,
) -> Result<Option<ThreadHold>, sqlx::Error> {
    type Row = (
        Uuid,
        String,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<serde_json::Value>,
    );
    let last: Option<Row> = sqlx::query_as(
        "SELECT id, event_type, sequence, payload->>'reason', payload->>'hold_id', \
                payload->>'root_thread_id', payload->'actor' \
         FROM events WHERE thread_id = $1 AND event_type = ANY($2) \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(&HOLD_EVENT_TYPES[..])
    .fetch_optional(pool)
    .await?;
    let Some((id, event_type, sequence, reason, hold_id, root, actor)) = last else {
        return Ok(None);
    };
    let parse = |raw: Option<String>| raw.and_then(|raw| Uuid::parse_str(&raw).ok());
    let (hold_id, root_thread_id) = match event_type.as_str() {
        "ThreadHoldPlaced" => (
            parse(hold_id).unwrap_or(id),
            parse(root).unwrap_or(thread_id),
        ),
        "AutomationPaused" => (id, thread_id),
        _ => return Ok(None),
    };
    Ok(Some(ThreadHold {
        reason: reason.unwrap_or_default(),
        hold_id,
        root_thread_id,
        placed_sequence: sequence,
        placed_by: actor.and_then(|actor| serde_json::from_value(actor).ok()),
    }))
}

/// What keeps Lucidos from starting a turn on a thread by itself.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnHold {
    /// The owner's Stop (ADR 0696).
    Stop,
    /// A card is open on the thread, and no turn starts until it closes
    /// (ADR 0803). Its close releases the wakes it held.
    OpenCard { card_id: String },
    /// The thread is on hold until Resume.
    Hold(Box<ThreadHold>),
}

/// The hold that takes precedence on `thread_id`, if any.
pub async fn turn_hold(pool: &PgPool, thread_id: Uuid) -> Result<Option<TurnHold>, sqlx::Error> {
    let mut connection = pool.acquire().await?;
    if super::thread_stop::stopped_before(&mut connection, thread_id, i64::MAX).await? {
        return Ok(Some(TurnHold::Stop));
    }
    if let Some(card) =
        crate::engine::question_park_sql::open_card(&mut connection, thread_id).await?
    {
        return Ok(Some(TurnHold::OpenCard { card_id: card.id }));
    }
    Ok(thread_hold(&mut *connection, thread_id)
        .await?
        .map(|hold| TurnHold::Hold(Box::new(hold))))
}

/// Whether Lucidos may start a turn on `thread_id` by itself: a report
/// wake, an event-wait delivery, a turn-end nudge, a handoff, a standing
/// apply or a boot recovery. An unreadable state holds: a delayed wake is
/// recoverable, an unwanted turn is not.
pub async fn may_start_turn(pool: &PgPool, thread_id: Uuid) -> bool {
    match turn_hold(pool, thread_id).await {
        Ok(hold) => hold.is_none(),
        Err(e) => {
            crate::log_legacy!("[TurnHold] State of {thread_id} unreadable, holding: {e}");
            false
        }
    }
}

/// Whether `thread_id` is on hold. An unreadable state counts as held.
pub async fn thread_is_held(pool: &PgPool, thread_id: Uuid) -> bool {
    match thread_hold(pool, thread_id).await {
        Ok(hold) => hold.is_some(),
        Err(e) => {
            crate::log_legacy!("[TurnHold] Hold of {thread_id} unreadable, holding: {e}");
            true
        }
    }
}
