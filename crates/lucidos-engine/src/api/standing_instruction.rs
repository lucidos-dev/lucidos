//! Does this caller carry the workspace owner's standing instruction?
//!
//! A thread acts inside its own subtree on its own authority. Anything wider
//! is the owner's button, and only a standing instruction lets a thread press
//! one (ADR 0168 clauses 3 to 5). `api::thread_reach` answers which threads a
//! caller may aim at. This answers on whose behalf it acts.
//!
//! **One definition, asked by every clause-4 verb.** A second spelling is how
//! four routes drifted apart, so the verbs call the function below rather than
//! re-deriving it.
//!
//! Both inputs are prefix fields of the *thread-bound origin token*, under its
//! MAC (`api::actor`), so neither is a claim a caller can edit. The turn-start
//! set is `agent_recovery::THREAD_START_EVENTS_SQL`, shared with the recovery
//! gates: "the owner opened this turn" must mean one thing.
//!
//! **Nothing here can be self-granted.** An agent creates triggers freely, and
//! an unchecked fire would promote its own thread to the owner's authority in
//! two tool calls. So a fire is weighed by who authorized the trigger.

use sqlx::PgPool;
use uuid::Uuid;

/// Does this caller carry the owner's standing instruction? Two shapes, and
/// no third.
///
/// **A turn the owner opened.** Their words in that turn are the press, and
/// the thread's newest turn-start event carries a `Device` actor.
///
/// **A trigger firing the owner authorized.** The same decision, made in
/// advance. A fire reaches the engine two ways, so the shape has two records.
/// An intent trigger runs on a thread whose turn starts with `TriggerStarted`.
/// A script trigger has no thread, and its subprocess carries the fire's id on
/// its own token instead. Either way the trigger's own provenance decides, per
/// [`owner_authorized_trigger`].
///
/// Nothing is inherited. A thread spawned by one carrying the instruction
/// opens its own turn with a `ThreadLink` actor, so it carries none.
///
/// **An unanswered probe is a no.** A read error, a thread that never started
/// a turn, and a caller with neither input all answer false. Apply is not
/// recoverable, so an unknown must not stand in for the owner.
pub(crate) async fn carries_standing_instruction(
    pool: &PgPool,
    caller_thread_id: Option<Uuid>,
    emitting_trigger_id: Option<&str>,
) -> bool {
    // The subprocess IS the fire, and its token says which one under the MAC.
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

/// What opened this thread's current turn, as far as the standing instruction
/// is concerned.
enum TurnStart {
    /// A `Device` actor: the workspace owner at one of their own clients.
    OwnerOpened,
    /// A trigger fired. Whose trigger is a separate question.
    TriggerFire { trigger_id: String },
    /// An agent, another thread, or the engine itself.
    SomebodyElse,
}

/// Read the thread's newest turn-start event. `None` when the thread has never
/// started a turn, or the read failed.
async fn newest_turn_start(pool: &PgPool, thread_id: Uuid) -> Option<TurnStart> {
    // `origin` before `actor` because a turn-start event naming a person
    // carries the structured origin: `MessageReceived` stamps it at the chat
    // boundary. `actor` is the `EventMeta` slot, for a start event that records
    // its initiator there instead. Both hold a `MessageOrigin`, so `kind` means
    // the same in either.
    //
    // `task_id` is `TriggerStarted`'s legacy spelling of `trigger_id`, aliased
    // on the enum and therefore still live in old rows.
    let sql = format!(
        "SELECT event_type, \
                COALESCE(payload->'origin', payload->'actor')->>'kind' = 'device', \
                COALESCE(payload->>'trigger_id', payload->>'task_id') \
         FROM events \
         WHERE aggregate_id = $1 AND event_type IN ({starts}) \
         ORDER BY sequence DESC LIMIT 1",
        starts = crate::engine::agent_recovery::THREAD_START_EVENTS_SQL,
    );
    let row: Option<(String, Option<bool>, Option<String>)> = sqlx::query_as(&sql)
        .bind(thread_id.to_string())
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| {
            crate::log!(
                "[StandingInstruction] Could not read the turn start of thread {}: {}; \
                 treating it as carrying none",
                thread_id,
                e
            );
            None
        });
    let (event_type, by_device, trigger_id) = row?;
    if event_type == "TriggerStarted" {
        // A fire with no trigger id names nothing to check, so it is nobody's.
        return Some(match trigger_id {
            Some(trigger_id) => TurnStart::TriggerFire { trigger_id },
            None => TurnStart::SomebodyElse,
        });
    }
    Some(match by_device {
        Some(true) => TurnStart::OwnerOpened,
        _ => TurnStart::SomebodyElse,
    })
}

/// Did the workspace owner authorize this trigger, so that a firing of it is
/// their decision made in advance?
///
/// ADR 0168 puts it as "the owner wrote it and switched it on", and names both
/// halves as checkable. So the newest of the trigger's authoring events must
/// carry a `Device` actor: `TriggerCreated` is writing it, `TriggerEnabled` is
/// switching it on, and `TriggerUpdated` is rewriting what it does.
///
/// **Newest, not any.** Without this the gate is a two-step self-promotion: an
/// agent creates a trigger, fires it, and its thread now spends the owner's
/// authority. An agent that rewrites an owner's trigger takes it back off.
///
/// A trigger nobody can show the owner authored answers false. That is the
/// fail-closed direction, and it costs a legacy trigger only its reach OUTSIDE
/// its own subtree, which clause 3 never needed.
async fn owner_authorized_trigger(pool: &PgPool, trigger_id: &str) -> bool {
    let authored = sqlx::query_scalar::<_, Option<bool>>(
        "SELECT payload->'actor'->>'kind' = 'device' \
         FROM events \
         WHERE aggregate_id = $1 \
           AND event_type IN ('TriggerCreated','TriggerUpdated','TriggerEnabled') \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(trigger_id)
    .fetch_optional(pool)
    .await;
    match authored {
        Ok(row) => row.flatten().unwrap_or(false),
        Err(e) => {
            crate::log!(
                "[StandingInstruction] Could not read who authored trigger {}: {}; \
                 treating its fire as carrying none",
                trigger_id,
                e
            );
            false
        }
    }
}

#[cfg(test)]
#[path = "standing_instruction_tests.rs"]
mod tests;
