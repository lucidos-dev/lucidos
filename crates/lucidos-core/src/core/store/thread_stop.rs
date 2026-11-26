use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

/// INVARIANT: Unknown state holds: a delayed wake is recoverable, an unwanted turn is not.
pub async fn thread_is_stopped(pool: &PgPool, thread_id: Uuid) -> bool {
    let stopped = match pool.acquire().await {
        Ok(mut connection) => stopped_before(&mut connection, thread_id, i64::MAX).await,
        Err(e) => Err(e),
    };
    match stopped {
        Ok(stopped) => stopped,
        Err(e) => {
            crate::log_legacy!(
                "[ThreadStop] State of {thread_id} unreadable, holding automatic work: {e}"
            );
            true
        }
    }
}

pub async fn stopped_before(
    connection: &mut PgConnection,
    thread_id: Uuid,
    before: i64,
) -> Result<bool, sqlx::Error> {
    Ok(
        Transitions::before(connection, thread_id, before, &TRANSITION_EVENTS)
            .next()
            .await?
            .is_some_and(|(_, stopped)| stopped),
    )
}

/// INVARIANT: Every event type [`transition`] can read as a Stop or a resume.
const TRANSITION_EVENTS: [&str; 7] = [
    "ResponseCanceled",
    "MessageReceived",
    "ContinuationRequested",
    "ContinuationStarted",
    "UserQuestionAnswered",
    "ApplyNowStarted",
    "ChangeApplied",
];

const STOP_EVENTS: [&str; 1] = ["ResponseCanceled"];

struct Transitions<'a> {
    connection: &'a mut PgConnection,
    thread_id: Uuid,
    event_types: &'a [&'a str],
    before: i64,
    page: std::vec::IntoIter<(i64, bool)>,
    exhausted: bool,
}

impl<'a> Transitions<'a> {
    const PAGE: i64 = 32;

    fn before(
        connection: &'a mut PgConnection,
        thread_id: Uuid,
        before: i64,
        event_types: &'a [&'a str],
    ) -> Self {
        Self {
            connection,
            thread_id,
            event_types,
            before,
            page: Vec::new().into_iter(),
            exhausted: false,
        }
    }

    async fn next(&mut self) -> Result<Option<(i64, bool)>, sqlx::Error> {
        loop {
            if let Some(transition) = self.page.next() {
                return Ok(Some(transition));
            }
            if self.exhausted {
                return Ok(None);
            }
            // INVARIANT: Only the fields `transition` reads, never a message's text. A
            // message the user took back never ran, so it resumes nothing.
            let rows: Vec<(i64, String, serde_json::Value)> = sqlx::query_as(
                "SELECT e.sequence, e.event_type, jsonb_build_object( \
                   'actor', e.payload->'actor', 'cause', e.payload->'cause', \
                   'origin', e.payload->'origin', 'mode', e.payload->'mode', \
                   'device_id', e.payload->'device_id') \
                 FROM events e WHERE e.thread_id = $1 AND e.event_type = ANY($2) \
                   AND e.sequence < $3 \
                   AND NOT (e.event_type = 'MessageReceived' AND EXISTS ( \
                     SELECT 1 FROM events r WHERE r.thread_id = $1 \
                       AND r.event_type = 'QueuedMessageRemoved' \
                       AND r.payload->>'removed_message_id' = e.id::text)) \
                 ORDER BY e.sequence DESC LIMIT $4",
            )
            .bind(self.thread_id)
            .bind(self.event_types)
            .bind(self.before)
            .bind(Self::PAGE)
            .fetch_all(&mut *self.connection)
            .await?;
            self.exhausted = rows.len() < Self::PAGE as usize;
            if let Some((oldest, _, _)) = rows.last() {
                self.before = *oldest;
            }
            self.page = rows
                .into_iter()
                .filter_map(|(sequence, kind, payload)| {
                    transition(&kind, &payload).map(|stopped| (sequence, stopped))
                })
                .collect::<Vec<_>>()
                .into_iter();
        }
    }
}

pub async fn lifted_stop(
    pool: &PgPool,
    thread_id: Uuid,
    before: i64,
) -> Result<Option<i64>, sqlx::Error> {
    let mut connection = pool.acquire().await?;
    let mut walk = Transitions::before(&mut connection, thread_id, before, &TRANSITION_EVENTS);
    let mut first_stop = None;
    while let Some((sequence, stopped)) = walk.next().await? {
        if !stopped {
            break;
        }
        first_stop = Some(sequence);
    }
    Ok(first_stop)
}

pub async fn latest_stop_run(pool: &PgPool, thread_id: Uuid) -> Result<Option<i64>, sqlx::Error> {
    let latest_stop = Transitions::before(
        &mut *pool.acquire().await?,
        thread_id,
        i64::MAX,
        &STOP_EVENTS,
    )
    .next()
    .await?;
    match latest_stop {
        Some((latest_stop, _)) => lifted_stop(pool, thread_id, latest_stop + 1).await,
        None => Ok(None),
    }
}

/// INVARIANT: A transition is deliberately narrower than Human mode: an agent submitting
/// a saved draft is human content, but cannot lift the owner's Stop.
pub fn transition(event: &str, payload: &serde_json::Value) -> Option<bool> {
    let device_actor = payload["actor"]["kind"] == "device";
    match event {
        "ResponseCanceled" if device_actor && payload["cause"] == "user_stop" => Some(true),
        "MessageReceived"
            if payload["origin"]["kind"] == "device"
                || (payload["mode"] == "human" && payload["device_id"].is_string()) =>
        {
            Some(false)
        }
        "ContinuationRequested"
        | "ContinuationStarted"
        | "UserQuestionAnswered"
        | "ApplyNowStarted"
        | "ChangeApplied"
            if device_actor =>
        {
            Some(false)
        }
        _ => None,
    }
}
