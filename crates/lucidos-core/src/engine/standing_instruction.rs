use sqlx::PgPool;
use uuid::Uuid;

pub async fn carries_standing_instruction(
    pool: &PgPool,
    caller_thread_id: Option<Uuid>,
    emitting_trigger_id: Option<&str>,
) -> bool {
    // INVARIANT: The subprocess IS the fire, and its token says which one under the MAC.
    if let Some(trigger_id) = emitting_trigger_id {
        return owner_authorized_trigger(pool, trigger_id).await;
    }
    let Some(thread_id) = caller_thread_id else {
        return false;
    };
    match newest_turn_start(pool, thread_id).await {
        Some(TurnStart::OwnerOpened) => true,
        Some(TurnStart::TriggerFire { trigger_id }) => {
            owner_authorized_trigger(pool, &trigger_id).await
        }
        Some(TurnStart::SomebodyElse) | None => false,
    }
}

pub async fn owner_opened_current_turn(pool: &PgPool, thread_id: Uuid) -> bool {
    matches!(
        newest_turn_start(pool, thread_id).await,
        Some(TurnStart::OwnerOpened)
    )
}

enum TurnStart {
    OwnerOpened,
    TriggerFire { trigger_id: String },
    SomebodyElse,
}

pub(crate) fn opens_current_turn_sql() -> String {
    format!(
        "event_type IN ({starts}) \
         AND NOT (event_type IN ('ContinuationStarted', 'OrphanRecoveryStarted') \
              AND COALESCE(payload->'origin', payload->'actor')->>'kind' \
                  IS DISTINCT FROM 'device')",
        starts = crate::engine::switch_recovery_sql::THREAD_START_EVENTS_SQL,
    )
}

async fn newest_turn_start(pool: &PgPool, thread_id: Uuid) -> Option<TurnStart> {
    try_newest_turn_start(pool, thread_id)
        .await
        .unwrap_or_else(|e| {
            crate::log_legacy!("[StandingInstruction] Could not read the turn start of thread {}: {}; treating it as carrying none", thread_id, e);
            None
        })
        .map(|(_, start)| start)
}

/// INVARIANT: The event carrying the thread's standing instruction, if one holds. The
/// acceptance transaction rereads it under the caller append lock, so a
/// different turn cannot replace the instruction mid-send.
pub async fn standing_instruction_event(
    conn: &mut sqlx::PgConnection,
    thread_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let Some((event_id, start)) = try_newest_turn_start(&mut *conn, thread_id).await? else {
        return Ok(None);
    };
    let held = match start {
        TurnStart::OwnerOpened => true,
        TurnStart::TriggerFire { trigger_id } => {
            try_owner_authorized_trigger(&mut *conn, &trigger_id).await?
        }
        TurnStart::SomebodyElse => false,
    };
    Ok(held.then_some(event_id))
}

async fn try_newest_turn_start<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    thread_id: Uuid,
) -> Result<Option<(Uuid, TurnStart)>, sqlx::Error> {
    // INVARIANT: `task_id` is `TriggerStarted`'s legacy spelling of `trigger_id`, aliased
    // on the enum and therefore still live in old rows.
    let sql = format!(
        "SELECT id, event_type, \
                COALESCE(payload->'origin', payload->'actor')->>'kind' = 'device', \
                COALESCE(payload->>'trigger_id', payload->>'task_id') \
         FROM events \
         WHERE aggregate_id = $1 AND {opens} \
         ORDER BY sequence DESC LIMIT 1",
        opens = opens_current_turn_sql(),
    );
    let row: Option<(Uuid, String, Option<bool>, Option<String>)> = sqlx::query_as(&sql)
        .bind(thread_id.to_string())
        .fetch_optional(executor)
        .await?;
    let Some((event_id, event_type, by_device, trigger_id)) = row else {
        return Ok(None);
    };
    if event_type == "TriggerStarted" {
        return Ok(Some((
            event_id,
            match trigger_id {
                Some(trigger_id) => TurnStart::TriggerFire { trigger_id },
                None => TurnStart::SomebodyElse,
            },
        )));
    }
    Ok(Some((
        event_id,
        match by_device {
            Some(true) => TurnStart::OwnerOpened,
            _ => TurnStart::SomebodyElse,
        },
    )))
}

async fn owner_authorized_trigger(pool: &PgPool, trigger_id: &str) -> bool {
    match try_owner_authorized_trigger(pool, trigger_id).await {
        Ok(held) => held,
        Err(e) => {
            crate::log_legacy!("[StandingInstruction] Could not read who authored trigger {}: {}; treating its fire as carrying none", trigger_id, e);
            false
        }
    }
}

async fn try_owner_authorized_trigger<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    trigger_id: &str,
) -> Result<bool, sqlx::Error> {
    let authored = sqlx::query_scalar::<_, Option<bool>>(
        "SELECT payload->'actor'->>'kind' = 'device' \
         FROM events \
         WHERE aggregate_id = $1 \
           AND event_type IN ('TriggerCreated','TriggerUpdated','TriggerEnabled') \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(trigger_id)
    .fetch_optional(executor)
    .await?;
    Ok(authored.flatten().unwrap_or(false))
}

#[cfg(test)]
#[path = "standing_instruction_tests.rs"]
pub(crate) mod tests;
