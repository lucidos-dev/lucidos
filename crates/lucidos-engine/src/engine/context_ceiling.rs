//! The *context ceiling* (ADR 0629): the context size at which a coding-agent
//! session hands off to a *successor session*, whatever its flow says.
//!
//! One parser and one wire form for every layer: the workspace preference
//! `coding_agent_context_ceiling`, a trigger's `context_ceiling`, the
//! `ContextCeilingSet` event, the HTTP route and the CLI. The wire form is
//! `off`, a size such as `300k`, or a window share such as `30%`.

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[cfg(test)]
use super::handoff_estimate::HANDOFF_FLOOR_TOKENS;
use super::handoff_estimate::OFFER_DECLINE_HOLD_TOKENS;
use super::turn_requirements::CeilingFacts;
use crate::engine::handoff_state::SessionReading;

pub use crate::engine::thread_events::{CeilingCrossing, CeilingSource, ContextCeiling};

/// What the route answers an agent that tries to raise a ceiling.
pub const RAISE_NEEDS_A_PERSON: &str = "ceiling-raise-needs-a-person";

/// The longest parent chain [`thread_ceiling`] walks.
const MAX_ANCESTRY_HOPS: i32 = 16;

/// The thread's own ceiling, else the one its parent held when it spawned the
/// thread, and so on up the chain. `None` means the thread follows the
/// workspace preference. A parent's later change never reaches a child,
/// because each hop reads only events before the child's first.
pub async fn thread_ceiling(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<ContextCeiling>, sqlx::Error> {
    let raw: Option<String> = sqlx::query_scalar(
        "WITH RECURSIVE chain(thread_id, before, depth) AS ( \
           SELECT $1::uuid, NULL::bigint, 0 \
           UNION ALL \
           SELECT ts.parent_thread_id, \
                  (SELECT MIN(sequence) FROM events WHERE thread_id = chain.thread_id), \
                  chain.depth + 1 \
             FROM chain JOIN thread_summaries ts ON ts.thread_id = chain.thread_id \
            WHERE ts.parent_thread_id IS NOT NULL AND chain.depth < $2 \
         ) \
         SELECT set.ceiling FROM chain CROSS JOIN LATERAL ( \
           SELECT payload->>'ceiling' AS ceiling FROM events \
            WHERE thread_id = chain.thread_id AND event_type = 'ContextCeilingSet' \
              AND (chain.before IS NULL OR sequence < chain.before) \
            ORDER BY sequence DESC LIMIT 1 \
         ) set \
         ORDER BY chain.depth LIMIT 1",
    )
    .bind(thread_id)
    .bind(MAX_ANCESTRY_HOPS)
    .fetch_optional(pool)
    .await?
    .flatten();
    Ok(raw.and_then(|r| ContextCeiling::parse(&r).ok()))
}

/// The workspace's ceiling for a thread that sets none.
pub async fn workspace_ceiling(pool: &PgPool) -> ContextCeiling {
    let raw = crate::core::prefs::CODING_AGENT_CONTEXT_CEILING
        .read(pool)
        .await;
    ContextCeiling::parse(&raw).expect("the catalog validates every value it resolves to")
}

/// The ceiling in force on a thread: its own or inherited one, else the
/// workspace's.
pub async fn ceiling_of(pool: &PgPool, thread_id: Uuid) -> Result<ContextCeiling, sqlx::Error> {
    Ok(match thread_ceiling(pool, thread_id).await? {
        Some(ceiling) => ceiling,
        None => workspace_ceiling(pool).await,
    })
}

/// Who sets a ceiling: a person, or an agent from the thread it runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CeilingSetter {
    Person,
    Agent(Option<Uuid>),
}

/// Why a setter may not write a ceiling: a reason slug and the words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CeilingRefusal {
    pub reason: &'static str,
    pub message: String,
}

/// What the setter is told about a thread outside its reach.
pub const OUT_OF_REACH: &str = "ceiling-thread-out-of-reach";

/// What [`set_ceiling`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CeilingSet {
    Set,
    Refused(CeilingRefusal),
    NoThread,
}

fn raise_refusal(ceiling: ContextCeiling, over: ContextCeiling, whose: &str) -> CeilingRefusal {
    CeilingRefusal {
        reason: RAISE_NEEDS_A_PERSON,
        message: format!(
            "Only a person raises a context ceiling or turns it off: {ceiling} is above \
             {whose} {over}. Lower it, or ask the user to raise it."
        ),
    }
}

/// Refuse a ceiling an agent may not write on a trigger: one above the
/// workspace's. A person's passes.
pub async fn refuse_trigger_ceiling(
    pool: &PgPool,
    setter: CeilingSetter,
    ceiling: ContextCeiling,
) -> Option<CeilingRefusal> {
    if setter == CeilingSetter::Person {
        return None;
    }
    let workspace = workspace_ceiling(pool).await;
    ceiling
        .raises(workspace, None)
        .then(|| raise_refusal(ceiling, workspace, "the workspace's"))
}

/// Set a thread's ceiling for `setter`. A person may set any value. An agent
/// may lower its own thread's, and set a direct child's at or below both the
/// child's and its own.
pub async fn set_ceiling(
    bus: &super::event_bus::EventBus,
    setter: CeilingSetter,
    thread_id: Uuid,
    ceiling: ContextCeiling,
    meta: super::thread_events::EventMeta,
) -> Result<CeilingSet, Box<dyn std::error::Error + Send + Sync>> {
    let pool = bus.pool();
    let parent: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT parent_thread_id FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await?;
    let Some(parent) = parent else {
        return Ok(CeilingSet::NoThread);
    };
    if let CeilingSetter::Agent(caller) = setter {
        if let Some(refusal) = refuse_agent(pool, caller, thread_id, parent, ceiling).await? {
            return Ok(CeilingSet::Refused(refusal));
        }
    }
    bus.emit(super::event_bus::BusEvent::Thread {
        thread_id,
        event: super::thread_events::ThreadEvent::ContextCeilingSet {
            ceiling,
            source: CeilingSource::Thread,
        },
        meta,
    })
    .await?;
    Ok(CeilingSet::Set)
}

async fn refuse_agent(
    pool: &PgPool,
    caller: Option<Uuid>,
    thread_id: Uuid,
    parent: Option<Uuid>,
    ceiling: ContextCeiling,
) -> Result<Option<CeilingRefusal>, sqlx::Error> {
    let own = caller == Some(thread_id);
    let child = caller.filter(|c| parent == Some(*c));
    if !own && child.is_none() {
        return Ok(Some(CeilingRefusal {
            reason: OUT_OF_REACH,
            message: "An agent sets the context ceiling of its own thread or a direct child only"
                .to_string(),
        }));
    }
    let window = crate::engine::handoff_state::session_reading(pool, thread_id)
        .await?
        .window;
    let current = ceiling_of(pool, thread_id).await?;
    if ceiling.raises(current, window) {
        return Ok(Some(raise_refusal(ceiling, current, "this thread's")));
    }
    if let Some(caller) = child {
        let mine = ceiling_of(pool, caller).await?;
        if ceiling.raises(mine, window) {
            return Ok(Some(raise_refusal(ceiling, mine, "your own")));
        }
    }
    Ok(None)
}

/// Delivery facts for this session's durable crossing.
pub(crate) async fn ceiling_facts(
    pool: &PgPool,
    thread_id: Uuid,
    unread_input: bool,
) -> Result<CeilingFacts, sqlx::Error> {
    let (card_open, handoff_pending): (bool, bool) = sqlx::query_as(&format!(
        "SELECT EXISTS (SELECT 1 FROM events q \
           WHERE q.thread_id = $1 AND q.event_type IN ('UserQuestionAsked', 'CodingAgentPermissionRequest') \
             AND {scope} \
             AND NOT EXISTS (SELECT 1 FROM events a \
               WHERE a.thread_id = $1 AND ((q.event_type = 'UserQuestionAsked' AND a.event_type = 'UserQuestionAnswered' \
                 AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id') \
                 OR (q.event_type = 'CodingAgentPermissionRequest' AND a.event_type = 'CodingAgentPermissionResolved' \
                 AND a.payload->>'request_id' = q.payload->>'request_id')))), {pending}",
        scope = super::session_scope::in_current_session_sql("$1", "q.sequence"),
        pending = crate::engine::handoff_sql::pending_handoff_sql("$1"),
    ))
    .bind(thread_id)
    .fetch_one(pool)
    .await?;
    let blocked = crate::engine::handoff_state::handoff_block(pool, thread_id)
        .await?
        .is_some();
    Ok(CeilingFacts {
        owed: past_ceiling(pool, thread_id).await?,
        busy: unread_input || card_open || handoff_pending || blocked,
    })
}

/// Record the first crossing after a main-session capture. Delivery waits separately.
/// The caller serializes captures within the session and excludes restored usage.
pub async fn record_capture_crossing(
    bus: &super::event_bus::EventBus,
    thread_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = bus.pool();
    let (reached, opening): (bool, Option<serde_json::Value>) = sqlx::query_as(&format!(
        "SELECT EXISTS (SELECT 1 FROM events WHERE thread_id = $1 \
           AND event_type = 'ContextCeilingReached' AND {scope}), \
         (SELECT payload FROM events WHERE thread_id = $1 AND {capture} \
           ORDER BY sequence LIMIT 1)",
        scope = super::session_scope::in_current_session_sql("$1", "sequence"),
        capture = crate::engine::session_capture_sql::current_session_capture_sql("$1"),
    ))
    .bind(thread_id)
    .fetch_one(pool)
    .await?;
    if reached {
        return Ok(());
    }
    let ceiling = ceiling_of(pool, thread_id).await?;
    let opening = opening
        .map(|p| SessionReading {
            tokens: p["usage"]["input_tokens"].as_u64(),
            window: p["context_window"].as_u64(),
        })
        .unwrap_or_default();
    if ceiling.crossed_by(opening).is_some() {
        return Ok(());
    }
    if let Some(crossing) =
        ceiling.crossed_by(crate::engine::handoff_state::session_reading(pool, thread_id).await?)
    {
        if crate::engine::handoff_state::handoff_block(pool, thread_id)
            .await?
            .is_none()
        {
            record_ceiling_reached(bus, thread_id, crossing).await?;
        }
    }
    Ok(())
}

/// The crossing `thread_id`'s current session recorded, while the session is
/// still past the ceiling in force now. The size is its newest reading, never
/// below the crossing, so a raise the session has outgrown still binds. Only a
/// person raises a ceiling, so `None` after a recorded crossing means a person
/// raised it above the session's size.
pub async fn past_ceiling(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<CeilingCrossing>, sqlx::Error> {
    let payload: Option<serde_json::Value> = sqlx::query_scalar(&format!(
        "SELECT payload FROM events \
          WHERE thread_id = $1 AND event_type = 'ContextCeilingReached' AND {} \
          ORDER BY sequence DESC LIMIT 1",
        super::session_scope::in_current_session_sql("$1", "sequence")
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let Some(reached) = payload.and_then(|p| serde_json::from_value::<CeilingCrossing>(p).ok())
    else {
        return Ok(None);
    };
    let reading = crate::engine::handoff_state::session_reading(pool, thread_id).await?;
    let size = reading
        .tokens
        .map_or(reached.context_tokens, |t| t.max(reached.context_tokens));
    let ceiling = ceiling_of(pool, thread_id)
        .await?
        .effective(reading.window.or(reached.window));
    Ok(ceiling.is_some_and(|c| size > c).then_some(reached))
}

/// Whether the latest ceiling re-entry reached this session before its turn ended.
/// Missing evidence never authorizes starting a successor without input.
pub async fn reentry_cut_turn_short(pool: &PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "WITH crossing AS ( \
           SELECT sequence FROM events WHERE thread_id = $1 \
             AND event_type = 'ContextCeilingReached' AND {scope} \
           ORDER BY sequence DESC LIMIT 1), \
         anchor AS ( \
           SELECT e.sequence FROM events e, crossing c \
            WHERE e.thread_id = $1 AND e.sequence > c.sequence \
              AND e.event_type = 'PromptInjected' \
              AND e.payload->'origin'->>'kind' = 'engine' \
              AND e.payload->'origin'->'reason'->>'kind' = 'context_ceiling' \
            ORDER BY e.sequence DESC LIMIT 1) \
         SELECT EXISTS (SELECT 1 FROM crossing c, anchor a WHERE NOT EXISTS ( \
           SELECT 1 FROM events e WHERE e.thread_id = $1 \
             AND e.sequence > c.sequence AND e.sequence < a.sequence \
             AND e.event_type = ANY($2)))",
        scope = super::session_scope::in_current_session_sql("$1", "sequence"),
    ))
    .bind(thread_id)
    .bind(super::summary_tree::log::TURN_END_EVENT_TYPES)
    .fetch_one(pool)
    .await
}

/// Whether a *handoff checkpoint* run on `thread_id` hands off for its context
/// ceiling, whatever the estimate says. It does once the current session
/// recorded a crossing nothing raised, or mid-turn once its reading tops the
/// ceiling in force. A thread a handoff refuses never does.
pub(crate) async fn checkpoint_past_ceiling(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    if crate::engine::handoff_state::handoff_block(pool, thread_id)
        .await?
        .is_some()
    {
        return Ok(false);
    }
    if past_ceiling(pool, thread_id).await?.is_some() {
        return Ok(true);
    }
    let reading = crate::engine::handoff_state::session_reading(pool, thread_id).await?;
    Ok(ceiling_of(pool, thread_id)
        .await?
        .crossed_by(reading)
        .is_some())
}

/// Record the brief handoff a session past its ceiling owes: it reached the
/// ceiling, nothing raised it, and no handoff is pending. `false` when none
/// was owed. The successor session starts on the thread's next input.
pub(crate) async fn record_ceiling_brief(
    bus: &super::event_bus::EventBus,
    thread_id: Uuid,
    implementer: crate::engine::Implementer,
    running: &crate::engine::handoff_state::StillRunning,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    use super::thread_events::{EngineReason, EventMeta, HandoffCause, HandoffOpening};
    use crate::engine::handoff_state::{
        handoff_brief, last_applied_summary, BriefCause, NewHandoff,
    };

    let pool = bus.pool();
    let Some(crossing) = past_ceiling(pool, thread_id).await? else {
        return Ok(false);
    };
    if crate::engine::handoff_state::handoff_block(pool, thread_id)
        .await?
        .is_some()
    {
        return Ok(false);
    }
    // The next input always follows the brief into the first message.
    let brief = handoff_brief(
        thread_id,
        BriefCause::NoNarrative,
        true,
        last_applied_summary(pool, thread_id).await.as_deref(),
    );
    let handoff = NewHandoff {
        opening: HandoffOpening::Brief,
        message: running.after(brief),
        checkpoint: None,
        progress: None,
        implementer,
        request: None,
        cause: HandoffCause::ContextCeiling,
        starts: super::thread_events::SessionStart::OnNextInput,
        crossing: Some(crossing),
    };
    let origin = super::thread_events::MessageOrigin::engine(EngineReason::ContextCeiling);
    let record = crate::engine::handoff_state::record_handoff(
        bus,
        thread_id,
        handoff,
        EventMeta::with_actor(Some(origin)),
    )
    .await?;
    Ok(record == crate::engine::handoff_state::HandoffRecord::Recorded)
}

/// A ceiling handoff waiting for the thread's next input, as the composer card
/// shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CeilingHandoff {
    pub opening: super::thread_events::HandoffOpening,
    pub crossing: CeilingCrossing,
    /// The session's size: its newest reading, never below the crossing.
    #[serde(skip)]
    pub size: u64,
    /// The ceiling *Return to the old session* sets, in tokens. `None` when
    /// the window cap keeps every ceiling at or under the session's size.
    pub raises_to: Option<u64>,
}

/// The ceiling that lets a session of `size` tokens carry on: its size plus
/// the Successor card's hold, in whole thousands. `None` when the window cap
/// keeps it at or under `size`.
fn return_ceiling(size: u64, window: Option<u64>) -> Option<u64> {
    let raised = (size + OFFER_DECLINE_HOLD_TOKENS).div_ceil(1_000) * 1_000;
    let effective = ContextCeiling::Tokens(raised).effective(window)? / 1_000 * 1_000;
    (effective > size).then_some(effective)
}

/// `thread_id`'s pending handoff, when it is a ceiling handoff that waits for
/// the next input. The size is the session's newest reading, never below the
/// crossing.
pub async fn pending_ceiling_handoff(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<CeilingHandoff>, sqlx::Error> {
    let Some(handoff) = super::pending_handoff::pending_handoff(pool, thread_id).await? else {
        return Ok(None);
    };
    let Some(crossing) = handoff.crossing.filter(|_| !handoff.starts_unprompted()) else {
        return Ok(None);
    };
    let reading = crate::engine::handoff_state::session_reading(pool, thread_id).await?;
    let size = reading
        .tokens
        .map_or(crossing.context_tokens, |t| t.max(crossing.context_tokens));
    Ok(Some(CeilingHandoff {
        opening: handoff.opening,
        crossing,
        size,
        raises_to: return_ceiling(size, reading.window.or(crossing.window)),
    }))
}

/// What [`withdraw_ceiling_handoff`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Withdrawal {
    /// The handoff is withdrawn, and the thread's ceiling is now this many
    /// tokens.
    Withdrawn(u64),
    NeedsAPerson,
    NoCeilingHandoff,
    /// The window cap keeps every ceiling under the session's size.
    CannotRaise,
    /// A run holds the thread, so the successor session is already starting.
    Starting,
}

/// *Return to the old session*: withdraw `thread_id`'s pending ceiling
/// handoff, and raise its ceiling past the session's size first. A person
/// only. The raise lands first, so a failure between the two leaves a handoff
/// that still stands.
///
/// A run claims the thread before it reads the pending handoff under
/// [`crate::engine::handoff_state::lock_handoff`]. So a run that holds the thread here may
/// already be starting the successor, and the withdraw refuses.
pub async fn withdraw_ceiling_handoff(
    bus: &super::event_bus::EventBus,
    sessions: &tokio::sync::Mutex<std::collections::HashMap<Uuid, super::types::AgentSession>>,
    setter: CeilingSetter,
    thread_id: Uuid,
    meta: super::thread_events::EventMeta,
) -> Result<Withdrawal, Box<dyn std::error::Error + Send + Sync>> {
    use super::event_bus::BusEvent;
    use super::thread_events::ThreadEvent;

    if setter != CeilingSetter::Person {
        return Ok(Withdrawal::NeedsAPerson);
    }
    let pool = bus.pool();
    let claim = crate::engine::handoff_state::lock_handoff(pool, thread_id).await?;
    let Some(pending) = pending_ceiling_handoff(pool, thread_id).await? else {
        return Ok(Withdrawal::NoCeilingHandoff);
    };
    let held = sessions
        .lock()
        .await
        .get(&thread_id)
        .is_some_and(crate::engine::session_claim::holds_thread);
    if held {
        return Ok(Withdrawal::Starting);
    }
    let Some(raises_to) = pending.raises_to else {
        return Ok(Withdrawal::CannotRaise);
    };
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ContextCeilingSet {
            ceiling: ContextCeiling::Tokens(raises_to),
            source: CeilingSource::Thread,
        },
        meta: meta.clone(),
    })
    .await?;
    // A return is a *Stay here*: the Successor card holds back until the
    // session grows past it.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SuccessorOfferDeclined {
            input_tokens: pending.size,
        },
        meta: meta.clone(),
    })
    .await?;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionHandoffWithdrawn,
        meta,
    })
    .await?;
    claim.commit().await?;
    Ok(Withdrawal::Withdrawn(raises_to))
}

impl super::LucidosEngine {
    /// Record the brief handoff `thread_id` owes for its context ceiling, if
    /// it owes one ([`record_ceiling_brief`]). Runs where a ceiling re-entry
    /// turn ends, and where a real spawn claims the thread after a restart.
    pub async fn settle_owed_ceiling_handoff(&self, thread_id: Uuid) {
        match past_ceiling(self.pool(), thread_id).await {
            Ok(Some(_)) => {}
            Ok(None) => return,
            Err(e) => {
                log_legacy!(
                    "[ContextCeiling] Could not read whether {} is past its ceiling: {}",
                    thread_id,
                    e
                );
                return;
            }
        }
        let (model, reasoning_effort) = match self.cc_thread_runs_on(thread_id).await {
            Ok(pair) => pair,
            Err(e) => {
                log_legacy!("[ContextCeiling] Implementer of {}: {}", thread_id, e);
                return;
            }
        };
        let implementer = crate::engine::Implementer {
            coding_agent: self.thread_coding_agent(thread_id).await,
            model,
            reasoning_effort,
        };
        let running = self.still_running(thread_id).await;
        match record_ceiling_brief(&self.event_bus, thread_id, implementer, &running).await {
            Ok(true) => log_legacy!(
                "[ContextCeiling] Thread {} hands off from a brief: its re-entry wrote no narrative",
                thread_id
            ),
            Ok(false) => {}
            Err(e) => log_legacy!(
                "[ContextCeiling] Could not record the brief handoff of {}: {}",
                thread_id,
                e
            ),
        }
    }
}

/// The recorded crossing due for delivery. A failed read sends nothing.
pub async fn ceiling_crossing(
    pool: &PgPool,
    thread_id: Uuid,
    unread_input: bool,
) -> Option<CeilingCrossing> {
    match ceiling_facts(pool, thread_id, unread_input).await {
        Ok(facts) => facts.crossing(),
        Err(e) => {
            log_legacy!(
                "[ContextCeiling] Could not read the ceiling facts of {}: {}",
                thread_id,
                e
            );
            None
        }
    }
}

/// A later hold invalidates an approval still waiting for a safe boundary.
pub async fn recheck_ceiling_approval(
    bus: &super::event_bus::EventBus,
    thread_id: Uuid,
    approved: &mut Option<super::AgentUserInput>,
) {
    if approved.is_some() && super::thread_hold::thread_is_held(bus.pool(), thread_id).await {
        *approved = None;
    }
}

/// Prepare a safe steer. A refusal to steer leaves the debt for the turn end.
pub async fn ceiling_steer(
    bus: &super::event_bus::EventBus,
    thread_id: Uuid,
    unread: bool,
    can_steer: bool,
    ledger: &mut super::turn_requirements::ReentryLedger,
    approved: &mut Option<super::AgentUserInput>,
) -> Result<Option<super::AgentUserInput>, Box<dyn std::error::Error + Send + Sync>> {
    recheck_ceiling_approval(bus, thread_id, approved).await;
    if !can_steer {
        return Ok(None);
    }
    let crossing = ceiling_crossing(bus.pool(), thread_id, unread).await;
    let gate = super::turn_requirements::TurnEndState::at_session_idle(
        true,
        false,
        super::turn_requirements::SessionFacts {
            head: None,
            held: None,
            approved_plan_unstarted: false,
        },
        crossing,
    );
    let Some(due) = ledger.take_due(&gate) else {
        return Ok(None);
    };
    if approved.is_none() && !super::thread_hold::may_start_turn(bus.pool(), thread_id).await {
        ledger.forget(due);
        return Ok(None);
    }
    if let Some(input) = approved.take() {
        return Ok(Some(input));
    }
    match record_ceiling_reentry(bus, thread_id, crossing.expect("ceiling-only gate")).await {
        Ok(input) => Ok(Some(input)),
        Err(e) => {
            ledger.forget(due);
            Err(e)
        }
    }
}

/// Record the re-entry anchor after the durable crossing, then return its input.
pub async fn record_ceiling_reentry(
    bus: &super::event_bus::EventBus,
    thread_id: Uuid,
    crossing: CeilingCrossing,
) -> Result<super::AgentUserInput, Box<dyn std::error::Error + Send + Sync>> {
    ceiling_reentry_input(bus, thread_id, crossing).await
}

fn ceiling_meta() -> super::thread_events::EventMeta {
    use super::thread_events::{EngineReason, EventChannel, EventMeta, MessageOrigin};
    EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        actor: Some(MessageOrigin::engine(EngineReason::ContextCeiling)),
        ..EventMeta::NONE
    }
}

/// Record that the session crossed its ceiling, so it owes a handoff.
pub async fn record_ceiling_reached(
    bus: &super::event_bus::EventBus,
    thread_id: Uuid,
    crossing: CeilingCrossing,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use super::event_bus::BusEvent;
    use super::thread_events::ThreadEvent;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ContextCeilingReached {
            context_tokens: crossing.context_tokens,
            ceiling: crossing.ceiling,
            window: crossing.window,
        },
        meta: ceiling_meta(),
    })
    .await?;
    Ok(())
}

/// The ceiling re-entry for a live session. Its `PromptInjected` anchor
/// starts its exchange.
pub(crate) async fn ceiling_reentry_input(
    bus: &super::event_bus::EventBus,
    thread_id: Uuid,
    crossing: CeilingCrossing,
) -> Result<super::AgentUserInput, Box<dyn std::error::Error + Send + Sync>> {
    use super::event_bus::BusEvent;
    use super::thread_events::{ActorMode, EngineReason, MessageOrigin, ThreadEvent};
    let text = ceiling_reentry_prompt(crossing);
    let anchor = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::PromptInjected {
                text: text.clone(),
                mode: ActorMode::Engine,
                origin: Some(MessageOrigin::engine(EngineReason::ContextCeiling)),
                injected_message_id: None,
                delivered_event_id: None,
            },
            meta: ceiling_meta(),
        })
        .await?;
    Ok(super::AgentUserInput {
        text,
        images: None,
        origin_event_id: anchor.map(|a| a.event_id),
        kind: super::AgentInputKind::CeilingReentry,
        completion: None,
    })
}

impl super::LucidosEngine {
    /// Copy the firing trigger's ceiling onto its thread, through
    /// [`copy_trigger_ceiling`].
    pub async fn copy_trigger_ceiling(&self, thread_id: Uuid, trigger_id: &str) {
        let ceiling = self
            .trigger_configs
            .read()
            .ok()
            .and_then(|m| m.get(trigger_id).and_then(|c| c.context_ceiling));
        if let Some(ceiling) = ceiling {
            if let Err(e) = copy_trigger_ceiling(&self.event_bus, thread_id, ceiling).await {
                log_legacy!(
                    "[ContextCeiling] Could not copy {}'s ceiling: {}",
                    trigger_id,
                    e
                );
            }
        }
    }
}

/// Record a trigger's ceiling on a thread it fired, once. A thread that
/// already holds a `ContextCeilingSet` keeps it, so a continuing fire never
/// undoes a person's raise.
pub(crate) async fn copy_trigger_ceiling(
    bus: &super::event_bus::EventBus,
    thread_id: Uuid,
    ceiling: ContextCeiling,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let held: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events \
          WHERE thread_id = $1 AND event_type = 'ContextCeilingSet')",
    )
    .bind(thread_id)
    .fetch_one(bus.pool())
    .await?;
    if held {
        return Ok(());
    }
    bus.emit(super::event_bus::BusEvent::Thread {
        thread_id,
        event: super::thread_events::ThreadEvent::ContextCeilingSet {
            ceiling,
            source: CeilingSource::Trigger,
        },
        meta: super::thread_events::EventMeta::NONE,
    })
    .await?;
    Ok(())
}

/// The *ceiling re-entry*: what a session past its *context ceiling* is told
/// at its next safe boundary (ADR 0629).
pub fn ceiling_reentry_prompt(crossing: crate::engine::context_ceiling::CeilingCrossing) -> String {
    let thousands = |tokens: u64| tokens.div_ceil(1_000);
    let sections = crate::engine::handoff_state::NARRATIVE_SECTIONS.join(", ");
    format!(
        "CONTEXT CEILING: This session's context holds {}k tokens, past this thread's context \
         ceiling of {}k. Hand off to a fresh session now, and do no more work on the task.\n\n\
         Stop the step you are on at a safe point. Leave running background tasks alone; \
         the successor inherits them.\n\n\
         1. Commit any finished work.\n\
         2. Write a handoff narrative to a file. Give it these markdown headings: {sections}.\n\
         3. Run `lucidos successor start --narrative <file>`, then end your turn.\n\n\
         Ask no question in this turn. The fresh session reads your narrative first.",
        thousands(crossing.context_tokens),
        thousands(crossing.ceiling),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW_1M: Option<u64> = Some(1_000_000);

    #[test]
    fn parses_each_form() {
        assert_eq!(ContextCeiling::parse("off"), Ok(ContextCeiling::Off));
        assert_eq!(
            ContextCeiling::parse(" 300k "),
            Ok(ContextCeiling::Tokens(300_000))
        );
        assert_eq!(
            ContextCeiling::parse("30%"),
            Ok(ContextCeiling::WindowShare(30))
        );
        for raw in [
            "", "300", "300K", "0k", "-5k", "0%", "101%", "2.5k", "Off", "k",
        ] {
            assert!(
                ContextCeiling::parse(raw).is_err(),
                "{raw:?} must be refused"
            );
        }
        for ceiling in [
            ContextCeiling::Off,
            ContextCeiling::Tokens(300_000),
            ContextCeiling::WindowShare(30),
        ] {
            assert_eq!(ContextCeiling::parse(&ceiling.to_string()), Ok(ceiling));
            let wire = serde_json::to_value(ceiling).unwrap();
            assert_eq!(wire, serde_json::Value::from(ceiling.to_string()));
            assert_eq!(
                serde_json::from_value::<ContextCeiling>(wire).unwrap(),
                ceiling
            );
        }
    }

    #[test]
    fn caps_at_75_percent() {
        assert_eq!(
            ContextCeiling::Tokens(900_000).effective(WINDOW_1M),
            Some(750_000)
        );
        assert_eq!(
            ContextCeiling::Tokens(300_000).effective(Some(200_000)),
            Some(150_000)
        );
        assert_eq!(
            ContextCeiling::WindowShare(90).effective(WINDOW_1M),
            Some(750_000)
        );
        assert_eq!(
            ContextCeiling::WindowShare(30).effective(WINDOW_1M),
            Some(300_000)
        );
    }

    #[test]
    fn floors_at_handoff_floor() {
        assert_eq!(
            ContextCeiling::Tokens(50_000).effective(WINDOW_1M),
            Some(HANDOFF_FLOOR_TOKENS)
        );
        assert_eq!(
            ContextCeiling::Tokens(300_000).effective(Some(120_000)),
            Some(HANDOFF_FLOOR_TOKENS)
        );
        assert_eq!(
            ContextCeiling::WindowShare(1).effective(WINDOW_1M),
            Some(HANDOFF_FLOOR_TOKENS)
        );
    }

    #[test]
    fn no_window_no_cap() {
        assert_eq!(
            ContextCeiling::Tokens(900_000).effective(None),
            Some(900_000)
        );
        assert_eq!(
            ContextCeiling::Tokens(900_000).effective(Some(0)),
            Some(900_000)
        );
        assert_eq!(ContextCeiling::WindowShare(30).effective(None), None);
    }

    #[test]
    fn off_never_fires() {
        assert_eq!(ContextCeiling::Off.effective(WINDOW_1M), None);
        assert_eq!(ContextCeiling::Off.effective(None), None);
    }

    #[test]
    fn raises_compares_what_it_can_and_refuses_the_rest() {
        let k = ContextCeiling::Tokens;
        assert!(k(400_000).raises(k(300_000), None));
        assert!(!k(200_000).raises(k(300_000), None));
        assert!(!k(300_000).raises(k(300_000), None));
        assert!(ContextCeiling::Off.raises(k(300_000), None));
        assert!(!k(300_000).raises(ContextCeiling::Off, None));
        assert!(!ContextCeiling::Off.raises(ContextCeiling::Off, None));
        assert!(!k(200_000).raises(ContextCeiling::WindowShare(30), WINDOW_1M));
        assert!(k(400_000).raises(ContextCeiling::WindowShare(30), WINDOW_1M));
        // With no window a mixed comparison is unknown, so it is a raise.
        assert!(k(200_000).raises(ContextCeiling::WindowShare(30), None));
    }

    mod db {
        use super::super::*;
        use crate::core::PreferenceStore;
        use crate::engine::event_bus::{BusEvent, EventBus};
        use crate::engine::thread_events::{EventMeta, ThreadEvent};
        use crate::test_support::{
            seed_chat_thread, seed_coding_thread, setup_test_db, teardown_test_db,
        };

        pub(crate) async fn set(bus: &EventBus, thread_id: Uuid, ceiling: ContextCeiling) {
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ContextCeilingSet {
                    ceiling,
                    source: CeilingSource::Thread,
                },
                meta: EventMeta::NONE,
            })
            .await
            .expect("emit");
        }

        /// I8: a thread that sets none follows the workspace preference live.
        #[tokio::test]
        async fn unset_thread_follows_the_preference() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            assert_eq!(thread_ceiling(&pool, thread).await.unwrap(), None);
            assert_eq!(
                ceiling_of(&pool, thread).await.unwrap(),
                ContextCeiling::Tokens(300_000)
            );
            PreferenceStore::set(
                &pool,
                &bus,
                crate::core::prefs::CODING_AGENT_CONTEXT_CEILING.key(),
                "off",
                None,
            )
            .await
            .unwrap();
            assert_eq!(
                ceiling_of(&pool, thread).await.unwrap(),
                ContextCeiling::Off
            );
            set(&bus, thread, ContextCeiling::Tokens(200_000)).await;
            assert_eq!(
                ceiling_of(&pool, thread).await.unwrap(),
                ContextCeiling::Tokens(200_000),
                "the thread's own ceiling wins over the workspace's"
            );

            teardown_test_db(&db).await;
        }

        /// I8: a child takes the ceiling its parent held at spawn, and a later
        /// change on the parent never reaches it. A grandchild inherits the same.
        #[tokio::test]
        async fn child_ceiling_frozen_at_spawn() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let parent = seed_coding_thread(&bus, None).await;
            set(&bus, parent, ContextCeiling::Tokens(200_000)).await;
            let child = seed_coding_thread(&bus, Some(parent)).await;
            let grandchild = seed_coding_thread(&bus, Some(child)).await;
            set(&bus, parent, ContextCeiling::Tokens(500_000)).await;
            for thread in [child, grandchild] {
                assert_eq!(
                    ceiling_of(&pool, thread).await.unwrap(),
                    ContextCeiling::Tokens(200_000)
                );
            }
            set(&bus, child, ContextCeiling::Tokens(150_000)).await;
            assert_eq!(
                ceiling_of(&pool, grandchild).await.unwrap(),
                ContextCeiling::Tokens(200_000),
                "the grandchild spawned before its parent's change"
            );
            let late = seed_coding_thread(&bus, Some(child)).await;
            assert_eq!(
                ceiling_of(&pool, late).await.unwrap(),
                ContextCeiling::Tokens(150_000)
            );

            teardown_test_db(&db).await;
        }

        /// A trigger's ceiling lands on its fired thread once, and never over
        /// one the thread already holds.
        #[tokio::test]
        async fn trigger_ceiling_copied_at_fire() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            // A fired thread is a Chat thread; its coding-agent children
            // inherit the copy at spawn.
            let fired = seed_chat_thread(&bus, "fire").await;
            copy_trigger_ceiling(&bus, fired, ContextCeiling::Tokens(200_000))
                .await
                .unwrap();
            assert_eq!(
                thread_ceiling(&pool, fired).await.unwrap(),
                Some(ContextCeiling::Tokens(200_000))
            );
            let child = seed_coding_thread(&bus, Some(fired)).await;
            assert_eq!(
                thread_ceiling(&pool, child).await.unwrap(),
                Some(ContextCeiling::Tokens(200_000))
            );
            set(&bus, fired, ContextCeiling::Off).await;
            copy_trigger_ceiling(&bus, fired, ContextCeiling::Tokens(200_000))
                .await
                .unwrap();
            assert_eq!(
                thread_ceiling(&pool, fired).await.unwrap(),
                Some(ContextCeiling::Off),
                "a continuing fire keeps the person's raise"
            );

            teardown_test_db(&db).await;
        }

        fn refused(outcome: CeilingSet, reason: &str) {
            match outcome {
                CeilingSet::Refused(r) => assert_eq!(r.reason, reason, "{}", r.message),
                other => panic!("expected a refusal, got {other:?}"),
            }
        }

        async fn set_as(
            bus: &EventBus,
            setter: CeilingSetter,
            thread: Uuid,
            ceiling: ContextCeiling,
        ) -> CeilingSet {
            set_ceiling(bus, setter, thread, ceiling, EventMeta::NONE)
                .await
                .unwrap()
        }

        /// I7: a person sets any value, and an unknown thread is named.
        #[tokio::test]
        async fn ceiling_raise_by_a_person_is_allowed() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            for ceiling in [ContextCeiling::Off, ContextCeiling::Tokens(900_000)] {
                assert_eq!(
                    set_as(&bus, CeilingSetter::Person, thread, ceiling).await,
                    CeilingSet::Set
                );
                assert_eq!(thread_ceiling(&pool, thread).await.unwrap(), Some(ceiling));
            }
            assert_eq!(
                set_as(
                    &bus,
                    CeilingSetter::Person,
                    Uuid::new_v4(),
                    ContextCeiling::Off
                )
                .await,
                CeilingSet::NoThread
            );
            teardown_test_db(&db).await;
        }

        /// I7: an agent lowers its own ceiling, and never raises it or turns
        /// it off.
        #[tokio::test]
        async fn an_agent_lowers_its_own_ceiling_and_never_raises_it() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            let agent = CeilingSetter::Agent(Some(thread));
            assert_eq!(
                set_as(&bus, agent, thread, ContextCeiling::Tokens(200_000)).await,
                CeilingSet::Set
            );
            for ceiling in [ContextCeiling::Off, ContextCeiling::Tokens(250_000)] {
                refused(
                    set_as(&bus, agent, thread, ceiling).await,
                    RAISE_NEEDS_A_PERSON,
                );
            }
            assert_eq!(
                thread_ceiling(&pool, thread).await.unwrap(),
                Some(ContextCeiling::Tokens(200_000))
            );
            teardown_test_db(&db).await;
        }

        /// I7: a child's ceiling goes no higher than the child's or its
        /// parent's own. Any other thread is out of reach.
        #[tokio::test]
        async fn a_child_ceiling_is_capped_at_the_parent() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let parent = seed_coding_thread(&bus, None).await;
            let child = seed_coding_thread(&bus, Some(parent)).await;
            let stranger = seed_coding_thread(&bus, None).await;
            // Lowered after the spawn, so the child still holds 300k.
            set(&bus, parent, ContextCeiling::Tokens(200_000)).await;
            let agent = CeilingSetter::Agent(Some(parent));
            refused(
                set_as(&bus, agent, child, ContextCeiling::Tokens(250_000)).await,
                RAISE_NEEDS_A_PERSON,
            );
            assert_eq!(
                set_as(&bus, agent, child, ContextCeiling::Tokens(150_000)).await,
                CeilingSet::Set
            );
            refused(
                set_as(&bus, agent, stranger, ContextCeiling::Tokens(100_000)).await,
                OUT_OF_REACH,
            );
            refused(
                set_as(
                    &bus,
                    CeilingSetter::Agent(None),
                    child,
                    ContextCeiling::Tokens(100_000),
                )
                .await,
                OUT_OF_REACH,
            );
            teardown_test_db(&db).await;
        }

        async fn insert(
            pool: &PgPool,
            thread_id: Uuid,
            event_type: &str,
            payload: serde_json::Value,
        ) {
            sqlx::query(
                "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
                 VALUES (gen_random_uuid(), $1, $2, $3, 'thread', $3::text)",
            )
            .bind(event_type)
            .bind(payload)
            .bind(thread_id)
            .execute(pool)
            .await
            .expect("insert");
        }

        async fn turn_at(pool: &PgPool, thread_id: Uuid, tokens: u64, idle_reason: Option<&str>) {
            insert(
                pool,
                thread_id,
                "ContextCaptured",
                serde_json::json!({
                    "usage": {"input_tokens": tokens},
                    "context_window": 1_000_000,
                }),
            )
            .await;
            let idle = match idle_reason {
                Some(reason) => serde_json::json!({ "reason": reason }),
                None => serde_json::json!({}),
            };
            insert(pool, thread_id, "CodingAgentIdled", idle).await;
        }

        #[tokio::test]
        async fn a_successor_that_grows_past_the_ceiling_in_its_opening_turn_hands_off() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            for tokens in [60_000, 299_000, 301_000] {
                insert(
                    &pool,
                    thread,
                    "ContextCaptured",
                    serde_json::json!({
                        "usage": {"input_tokens": tokens},
                        "context_window": 1_000_000,
                    }),
                )
                .await;
                record_capture_crossing(&bus, thread).await.unwrap();
            }
            let facts = ceiling_facts(&pool, thread, false).await.unwrap();
            assert_eq!(facts.crossing().map(|c| c.context_tokens), Some(301_000));
            teardown_test_db(&db).await;
        }

        #[tokio::test]
        async fn hardening_thread_shape_records_at_the_first_capture_past_the_ceiling() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            for tokens in [60_000, 299_000, 301_000, 365_000, 385_000, 474_000, 654_000] {
                insert(
                    &pool,
                    thread,
                    "ContextCaptured",
                    serde_json::json!({
                        "usage": {"input_tokens": tokens}, "context_window": 1_000_000,
                    }),
                )
                .await;
                record_capture_crossing(&bus, thread).await.unwrap();
            }
            let crossings: Vec<serde_json::Value> = sqlx::query_scalar(
                "SELECT payload FROM events WHERE thread_id = $1 AND event_type = 'ContextCeilingReached'"
            ).bind(thread).fetch_all(&pool).await.unwrap();
            assert_eq!(crossings.len(), 1);
            assert_eq!(crossings[0]["context_tokens"], 301_000);
            assert!(ceiling_crossing(&pool, thread, false).await.is_some());
            assert!(ceiling_crossing(&pool, thread, true).await.is_none());
            teardown_test_db(&db).await;
        }

        #[tokio::test]
        async fn a_session_whose_first_capture_tops_the_ceiling_never_fires() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            for tokens in [301_000, 350_000, 654_000] {
                turn_at(&pool, thread, tokens, None).await;
                record_capture_crossing(&bus, thread).await.unwrap();
                assert_eq!(past_ceiling(&pool, thread).await.unwrap(), None);
                assert_eq!(ceiling_crossing(&pool, thread, false).await, None);
            }
            teardown_test_db(&db).await;
        }

        #[tokio::test]
        async fn a_crossing_on_the_first_turn_after_a_shutdown_resume_hands_off() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            turn_at(&pool, thread, 60_000, None).await;
            insert(
                &pool,
                thread,
                "SessionStarted",
                serde_json::json!({
                    "coding_agent": "claude-code", "session_id": "resumed", "branch": "b",
                    "continuity": {"kind": "resumed"},
                }),
            )
            .await;
            insert(
                &pool,
                thread,
                "ContextCaptured",
                serde_json::json!({
                    "usage": {"input_tokens": 301_000}, "context_window": 1_000_000,
                }),
            )
            .await;
            record_capture_crossing(&bus, thread).await.unwrap();
            assert_eq!(
                ceiling_crossing(&pool, thread, false)
                    .await
                    .unwrap()
                    .context_tokens,
                301_000
            );
            teardown_test_db(&db).await;
        }

        #[tokio::test]
        async fn a_crossing_on_the_first_turn_after_a_stop_hands_off() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            turn_at(&pool, thread, 60_000, None).await;
            insert(
                &pool,
                thread,
                "ResponseCanceled",
                serde_json::json!({"cause": "user_stop"}),
            )
            .await;
            insert(
                &pool,
                thread,
                "ContextCaptured",
                serde_json::json!({
                    "usage": {"input_tokens": 301_000}, "context_window": 1_000_000,
                }),
            )
            .await;
            record_capture_crossing(&bus, thread).await.unwrap();
            assert_eq!(
                ceiling_crossing(&pool, thread, false)
                    .await
                    .unwrap()
                    .context_tokens,
                301_000
            );
            teardown_test_db(&db).await;
        }

        /// I2: a session opened after the crossing starts over: its own first
        /// turn, and no crossing yet.
        #[tokio::test]
        async fn a_new_session_starts_the_ceiling_over() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            turn_at(&pool, thread, 400_000, None).await;
            turn_at(&pool, thread, 400_000, None).await;
            insert(
                &pool,
                thread,
                "ContextCeilingReached",
                serde_json::json!({"context_tokens": 400_000, "ceiling": 300_000}),
            )
            .await;
            insert(
                &pool,
                thread,
                "SessionStarted",
                serde_json::json!({
                    "coding_agent": "claude-code",
                    "session_id": "successor",
                    "branch": "b",
                    "continuity": {"kind": "handoff", "opening": "narrative", "message": "m"},
                }),
            )
            .await;
            turn_at(&pool, thread, 60_000, None).await;
            let fresh = ceiling_facts(&pool, thread, false).await.unwrap();
            assert_eq!(fresh.owed, None);
            assert_eq!(
                crate::engine::handoff_state::session_reading(&pool, thread)
                    .await
                    .unwrap()
                    .tokens,
                Some(60_000)
            );

            teardown_test_db(&db).await;
        }

        /// I16: a card this session left open holds the ceiling off. One an
        /// older session left unanswered does not.
        #[tokio::test]
        async fn only_this_sessions_open_card_holds_the_ceiling_off() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            let ask = |id: &str| serde_json::json!({"tool_use_id": id, "question": "Which?"});
            insert(&pool, thread, "UserQuestionAsked", ask("old")).await;
            insert(
                &pool,
                thread,
                "SessionStarted",
                serde_json::json!({
                    "coding_agent": "claude-code",
                    "session_id": "fresh",
                    "branch": "b",
                    "continuity": {"kind": "fresh", "reason": {"kind": "change_applied"}},
                }),
            )
            .await;
            assert!(!ceiling_facts(&pool, thread, false).await.unwrap().busy);
            insert(&pool, thread, "UserQuestionAsked", ask("q1")).await;
            assert!(ceiling_facts(&pool, thread, false).await.unwrap().busy);
            insert(
                &pool,
                thread,
                "UserQuestionAnswered",
                serde_json::json!({"tool_use_id": "q1", "answer": {"kind": "canceled"}}),
            )
            .await;
            assert!(!ceiling_facts(&pool, thread, false).await.unwrap().busy);

            teardown_test_db(&db).await;
        }

        #[tokio::test]
        async fn context_ceiling_steer_rechecks_unread_cards_permissions_and_driver() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            turn_at(&pool, thread, 100_000, None).await;
            turn_at(&pool, thread, 310_000, None).await;
            record_capture_crossing(&bus, thread).await.unwrap();
            let mut ledger = crate::engine::turn_requirements::ReentryLedger::default();
            assert!(
                ceiling_steer(&bus, thread, true, true, &mut ledger, &mut None)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                ceiling_steer(&bus, thread, false, false, &mut ledger, &mut None)
                    .await
                    .unwrap()
                    .is_none()
            );
            insert(
                &pool,
                thread,
                "UserQuestionAsked",
                serde_json::json!({"tool_use_id":"q","question":"Pick"}),
            )
            .await;
            assert!(
                ceiling_steer(&bus, thread, false, true, &mut ledger, &mut None)
                    .await
                    .unwrap()
                    .is_none()
            );
            insert(
                &pool,
                thread,
                "UserQuestionAnswered",
                serde_json::json!({"tool_use_id":"q"}),
            )
            .await;
            insert(
                &pool,
                thread,
                "CodingAgentPermissionRequest",
                serde_json::json!({"request_id":"p"}),
            )
            .await;
            assert!(
                ceiling_steer(&bus, thread, false, true, &mut ledger, &mut None)
                    .await
                    .unwrap()
                    .is_none()
            );
            insert(
                &pool,
                thread,
                "CodingAgentPermissionResolved",
                serde_json::json!({"request_id":"p"}),
            )
            .await;
            let input = ceiling_steer(&bus, thread, false, true, &mut ledger, &mut None)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(input.kind, crate::engine::AgentInputKind::CeilingReentry);
            assert!(input.text.contains("safe point"));
            assert!(input.text.contains("successor inherits"));
            assert!(
                ceiling_steer(&bus, thread, false, true, &mut ledger, &mut None)
                    .await
                    .unwrap()
                    .is_none()
            );
            let gate = crate::engine::turn_requirements::TurnEndState::at_session_idle(
                true,
                false,
                crate::engine::turn_requirements::SessionFacts {
                    head: None,
                    held: None,
                    approved_plan_unstarted: false,
                },
                past_ceiling(&pool, thread).await.unwrap(),
            );
            assert!(
                ledger.take_due(&gate).is_none(),
                "turn end cannot send a second prompt"
            );
            let handoffs: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'SessionHandedOff'")
                .bind(thread).fetch_one(&pool).await.unwrap();
            assert_eq!(handoffs, 0, "a deferral must not write a premature brief");
            assert!(brief(&bus, thread).await);
            let handoff = crate::engine::pending_handoff::pending_handoff(&pool, thread)
                .await
                .unwrap()
                .unwrap();
            assert!(
                handoff.message.contains("the full e2e suite"),
                "the mid-turn path retains running work"
            );
            teardown_test_db(&db).await;
        }

        /// A hold keeps the ceiling steer from starting work, and drops an
        /// approval that waited for a safe boundary. It raises no card.
        #[tokio::test]
        async fn a_hold_withholds_the_ceiling_steer() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            turn_at(&pool, thread, 100_000, None).await;
            turn_at(&pool, thread, 310_000, None).await;
            record_capture_crossing(&bus, thread).await.unwrap();
            let crossing = past_ceiling(&pool, thread).await.unwrap().unwrap();
            let mut approved = Some(ceiling_reentry_input(&bus, thread, crossing).await.unwrap());
            crate::test_support::seed_hold(&bus, thread).await;
            let mut ledger = crate::engine::turn_requirements::ReentryLedger::default();
            assert!(
                ceiling_steer(&bus, thread, false, true, &mut ledger, &mut approved)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(approved.is_none(), "the hold drops the waiting approval");
            let cards: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM events WHERE thread_id = $1 \
                 AND event_type IN ('UserQuestionAsked', 'UserQuestionQueued')",
            )
            .bind(thread)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(cards, 0, "a hold raises no card");
            teardown_test_db(&db).await;
        }

        #[tokio::test]
        async fn ceiling_reentry_distinguishes_interrupted_and_finished_turns() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            for end in [None, Some("ResponseGenerated"), Some("CodingAgentIdled")] {
                let thread = seed_coding_thread(&bus, None).await;
                assert!(!reentry_cut_turn_short(&pool, thread).await.unwrap());
                let crossing = CeilingCrossing {
                    context_tokens: 335_000,
                    ceiling: 300_000,
                    window: Some(1_000_000),
                };
                record_ceiling_reached(&bus, thread, crossing)
                    .await
                    .unwrap();
                assert!(!reentry_cut_turn_short(&pool, thread).await.unwrap());
                if let Some(end) = end {
                    insert(&pool, thread, end, serde_json::json!({})).await;
                }
                record_ceiling_reentry(&bus, thread, crossing)
                    .await
                    .unwrap();
                let cut = reentry_cut_turn_short(&pool, thread).await.unwrap();
                assert_eq!(cut, end.is_none());
                let (_, starts) =
                    crate::engine::handoff_state::cause_and_start(true, false, false, true, cut);
                assert_eq!(
                    starts,
                    if cut {
                        crate::engine::thread_events::SessionStart::AfterThisTurn
                    } else {
                        crate::engine::thread_events::SessionStart::OnNextInput
                    }
                );
                // The narrative turn ending later does not change the delivery.
                insert(&pool, thread, "CodingAgentIdled", serde_json::json!({})).await;
                assert_eq!(reentry_cut_turn_short(&pool, thread).await.unwrap(), cut);
                insert(
                    &pool,
                    thread,
                    "SessionStarted",
                    serde_json::json!({"continuity": {"kind": "fresh"}}),
                )
                .await;
                assert!(!reentry_cut_turn_short(&pool, thread).await.unwrap());
            }
            teardown_test_db(&db).await;
        }

        /// The crossing is recorded before the re-entry, so a session that
        /// dies unread still owes its handoff. The re-entry is the engine's,
        /// and names the six narrative sections.
        #[tokio::test]
        async fn the_crossing_is_recorded_before_the_reentry() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            let crossing = CeilingCrossing {
                context_tokens: 335_000,
                ceiling: 300_000,
                window: Some(1_000_000),
            };
            record_ceiling_reached(&bus, thread, crossing)
                .await
                .unwrap();
            let input = record_ceiling_reentry(&bus, thread, crossing)
                .await
                .unwrap();

            let rows: Vec<(String, serde_json::Value, uuid::Uuid)> = sqlx::query_as(
                "SELECT event_type, payload, id FROM events WHERE thread_id = $1 \
                   AND event_type IN ('ContextCeilingReached', 'PromptInjected') \
                 ORDER BY sequence",
            )
            .bind(thread)
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].0, "ContextCeilingReached");
            assert_eq!(rows[0].1["context_tokens"], 335_000);
            assert_eq!(rows[0].1["ceiling"], 300_000);
            assert_eq!(rows[0].1["window"], 1_000_000);
            assert_eq!(rows[0].1["actor"]["reason"]["kind"], "context_ceiling");
            assert_eq!(rows[1].0, "PromptInjected");
            assert_eq!(rows[1].1["mode"], "engine");
            assert_eq!(rows[1].1["origin"]["reason"]["kind"], "context_ceiling");

            assert_eq!(input.kind, crate::engine::AgentInputKind::CeilingReentry);
            assert_eq!(input.origin_event_id, Some(rows[1].2));
            assert_eq!(input.user_origin(), None);
            assert!(!input.kind.writes_prompt_sent());
            assert!(input.text.contains("335k") && input.text.contains("300k"));
            assert!(input.text.contains("lucidos successor start --narrative"));
            for section in crate::engine::handoff_state::NARRATIVE_SECTIONS {
                assert!(input.text.contains(section), "{section}");
            }

            teardown_test_db(&db).await;
        }

        /// I7: on a trigger, an agent sets a ceiling up to the workspace's.
        #[tokio::test]
        async fn an_agent_sets_no_trigger_ceiling_above_the_workspace() {
            let (pool, db) = setup_test_db().await;
            let agent = CeilingSetter::Agent(None);
            assert_eq!(
                refuse_trigger_ceiling(&pool, agent, ContextCeiling::Tokens(300_000)).await,
                None
            );
            for ceiling in [ContextCeiling::Off, ContextCeiling::Tokens(400_000)] {
                assert!(refuse_trigger_ceiling(&pool, agent, ceiling)
                    .await
                    .is_some());
                assert_eq!(
                    refuse_trigger_ceiling(&pool, CeilingSetter::Person, ceiling).await,
                    None
                );
            }
            teardown_test_db(&db).await;
        }

        /// A coding thread whose current session reached the default 300k
        /// ceiling at 400k.
        async fn past_the_ceiling(pool: &PgPool, bus: &EventBus) -> Uuid {
            let thread = seed_coding_thread(bus, None).await;
            turn_at(pool, thread, 400_000, None).await;
            turn_at(pool, thread, 400_000, None).await;
            insert(
                pool,
                thread,
                "ContextCeilingReached",
                serde_json::json!({
                    "context_tokens": 400_000, "ceiling": 300_000, "window": 1_000_000,
                }),
            )
            .await;
            thread
        }

        fn implementer() -> crate::engine::Implementer {
            crate::engine::Implementer {
                coding_agent: crate::runtime::CodingAgent::Codex,
                model: Some("gpt-5.5".to_string()),
                reasoning_effort: None,
            }
        }

        async fn brief(bus: &EventBus, thread: Uuid) -> bool {
            let running = crate::engine::handoff_state::StillRunning {
                tasks: vec!["the full e2e suite (`task-1`)".to_string()],
                ..Default::default()
            };
            record_ceiling_brief(bus, thread, implementer(), &running)
                .await
                .unwrap()
        }

        /// I3: a session stays past its ceiling until a person raises it
        /// above its size. A lower value, or one it outgrows, keeps it past.
        #[tokio::test]
        async fn only_a_raise_above_its_size_ends_past_the_ceiling() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = past_the_ceiling(&pool, &bus).await;
            assert_eq!(
                past_ceiling(&pool, thread).await.unwrap(),
                Some(CeilingCrossing {
                    context_tokens: 400_000,
                    ceiling: 300_000,
                    window: Some(1_000_000),
                })
            );
            set(&bus, thread, ContextCeiling::Tokens(200_000)).await;
            assert!(past_ceiling(&pool, thread).await.unwrap().is_some());
            set(&bus, thread, ContextCeiling::Tokens(500_000)).await;
            assert_eq!(past_ceiling(&pool, thread).await.unwrap(), None);
            turn_at(&pool, thread, 600_000, None).await;
            assert!(
                past_ceiling(&pool, thread).await.unwrap().is_some(),
                "a raise the session has outgrown still binds"
            );
            set(&bus, thread, ContextCeiling::Off).await;
            assert_eq!(past_ceiling(&pool, thread).await.unwrap(), None);

            teardown_test_db(&db).await;
        }

        /// I4, I5, I6: a re-entry that ended without a handoff gets a brief
        /// one, once. Its successor session waits for the next input, and its
        /// opening lists the running work.
        #[tokio::test]
        async fn a_reentry_without_a_handoff_ends_in_a_brief() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = past_the_ceiling(&pool, &bus).await;
            assert!(brief(&bus, thread).await);
            assert!(!brief(&bus, thread).await, "one handoff per session");

            let handoff = crate::engine::pending_handoff::pending_handoff(&pool, thread)
                .await
                .unwrap()
                .expect("the brief handoff is pending");
            use crate::engine::thread_events::{HandoffCause, HandoffOpening, SessionStart};
            assert_eq!(handoff.opening, HandoffOpening::Brief);
            assert_eq!(handoff.cause, Some(HandoffCause::ContextCeiling));
            assert_eq!(handoff.starts, SessionStart::OnNextInput);
            assert_eq!(handoff.implementer(), implementer());
            assert!(handoff.message.contains("context ceiling"));
            assert!(!handoff.message.contains("typed no request"));
            assert!(handoff.message.contains("## Still running"));
            assert!(handoff.message.contains("the full e2e suite (`task-1`)"));

            teardown_test_db(&db).await;
        }

        /// I4: a narrative handoff owes no brief. A Stop that withdrew it
        /// does, and a thread no handoff may leave gets neither.
        #[tokio::test]
        async fn a_brief_only_where_no_handoff_stands_and_one_may() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = past_the_ceiling(&pool, &bus).await;
            insert(
                &pool,
                thread,
                "SessionHandedOff",
                serde_json::json!({
                    "opening": "narrative", "message": "m", "coding_agent": "claude-code",
                    "cause": "context-ceiling", "starts": "on-next-input",
                }),
            )
            .await;
            assert!(!brief(&bus, thread).await, "the narrative stands");
            insert(
                &pool,
                thread,
                "ResponseCanceled",
                serde_json::json!({"cause": "user_stop"}),
            )
            .await;
            assert!(brief(&bus, thread).await, "the Stop withdrew the narrative");

            let archived = past_the_ceiling(&pool, &bus).await;
            sqlx::query(
                "UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1",
            )
            .bind(archived)
            .execute(&pool)
            .await
            .unwrap();
            assert!(!brief(&bus, archived).await);
            assert!(
                ceiling_facts(&pool, archived, false).await.unwrap().busy,
                "an archived thread never gets the re-entry"
            );

            let integrated = past_the_ceiling(&pool, &bus).await;
            sqlx::query(
                "UPDATE thread_summaries SET coding_agent_integrated = true WHERE thread_id = $1",
            )
            .bind(integrated)
            .execute(&pool)
            .await
            .unwrap();
            assert!(
                brief(&bus, integrated).await,
                "an integrated sub-thread hands off like any other"
            );

            teardown_test_db(&db).await;
        }

        /// I10: a ceiling handoff is no checkpoint run, so the estimate's
        /// history gains no sample from it.
        #[tokio::test]
        async fn ceiling_handoff_adds_no_sample() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = past_the_ceiling(&pool, &bus).await;
            assert!(brief(&bus, thread).await);
            let (samples, checkpoint): (i64, Option<String>) = sqlx::query_as(
                "SELECT (SELECT count(*) FROM events \
                          WHERE thread_id = $1 AND event_type = 'HandoffCheckpointReached'), \
                        (SELECT payload->>'checkpoint' FROM events \
                          WHERE thread_id = $1 AND event_type = 'SessionHandedOff')",
            )
            .bind(thread)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(samples, 0);
            assert_eq!(checkpoint, None);

            teardown_test_db(&db).await;
        }

        /// Decision 9: a checkpoint run past the ceiling hands off whatever
        /// `pays` says, mid-turn before any crossing too. Under the ceiling,
        /// after a raise, or on a thread no handoff may leave, it does not.
        #[tokio::test]
        async fn a_checkpoint_past_the_ceiling_hands_off_whatever_pays() {
            use crate::engine::handoff_estimate::{handoff_verdict, CheckpointRun};
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let run = CheckpointRun::parse("phase-commit", None).unwrap();
            let verdict = |thread| handoff_verdict(&pool, thread, &run);
            let capture = |thread, tokens: u64| {
                insert(
                    &pool,
                    thread,
                    "ContextCaptured",
                    serde_json::json!({
                        "usage": {"input_tokens": tokens},
                        "context_window": 1_000_000,
                    }),
                )
            };

            let thread = seed_coding_thread(&bus, None).await;
            capture(thread, 250_000).await;
            assert!(!verdict(thread).await.unwrap().past_ceiling);
            capture(thread, 350_000).await;
            let mid_turn = verdict(thread).await.unwrap();
            assert!(mid_turn.past_ceiling, "no crossing is recorded yet");
            assert_eq!(
                mid_turn.pays, None,
                "a capture with no model has no estimate"
            );
            set(&bus, thread, ContextCeiling::Tokens(400_000)).await;
            assert!(!verdict(thread).await.unwrap().past_ceiling);

            let reached = past_the_ceiling(&pool, &bus).await;
            assert!(verdict(reached).await.unwrap().past_ceiling);
            sqlx::query(
                "UPDATE thread_summaries SET coding_agent_integrated = true WHERE thread_id = $1",
            )
            .bind(reached)
            .execute(&pool)
            .await
            .unwrap();
            assert!(
                verdict(reached).await.unwrap().past_ceiling,
                "an integrated sub-thread hands off like any other"
            );
            sqlx::query(
                "UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1",
            )
            .bind(reached)
            .execute(&pool)
            .await
            .unwrap();
            assert!(!verdict(reached).await.unwrap().past_ceiling);

            teardown_test_db(&db).await;
        }

        async fn withdraw(bus: &EventBus, setter: CeilingSetter, thread: Uuid) -> Withdrawal {
            withdraw_ceiling_handoff(bus, &Default::default(), setter, thread, EventMeta::NONE)
                .await
                .unwrap()
        }

        /// I15: *Return to the old session* withdraws the ceiling handoff. It
        /// raises the ceiling to the session's size plus 100k, so the session
        /// is no longer past it. A second tap finds nothing.
        #[tokio::test]
        async fn withdraw_resumes_the_old_session() {
            use crate::engine::thread_events::HandoffOpening;
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = past_the_ceiling(&pool, &bus).await;
            assert_eq!(pending_ceiling_handoff(&pool, thread).await.unwrap(), None);
            assert!(brief(&bus, thread).await);
            assert_eq!(
                pending_ceiling_handoff(&pool, thread).await.unwrap(),
                Some(CeilingHandoff {
                    opening: HandoffOpening::Brief,
                    crossing: CeilingCrossing {
                        context_tokens: 400_000,
                        ceiling: 300_000,
                        window: Some(1_000_000),
                    },
                    size: 400_000,
                    raises_to: Some(500_000),
                })
            );

            assert_eq!(
                withdraw(&bus, CeilingSetter::Person, thread).await,
                Withdrawal::Withdrawn(500_000)
            );
            assert_eq!(
                crate::engine::pending_handoff::pending_handoff(&pool, thread)
                    .await
                    .unwrap(),
                None
            );
            assert_eq!(past_ceiling(&pool, thread).await.unwrap(), None);
            assert_eq!(
                ceiling_of(&pool, thread).await.unwrap(),
                ContextCeiling::Tokens(500_000)
            );
            let order: Vec<String> = sqlx::query_scalar(
                "SELECT event_type FROM events WHERE thread_id = $1 \
                   AND event_type IN \
                       ('ContextCeilingSet', 'SuccessorOfferDeclined', 'SessionHandoffWithdrawn') \
                 ORDER BY sequence",
            )
            .bind(thread)
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(
                order,
                [
                    "ContextCeilingSet",
                    "SuccessorOfferDeclined",
                    "SessionHandoffWithdrawn"
                ]
            );
            assert_eq!(
                crate::engine::handoff_state::latest_offer_decline(&pool, thread)
                    .await
                    .unwrap(),
                Some(400_000),
                "the Successor card holds back after a return"
            );
            assert_eq!(
                withdraw(&bus, CeilingSetter::Person, thread).await,
                Withdrawal::NoCeilingHandoff
            );
            assert!(!brief(&bus, thread).await, "a raised session owes no brief");
            // A ceiling handoff read past the ceiling before the return
            // re-reads it under the lock.
            let stale = crate::engine::handoff_state::NewHandoff {
                opening: HandoffOpening::Brief,
                message: "brief".into(),
                checkpoint: None,
                progress: None,
                implementer: implementer(),
                request: None,
                cause: crate::engine::thread_events::HandoffCause::ContextCeiling,
                starts: crate::engine::thread_events::SessionStart::OnNextInput,
                crossing: None,
            };
            assert_eq!(
                crate::engine::handoff_state::record_handoff(&bus, thread, stale, EventMeta::NONE)
                    .await
                    .unwrap(),
                crate::engine::handoff_state::HandoffRecord::CeilingLifted
            );

            teardown_test_db(&db).await;
        }

        /// I15, I7: only a person withdraws, and only a ceiling handoff. An
        /// agent's attempt leaves the handoff standing.
        #[tokio::test]
        async fn withdraw_needs_a_person() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = past_the_ceiling(&pool, &bus).await;
            assert!(brief(&bus, thread).await);
            assert_eq!(
                withdraw(&bus, CeilingSetter::Agent(Some(thread)), thread).await,
                Withdrawal::NeedsAPerson
            );
            assert!(crate::engine::pending_handoff::handoff_pending(&pool, thread).await);

            let plain = seed_coding_thread(&bus, None).await;
            insert(
                &pool,
                plain,
                "SessionHandedOff",
                serde_json::json!({
                    "opening": "narrative", "message": "m", "coding_agent": "claude-code",
                }),
            )
            .await;
            assert_eq!(
                withdraw(&bus, CeilingSetter::Person, plain).await,
                Withdrawal::NoCeilingHandoff,
                "a checkpoint or user handoff is no ceiling handoff"
            );

            teardown_test_db(&db).await;
        }

        /// A run that claimed the thread may already be starting the
        /// successor, so the withdraw refuses and the handoff stands.
        #[tokio::test]
        async fn withdraw_refuses_once_a_run_holds_the_thread() {
            use crate::engine::session_claim::{claim_thread_for, ClaimFor};
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = past_the_ceiling(&pool, &bus).await;
            assert!(brief(&bus, thread).await);
            let sessions = tokio::sync::Mutex::new(std::collections::HashMap::new());
            let _claim = claim_thread_for(
                &mut *sessions.lock().await,
                thread,
                crate::runtime::CodingAgent::ClaudeCode,
                ClaimFor::Run,
            )
            .expect("a free thread is claimed");
            let outcome = withdraw_ceiling_handoff(
                &bus,
                &sessions,
                CeilingSetter::Person,
                thread,
                EventMeta::NONE,
            )
            .await
            .unwrap();
            assert_eq!(outcome, Withdrawal::Starting);
            assert!(crate::engine::pending_handoff::handoff_pending(&pool, thread).await);

            teardown_test_db(&db).await;
        }

        /// A session above the window cap stays past any raise. So *Return to
        /// the old session* is refused, and the handoff stands.
        #[tokio::test]
        async fn withdraw_refuses_a_session_above_the_window_cap() {
            let (pool, db) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            turn_at(&pool, thread, 760_000, None).await;
            turn_at(&pool, thread, 760_000, None).await;
            insert(
                &pool,
                thread,
                "ContextCeilingReached",
                serde_json::json!({
                    "context_tokens": 760_000, "ceiling": 300_000, "window": 1_000_000,
                }),
            )
            .await;
            assert!(brief(&bus, thread).await);
            let pending = pending_ceiling_handoff(&pool, thread).await.unwrap();
            assert_eq!(pending.map(|p| p.raises_to), Some(None));
            assert_eq!(
                withdraw(&bus, CeilingSetter::Person, thread).await,
                Withdrawal::CannotRaise
            );
            assert!(crate::engine::pending_handoff::handoff_pending(&pool, thread).await);

            teardown_test_db(&db).await;
        }
    }

    /// The return ceiling clears the size by the hold, in whole `k`, and the
    /// window cap may shorten it. One the cap keeps under the size is none.
    #[test]
    fn the_return_ceiling_clears_the_size() {
        assert_eq!(return_ceiling(352_400, WINDOW_1M), Some(453_000));
        assert_eq!(return_ceiling(352_000, None), Some(452_000));
        assert_eq!(return_ceiling(700_000, WINDOW_1M), Some(750_000));
        assert_eq!(return_ceiling(700_000, Some(1_050_000)), Some(787_000));
        assert_eq!(return_ceiling(750_000, WINDOW_1M), None);
    }
}
