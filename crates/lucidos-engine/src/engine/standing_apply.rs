use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use uuid::Uuid;

use crate::core::changes::ChangeStatus;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::{MessageOrigin, ThreadEvent, UnproposedReason};
use crate::engine::thread_lifecycle::CodingAgentChangeState;
use crate::engine::types::AgentSession;
use crate::engine::LucidosEngine;

pub type AgentSessions = tokio::sync::Mutex<HashMap<Uuid, AgentSession>>;

#[derive(Debug, Clone)]
pub struct StandingApply {
    pub thread_id: Uuid,
    pub change_id: Option<Uuid>,
    pub batch_id: Option<Uuid>,
    pub actor: Option<MessageOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArmedChange {
    Ready(Uuid),
    Gone(&'static str),
    Unproposed,
}

#[derive(Debug, Clone)]
pub struct SettleFacts {
    pub status: String,
    pub parked_on_question: bool,
    pub live_event_waits: bool,
    pub change_state: CodingAgentChangeState,
    pub armed_change: ArmedChange,
    pub turn_settle: TurnSettle,
}

/// INVARIANT: Where the thread's last agent turn stands between its terminal and its
/// idle.
///
/// The terminal sets status `idle`, and then the run loop auto-commits,
/// proposes and emits `CodingAgentIdled`. A verdict taken in between would
/// merge a branch still being written. Every resolve reads this, whatever
/// triggered it, so no path can fire in that window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnSettle {
    Settled,
    InFlight,
    /// INVARIANT: The terminal is the newest closer, but no live session will send the
    /// idle. After a restart every session is gone, so a turn the old engine
    /// killed mid-settle lands here, possibly with its last edits uncommitted.
    CutOff,
}

/// INVARIANT: Classify the settle from its two halves. Pure.
///
/// `terminal_is_newest`: the thread's newest turn closer is a terminal, not
/// the idle. The run loop's Result arm idles after every terminal it sends,
/// a Stop's included, unless the engine is shutting down.
fn classify_turn_settle(terminal_is_newest: bool, session_live: bool) -> TurnSettle {
    match (terminal_is_newest, session_live) {
        (false, _) => TurnSettle::Settled,
        (true, true) => TurnSettle::InFlight,
        (true, false) => TurnSettle::CutOff,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StandingVerdict {
    Wait,
    Fire(Uuid),
    Drop(&'static str),
}

pub(crate) const PARKED_ON_QUESTION: &str = "The thread parked on a question.";
pub(crate) const TURN_FAILED: &str = "The turn failed.";
pub(crate) const NOTHING_PROPOSED: &str = "The thread settled without proposing a change.";
pub(crate) const CHANGE_RESOLVED: &str = "The change was already applied or discarded.";
pub(crate) const CHANGE_EMPTY: &str = "The change has no file changes left.";
pub(crate) const CHANGE_WITHDRAWN: &str = "The change went back to unfinished work on its branch.";
pub(crate) const CHANGE_SET_ASIDE: &str = "The change was set aside.";
pub(crate) const THREAD_GONE: &str = "The thread no longer exists.";
pub(crate) const TURN_CUT_OFF: &str = "The session stopped before its turn finished settling.";
pub const DISARMED_BY_OWNER: &str = "Canceled.";

pub use crate::core::changes::settling_thread_ids;
use crate::core::changes::SETTLING_THREAD_SQL;

/// INVARIANT: A `thread_summaries` row whose change Lucidos would apply. The standing
/// apply IS an Apply, one settle later, so it is offered exactly where Apply
/// is. An external repo is reviewed and pushed from the repo itself.
///
/// Such a thread never proposes at all (`idle_change_write`), so
/// an arm on one reads `Unproposed` forever, and its branch diff keeps the
/// verdict at `Wait`. That is the one shape breaking this module's promise
/// that an arm always ends, which is why the refusal is structural.
///
/// Both columns are read. `coding_agent_kind` is written at `SessionStarted`
/// and is the modern fact. A legacy row carries the bool with a NULL kind (see
/// `core::store::threads::backfill`), and reading the kind alone would let one
/// through.
///
/// The column names are unqualified, so a query joining a second table must
/// keep `thread_summaries` the only source of them.
const LUCIDOS_APPLIES_SQL: &str =
    "coding_agent_kind IS DISTINCT FROM 'external' AND coding_agent_is_external_repo = FALSE";

const EXTERNAL_REPO_REFUSAL: &str =
    "Lucidos does not apply changes into an external repo. Review and push that \
     work from the repo itself.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisarmScope {
    Sweep,
    All,
}

impl DisarmScope {
    fn predicate(self) -> &'static str {
        match self {
            DisarmScope::Sweep => "WHERE batch_id IS NOT NULL",
            DisarmScope::All => "",
        }
    }
}

/// INVARIANT: Resolve one arm against the current facts. Pure.
///
/// **It always ends.** Only a *settling* thread waits: `running`, `paused`,
/// watching an event, or a turn in flight to its idle. Each promises a settle,
/// since every wait ends by itself. A question card never does, so parking on
/// one drops the arm and reports it. So does a turn no session will finish.
///
/// A change that can never be applied ends the arm whatever the thread is
/// doing. Waiting for a turn to finish cannot bring a discarded change back.
pub fn standing_verdict(facts: &SettleFacts) -> StandingVerdict {
    if let ArmedChange::Gone(reason) = facts.armed_change {
        return StandingVerdict::Drop(reason);
    }
    match facts.status.as_str() {
        "running" => StandingVerdict::Wait,
        "paused" => StandingVerdict::Wait,
        "waiting_for_user_answer" => StandingVerdict::Drop(PARKED_ON_QUESTION),
        "failed" => StandingVerdict::Drop(TURN_FAILED),
        _ if facts.parked_on_question => StandingVerdict::Drop(PARKED_ON_QUESTION),
        _ if facts.live_event_waits => StandingVerdict::Wait,
        _ if facts.turn_settle == TurnSettle::InFlight => StandingVerdict::Wait,
        // INVARIANT: Nothing will finish this settle, and firing could miss its last edits.
        _ if facts.turn_settle == TurnSettle::CutOff => StandingVerdict::Drop(TURN_CUT_OFF),
        _ => match facts.armed_change {
            ArmedChange::Ready(change_id) => StandingVerdict::Fire(change_id),
            ArmedChange::Unproposed => match facts.change_state {
                CodingAgentChangeState::Unproposed { reason: None } => StandingVerdict::Wait,
                CodingAgentChangeState::Unproposed {
                    reason: Some(reason),
                } => StandingVerdict::Drop(withheld(reason)),
                _ => StandingVerdict::Drop(NOTHING_PROPOSED),
            },
            ArmedChange::Gone(reason) => StandingVerdict::Drop(reason),
        },
    }
}

pub fn thread_to_resolve(event: &BusEvent) -> Option<Uuid> {
    match event {
        BusEvent::Thread {
            event: ThreadEvent::EventWaitDelivered { .. } | ThreadEvent::EventWaitExpired { .. },
            ..
        } => None,
        BusEvent::Thread { thread_id, .. } => Some(*thread_id),
        BusEvent::System(_) => None,
    }
}

fn withheld(reason: UnproposedReason) -> &'static str {
    match reason {
        UnproposedReason::PlanChanged => {
            "The plan changed after approval. Ask the agent to restore the approved text."
        }
        UnproposedReason::PlanMissing => "The work is not ready: it needs an implementation plan.",
        UnproposedReason::PlanUnreadable => "Lucidos could not verify the implementation plan. Try again.",
        UnproposedReason::PlanAwaitingApproval => {
            "The work is not ready: its implementation plan awaits approval."
        }
        UnproposedReason::PlanOnlyCode => "The plan was approved for later, but the branch also changes code.",
        UnproposedReason::OutsideBound => {
            "The work is not ready: it reaches past the files its security fix named."
        }
        UnproposedReason::HardeningMissing => "The work is not ready: it has not been hardened.",
        UnproposedReason::TurnIncomplete => "The work is not ready: its turn did not finish.",
        UnproposedReason::AwaitingLanding => {
            "The work is not ready: the thread's change is still waiting in the landing lane."
        }
        UnproposedReason::IntegratedIntoParent => {
            "The work never becomes a change: the thread is integrated into its parent, whose \
             change carries it."
        }
        UnproposedReason::CarriedBySuccessor => {
            "The work never becomes a change here: the thread continued in a legacy successor thread, \
             whose change carries it."
        }
        UnproposedReason::HookRefused => {
            "The work is not ready: an event hook refused its proposal."
        }
    }
}

pub(crate) type ArmedChangeRow = (Uuid, ChangeStatus, i32);

pub(crate) fn classify_bound_change(row: Option<ArmedChangeRow>) -> ArmedChange {
    match row {
        None => ArmedChange::Gone(CHANGE_RESOLVED),
        Some((_, ChangeStatus::SetAside, _)) => ArmedChange::Gone(CHANGE_SET_ASIDE),
        Some((_, ChangeStatus::Withdrawn, _)) => ArmedChange::Gone(CHANGE_WITHDRAWN),
        Some((_, status, _)) if status != ChangeStatus::Pending => {
            ArmedChange::Gone(CHANGE_RESOLVED)
        }
        Some((_, _, 0)) => ArmedChange::Gone(CHANGE_EMPTY),
        Some((id, _, _)) => ArmedChange::Ready(id),
    }
}

#[derive(Debug, Default)]
pub(crate) struct ArmedThreads {
    inner: std::sync::RwLock<HashSet<Uuid>>,
}

impl ArmedThreads {
    fn contains(&self, thread_id: Uuid) -> bool {
        self.read().contains(&thread_id)
    }

    fn insert(&self, thread_id: Uuid) {
        self.write().insert(thread_id);
    }

    fn remove(&self, thread_id: Uuid) {
        self.write().remove(&thread_id);
    }

    fn snapshot(&self) -> Vec<Uuid> {
        self.read().iter().copied().collect()
    }

    fn replace(&self, ids: HashSet<Uuid>) {
        *self.write() = ids;
    }

    /// INVARIANT: A poisoned lock means a panic while the set was borrowed. This set is a
    /// filter rather than the authority, so recovering the inner value keeps
    /// the resolver working. The next boot rebuilds it.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashSet<Uuid>> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashSet<Uuid>> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// INVARIANT: The outcome of reading an arm's facts. `Unknown` keeps the arm: a probe
/// that could not run is not evidence the thread settled or died.
enum FactsProbe {
    Ready(SettleFacts),
    ThreadGone,
    Unknown,
}

type StandingApplyRow = (
    Uuid,
    Option<Uuid>,
    Option<Uuid>,
    Option<serde_json::Value>,
    chrono::DateTime<chrono::Utc>,
);

const ARM_COLUMNS: &str = "thread_id, change_id, batch_id, actor, created_at";

/// INVARIANT: An arm as read back, carrying the generation token that says WHICH arm it
/// is.
///
/// Re-arming rewrites the row in place and bumps `created_at`, so a resolver
/// that loaded the old one must not consume the new one. Every read-then-delete
/// path deletes on this token, never on the thread id alone.
struct LoadedArm {
    arm: StandingApply,
    generation: chrono::DateTime<chrono::Utc>,
}

fn hydrate_arm(row: StandingApplyRow) -> LoadedArm {
    let (thread_id, change_id, batch_id, actor_json, created_at) = row;
    LoadedArm {
        arm: StandingApply {
            thread_id,
            change_id,
            batch_id,
            actor: actor_json.and_then(|v| serde_json::from_value(v).ok()),
        },
        generation: created_at,
    }
}

async fn insert_arm(pool: &sqlx::PgPool, arm: &StandingApply) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO standing_applies (thread_id, change_id, batch_id, actor) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (thread_id) DO UPDATE SET \
           change_id = EXCLUDED.change_id, \
           batch_id = EXCLUDED.batch_id, \
           actor = EXCLUDED.actor, \
           created_at = NOW()",
    )
    .bind(arm.thread_id)
    .bind(arm.change_id)
    .bind(arm.batch_id)
    .bind(serde_json::to_value(&arm.actor).ok())
    .execute(pool)
    .await?;
    Ok(())
}

async fn load_arm(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<Option<LoadedArm>, sqlx::Error> {
    let row: Option<StandingApplyRow> = sqlx::query_as(&format!(
        "SELECT {ARM_COLUMNS} FROM standing_applies WHERE thread_id = $1"
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(hydrate_arm))
}

async fn take_arm(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<Option<LoadedArm>, sqlx::Error> {
    let row: Option<StandingApplyRow> = sqlx::query_as(&format!(
        "DELETE FROM standing_applies WHERE thread_id = $1 RETURNING {ARM_COLUMNS}"
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(hydrate_arm))
}

/// INVARIANT: Delete one arm only if it is still the one the caller read.
///
/// A resolution reads the arm, awaits its probes, and then acts. A re-arm
/// landing in that window rewrites the row, so deleting by thread id would
/// consume the NEW instruction while carrying out the old one. Matching on the
/// generation makes the consume atomic against that.
async fn take_arm_generation(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    generation: chrono::DateTime<chrono::Utc>,
) -> Result<Option<LoadedArm>, sqlx::Error> {
    let row: Option<StandingApplyRow> = sqlx::query_as(&format!(
        "DELETE FROM standing_applies WHERE thread_id = $1 AND created_at = $2 \
         RETURNING {ARM_COLUMNS}"
    ))
    .bind(thread_id)
    .bind(generation)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(hydrate_arm))
}

/// INVARIANT: End archived instructions before subscribers or reconstruction can dispatch.
/// A durable announcement precedes deletion. The row lock excludes a concurrent
/// re-arm, and a post-boundary announcement makes a crash retry idempotent.
pub(crate) async fn settle_restored_standing_applies(
    pool: &sqlx::PgPool,
    bus: &EventBus,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(boundary) = crate::core::snapshot::boundary::read(pool).await? else {
        return Ok(());
    };
    let rows: Vec<StandingApplyRow> = sqlx::query_as(&format!(
        "SELECT {ARM_COLUMNS} FROM standing_applies WHERE created_at <= $1"
    ))
    .bind(boundary.restored_at)
    .fetch_all(pool)
    .await?;
    for row in rows {
        settle_restored_arm(pool, bus, &hydrate_arm(row)).await?;
    }
    Ok(())
}

async fn settle_restored_arm(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    loaded: &LoadedArm,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let Some(boundary) = crate::core::snapshot::boundary::read(pool).await? else {
        return Ok(false);
    };
    if loaded.generation > boundary.restored_at {
        return Ok(false);
    }
    let mut tx = pool.begin().await?;
    let row: Option<StandingApplyRow> = sqlx::query_as(&format!(
        "SELECT {ARM_COLUMNS} FROM standing_applies WHERE thread_id = $1 AND created_at = $2 FOR UPDATE"
    ))
    .bind(loaded.arm.thread_id)
    .bind(loaded.generation)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let arm = hydrate_arm(row).arm;
    let announced: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM events WHERE sequence > $1 \
         AND event_type = 'StandingApplyDropped' \
         AND payload->'data'->>'thread_id' = $2 \
         AND payload->'data'->>'reason' = 'snapshot-restored')",
    )
    .bind(boundary.event_sequence)
    .bind(arm.thread_id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    if !announced {
        bus.emit(BusEvent::System(SystemEvent::StandingApplyDropped {
            thread_id: arm.thread_id,
            change_id: arm.change_id,
            batch_id: arm.batch_id,
            reason: "snapshot-restored".into(),
            actor: arm.actor,
        }))
        .await?;
    }
    sqlx::query("DELETE FROM standing_applies WHERE thread_id = $1 AND created_at = $2")
        .bind(arm.thread_id)
        .bind(loaded.generation)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

async fn take_arm_to_fire(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    loaded: &LoadedArm,
    change_id: Uuid,
) -> Result<Option<LoadedArm>, sqlx::Error> {
    if !crate::core::snapshot::boundary::permits_slot(pool, loaded.generation).await? {
        return Ok(None);
    }
    let taken = take_arm_generation(pool, loaded.arm.thread_id, loaded.generation).await?;
    if let Some(fired) = &taken {
        bus.emit_or_log(
            BusEvent::System(SystemEvent::StandingApplyFired {
                thread_id: fired.arm.thread_id,
                change_id,
                batch_id: fired.arm.batch_id,
                actor: fired.arm.actor.clone(),
            }),
            "[StandingApply] StandingApplyFired",
        )
        .await;
    }
    Ok(taken)
}

async fn read_settle_facts(
    pool: &sqlx::PgPool,
    arm: &StandingApply,
    sessions: &AgentSessions,
) -> FactsProbe {
    type SettleRow = (String, i32, String, Option<String>, bool, bool);
    let probe: Result<Option<SettleRow>, sqlx::Error> = sqlx::query_as(
        "SELECT status, live_event_wait_count, coding_agent_change_state, \
                coding_agent_unproposed_reason, coding_agent_requires_restart, is_coding_agent \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(arm.thread_id)
    .fetch_optional(pool)
    .await;
    let (status, live_waits, kind, reason, requires_restart, is_coding_agent) = match probe {
        Ok(Some(row)) => row,
        Ok(None) => return FactsProbe::ThreadGone,
        Err(e) => {
            log_legacy!(
                "[StandingApply] thread lookup for {} failed: {}",
                arm.thread_id,
                e
            );
            return FactsProbe::Unknown;
        }
    };
    let parked_on_question = if matches!(status.as_str(), "running" | "paused") {
        false
    } else {
        match crate::engine::question_park_sql::thread_parked_on_question(pool, arm.thread_id).await
        {
            Ok(parked) => parked,
            Err(e) => {
                log_legacy!(
                    "[StandingApply] question lookup for {} failed: {}",
                    arm.thread_id,
                    e
                );
                return FactsProbe::Unknown;
            }
        }
    };
    let Some(armed_change) = read_armed_change(pool, arm).await else {
        return FactsProbe::Unknown;
    };
    let turn_settle = if is_coding_agent {
        match read_turn_settle(pool, arm.thread_id, sessions).await {
            Some(settle) => settle,
            None => return FactsProbe::Unknown,
        }
    } else {
        TurnSettle::Settled
    };
    FactsProbe::Ready(SettleFacts {
        status,
        parked_on_question,
        live_event_waits: live_waits > 0,
        change_state: CodingAgentChangeState::from_columns(
            &kind,
            reason.as_deref(),
            requires_restart,
        ),
        armed_change,
        turn_settle,
    })
}

/// INVARIANT: The events that close an agent turn: its idle, and its four terminals.
///
/// A coding-agent thread also carries the Lucidos Agent's turns, for example
/// in a voice call. Their replies carry no `claude_code` channel, and no idle
/// follows them, so they must not read as an agent settle. Every agent session
/// stamps that channel, Codex included.
const AGENT_TURN_CLOSER_SQL: &str = "(event_type = 'CodingAgentIdled' \
     OR (event_type IN ('ResponseGenerated', 'ResponseCanceled', 'ResponseFailed', 'ResponseAborted') \
         AND payload->>'channel' = 'claude_code'))";

/// INVARIANT: Read where the thread's last turn stands. `None` = the probe failed.
///
/// The durable half comes from the event log. `EventBus::emit` writes an event
/// before it broadcasts it, so a resolve that the idle triggered already sees
/// the idle. The session map is read only when the terminal is newest, so a
/// settled thread never takes the lock.
pub async fn read_turn_settle(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    sessions: &AgentSessions,
) -> Option<TurnSettle> {
    let newest: Result<Option<String>, sqlx::Error> = sqlx::query_scalar(&format!(
        "SELECT event_type FROM events \
          WHERE thread_id = $1 AND {AGENT_TURN_CLOSER_SQL} \
          ORDER BY sequence DESC LIMIT 1"
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await;
    let terminal_is_newest = match newest {
        Ok(event_type) => event_type.is_some_and(|t| t != "CodingAgentIdled"),
        Err(e) => {
            log_legacy!(
                "[StandingApply] turn closer lookup for {} failed: {}",
                thread_id,
                e
            );
            return None;
        }
    };
    let session_live = terminal_is_newest
        && sessions
            .lock()
            .await
            .get(&thread_id)
            .is_some_and(|s| s.is_live());
    Some(classify_turn_settle(terminal_is_newest, session_live))
}

async fn read_armed_change(pool: &sqlx::PgPool, arm: &StandingApply) -> Option<ArmedChange> {
    let probe: Result<Option<ArmedChangeRow>, sqlx::Error> = match arm.change_id {
        Some(change_id) => {
            sqlx::query_as(
                "SELECT id, status, file_count FROM changes \
                 WHERE id = $1 AND thread_id = $2",
            )
            .bind(change_id)
            .bind(arm.thread_id)
            .fetch_optional(pool)
            .await
        }
        None => {
            sqlx::query_as(
                "SELECT id, status, file_count FROM changes \
                 WHERE thread_id = $1 AND status = $2 \
                 ORDER BY created_at LIMIT 1",
            )
            .bind(arm.thread_id)
            .bind(ChangeStatus::Pending)
            .fetch_optional(pool)
            .await
        }
    };
    match probe {
        Ok(None) if arm.change_id.is_none() => Some(ArmedChange::Unproposed),
        Ok(row) => Some(classify_bound_change(row)),
        Err(e) => {
            log_legacy!(
                "[StandingApply] change lookup for {} failed: {}",
                arm.thread_id,
                e
            );
            None
        }
    }
}

async fn check_repo_is_appliable(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let appliable: Option<bool> = sqlx::query_scalar(&format!(
        "SELECT {LUCIDOS_APPLIES_SQL} FROM thread_summaries WHERE thread_id = $1"
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    match appliable {
        Some(false) => Err(EXTERNAL_REPO_REFUSAL.into()),
        _ => Ok(()),
    }
}

async fn check_binding(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    change_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let row: Option<(Option<Uuid>, ChangeStatus)> =
        sqlx::query_as("SELECT thread_id, status FROM changes WHERE id = $1")
            .bind(change_id)
            .fetch_optional(pool)
            .await?;
    match row {
        None => Err("Change not found".into()),
        Some((owner, _)) if owner != Some(thread_id) => {
            Err("That change belongs to a different thread".into())
        }
        Some((_, status)) if status != ChangeStatus::Pending => {
            Err("That change has already been applied or discarded".into())
        }
        Some(_) => Ok(()),
    }
}

async fn check_arm_has_a_settle(
    pool: &sqlx::PgPool,
    arm: &StandingApply,
    sessions: &AgentSessions,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let reason = match read_settle_facts(pool, arm, sessions).await {
        FactsProbe::Ready(facts) => match standing_verdict(&facts) {
            StandingVerdict::Drop(reason) => reason,
            StandingVerdict::Wait | StandingVerdict::Fire(_) => return Ok(()),
        },
        FactsProbe::ThreadGone => THREAD_GONE,
        FactsProbe::Unknown => return Ok(()),
    };
    Err(format!("{reason} There is nothing to wait for, so nothing was armed.").into())
}

async fn read_scoped_arms(
    pool: &sqlx::PgPool,
    scope: DisarmScope,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let sql = format!(
        "SELECT thread_id FROM standing_applies {}",
        scope.predicate()
    );
    let rows: Vec<(Uuid,)> = sqlx::query_as(&sql).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

async fn read_sweep_candidates(
    pool: &sqlx::PgPool,
) -> Result<Vec<(Uuid, Option<Uuid>)>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT t.thread_id, \
                (SELECT c.id FROM changes c \
                  WHERE c.thread_id = t.thread_id AND c.status = $1 \
                  ORDER BY c.created_at LIMIT 1) \
           FROM thread_summaries t \
          WHERE t.is_coding_agent = TRUE AND {settling} \
            AND {LUCIDOS_APPLIES_SQL}",
        settling = &*SETTLING_THREAD_SQL,
    ))
    .bind(ChangeStatus::Pending)
    .fetch_all(pool)
    .await
}

impl LucidosEngine {
    pub fn armed_standing_apply_threads(&self) -> Vec<Uuid> {
        self.standing_applies.snapshot()
    }

    /// INVARIANT: Record the owner's instruction to apply `thread_id`'s change once it
    /// settles. Re-arming a thread replaces the previous arm, because the new
    /// one may name a different change.
    ///
    /// A named change must be that thread's own pending one, and the thread's
    /// repo must be one Lucidos applies into. There must also be a settle to
    /// wait for. The checks live here rather than in one caller, so the HTTP
    /// route and the agent tool are held to the same rule.
    ///
    /// Resolves once immediately, so arming a settled thread with a pending
    /// change applies now rather than waiting for an event that will not come.
    pub async fn arm_standing_apply(
        self: &Arc<Self>,
        arm: StandingApply,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        check_repo_is_appliable(self.pool(), arm.thread_id).await?;
        if let Some(change_id) = arm.change_id {
            check_binding(self.pool(), arm.thread_id, change_id).await?;
        }
        check_arm_has_a_settle(self.pool(), &arm, &self.agent_sessions).await?;
        insert_arm(self.pool(), &arm).await?;
        self.standing_applies.insert(arm.thread_id);
        self.event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::StandingApplyArmed {
                    thread_id: arm.thread_id,
                    change_id: arm.change_id,
                    batch_id: arm.batch_id,
                    actor: arm.actor.clone(),
                }),
                "[StandingApply] StandingApplyArmed",
            )
            .await;
        log_legacy!(
            "[StandingApply] armed thread {} (change {:?}, batch {:?})",
            arm.thread_id,
            arm.change_id,
            arm.batch_id
        );
        self.resolve_standing_apply(arm.thread_id).await;
        Ok(())
    }

    /// INVARIANT: End one arm and report why. `Ok(false)` means nothing was armed.
    ///
    /// A failed delete is an `Err`, never `Ok(false)`. The arm is still live
    /// then, and a caller told "nothing was armed" would clear its flag over an
    /// instruction that will still fire.
    pub async fn drop_standing_apply(
        &self,
        thread_id: Uuid,
        reason: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let Some(arm) = self.take_standing_apply(thread_id).await? else {
            return Ok(false);
        };
        self.report_standing_apply_dropped(&arm, reason, actor)
            .await;
        Ok(true)
    }

    /// INVARIANT: Arm every thread that will settle, for the Apply All sweep.
    ///
    /// The sweep covers every *settling* thread when the owner pressed it
    /// ([`SETTLING_THREAD_SQL`]). So it never arms a thread whose arm would
    /// drop on its first resolution. Coding-agent
    /// threads only: nothing else proposes a change. Each thread's pending
    /// change is bound where it has one.
    ///
    /// `applying_now` names the changes the caller is applying in this same
    /// press, and those threads are skipped. A `paused` thread is sweepable AND
    /// its change is appliable. So without this, one press both applies a
    /// change and arms a standing apply for it. The arm then reports a drop for
    /// work that succeeded, and its resolver races the batch driver.
    ///
    /// Returns how many threads were armed.
    pub async fn sweep_standing_applies(
        self: &Arc<Self>,
        batch_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
        applying_now: &[Uuid],
    ) -> usize {
        let rows = match read_sweep_candidates(self.pool()).await {
            Ok(rows) => rows,
            Err(e) => {
                log_legacy!("[StandingApply] sweep lookup failed: {}", e);
                return 0;
            }
        };
        let mut armed = 0;
        for (thread_id, change_id) in rows {
            if change_id.is_some_and(|id| applying_now.contains(&id)) {
                continue;
            }
            let arm = StandingApply {
                thread_id,
                change_id,
                batch_id,
                actor: actor.clone(),
            };
            match self.arm_standing_apply(arm).await {
                Ok(()) => armed += 1,
                Err(e) => log_legacy!("[StandingApply] sweep arm for {} failed: {}", thread_id, e),
            }
        }
        armed
    }

    pub async fn drop_standing_applies(
        &self,
        scope: DisarmScope,
        reason: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<usize, sqlx::Error> {
        let mut dropped = 0;
        for thread_id in read_scoped_arms(self.pool(), scope).await? {
            if self
                .drop_standing_apply(thread_id, reason, actor.clone())
                .await?
            {
                dropped += 1;
            }
        }
        Ok(dropped)
    }

    async fn take_standing_apply(
        &self,
        thread_id: Uuid,
    ) -> Result<Option<StandingApply>, sqlx::Error> {
        let taken = take_arm(self.pool(), thread_id).await?;
        self.standing_applies.remove(thread_id);
        Ok(taken.map(|l| l.arm))
    }

    async fn take_standing_apply_generation(&self, loaded: &LoadedArm) -> Option<StandingApply> {
        let thread_id = loaded.arm.thread_id;
        let taken = take_arm_generation(self.pool(), thread_id, loaded.generation).await;
        self.consume_generation(thread_id, taken)
    }

    /// INVARIANT: The generation-matched take's tail: forget the thread only once its own
    /// arm was deleted.
    ///
    /// A generation miss means somebody re-armed, so the thread is still armed
    /// and the filter must keep saying so. A failed DELETE leaves the durable
    /// arm live, so the filter keeps it too. The next event retries.
    fn consume_generation(
        &self,
        thread_id: Uuid,
        taken: Result<Option<LoadedArm>, sqlx::Error>,
    ) -> Option<StandingApply> {
        match taken {
            Ok(None) => None,
            Ok(Some(loaded)) => {
                self.standing_applies.remove(thread_id);
                Some(loaded.arm)
            }
            Err(e) => {
                log_legacy!("[StandingApply] delete for {} failed: {}", thread_id, e);
                None
            }
        }
    }

    async fn report_standing_apply_dropped(
        &self,
        arm: &StandingApply,
        reason: &str,
        actor: Option<MessageOrigin>,
    ) {
        log_legacy!(
            "[StandingApply] dropped thread {} ({})",
            arm.thread_id,
            reason
        );
        self.event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::StandingApplyDropped {
                    thread_id: arm.thread_id,
                    change_id: arm.change_id,
                    batch_id: arm.batch_id,
                    reason: reason.to_string(),
                    actor: actor.or_else(|| arm.actor.clone()),
                }),
                "[StandingApply] StandingApplyDropped",
            )
            .await;
    }

    pub(crate) async fn resolve_standing_apply(self: &Arc<Self>, thread_id: Uuid) {
        if !self.standing_applies.contains(thread_id) {
            return;
        }
        let loaded = match load_arm(self.pool(), thread_id).await {
            Ok(arm) => arm,
            Err(e) => {
                log_legacy!("[StandingApply] load for {} failed: {}", thread_id, e);
                return;
            }
        };
        let Some(loaded) = loaded else {
            self.standing_applies.remove(thread_id);
            return;
        };

        // INVARIANT: A restored instruction is settled even if its thread is still
        // running or parked. Apply authority never survives the restore, and
        // this check must cover later event-driven resolutions as well as boot.
        match crate::core::snapshot::boundary::permits_slot(self.pool(), loaded.generation).await {
            Ok(true) => {}
            Ok(false) => {
                self.end_standing_apply(&loaded, "snapshot-restored").await;
                return;
            }
            Err(error) => {
                log_legacy!("[StandingApply] restore boundary unavailable: {}", error);
                return;
            }
        }

        let facts = match read_settle_facts(self.pool(), &loaded.arm, &self.agent_sessions).await {
            FactsProbe::Ready(facts) => facts,
            FactsProbe::Unknown => return,
            FactsProbe::ThreadGone => {
                self.end_standing_apply(&loaded, THREAD_GONE).await;
                return;
            }
        };

        match standing_verdict(&facts) {
            StandingVerdict::Wait => {}
            StandingVerdict::Drop(reason) => self.end_standing_apply(&loaded, reason).await,
            StandingVerdict::Fire(change_id) => self.fire_standing_apply(&loaded, change_id).await,
        }
    }

    async fn end_standing_apply(&self, loaded: &LoadedArm, reason: &str) {
        if reason == "snapshot-restored" {
            match settle_restored_arm(self.pool(), &self.event_bus, loaded).await {
                Ok(true) => self.standing_applies.remove(loaded.arm.thread_id),
                Ok(false) => {}
                Err(error) => log_legacy!("[StandingApply] restore settlement failed: {}", error),
            }
            return;
        }
        if self.take_standing_apply_generation(loaded).await.is_some() {
            self.report_standing_apply_dropped(&loaded.arm, reason, None)
                .await;
        }
    }

    /// INVARIANT: Apply the change this arm named.
    ///
    /// The arm is taken BEFORE the apply. `apply_change` emits `ChangeApplied`,
    /// which re-enters the resolver through the bus subscriber. An arm still in
    /// the table would then resolve a second time and report a spurious drop.
    /// It consumes the arm this resolution read, so a re-arm landing in the
    /// window keeps its own instruction and this one stands down.
    async fn fire_standing_apply(self: &Arc<Self>, loaded: &LoadedArm, change_id: Uuid) {
        let taken = take_arm_to_fire(self.pool(), &self.event_bus, loaded, change_id).await;
        let Some(arm) = self.consume_generation(loaded.arm.thread_id, taken) else {
            return;
        };
        log_legacy!(
            "[StandingApply] thread {} settled, applying change {}",
            arm.thread_id,
            change_id
        );
        let engine = self.clone();
        let actor = arm.actor.clone();
        tokio::spawn(async move {
            if let Err(e) = engine.apply_change(change_id, actor).await {
                log_legacy!(
                    "[StandingApply] apply_change({change_id}) returned Err: {e}. \
                     The apply path emits ChangeApplyFailed, which reports to the owner",
                );
            }
            engine.broadcast_changes_updated().await;
        });
    }

    pub fn start_standing_apply_resolver(self: &Arc<Self>) {
        let mut rx = self.event_bus.subscribe();
        let engine = self.clone();
        tokio::spawn(async move {
            log_legacy!("[StandingApply] resolver started");
            loop {
                match rx.recv().await {
                    Ok(emitted) => {
                        if let Some(thread_id) = thread_to_resolve(&emitted.typed) {
                            engine.resolve_standing_apply(thread_id).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // INVARIANT: The skipped batch may hold the settle an arm was
                        // waiting for, so re-resolve them all rather than
                        // stranding the owner's instruction.
                        log_legacy!(
                            "[StandingApply] subscriber lagged by {} events, re-resolving",
                            n
                        );
                        for thread_id in engine.standing_applies.snapshot() {
                            engine.resolve_standing_apply(thread_id).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        log_legacy!("[StandingApply] EventBus closed, resolver stopping");
                        break;
                    }
                }
            }
        });
    }

    /// INVARIANT: Rebuild the armed set from the durable table and re-take every verdict.
    ///
    /// A thread that settled while the engine was down gets its apply now, and
    /// one that failed gets its report. Without this an arm survives the
    /// restart as a row nobody reads.
    ///
    /// MUST run after agent recovery, so a thread whose session auto-resumes is
    /// observed as running rather than settled.
    ///
    /// No session is live here: each one died with the previous process. So a
    /// turn caught between its terminal and its idle reads [`TurnSettle::CutOff`]
    /// and drops, rather than firing without its last edits.
    pub async fn recover_standing_applies(self: &Arc<Self>) {
        let rows: Vec<(Uuid,)> = match sqlx::query_as("SELECT thread_id FROM standing_applies")
            .fetch_all(self.pool())
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                log_legacy!("[StandingApply] recovery query failed: {}", e);
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        log_legacy!("[StandingApply] recovering {} armed thread(s)", rows.len());
        let ids: HashSet<Uuid> = rows.iter().map(|(id,)| *id).collect();
        self.standing_applies.replace(ids.clone());
        for thread_id in ids {
            self.resolve_standing_apply(thread_id).await;
        }
    }
}

#[cfg(test)]
mod db_tests {
    use super::*;
    use crate::test_support::{
        seed_change_state, seed_thread_event, setup_test_db, start_cc_session, teardown_test_db,
    };
    use sqlx::PgPool;

    async fn seed_thread(pool: &PgPool, thread_id: Uuid, status: &str) {
        sqlx::query(
            "INSERT INTO thread_summaries \
               (thread_id, title, source, message_count, last_activity, has_response, \
                is_saved, status, is_coding_agent) \
             VALUES ($1, 'T', 'claude_code', 0, NOW(), false, false, $2, true)",
        )
        .bind(thread_id)
        .bind(status)
        .execute(pool)
        .await
        .expect("seed thread_summary");
    }

    async fn set_external_repo(
        pool: &PgPool,
        thread_id: Uuid,
        kind: Option<&str>,
        bool_flag: bool,
    ) {
        sqlx::query(
            "UPDATE thread_summaries \
                SET coding_agent_kind = $2, coding_agent_is_external_repo = $3 \
              WHERE thread_id = $1",
        )
        .bind(thread_id)
        .bind(kind)
        .bind(bool_flag)
        .execute(pool)
        .await
        .expect("mark external repo");
    }

    async fn set_status(pool: &PgPool, thread_id: Uuid, status: &str) {
        sqlx::query("UPDATE thread_summaries SET status = $2 WHERE thread_id = $1")
            .bind(thread_id)
            .bind(status)
            .execute(pool)
            .await
            .expect("update status");
    }

    async fn set_parked_on_event_wait(pool: &PgPool, thread_id: Uuid, waits: i32) {
        sqlx::query(
            "UPDATE thread_summaries SET status = 'idle', live_event_wait_count = $2 \
             WHERE thread_id = $1",
        )
        .bind(thread_id)
        .bind(waits)
        .execute(pool)
        .await
        .expect("park on an event wait");
    }

    async fn seed_change(pool: &PgPool, change_id: Uuid, thread_id: Uuid, status: ChangeStatus) {
        sqlx::query(
            "INSERT INTO changes \
               (id, request_id, thread_id, branch_name, repo_root, description, \
                file_count, files, requires_restart, status) \
             VALUES ($1, $2, $3, $4, '/repo', 'd', 1, ARRAY['a.rs'], false, $5)",
        )
        .bind(change_id)
        .bind(Uuid::new_v4())
        .bind(thread_id)
        .bind(format!("b-{change_id}"))
        .bind(status)
        .execute(pool)
        .await
        .expect("seed change");
    }

    fn arm_for(thread_id: Uuid, change_id: Option<Uuid>) -> StandingApply {
        StandingApply {
            thread_id,
            change_id,
            batch_id: None,
            actor: None,
        }
    }

    fn swept_arm(thread_id: Uuid) -> StandingApply {
        StandingApply {
            thread_id,
            change_id: None,
            batch_id: Some(Uuid::new_v4()),
            actor: None,
        }
    }

    fn no_sessions() -> AgentSessions {
        AgentSessions::default()
    }

    fn live_session(
        thread_id: Uuid,
    ) -> (
        AgentSessions,
        tokio::sync::mpsc::UnboundedReceiver<crate::engine::AgentUserInput>,
    ) {
        let (session, rx) = AgentSession::for_test();
        (
            AgentSessions::new(HashMap::from([(thread_id, session)])),
            rx,
        )
    }

    fn terminal() -> ThreadEvent {
        ThreadEvent::ResponseGenerated {
            text: "done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        }
    }

    fn idle() -> ThreadEvent {
        ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        }
    }

    async fn seed_agent_event(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
        let event_type = event.event_type();
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: crate::engine::thread_events::EventMeta {
                channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
                ..crate::engine::thread_events::EventMeta::NONE
            },
        })
        .await
        .unwrap_or_else(|e| panic!("seed {event_type}: {e}"))
        .unwrap_or_else(|| panic!("seed {event_type} was not persisted"));
    }

    fn stop_terminal() -> ThreadEvent {
        ThreadEvent::ResponseCanceled {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        }
    }

    async fn seed_mid_settle(
        pool: &PgPool,
        bus: &EventBus,
        terminal: ThreadEvent,
    ) -> (Uuid, StandingApply) {
        let thread = Uuid::new_v4();
        let change = Uuid::new_v4();
        start_cc_session(bus, thread, &format!("claude-code/{thread}"), None).await;
        seed_change(pool, change, thread, ChangeStatus::Pending).await;
        seed_agent_event(bus, thread, terminal).await;
        let arm = arm_for(thread, Some(change));
        insert_arm(pool, &arm).await.expect("insert arm");
        (thread, arm)
    }

    async fn verdict(
        pool: &PgPool,
        arm: &StandingApply,
        sessions: &AgentSessions,
    ) -> StandingVerdict {
        let FactsProbe::Ready(facts) = read_settle_facts(pool, arm, sessions).await else {
            panic!("facts must read back");
        };
        assert_eq!(
            facts.status, "idle",
            "the terminal reads as settled on its own"
        );
        standing_verdict(&facts)
    }

    #[tokio::test]
    async fn an_arm_set_mid_settle_waits_for_the_idle() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let (thread, arm) = seed_mid_settle(&pool, &bus, terminal()).await;
        let (sessions, _session_rx) = live_session(thread);

        assert_eq!(verdict(&pool, &arm, &sessions).await, StandingVerdict::Wait);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_lagged_re_resolve_waits_until_the_idle_is_written() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let (thread, arm) = seed_mid_settle(&pool, &bus, terminal()).await;
        let (sessions, _session_rx) = live_session(thread);

        assert_eq!(verdict(&pool, &arm, &sessions).await, StandingVerdict::Wait);

        seed_agent_event(&bus, thread, idle()).await;
        assert_eq!(
            verdict(&pool, &arm, &sessions).await,
            StandingVerdict::Fire(arm.change_id.expect("bound")),
            "the idle is written before it is broadcast, so the re-resolve fires"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_stopped_turn_holds_the_arm_until_its_idle() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let (thread, arm) = seed_mid_settle(&pool, &bus, stop_terminal()).await;
        let (sessions, _session_rx) = live_session(thread);

        assert_eq!(verdict(&pool, &arm, &sessions).await, StandingVerdict::Wait);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_turn_cut_off_by_a_restart_drops_its_arm() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let (_thread, arm) = seed_mid_settle(&pool, &bus, terminal()).await;

        assert_eq!(
            verdict(&pool, &arm, &no_sessions()).await,
            StandingVerdict::Drop(TURN_CUT_OFF)
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_lucidos_agent_reply_after_the_idle_does_not_cut_the_turn_off() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let (thread, arm) = seed_mid_settle(&pool, &bus, terminal()).await;
        seed_agent_event(&bus, thread, idle()).await;
        seed_thread_event(&bus, thread, terminal()).await;

        assert_eq!(
            verdict(&pool, &arm, &no_sessions()).await,
            StandingVerdict::Fire(arm.change_id.expect("bound"))
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_thread_with_no_coding_agent_settles_on_its_terminal() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        let change = Uuid::new_v4();
        seed_thread(&pool, thread, "running").await;
        sqlx::query("UPDATE thread_summaries SET is_coding_agent = false WHERE thread_id = $1")
            .bind(thread)
            .execute(&pool)
            .await
            .expect("make it a chat thread");
        seed_change(&pool, change, thread, ChangeStatus::Pending).await;
        seed_thread_event(&bus, thread, terminal()).await;
        let arm = arm_for(thread, Some(change));
        insert_arm(&pool, &arm).await.expect("insert arm");

        assert_eq!(
            verdict(&pool, &arm, &no_sessions()).await,
            StandingVerdict::Fire(change)
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn an_arm_never_names_a_second_change_on_its_thread() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        seed_thread(&pool, thread, "running").await;
        seed_change(&pool, first, thread, ChangeStatus::Pending).await;

        let arm = arm_for(thread, Some(first));
        insert_arm(&pool, &arm).await.expect("insert arm");
        set_status(&pool, thread, "idle").await;
        let FactsProbe::Ready(facts) = read_settle_facts(&pool, &arm, &no_sessions()).await else {
            panic!("facts must read back");
        };
        assert_eq!(standing_verdict(&facts), StandingVerdict::Fire(first));

        sqlx::query("UPDATE changes SET status = 'applied' WHERE id = $1")
            .bind(first)
            .execute(&pool)
            .await
            .expect("resolve first change");
        seed_change(&pool, second, thread, ChangeStatus::Pending).await;

        let FactsProbe::Ready(after) = read_settle_facts(&pool, &arm, &no_sessions()).await else {
            panic!("facts must read back");
        };
        assert_eq!(
            standing_verdict(&after),
            StandingVerdict::Drop(CHANGE_RESOLVED),
            "the arm must end rather than reach the change nobody armed"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_parked_thread_ends_the_arm() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        let change = Uuid::new_v4();
        seed_thread(&pool, thread, "running").await;
        seed_change(&pool, change, thread, ChangeStatus::Pending).await;
        let arm = arm_for(thread, Some(change));
        insert_arm(&pool, &arm).await.expect("insert arm");

        set_status(&pool, thread, "waiting_for_user_answer").await;
        let FactsProbe::Ready(facts) = read_settle_facts(&pool, &arm, &no_sessions()).await else {
            panic!("facts must read back");
        };
        assert_eq!(
            standing_verdict(&facts),
            StandingVerdict::Drop(PARKED_ON_QUESTION)
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn an_event_wait_keeps_the_arm_and_its_delivery_does_not_fire_it() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        let change = Uuid::new_v4();
        seed_thread(&pool, thread, "running").await;
        seed_change(&pool, change, thread, ChangeStatus::Pending).await;
        let arm = arm_for(thread, Some(change));
        insert_arm(&pool, &arm).await.expect("insert arm");

        set_parked_on_event_wait(&pool, thread, 1).await;
        let FactsProbe::Ready(facts) = read_settle_facts(&pool, &arm, &no_sessions()).await else {
            panic!("facts must read back");
        };
        assert_eq!(standing_verdict(&facts), StandingVerdict::Wait);

        set_parked_on_event_wait(&pool, thread, 0).await;
        let FactsProbe::Ready(gap) = read_settle_facts(&pool, &arm, &no_sessions()).await else {
            panic!("facts must read back");
        };
        assert_eq!(
            standing_verdict(&gap),
            StandingVerdict::Fire(change),
            "the gap reads as settled, so only the skip keeps it from firing"
        );
        let delivery = BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::EventWaitDelivered {
                wait_id: Uuid::new_v4(),
                event_id: Uuid::new_v4(),
                event_type: "E2ELockReleased".to_string(),
                payload: serde_json::json!({}),
                matched_index: 0,
            },
            meta: Default::default(),
        };
        assert_eq!(thread_to_resolve(&delivery), None);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_thread_watching_an_event_is_settling() {
        let (pool, db) = setup_test_db().await;
        let watching = Uuid::new_v4();
        let asking = Uuid::new_v4();
        seed_thread(&pool, watching, "running").await;
        seed_thread(&pool, asking, "running").await;
        set_parked_on_event_wait(&pool, watching, 1).await;
        set_status(&pool, asking, "waiting_for_user_answer").await;
        sqlx::query("UPDATE thread_summaries SET live_event_wait_count = 1 WHERE thread_id = $1")
            .bind(asking)
            .execute(&pool)
            .await
            .expect("a question card with a wait still open");

        let settling = settling_thread_ids(&pool, [watching, asking].into_iter())
            .await
            .expect("settling read");
        assert_eq!(settling, HashSet::from([watching]));
        let swept: Vec<Uuid> = read_sweep_candidates(&pool)
            .await
            .expect("sweep read")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(swept, vec![watching]);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_parent_waiting_only_on_sub_threads_fires_its_arm() {
        let (pool, db) = setup_test_db().await;
        let parent = Uuid::new_v4();
        let change = Uuid::new_v4();
        seed_thread(&pool, parent, "running").await;
        seed_change(&pool, change, parent, ChangeStatus::Pending).await;
        let arm = arm_for(parent, Some(change));
        insert_arm(&pool, &arm).await.expect("insert arm");

        sqlx::query(
            "UPDATE thread_summaries SET status = 'idle', active_children_count = 2 \
             WHERE thread_id = $1",
        )
        .bind(parent)
        .execute(&pool)
        .await
        .expect("settle with running children");
        let FactsProbe::Ready(facts) = read_settle_facts(&pool, &arm, &no_sessions()).await else {
            panic!("facts must read back");
        };
        assert_eq!(standing_verdict(&facts), StandingVerdict::Fire(change));
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn an_arm_is_stored_once_and_taken_once() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        let change = Uuid::new_v4();
        seed_thread(&pool, thread, "running").await;
        seed_change(&pool, change, thread, ChangeStatus::Pending).await;

        insert_arm(&pool, &arm_for(thread, None))
            .await
            .expect("arm");
        insert_arm(&pool, &arm_for(thread, Some(change)))
            .await
            .expect("re-arm");
        let loaded = load_arm(&pool, thread).await.expect("load").expect("armed");
        assert_eq!(loaded.arm.change_id, Some(change));

        let taken = take_arm(&pool, thread).await.expect("take");
        assert!(taken.is_some());
        assert!(
            take_arm(&pool, thread)
                .await
                .expect("second take")
                .is_none(),
            "an arm is one-shot"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn an_arm_cannot_name_another_threads_change() {
        let (pool, db) = setup_test_db().await;
        let mine = Uuid::new_v4();
        let theirs = Uuid::new_v4();
        let their_change = Uuid::new_v4();
        seed_thread(&pool, mine, "running").await;
        seed_thread(&pool, theirs, "running").await;
        seed_change(&pool, their_change, theirs, ChangeStatus::Pending).await;

        assert!(
            check_binding(&pool, mine, their_change).await.is_err(),
            "arming across threads must be refused"
        );

        let arm = arm_for(mine, Some(their_change));
        insert_arm(&pool, &arm).await.expect("insert arm");
        set_status(&pool, mine, "idle").await;
        let FactsProbe::Ready(facts) = read_settle_facts(&pool, &arm, &no_sessions()).await else {
            panic!("facts must read back");
        };
        assert_eq!(
            standing_verdict(&facts),
            StandingVerdict::Drop(CHANGE_RESOLVED)
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_re_arm_is_not_consumed_by_the_resolution_it_replaced() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        seed_thread(&pool, thread, "running").await;
        seed_change(&pool, first, thread, ChangeStatus::Pending).await;
        seed_change(&pool, second, thread, ChangeStatus::Pending).await;

        insert_arm(&pool, &arm_for(thread, Some(first)))
            .await
            .expect("arm");
        let loaded = load_arm(&pool, thread).await.expect("load").expect("armed");

        insert_arm(&pool, &arm_for(thread, Some(second)))
            .await
            .expect("re-arm");

        assert!(
            take_arm_generation(&pool, thread, loaded.generation)
                .await
                .expect("generation take")
                .is_none(),
            "the stale resolution must consume nothing"
        );
        let still = load_arm(&pool, thread).await.expect("load").expect("armed");
        assert_eq!(
            still.arm.change_id,
            Some(second),
            "the newer instruction survives"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_sweep_skips_a_change_the_same_press_is_applying() {
        let (pool, db) = setup_test_db().await;
        let paused = Uuid::new_v4();
        let change = Uuid::new_v4();
        seed_thread(&pool, paused, "paused").await;
        seed_change(&pool, change, paused, ChangeStatus::Pending).await;

        let candidates = read_sweep_candidates(&pool).await.expect("sweep read");
        assert_eq!(
            candidates,
            vec![(paused, Some(change))],
            "a paused thread is a sweep candidate"
        );
        let applying_now = [change];
        let armed: Vec<Uuid> = candidates
            .iter()
            .filter(|(_, c)| !c.is_some_and(|id| applying_now.contains(&id)))
            .map(|(t, _)| *t)
            .collect();
        assert!(armed.is_empty());
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn only_a_settling_thread_reads_as_settling() {
        let (pool, db) = setup_test_db().await;
        let running = Uuid::new_v4();
        let parked = Uuid::new_v4();
        seed_thread(&pool, running, "running").await;
        seed_thread(&pool, parked, "waiting_for_user_answer").await;

        let settling = settling_thread_ids(&pool, [running, parked].into_iter())
            .await
            .expect("settling read");
        assert!(settling.contains(&running));
        assert!(!settling.contains(&parked));
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_sweep_arms_only_threads_that_will_settle() {
        let (pool, db) = setup_test_db().await;
        let running = Uuid::new_v4();
        let idle = Uuid::new_v4();
        let parked = Uuid::new_v4();
        seed_thread(&pool, running, "running").await;
        seed_thread(&pool, idle, "idle").await;
        seed_thread(&pool, parked, "waiting_for_user_answer").await;
        let chat = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_summaries \
               (thread_id, title, source, message_count, last_activity, has_response, \
                is_saved, status, is_coding_agent) \
             VALUES ($1, 'C', 'chat', 0, NOW(), false, false, 'running', false)",
        )
        .bind(chat)
        .execute(&pool)
        .await
        .expect("seed chat thread");

        let candidates = read_sweep_candidates(&pool).await.expect("sweep read");
        let ids: Vec<Uuid> = candidates.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![running]);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_sweep_passes_over_a_repo_lucidos_never_applies_into() {
        let (pool, db) = setup_test_db().await;
        let lucidos = Uuid::new_v4();
        let external = Uuid::new_v4();
        let legacy_external = Uuid::new_v4();
        seed_thread(&pool, lucidos, "running").await;
        seed_thread(&pool, external, "running").await;
        seed_thread(&pool, legacy_external, "running").await;
        set_external_repo(&pool, external, Some("external"), true).await;
        set_external_repo(&pool, legacy_external, None, true).await;

        let candidates = read_sweep_candidates(&pool).await.expect("sweep read");
        let ids: Vec<Uuid> = candidates.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![lucidos]);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn arming_a_repo_lucidos_never_applies_into_is_refused() {
        let (pool, db) = setup_test_db().await;
        let lucidos = Uuid::new_v4();
        let external = Uuid::new_v4();
        let legacy_external = Uuid::new_v4();
        seed_thread(&pool, lucidos, "running").await;
        seed_thread(&pool, external, "running").await;
        seed_thread(&pool, legacy_external, "running").await;
        set_external_repo(&pool, external, Some("external"), true).await;
        set_external_repo(&pool, legacy_external, None, true).await;

        assert!(check_repo_is_appliable(&pool, lucidos).await.is_ok());
        assert!(
            check_repo_is_appliable(&pool, external).await.is_err(),
            "an external-repo thread must be refused"
        );
        assert!(
            check_repo_is_appliable(&pool, legacy_external)
                .await
                .is_err(),
            "a legacy row carrying the bool with no kind must be refused too"
        );
        assert!(check_repo_is_appliable(&pool, Uuid::new_v4()).await.is_ok());
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn an_arm_with_no_settle_to_wait_for_is_refused() {
        let (pool, db) = setup_test_db().await;
        let settled_empty = Uuid::new_v4();
        let settled_with_change = Uuid::new_v4();
        let running = Uuid::new_v4();
        let parked = Uuid::new_v4();
        seed_thread(&pool, settled_empty, "idle").await;
        seed_thread(&pool, settled_with_change, "idle").await;
        seed_thread(&pool, running, "running").await;
        seed_thread(&pool, parked, "waiting_for_user_answer").await;
        seed_change(
            &pool,
            Uuid::new_v4(),
            settled_with_change,
            ChangeStatus::Pending,
        )
        .await;

        async fn refusal(pool: &PgPool, thread: Uuid) -> Option<String> {
            check_arm_has_a_settle(pool, &arm_for(thread, None), &no_sessions())
                .await
                .err()
                .map(|e| e.to_string())
        }
        let empty = refusal(&pool, settled_empty)
            .await
            .expect("nothing to wait for");
        assert!(empty.contains(NOTHING_PROPOSED), "{empty}");
        let question = refusal(&pool, parked)
            .await
            .expect("a question never settles");
        assert!(question.contains(PARKED_ON_QUESTION), "{question}");
        let gone = refusal(&pool, Uuid::new_v4())
            .await
            .expect("no thread to settle");
        assert!(gone.contains(THREAD_GONE), "{gone}");

        assert_eq!(
            refusal(&pool, running).await,
            None,
            "a running thread will settle"
        );
        assert_eq!(
            refusal(&pool, settled_with_change).await,
            None,
            "a settled thread with a pending change applies now"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_withheld_settle_refuses_an_unbound_arm_with_its_reason() {
        let (pool, db) = setup_test_db().await;
        let running = Uuid::new_v4();
        let settled = Uuid::new_v4();
        seed_thread(&pool, running, "running").await;
        seed_thread(&pool, settled, "idle").await;
        let withheld_work = CodingAgentChangeState::Unproposed {
            reason: Some(UnproposedReason::TurnIncomplete),
        };
        seed_change_state(&pool, running, withheld_work).await;
        seed_change_state(&pool, settled, withheld_work).await;

        async fn refusal(pool: &PgPool, thread: Uuid) -> Option<String> {
            check_arm_has_a_settle(pool, &arm_for(thread, None), &no_sessions())
                .await
                .err()
                .map(|e| e.to_string())
        }
        assert_eq!(refusal(&pool, running).await, None);
        let refused = refusal(&pool, settled)
            .await
            .expect("a settled thread's withheld work has nothing to wait for");
        assert!(
            refused.contains(withheld(UnproposedReason::TurnIncomplete)),
            "{refused}"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_bound_arm_on_a_withdrawn_change_reads_it_as_gone() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        let change = Uuid::new_v4();
        seed_thread(&pool, thread, "running").await;
        seed_change(&pool, change, thread, ChangeStatus::Pending).await;
        let arm = arm_for(thread, Some(change));
        insert_arm(&pool, &arm).await.expect("insert arm");
        sqlx::query("UPDATE changes SET status = $2 WHERE id = $1")
            .bind(change)
            .bind(ChangeStatus::Withdrawn)
            .execute(&pool)
            .await
            .expect("withdraw the change");

        let FactsProbe::Ready(facts) = read_settle_facts(&pool, &arm, &no_sessions()).await else {
            panic!("facts must read back");
        };
        assert_eq!(facts.armed_change, ArmedChange::Gone(CHANGE_WITHDRAWN));
        assert_eq!(
            standing_verdict(&facts),
            StandingVerdict::Drop(CHANGE_WITHDRAWN)
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_workspace_scope_off_covers_both_kinds_of_arm() {
        let (pool, db) = setup_test_db().await;
        let single = Uuid::new_v4();
        let swept = Uuid::new_v4();
        seed_thread(&pool, single, "running").await;
        seed_thread(&pool, swept, "running").await;
        insert_arm(&pool, &arm_for(single, None))
            .await
            .expect("arm");
        insert_arm(&pool, &swept_arm(swept))
            .await
            .expect("sweep arm");

        let mut all = read_scoped_arms(&pool, DisarmScope::All)
            .await
            .expect("read all");
        all.sort();
        let mut both = vec![single, swept];
        both.sort();
        assert_eq!(all, both, "the workspace off covers every arm");

        assert_eq!(
            read_scoped_arms(&pool, DisarmScope::Sweep)
                .await
                .expect("read sweep"),
            vec![swept],
            "Apply All Cancel leaves an arm the owner set on one change"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_settle_after_a_disarm_has_no_arm_to_fire() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        let change = Uuid::new_v4();
        seed_thread(&pool, thread, "running").await;
        seed_change(&pool, change, thread, ChangeStatus::Pending).await;
        insert_arm(&pool, &arm_for(thread, Some(change)))
            .await
            .expect("arm");

        assert!(take_arm(&pool, thread).await.expect("disarm").is_some());

        set_status(&pool, thread, "idle").await;
        assert!(
            load_arm(&pool, thread).await.expect("load").is_none(),
            "a settled thread must carry no instruction after the owner canceled"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_fire_stands_down_when_the_disarm_won_the_race() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        let change = Uuid::new_v4();
        seed_thread(&pool, thread, "running").await;
        seed_change(&pool, change, thread, ChangeStatus::Pending).await;
        insert_arm(&pool, &arm_for(thread, Some(change)))
            .await
            .expect("arm");

        let loaded = load_arm(&pool, thread).await.expect("load").expect("armed");
        assert!(take_arm(&pool, thread).await.expect("disarm").is_some());

        assert!(
            take_arm_generation(&pool, thread, loaded.generation)
                .await
                .expect("generation take")
                .is_none(),
            "the fire must consume nothing, so it returns before applying"
        );
        teardown_test_db(&db).await;
    }

    async fn fired_events(pool: &PgPool, thread_id: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM events \
              WHERE event_type = 'StandingApplyFired' \
                AND payload->'data'->>'thread_id' = $1",
        )
        .bind(thread_id.to_string())
        .fetch_one(pool)
        .await
        .expect("count fired events")
    }

    #[tokio::test]
    async fn a_fire_consumes_its_arm_and_announces_it() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        let change = Uuid::new_v4();
        seed_thread(&pool, thread, "idle").await;
        seed_change(&pool, change, thread, ChangeStatus::Pending).await;
        insert_arm(&pool, &arm_for(thread, Some(change)))
            .await
            .expect("arm");
        let loaded = load_arm(&pool, thread).await.expect("load").expect("armed");

        let fired = take_arm_to_fire(&pool, &bus, &loaded, change)
            .await
            .expect("take")
            .expect("the arm was still there");
        assert_eq!(fired.arm.thread_id, thread);
        assert!(load_arm(&pool, thread).await.expect("load").is_none());
        assert_eq!(fired_events(&pool, thread).await, 1);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_fire_that_lost_the_race_announces_nothing() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        let change = Uuid::new_v4();
        seed_thread(&pool, thread, "idle").await;
        seed_change(&pool, change, thread, ChangeStatus::Pending).await;
        insert_arm(&pool, &arm_for(thread, Some(change)))
            .await
            .expect("arm");
        let loaded = load_arm(&pool, thread).await.expect("load").expect("armed");
        assert!(take_arm(&pool, thread).await.expect("disarm").is_some());

        assert!(take_arm_to_fire(&pool, &bus, &loaded, change)
            .await
            .expect("take")
            .is_none());
        assert_eq!(fired_events(&pool, thread).await, 0);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn restored_arm_settlement_is_durable_and_retryable() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        seed_thread(&pool, thread, "running").await;
        insert_arm(&pool, &arm_for(thread, None)).await.unwrap();
        sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) VALUES ($1, 'snapshot', now(), 0)")
            .bind(Uuid::new_v4()).execute(&pool).await.unwrap();
        crate::test_support::refuse_event_type(&pool, "StandingApplyDropped").await;
        assert!(settle_restored_standing_applies(&pool, &bus).await.is_err());
        assert!(load_arm(&pool, thread).await.unwrap().is_some());
        sqlx::query("DROP TRIGGER refuse_event ON events")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE FUNCTION refuse_arm_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'delete refused'; END $$")
            .execute(&pool).await.unwrap();
        sqlx::query("CREATE TRIGGER refuse_arm_delete BEFORE DELETE ON standing_applies FOR EACH ROW EXECUTE FUNCTION refuse_arm_delete()")
            .execute(&pool).await.unwrap();
        assert!(settle_restored_standing_applies(&pool, &bus).await.is_err());
        assert!(load_arm(&pool, thread).await.unwrap().is_some());
        sqlx::query("DROP TRIGGER refuse_arm_delete ON standing_applies")
            .execute(&pool)
            .await
            .unwrap();
        for _ in 0..2 {
            settle_restored_standing_applies(&pool, &bus).await.unwrap();
            assert!(load_arm(&pool, thread).await.unwrap().is_none());
        }
        let drops: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE event_type = 'StandingApplyDropped'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(drops, 1);
        insert_arm(&pool, &arm_for(thread, None)).await.unwrap();
        settle_restored_standing_applies(&pool, &bus).await.unwrap();
        assert!(load_arm(&pool, thread).await.unwrap().is_some());
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn restored_standing_arm_cannot_fire_but_a_new_instruction_can() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        let change = Uuid::new_v4();
        seed_thread(&pool, thread, "idle").await;
        seed_change(&pool, change, thread, ChangeStatus::Pending).await;
        insert_arm(&pool, &arm_for(thread, Some(change)))
            .await
            .unwrap();
        sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) VALUES ($1, $2, now(), 0)")
            .bind(Uuid::new_v4()).bind(crate::core::snapshot::handle::Handle::generate().as_str())
            .execute(&pool).await.unwrap();
        for _ in 0..2 {
            let loaded = load_arm(&pool, thread).await.unwrap().unwrap();
            assert!(take_arm_to_fire(&pool, &bus, &loaded, change)
                .await
                .unwrap()
                .is_none());
            assert_eq!(fired_events(&pool, thread).await, 0);
        }
        insert_arm(&pool, &arm_for(thread, Some(change)))
            .await
            .unwrap();
        let loaded = load_arm(&pool, thread).await.unwrap().unwrap();
        assert!(take_arm_to_fire(&pool, &bus, &loaded, change)
            .await
            .unwrap()
            .is_some());
        assert_eq!(fired_events(&pool, thread).await, 1);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_sweep_binds_a_pending_change_where_there_is_one() {
        let (pool, db) = setup_test_db().await;
        let with_change = Uuid::new_v4();
        let change = Uuid::new_v4();
        seed_thread(&pool, with_change, "running").await;
        seed_change(&pool, change, with_change, ChangeStatus::Pending).await;

        let candidates = read_sweep_candidates(&pool).await.expect("sweep read");
        assert_eq!(candidates, vec![(with_change, Some(change))]);
        teardown_test_db(&db).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(status: &str, armed_change: ArmedChange) -> SettleFacts {
        SettleFacts {
            status: status.to_string(),
            parked_on_question: false,
            live_event_waits: false,
            change_state: CodingAgentChangeState::None,
            armed_change,
            turn_settle: TurnSettle::Settled,
        }
    }

    #[test]
    fn an_open_question_drops_the_arm_on_an_idle_row() {
        let id = Uuid::new_v4();
        for (status, waits) in [("idle", false), ("idle", true)] {
            let parked = SettleFacts {
                parked_on_question: true,
                live_event_waits: waits,
                ..facts(status, ArmedChange::Ready(id))
            };
            assert_eq!(
                standing_verdict(&parked),
                StandingVerdict::Drop(PARKED_ON_QUESTION),
                "{status}, waits={waits}"
            );
        }
    }

    #[test]
    fn a_settled_thread_fires_its_armed_change() {
        let id = Uuid::new_v4();
        assert_eq!(
            standing_verdict(&facts("idle", ArmedChange::Ready(id))),
            StandingVerdict::Fire(id)
        );
    }

    #[test]
    fn a_working_thread_keeps_waiting() {
        let id = Uuid::new_v4();
        assert_eq!(
            standing_verdict(&facts("running", ArmedChange::Ready(id))),
            StandingVerdict::Wait
        );
    }

    #[test]
    fn a_paused_turn_keeps_the_arm() {
        let id = Uuid::new_v4();
        assert_eq!(
            standing_verdict(&facts("paused", ArmedChange::Ready(id))),
            StandingVerdict::Wait
        );
    }

    #[test]
    fn a_parked_thread_drops_with_a_report() {
        let id = Uuid::new_v4();
        assert_eq!(
            standing_verdict(&facts("waiting_for_user_answer", ArmedChange::Ready(id))),
            StandingVerdict::Drop(PARKED_ON_QUESTION)
        );

        let mut both = facts("waiting_for_user_answer", ArmedChange::Ready(id));
        both.live_event_waits = true;
        assert_eq!(
            standing_verdict(&both),
            StandingVerdict::Drop(PARKED_ON_QUESTION)
        );
    }

    #[test]
    fn a_thread_watching_an_event_keeps_the_arm() {
        let id = Uuid::new_v4();
        let mut watching = facts("idle", ArmedChange::Ready(id));
        watching.live_event_waits = true;
        assert_eq!(standing_verdict(&watching), StandingVerdict::Wait);

        watching.live_event_waits = false;
        assert_eq!(standing_verdict(&watching), StandingVerdict::Fire(id));
    }

    #[test]
    fn a_wait_resolution_that_wakes_the_thread_does_not_re_resolve() {
        let thread_id = Uuid::new_v4();
        let wait_id = Uuid::new_v4();
        let on_thread = |event: ThreadEvent| BusEvent::Thread {
            thread_id,
            event,
            meta: Default::default(),
        };

        let delivered = on_thread(ThreadEvent::EventWaitDelivered {
            wait_id,
            event_id: Uuid::new_v4(),
            event_type: "E2ELockReleased".to_string(),
            payload: serde_json::json!({}),
            matched_index: 0,
        });
        let expired = on_thread(ThreadEvent::EventWaitExpired { wait_id });
        assert_eq!(thread_to_resolve(&delivered), None);
        assert_eq!(thread_to_resolve(&expired), None);

        let canceled = on_thread(ThreadEvent::EventWaitCanceled {
            wait_id,
            cause: crate::engine::thread_events::EventWaitCancelCause::UserStop,
            on: Vec::new(),
            reason: String::new(),
        });
        let anchor = on_thread(ThreadEvent::PromptInjected {
            text: "An event you subscribed to has arrived".to_string(),
            mode: crate::engine::thread_events::ActorMode::Agent,
            origin: None,
            injected_message_id: None,
            delivered_event_id: None,
        });
        assert_eq!(thread_to_resolve(&canceled), Some(thread_id));
        assert_eq!(thread_to_resolve(&anchor), Some(thread_id));
    }

    #[test]
    fn a_turn_in_flight_waits_and_a_cut_off_one_drops() {
        let id = Uuid::new_v4();
        let mut mid_settle = facts("idle", ArmedChange::Ready(id));
        mid_settle.turn_settle = TurnSettle::InFlight;
        assert_eq!(standing_verdict(&mid_settle), StandingVerdict::Wait);

        mid_settle.turn_settle = TurnSettle::CutOff;
        assert_eq!(
            standing_verdict(&mid_settle),
            StandingVerdict::Drop(TURN_CUT_OFF)
        );

        mid_settle.armed_change = ArmedChange::Gone(CHANGE_RESOLVED);
        assert_eq!(
            standing_verdict(&mid_settle),
            StandingVerdict::Drop(CHANGE_RESOLVED)
        );
    }

    #[test]
    fn the_settle_is_in_flight_only_behind_a_live_session() {
        assert_eq!(classify_turn_settle(false, true), TurnSettle::Settled);
        assert_eq!(classify_turn_settle(false, false), TurnSettle::Settled);
        assert_eq!(classify_turn_settle(true, true), TurnSettle::InFlight);
        assert_eq!(classify_turn_settle(true, false), TurnSettle::CutOff);
    }

    #[test]
    fn a_settle_step_re_resolves_and_the_facts_decide() {
        let thread_id = Uuid::new_v4();
        let on_thread = |event: ThreadEvent| BusEvent::Thread {
            thread_id,
            event,
            meta: Default::default(),
        };
        let terminal = on_thread(ThreadEvent::ResponseGenerated {
            text: "done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        });
        let diff_flag = on_thread(ThreadEvent::CodingAgentDiffChanged { has_diff: true });
        let idle = on_thread(ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        });
        let permission_card = on_thread(ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-1".into(),
            tool_use_id: "tool-1".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({}),
            summary: "Edit a file".into(),
            grant_offer: None,
        });
        for event in [&terminal, &diff_flag, &idle, &permission_card] {
            assert_eq!(thread_to_resolve(event), Some(thread_id));
        }
    }

    #[test]
    fn a_failed_turn_drops_with_a_report() {
        let id = Uuid::new_v4();
        assert_eq!(
            standing_verdict(&facts("failed", ArmedChange::Ready(id))),
            StandingVerdict::Drop(TURN_FAILED)
        );
    }

    #[test]
    fn only_a_settling_thread_waits() {
        let id = Uuid::new_v4();
        for status in [
            "idle",
            "waiting",
            "waiting_for_user_answer",
            "failed",
            "some-future-status",
        ] {
            assert_ne!(
                standing_verdict(&facts(status, ArmedChange::Ready(id))),
                StandingVerdict::Wait,
                "status {status} must resolve rather than wait"
            );
        }
    }

    #[test]
    fn a_resolved_change_drops_even_mid_turn() {
        assert_eq!(
            standing_verdict(&facts("running", ArmedChange::Gone(CHANGE_RESOLVED))),
            StandingVerdict::Drop(CHANGE_RESOLVED)
        );
    }

    #[test]
    fn a_withheld_settle_drops_with_the_reason_and_a_settling_one_waits() {
        for reason in UnproposedReason::ALL {
            let withheld_facts = SettleFacts {
                change_state: CodingAgentChangeState::Unproposed {
                    reason: Some(reason),
                },
                ..facts("idle", ArmedChange::Unproposed)
            };
            assert_eq!(
                standing_verdict(&withheld_facts),
                StandingVerdict::Drop(withheld(reason)),
                "{reason:?}"
            );
            for status in ["running", "paused"] {
                let settling = SettleFacts {
                    status: status.to_string(),
                    ..withheld_facts.clone()
                };
                assert_eq!(
                    standing_verdict(&settling),
                    StandingVerdict::Wait,
                    "{reason:?} {status}"
                );
            }
            let mut mid_settle = withheld_facts.clone();
            mid_settle.turn_settle = TurnSettle::InFlight;
            assert_eq!(standing_verdict(&mid_settle), StandingVerdict::Wait);
        }
    }

    #[test]
    fn each_withheld_reason_reads_differently() {
        let copies: HashSet<&str> = UnproposedReason::ALL.into_iter().map(withheld).collect();
        assert_eq!(copies.len(), UnproposedReason::ALL.len());
    }

    #[test]
    fn a_settled_thread_with_a_diff_waits_for_its_proposal() {
        let mut with_diff = facts("idle", ArmedChange::Unproposed);
        with_diff.change_state = CodingAgentChangeState::Unproposed { reason: None };
        assert_eq!(standing_verdict(&with_diff), StandingVerdict::Wait);

        let without = facts("idle", ArmedChange::Unproposed);
        assert_eq!(
            standing_verdict(&without),
            StandingVerdict::Drop(NOTHING_PROPOSED)
        );
    }

    #[test]
    fn bound_change_classification() {
        let id = Uuid::new_v4();
        assert_eq!(
            classify_bound_change(Some((id, ChangeStatus::Pending, 3))),
            ArmedChange::Ready(id)
        );
        assert_eq!(
            classify_bound_change(Some((id, ChangeStatus::Applied, 3))),
            ArmedChange::Gone(CHANGE_RESOLVED)
        );
        assert_eq!(
            classify_bound_change(Some((id, ChangeStatus::Discarded, 3))),
            ArmedChange::Gone(CHANGE_RESOLVED)
        );
        assert_eq!(
            classify_bound_change(Some((id, ChangeStatus::Pending, 0))),
            ArmedChange::Gone(CHANGE_EMPTY)
        );
        assert_eq!(
            classify_bound_change(Some((id, ChangeStatus::Withdrawn, 3))),
            ArmedChange::Gone(CHANGE_WITHDRAWN),
            "a withdrawn change went back to its branch, so the arm can never fire"
        );
        assert_eq!(
            classify_bound_change(Some((id, ChangeStatus::SetAside, 3))),
            ArmedChange::Gone(CHANGE_SET_ASIDE)
        );
        assert_eq!(
            classify_bound_change(None),
            ArmedChange::Gone(CHANGE_RESOLVED)
        );
    }

    #[test]
    fn armed_threads_tracks_arm_and_disarm() {
        let armed = ArmedThreads::default();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert!(!armed.contains(a));
        armed.insert(a);
        armed.insert(b);
        assert!(armed.contains(a) && armed.contains(b));
        armed.remove(a);
        assert!(!armed.contains(a) && armed.contains(b));
        armed.replace(HashSet::from([a]));
        assert_eq!(armed.snapshot(), vec![a]);
    }
}
