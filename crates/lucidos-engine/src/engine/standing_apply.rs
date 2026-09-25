//! The *standing apply*: the owner's instruction to apply a change once its
//! thread settles. ADR 0168 clause 5, and
//! `docs/plans/2026-08-30-a-thread-acts-in-its-own-subtree.md` phase 5.
//!
//! Apply is the owner's button. A press is one moment, and the work it acts on
//! often finishes hours later. So the press is recorded as engine state and
//! carried out by the engine. No thread waits on it, and no thread reaches
//! sideways to deliver it.
//!
//! Two forms share one record. A **single** arm names one change. A **sweep**
//! arms every thread still settling when the owner pressed Apply All with "Keep
//! going as the rest settle". Both live in `standing_applies`, one row per
//! armed thread, and both are one-shot.
//!
//! [`standing_verdict`] carries the two invariants. It always ends, and it acts
//! only on the change it was armed for. Each is argued where it is enforced.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use uuid::Uuid;

use crate::core::changes::ChangeStatus;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::{MessageOrigin, ThreadEvent};
use crate::engine::types::AgentSession;
use crate::engine::LucidosEngine;

/// The engine's in-memory agent sessions, keyed by thread.
type AgentSessions = tokio::sync::Mutex<HashMap<Uuid, AgentSession>>;

/// One armed standing apply, as stored.
#[derive(Debug, Clone)]
pub struct StandingApply {
    pub thread_id: Uuid,
    /// The change this arm is bound to. `None` means the thread was still
    /// working with nothing proposed, so the arm takes whatever it proposes.
    pub change_id: Option<Uuid>,
    /// The Apply All sweep that armed this thread, when a sweep did.
    pub batch_id: Option<Uuid>,
    pub actor: Option<MessageOrigin>,
}

/// What the armed change looks like right now, resolved against the arm's
/// binding before the verdict is taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArmedChange {
    /// Still pending, with files to merge.
    Ready(Uuid),
    /// It resolved, vanished, or emptied out, so this arm can never fire.
    Gone(&'static str),
    /// Nothing is bound and nothing is pending yet.
    Unproposed,
}

/// The thread-state facts the verdict reads. One struct so the decision is
/// testable without a database.
#[derive(Debug, Clone)]
pub(crate) struct SettleFacts {
    /// `thread_summaries.status`.
    pub status: String,
    pub live_event_waits: bool,
    /// `thread_summaries.coding_agent_has_diff`: the branch holds commits the
    /// projection has seen. A settled thread with a diff and no pending change
    /// is one whose `ChangeProposed` is still on its way.
    pub has_diff: bool,
    pub armed_change: ArmedChange,
    pub turn_settle: TurnSettle,
}

/// Where the thread's last agent turn stands between its terminal and its
/// idle.
///
/// The terminal sets status `idle`, and then the run loop auto-commits,
/// proposes and emits `CodingAgentIdled`. A verdict taken in between would
/// merge a branch still being written. Every resolve reads this, whatever
/// triggered it, so no path can fire in that window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnSettle {
    /// No agent turn sits between its terminal and its idle.
    Settled,
    /// A live session sent its terminal and is on its way to the idle.
    InFlight,
    /// The terminal is the newest closer, but no live session will send the
    /// idle. After a restart every session is gone, so a turn the old engine
    /// killed mid-settle lands here, possibly with its last edits uncommitted.
    CutOff,
}

/// Classify the settle from its two halves. Pure.
///
/// `terminal_is_newest`: the thread's newest turn closer is a
/// `ResponseGenerated`. For a coding agent only the run loop's Result arm
/// emits one, and that arm always idles unless the engine dies first.
fn classify_turn_settle(terminal_is_newest: bool, session_live: bool) -> TurnSettle {
    match (terminal_is_newest, session_live) {
        (false, _) => TurnSettle::Settled,
        (true, true) => TurnSettle::InFlight,
        (true, false) => TurnSettle::CutOff,
    }
}

/// What the resolver should do with an arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StandingVerdict {
    /// The thread is still settling, or its change has not landed yet.
    Wait,
    Fire(Uuid),
    /// End the arm and report this reason to the owner.
    Drop(&'static str),
}

/// Reported when a thread parks on a question card. ADR 0168 names this case:
/// such a thread never settles by itself, so the arm ends here.
pub(crate) const PARKED_ON_QUESTION: &str = "The thread parked on a question.";
pub(crate) const TURN_FAILED: &str = "The turn failed.";
pub(crate) const NOTHING_PROPOSED: &str = "The thread settled without proposing a change.";
pub(crate) const CHANGE_RESOLVED: &str = "The change was already applied or discarded.";
pub(crate) const CHANGE_EMPTY: &str = "The change has no file changes left.";
pub(crate) const THREAD_GONE: &str = "The thread no longer exists.";
pub(crate) const TURN_CUT_OFF: &str = "The session stopped before its turn finished settling.";
/// Reported when the owner takes the instruction back, by hand or by
/// cancelling the sweep that armed it.
pub const DISARMED_BY_OWNER: &str = "Canceled.";

/// A *settling* `thread_summaries` row: one that will settle by itself.
/// Running, paused, or watching an event without a question card open.
///
/// Exactly the thread states [`standing_verdict`] waits on. Every place that
/// offers or arms a standing apply reads this. So an arm always has a settle
/// to wait for, rather than dropping the moment it is set (ADR 0266).
///
/// Unqualified, like [`LUCIDOS_APPLIES_SQL`], so `thread_summaries` must be the
/// only source of these columns in the query using it.
const SETTLING_THREAD_SQL: &str = "(status IN ('running', 'paused') \
     OR (live_event_wait_count > 0 AND status NOT IN ('waiting_for_user_answer', 'failed')))";

/// A `thread_summaries` row whose change Lucidos would apply. The standing
/// apply IS an Apply, one settle later, so it is offered exactly where Apply
/// is. An external repo is reviewed and pushed from the repo itself.
///
/// Such a thread never proposes at all (`may_touch_change_state_at_idle`), so
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

/// Refused when the owner arms an apply on a repo Lucidos does not merge into.
const EXTERNAL_REPO_REFUSAL: &str =
    "Lucidos does not apply changes into an external repo. Review and push that \
     work from the repo itself.";

/// Which arms one bulk disarm takes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisarmScope {
    /// Only what a sweep armed. Apply All Cancel means "stop applying", and an
    /// arm the owner set on one change is not part of that press.
    Sweep,
    /// Every arm here. The Changes panel's own off: that surface draws one
    /// armed state for the whole workspace, so its off has to mean the same.
    All,
}

impl DisarmScope {
    /// The clause selecting this scope's rows. Empty means every row.
    fn predicate(self) -> &'static str {
        match self {
            DisarmScope::Sweep => "WHERE batch_id IS NOT NULL",
            DisarmScope::All => "",
        }
    }
}

/// Resolve one arm against the current facts. Pure.
///
/// **It always ends.** Only a *settling* thread waits: `running`, `paused`,
/// watching an event, or a turn in flight to its idle. Each promises a settle,
/// since every wait ends by itself. A question card never does, so parking on
/// one drops the arm and reports it. So does a turn no session will finish.
///
/// A change that can never be applied ends the arm whatever the thread is
/// doing. Waiting for a turn to finish cannot bring a discarded change back.
pub(crate) fn standing_verdict(facts: &SettleFacts) -> StandingVerdict {
    if let ArmedChange::Gone(reason) = facts.armed_change {
        return StandingVerdict::Drop(reason);
    }
    match facts.status.as_str() {
        // Still working. Nothing to decide yet.
        "running" => StandingVerdict::Wait,
        // The engine promised to resume this turn, so the arm keeps its place.
        "paused" => StandingVerdict::Wait,
        "waiting_for_user_answer" => StandingVerdict::Drop(PARKED_ON_QUESTION),
        "failed" => StandingVerdict::Drop(TURN_FAILED),
        // Watching an event: it wakes and may commit again, so it has not
        // settled (ADR 0106). A running sub-thread does not count (ADR 0249).
        _ if facts.live_event_waits => StandingVerdict::Wait,
        // The session is still committing and proposing. Its idle re-resolves.
        _ if facts.turn_settle == TurnSettle::InFlight => StandingVerdict::Wait,
        // Nothing will finish this settle, and firing could miss its last edits.
        _ if facts.turn_settle == TurnSettle::CutOff => StandingVerdict::Drop(TURN_CUT_OFF),
        _ => match facts.armed_change {
            ArmedChange::Ready(change_id) => StandingVerdict::Fire(change_id),
            // The proposal follows the turn's terminal, so a diff on the branch
            // means it is on its way and `ChangeProposed` re-drives this
            // resolution.
            ArmedChange::Unproposed if facts.has_diff => StandingVerdict::Wait,
            ArmedChange::Unproposed => StandingVerdict::Drop(NOTHING_PROPOSED),
            // Handled above, before the status match.
            ArmedChange::Gone(reason) => StandingVerdict::Drop(reason),
        },
    }
}

/// The thread whose arm this bus event re-resolves, if any. Pure.
///
/// **A wait's delivery or expiry never re-takes a verdict.** Each clears the
/// live-wait count while the status still reads `idle`. The `UserPromptInjected`
/// anchor right behind it moves the thread to `running`. A verdict taken
/// between the two would fire on work the wake is about to extend (ADR 0266).
/// A cancel starts no wake, so it resolves like any other event.
///
/// An agent session's settle needs no skip here. [`TurnSettle`] holds the arm
/// until the idle, on this path and on every resolve with no event behind it.
fn thread_to_resolve(event: &BusEvent) -> Option<Uuid> {
    match event {
        BusEvent::Thread {
            event: ThreadEvent::EventWaitDelivered { .. } | ThreadEvent::EventWaitExpired { .. },
            ..
        } => None,
        BusEvent::Thread { thread_id, .. } => Some(*thread_id),
        BusEvent::System(_) => None,
    }
}

/// Classify the change an arm is bound to from its stored row. Pure, so the
/// mapping is testable without a database. `None` = the row is gone.
pub(crate) fn classify_bound_change(row: Option<(Uuid, ChangeStatus, i32)>) -> ArmedChange {
    match row {
        None => ArmedChange::Gone(CHANGE_RESOLVED),
        Some((_, status, _)) if status != ChangeStatus::Pending => {
            ArmedChange::Gone(CHANGE_RESOLVED)
        }
        Some((_, _, 0)) => ArmedChange::Gone(CHANGE_EMPTY),
        Some((id, _, _)) => ArmedChange::Ready(id),
    }
}

/// In-memory set of armed thread ids.
///
/// The durable table is the authority. This is the cheap filter the bus
/// subscriber consults per event, so an engine with nothing armed runs no
/// query.
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

    /// A poisoned lock means a panic while the set was borrowed. This set is a
    /// filter rather than the authority, so recovering the inner value keeps
    /// the resolver working. The next boot rebuilds it.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashSet<Uuid>> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashSet<Uuid>> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// The outcome of reading an arm's facts. `Unknown` keeps the arm: a probe
/// that could not run is not evidence the thread settled or died.
enum FactsProbe {
    Ready(SettleFacts),
    ThreadGone,
    Unknown,
}

/// One `standing_applies` row as read back.
type StandingApplyRow = (
    Uuid,
    Option<Uuid>,
    Option<Uuid>,
    Option<serde_json::Value>,
    chrono::DateTime<chrono::Utc>,
);

const ARM_COLUMNS: &str = "thread_id, change_id, batch_id, actor, created_at";

/// An arm as read back, carrying the generation token that says WHICH arm it
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

/// Store one arm, replacing whatever that thread carried.
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

/// Read one arm without consuming it.
async fn load_arm(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<Option<LoadedArm>, sqlx::Error> {
    let row: Option<StandingApplyRow> = sqlx::query_as(&format!(
        "SELECT {ARM_COLUMNS} FROM standing_applies WHERE thread_id = $1"
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(hydrate_arm))
}

/// Delete whatever arm the thread carries, whoever armed it. The owner's own
/// disarm, which is about the thread rather than about one instruction.
async fn take_arm(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<Option<LoadedArm>, sqlx::Error> {
    let row: Option<StandingApplyRow> = sqlx::query_as(&format!(
        "DELETE FROM standing_applies WHERE thread_id = $1 RETURNING {ARM_COLUMNS}"
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(hydrate_arm))
}

/// Delete one arm only if it is still the one the caller read.
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

/// Consume the arm a resolution read for firing, and announce that it fired.
///
/// The announcement follows the consume. So a fire that lost the race to a
/// disarm or a re-arm says nothing, and whoever won reports the arm's end.
async fn take_arm_to_fire(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    loaded: &LoadedArm,
    change_id: Uuid,
) -> Result<Option<LoadedArm>, sqlx::Error> {
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

/// Read the facts one arm's verdict needs.
///
/// A probe that could not run answers [`FactsProbe::Unknown`], never a "no". An
/// unknown must not end an instruction the owner set, so the arm keeps its
/// place and the next event retries.
async fn read_settle_facts(
    pool: &sqlx::PgPool,
    arm: &StandingApply,
    sessions: &AgentSessions,
) -> FactsProbe {
    let probe: Result<Option<(String, i32, bool, bool)>, sqlx::Error> = sqlx::query_as(
        "SELECT status, live_event_wait_count, coding_agent_has_diff, is_coding_agent \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(arm.thread_id)
    .fetch_optional(pool)
    .await;
    let (status, live_waits, has_diff, is_coding_agent) = match probe {
        Ok(Some(row)) => row,
        Ok(None) => return FactsProbe::ThreadGone,
        Err(e) => {
            log!(
                "[StandingApply] thread lookup for {} failed: {}",
                arm.thread_id,
                e
            );
            return FactsProbe::Unknown;
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
        // Only an agent session idles, so a chat thread's terminal settles it.
        TurnSettle::Settled
    };
    FactsProbe::Ready(SettleFacts {
        status,
        live_event_waits: live_waits > 0,
        has_diff,
        armed_change,
        turn_settle,
    })
}

/// The events that close an agent turn: its idle, and its four terminals.
///
/// A coding-agent thread also carries the Lucidos Agent's turns, for example
/// in a voice call. Their replies carry no `claude_code` channel, and no idle
/// follows them, so they must not read as an agent settle. Every agent session
/// stamps that channel, Codex included.
const AGENT_TURN_CLOSER_SQL: &str = "(event_type = 'CodingAgentIdled' \
     OR (event_type IN ('ResponseGenerated', 'ResponseCanceled', 'ResponseFailed', 'ResponseAborted') \
         AND payload->>'channel' = 'claude_code'))";

/// Read where the thread's last turn stands. `None` = the probe failed.
///
/// The durable half comes from the event log. `EventBus::emit` writes an event
/// before it broadcasts it, so a resolve that the idle triggered already sees
/// the idle. The session map is read only when the terminal is newest, so a
/// settled thread never takes the lock.
async fn read_turn_settle(
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
        Ok(event_type) => event_type.as_deref() == Some("ResponseGenerated"),
        Err(e) => {
            log!(
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

/// What this arm would apply right now. `None` = the probe failed.
///
/// **It acts only on the change it was armed for.** A bound arm asks about its
/// own `change_id`, so a second change proposed on the same thread is a row the
/// arm cannot name. An unbound arm asks whether the thread has proposed
/// anything yet, and is consumed by the first one that lands.
async fn read_armed_change(pool: &sqlx::PgPool, arm: &StandingApply) -> Option<ArmedChange> {
    // Scoped to the arm's own thread, so a binding that named somebody else's
    // change can never fire whatever wrote it. `arm_standing_apply` refuses one
    // at the door; this is what makes the refusal structural.
    let probe: Result<Option<(Uuid, ChangeStatus, i32)>, sqlx::Error> = match arm.change_id {
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
                 WHERE thread_id = $1 AND status = 'pending' \
                 ORDER BY created_at LIMIT 1",
            )
            .bind(arm.thread_id)
            .fetch_optional(pool)
            .await
        }
    };
    match probe {
        Ok(None) if arm.change_id.is_none() => Some(ArmedChange::Unproposed),
        Ok(row) => Some(classify_bound_change(row)),
        Err(e) => {
            log!(
                "[StandingApply] change lookup for {} failed: {}",
                arm.thread_id,
                e
            );
            None
        }
    }
}

/// How many threads a sweep would arm right now.
///
/// `GET /api/v1/changes` carries it so the Changes panel can offer "Apply as
/// they settle" with nothing pending. The panel cannot derive it: its thread
/// map holds only the loaded window.
pub(crate) async fn count_sweep_candidates(pool: &sqlx::PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "SELECT count(*) FROM thread_summaries \
          WHERE is_coding_agent = TRUE AND {SETTLING_THREAD_SQL} AND {LUCIDOS_APPLIES_SQL}"
    ))
    .fetch_one(pool)
    .await
}

/// Refuse a thread whose repo Lucidos never applies into.
///
/// Same shape as [`check_binding`] and for the same reason: the instruction
/// could never be carried out, so it is refused at the door. A thread the
/// query cannot find falls through to [`check_arm_has_a_settle`], which names
/// `THREAD_GONE`.
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

/// Refuse a binding that names a change this thread does not own, or one that
/// has already resolved. Arming either is an instruction that can never be
/// carried out, so it is refused at the door rather than dropped later.
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

/// Refuse an arm whose first verdict would already drop it.
///
/// Such an arm has no settle to wait for: the thread settled with nothing to
/// apply, parked on a question, or failed. Stored, it would drop a moment
/// later and warn the owner about an instruction nobody needed. A probe that
/// could not run lets the arm through, and its own resolution retries.
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

/// Of the given threads, the *settling* ones, so a standing apply on them has
/// a settle to wait for.
///
/// `PendingThreadState::settling` is filled from this. It stops the Changes panel
/// offering an arm on a thread parked on a question, which would drop the
/// moment it was pressed.
pub async fn settling_thread_ids(
    pool: &sqlx::PgPool,
    thread_ids: impl Iterator<Item = Uuid>,
) -> Result<HashSet<Uuid>, sqlx::Error> {
    let ids: Vec<Uuid> = thread_ids.collect();
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let rows: Vec<(Uuid,)> = sqlx::query_as(&format!(
        "SELECT thread_id FROM thread_summaries \
          WHERE thread_id = ANY($1) AND is_coding_agent = TRUE AND {SETTLING_THREAD_SQL}"
    ))
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// The armed threads one [`DisarmScope`] covers.
///
/// Its own function so the scope's rule is one query the tests read directly,
/// rather than a string built inside the loop that consumes it.
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

/// Threads an Apply All sweep should arm, each with its pending change if it
/// has one.
async fn read_sweep_candidates(
    pool: &sqlx::PgPool,
) -> Result<Vec<(Uuid, Option<Uuid>)>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT t.thread_id, \
                (SELECT c.id FROM changes c \
                  WHERE c.thread_id = t.thread_id AND c.status = 'pending' \
                  ORDER BY c.created_at LIMIT 1) \
           FROM thread_summaries t \
          WHERE t.is_coding_agent = TRUE AND {SETTLING_THREAD_SQL} \
            AND {LUCIDOS_APPLIES_SQL}"
    ))
    .fetch_all(pool)
    .await
}

impl LucidosEngine {
    /// Thread ids with a live standing apply. Read by `GET /api/v1/changes` so
    /// every surface renders the armed state, including a thread that has no
    /// change row yet.
    pub fn armed_standing_apply_threads(&self) -> Vec<Uuid> {
        self.standing_applies.snapshot()
    }

    /// Record the owner's instruction to apply `thread_id`'s change once it
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
        log!(
            "[StandingApply] armed thread {} (change {:?}, batch {:?})",
            arm.thread_id,
            arm.change_id,
            arm.batch_id
        );
        self.resolve_standing_apply(arm.thread_id).await;
        Ok(())
    }

    /// End one arm and report why. `Ok(false)` means nothing was armed.
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

    /// Arm every thread that will settle, for the Apply All sweep.
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
                log!("[StandingApply] sweep lookup failed: {}", e);
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
                Err(e) => log!("[StandingApply] sweep arm for {} failed: {}", thread_id, e),
            }
        }
        armed
    }

    /// End every arm in `scope`, reporting each one. Returns how many ended.
    ///
    /// One loop for both bulk disarms, because they differ only in which rows
    /// they read. Apply All Cancel takes [`DisarmScope::Sweep`], and the
    /// Changes panel's off takes [`DisarmScope::All`].
    ///
    /// A scope holding nothing answers 0 rather than failing. An off switch
    /// pressed on an already-off state gave the owner what they asked for. A
    /// read or delete that failed is an `Err`, since arms may still be live.
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

    /// Delete whatever the thread carries and forget it. The owner's own
    /// disarm, which is about the thread rather than about one instruction.
    ///
    /// The filter forgets the thread only once the delete ran. A failed DELETE
    /// leaves the durable arm live, and forgetting it would stop every further
    /// event reaching it until the next boot.
    async fn take_standing_apply(
        &self,
        thread_id: Uuid,
    ) -> Result<Option<StandingApply>, sqlx::Error> {
        let taken = take_arm(self.pool(), thread_id).await?;
        self.standing_applies.remove(thread_id);
        Ok(taken.map(|l| l.arm))
    }

    /// Consume the arm a resolution read, and only that one.
    ///
    /// A re-arm landing between the read and here leaves the newer instruction
    /// in place, and this returns `None` so the caller drops its own resolution.
    async fn take_standing_apply_generation(&self, loaded: &LoadedArm) -> Option<StandingApply> {
        let thread_id = loaded.arm.thread_id;
        let taken = take_arm_generation(self.pool(), thread_id, loaded.generation).await;
        self.consume_generation(thread_id, taken)
    }

    /// The generation-matched take's tail: forget the thread only once its own
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
                log!("[StandingApply] delete for {} failed: {}", thread_id, e);
                None
            }
        }
    }

    /// Emit the report an ended arm owes the owner.
    ///
    /// The arm's own actor stamps it when nobody pressed anything, so a drop
    /// the engine decided still traces back to whoever armed it.
    async fn report_standing_apply_dropped(
        &self,
        arm: &StandingApply,
        reason: &str,
        actor: Option<MessageOrigin>,
    ) {
        log!(
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

    /// Take one arm's verdict and act on it. The single place an arm fires.
    pub(crate) async fn resolve_standing_apply(self: &Arc<Self>, thread_id: Uuid) {
        if !self.standing_applies.contains(thread_id) {
            return;
        }
        let loaded = match load_arm(self.pool(), thread_id).await {
            Ok(arm) => arm,
            Err(e) => {
                log!("[StandingApply] load for {} failed: {}", thread_id, e);
                return;
            }
        };
        let Some(loaded) = loaded else {
            // The table is the authority, so a stale filter entry just clears.
            self.standing_applies.remove(thread_id);
            return;
        };

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

    /// Consume the arm this resolution read, and report why it ended. Silent
    /// when a re-arm won the race: the newer instruction is live and owes no
    /// report.
    async fn end_standing_apply(&self, loaded: &LoadedArm, reason: &str) {
        if self.take_standing_apply_generation(loaded).await.is_some() {
            self.report_standing_apply_dropped(&loaded.arm, reason, None)
                .await;
        }
    }

    /// Apply the change this arm named.
    ///
    /// The arm is taken BEFORE the apply. `apply_change` emits `ChangeApplied`,
    /// which re-enters the resolver through the bus subscriber. An arm still in
    /// the table would then resolve a second time and report a spurious drop.
    /// It consumes the arm this resolution read, so a re-arm landing in the
    /// window keeps its own instruction and this one stands down.
    async fn fire_standing_apply(self: &Arc<Self>, loaded: &LoadedArm, change_id: Uuid) {
        let taken = take_arm_to_fire(self.pool(), &self.event_bus, loaded, change_id).await;
        let Some(arm) = self.consume_generation(loaded.arm.thread_id, taken) else {
            return; // a re-arm or another resolution won the race
        };
        log!(
            "[StandingApply] thread {} settled, applying change {}",
            arm.thread_id,
            change_id
        );
        let engine = self.clone();
        let actor = arm.actor.clone();
        tokio::spawn(async move {
            if let Err(e) = engine.apply_change(change_id, actor).await {
                log!(
                    "[StandingApply] apply_change({change_id}) returned Err: {e}. \
                     The apply path emits ChangeApplyFailed, which reports to the owner",
                );
            }
            engine.broadcast_changes_updated().await;
        });
    }

    /// Start the bus subscriber that resolves arms as their threads move.
    ///
    /// A thread event for an armed thread re-takes that arm's verdict, except
    /// the ones [`thread_to_resolve`] skips. The filter is the in-memory set, so
    /// an engine with nothing armed does no work per event.
    pub fn start_standing_apply_resolver(self: &Arc<Self>) {
        let mut rx = self.event_bus.subscribe();
        let engine = self.clone();
        tokio::spawn(async move {
            log!("[StandingApply] resolver started");
            loop {
                match rx.recv().await {
                    Ok(emitted) => {
                        if let Some(thread_id) = thread_to_resolve(&emitted.typed) {
                            engine.resolve_standing_apply(thread_id).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // The skipped batch may hold the settle an arm was
                        // waiting for, so re-resolve them all rather than
                        // stranding the owner's instruction.
                        log!(
                            "[StandingApply] subscriber lagged by {} events, re-resolving",
                            n
                        );
                        for thread_id in engine.standing_applies.snapshot() {
                            engine.resolve_standing_apply(thread_id).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        log!("[StandingApply] EventBus closed, resolver stopping");
                        break;
                    }
                }
            }
        });
    }

    /// Rebuild the armed set from the durable table and re-take every verdict.
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
                log!("[StandingApply] recovery query failed: {}", e);
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        log!("[StandingApply] recovering {} armed thread(s)", rows.len());
        let ids: HashSet<Uuid> = rows.iter().map(|(id,)| *id).collect();
        self.standing_applies.replace(ids.clone());
        for thread_id in ids {
            self.resolve_standing_apply(thread_id).await;
        }
    }
}

/// DB-backed tests for the two invariants the storage half carries.
///
/// They stand up a real Postgres through `setup_test_db` rather than an engine.
/// What is under test is the row and the query, not the merge.
#[cfg(test)]
mod db_tests {
    use super::*;
    use crate::test_support::{
        seed_thread_event, setup_test_db, start_cc_session, teardown_test_db,
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

    /// Mark a seeded thread as running against an external repo.
    ///
    /// `kind` carries the modern `coding_agent_kind`, `bool_flag` the legacy
    /// `coding_agent_is_external_repo`. They are set separately so a test can
    /// reproduce the legacy row that has the bool and no kind.
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

    /// Put a seeded thread at rest holding `waits` live event waits.
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
        // One pending change per branch name is a unique index, so each seeded
        // change gets its own branch.
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

    /// An arm a sweep set, which is the half `DisarmScope::Sweep` covers.
    fn swept_arm(thread_id: Uuid) -> StandingApply {
        StandingApply {
            thread_id,
            change_id: None,
            batch_id: Some(Uuid::new_v4()),
            actor: None,
        }
    }

    /// The session map a freshly booted engine holds: every session died with
    /// the previous process.
    fn no_sessions() -> AgentSessions {
        AgentSessions::default()
    }

    /// A map holding one live session for `thread_id`. Keep the receiver for
    /// the test's lifetime, or the session reads as a phantom.
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
        }
    }

    /// Emit `event` the way an agent session does, on the `claude_code`
    /// channel.
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

    /// A coding-agent thread whose turn just sent its terminal, with a pending
    /// change bound to an arm. The terminal sets `idle`, so only the settle
    /// fact tells this apart from a settled thread.
    async fn seed_mid_settle(pool: &PgPool, bus: &EventBus) -> (Uuid, StandingApply) {
        let thread = Uuid::new_v4();
        let change = Uuid::new_v4();
        start_cc_session(bus, thread, &format!("claude-code/{thread}"), None).await;
        seed_change(pool, change, thread, ChangeStatus::Pending).await;
        seed_agent_event(bus, thread, terminal()).await;
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

    /// **Arm time.** The owner arms a thread whose live session has sent its
    /// terminal and not yet its idle. The arm-time resolve has no event to
    /// skip, so the facts alone must hold it until the idle.
    #[tokio::test]
    async fn an_arm_set_mid_settle_waits_for_the_idle() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let (thread, arm) = seed_mid_settle(&pool, &bus).await;
        let (sessions, _session_rx) = live_session(thread);

        assert_eq!(verdict(&pool, &arm, &sessions).await, StandingVerdict::Wait);
        teardown_test_db(&db).await;
    }

    /// **Lag.** The subscriber skipped a batch and re-resolves every arm. The
    /// batch held the terminal but not the idle, so the facts must still wait.
    /// Once the idle is written, the same re-resolve fires.
    #[tokio::test]
    async fn a_lagged_re_resolve_waits_until_the_idle_is_written() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let (thread, arm) = seed_mid_settle(&pool, &bus).await;
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

    /// **Boot.** No session is live after a restart. A thread the old engine
    /// killed between its terminal and its idle never gets that idle, and its
    /// last edits may be uncommitted. So the arm ends with a report: it must
    /// neither fire nor wait forever.
    #[tokio::test]
    async fn a_turn_cut_off_by_a_restart_drops_its_arm() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let (_thread, arm) = seed_mid_settle(&pool, &bus).await;

        assert_eq!(
            verdict(&pool, &arm, &no_sessions()).await,
            StandingVerdict::Drop(TURN_CUT_OFF)
        );
        teardown_test_db(&db).await;
    }

    /// A coding-agent thread also carries the Lucidos Agent's turns, for
    /// example in a voice call. Its reply after the agent's idle is no agent
    /// settle, so the arm fires rather than reading as cut off.
    #[tokio::test]
    async fn a_lucidos_agent_reply_after_the_idle_does_not_cut_the_turn_off() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let (thread, arm) = seed_mid_settle(&pool, &bus).await;
        seed_agent_event(&bus, thread, idle()).await;
        seed_thread_event(&bus, thread, terminal()).await;

        assert_eq!(
            verdict(&pool, &arm, &no_sessions()).await,
            StandingVerdict::Fire(arm.change_id.expect("bound"))
        );
        teardown_test_db(&db).await;
    }

    /// Only an agent session idles. A thread with no coding agent settles on
    /// its terminal, so a boot with no sessions must not read it as cut off.
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

    /// **Invariant: a standing apply acts only on the change it was armed for.**
    /// Arm one change, land a second on the same thread, and the arm must never
    /// name the second.
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

        // The first change lands, and the thread proposes another.
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

    /// **Invariant: a standing apply always ends.** Park the thread on a
    /// question card and the arm drops with a report instead of waiting.
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

    /// **A standing apply waits through an event wait** (ADR 0266). Read
    /// through the real row: an idle thread with a live wait keeps its arm.
    ///
    /// The delivery then leaves the row idle with no wait, which alone reads
    /// as settled. That is why the delivery event must not re-take the verdict.
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

        // The delivery's projection: the wait is gone, the wake not yet begun.
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

    /// The settling predicate and the verdict agree on a watching thread. The
    /// sweep arms it and the Changes panel offers it, where a question card
    /// is passed over.
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
        assert_eq!(count_sweep_candidates(&pool).await.expect("count"), 1);
        teardown_test_db(&db).await;
    }

    /// **A delegating parent settles.** Idle apart from its running children,
    /// its change is whole, so the arm fires rather than dropping (ADR 0249).
    /// Read through the real row, so a probe that still read
    /// `active_children_count` would fail here.
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

    /// A row survives a restart, and taking it is one-shot.
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
        // Re-arming replaces rather than stacking, and the new binding wins.
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

    /// **Invariant: a standing apply acts only on the change it was armed
    /// for.** Binding one thread's arm to another thread's change is refused at
    /// the door. The resolver's own read is thread-scoped too, so no such
    /// binding can fire whatever wrote it.
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

        // Even written straight into the table, it can never fire.
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

    /// A re-arm landing between a resolution's read and its consume keeps its
    /// own instruction. Deleting by thread id would swallow the newer one while
    /// carrying out the older.
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

        // The owner re-arms while the resolution is still reading its facts.
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

    /// The sweep skips a thread whose change the same press is already
    /// applying. A paused thread is sweepable AND its change is appliable, so
    /// without this one press both applies and arms for the same change.
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
        // `sweep_standing_applies` filters this list against the batch, which is
        // what keeps the two off one change.
        let applying_now = [change];
        let armed: Vec<Uuid> = candidates
            .iter()
            .filter(|(_, c)| !c.is_some_and(|id| applying_now.contains(&id)))
            .map(|(t, _)| *t)
            .collect();
        assert!(armed.is_empty());
        teardown_test_db(&db).await;
    }

    /// `thread_settling` is the half of `thread_unsettled` an arm can wait
    /// through, so the Changes panel does not offer one on a question card.
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

    /// A sweep arms only what will settle: a coding-agent thread that is
    /// running, paused or watching an event. Everything else would drop on its
    /// first look. A watching thread is covered by its own test.
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

    /// **A standing apply is offered exactly where Apply is.** An external repo
    /// is one Lucidos never merges into, so the sweep passes it over rather
    /// than arming something that can never fire.
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
        // The legacy shape: the bool was written before the kind column existed.
        set_external_repo(&pool, legacy_external, None, true).await;

        let candidates = read_sweep_candidates(&pool).await.expect("sweep read");
        let ids: Vec<Uuid> = candidates.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![lucidos]);
        assert_eq!(
            count_sweep_candidates(&pool).await.expect("sweep count"),
            1,
            "the count the panel offers its sweep from must agree with the sweep"
        );
        teardown_test_db(&db).await;
    }

    /// **An arm always ends, so one that never could is refused at the door.**
    /// An external-repo thread proposes nothing, ever, and its branch diff
    /// holds the verdict at `Wait`. Both the HTTP route and the
    /// `apply_when_settled` tool reach `arm_standing_apply`, so the refusal
    /// lives there and covers both.
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
        // A thread the query cannot find falls through to `check_arm_has_a_settle`,
        // which names THREAD_GONE.
        assert!(check_repo_is_appliable(&pool, Uuid::new_v4()).await.is_ok());
        teardown_test_db(&db).await;
    }

    /// **An arm with no settle to wait for is refused, not stored and dropped.**
    /// A parent agent re-armed a child whose change had just landed. The arm
    /// dropped 8 ms later, the owner got a warning toast for it, and the agent
    /// was told it succeeded.
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

    /// The workspace-scope off takes back a single arm as well as a swept one.
    /// The Changes panel draws ONE armed state, so its off has to mean that.
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

    /// **A disarm stops a future settle.** Take the arm back, then settle the
    /// thread: there is no instruction left, so nothing fires. This is the
    /// `load_arm` branch `resolve_standing_apply` returns at.
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

    /// **A disarm claws nothing back, and the race has one winner.** The fire
    /// path consumes on the generation it read. A disarm landing first takes
    /// the row, so that consume answers `None` and the apply never starts.
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

        // The resolver reads the arm and is about to fire it.
        let loaded = load_arm(&pool, thread).await.expect("load").expect("armed");
        // The owner cancels in that window.
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

    /// How many `StandingApplyFired` events name `thread_id`.
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

    /// **Every end of an arm announces itself.** A fire ends the arm as surely
    /// as a drop does. Announcing nothing left every client drawing the flag as
    /// armed, and its cancel answered 404.
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

    /// A fire that lost the race to a disarm consumed nothing, so it announces
    /// nothing. The disarm already reported the arm's end.
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

    /// The sweep binds a thread's pending change where it has one, so the arm
    /// cannot reach past it to a later proposal.
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
            live_event_waits: false,
            has_diff: false,
            armed_change,
            turn_settle: TurnSettle::Settled,
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

    /// `Paused` is the one resting state that keeps the arm: the engine has
    /// promised to resume the turn, so the settle is still to come.
    #[test]
    fn a_paused_turn_keeps_the_arm() {
        let id = Uuid::new_v4();
        assert_eq!(
            standing_verdict(&facts("paused", ArmedChange::Ready(id))),
            StandingVerdict::Wait
        );
    }

    /// The invariant ADR 0168 names. A thread parked on a question card never
    /// settles by itself, so the arm ends with a report rather than waiting.
    #[test]
    fn a_parked_thread_drops_with_a_report() {
        let id = Uuid::new_v4();
        assert_eq!(
            standing_verdict(&facts("waiting_for_user_answer", ArmedChange::Ready(id))),
            StandingVerdict::Drop(PARKED_ON_QUESTION)
        );

        // A question card outranks a wait held beside it: only the user ends it.
        let mut both = facts("waiting_for_user_answer", ArmedChange::Ready(id));
        both.live_event_waits = true;
        assert_eq!(
            standing_verdict(&both),
            StandingVerdict::Drop(PARKED_ON_QUESTION)
        );
    }

    /// ADR 0266. An event wait ends by itself, so the arm waits through it
    /// rather than applying work the wake will extend. The next idle fires it.
    #[test]
    fn a_thread_watching_an_event_keeps_the_arm() {
        let id = Uuid::new_v4();
        let mut watching = facts("idle", ArmedChange::Ready(id));
        watching.live_event_waits = true;
        assert_eq!(standing_verdict(&watching), StandingVerdict::Wait);

        // Stop waiting, or the wake's own idle: no wait left, so it fires.
        watching.live_event_waits = false;
        assert_eq!(standing_verdict(&watching), StandingVerdict::Fire(id));
    }

    /// Only a wait's delivery and expiry are skipped: both lead into a wake.
    /// A cancel, the wake's anchor and the turn's end all re-take the verdict.
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
        let anchor = on_thread(ThreadEvent::UserPromptInjected {
            text: "An event you subscribed to has arrived".to_string(),
            mode: crate::engine::thread_events::ActorMode::Agent,
            origin: None,
            injected_message_id: None,
            delivered_event_id: None,
        });
        assert_eq!(thread_to_resolve(&canceled), Some(thread_id));
        assert_eq!(thread_to_resolve(&anchor), Some(thread_id));
    }

    /// A live session's terminal sets `idle` before the run loop commits,
    /// proposes and idles. The settle fact holds the arm through that window,
    /// whichever path took the verdict. A turn no session will finish ends the
    /// arm with a report rather than holding it forever.
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

        // A change that can never be applied still ends the arm first.
        mid_settle.armed_change = ArmedChange::Gone(CHANGE_RESOLVED);
        assert_eq!(
            standing_verdict(&mid_settle),
            StandingVerdict::Drop(CHANGE_RESOLVED)
        );
    }

    /// The settle is in flight only while a live session will send the idle.
    /// A session that stays live after its idle does not hold the arm.
    #[test]
    fn the_settle_is_in_flight_only_behind_a_live_session() {
        assert_eq!(classify_turn_settle(false, true), TurnSettle::Settled);
        assert_eq!(classify_turn_settle(false, false), TurnSettle::Settled);
        assert_eq!(classify_turn_settle(true, true), TurnSettle::InFlight);
        assert_eq!(classify_turn_settle(true, false), TurnSettle::CutOff);
    }

    /// A settle step re-resolves like any other event. The settle fact, not the
    /// event filter, keeps it from firing, so the idle and a permission card
    /// resolve on the same terms.
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
        });
        let permission_card = on_thread(ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-1".into(),
            tool_use_id: "tool-1".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({}),
            summary: "Edit a file".into(),
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

    /// Every state resolves to something. With no wait live, waiting is
    /// reachable only from the two statuses that promise more work. A live
    /// wait is the third way, covered above, and it ends by itself.
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

    /// A change that resolved while the arm was live can never fire, whatever
    /// the thread is doing. Checked before the status match so a running thread
    /// does not hold a dead arm open.
    #[test]
    fn a_resolved_change_drops_even_mid_turn() {
        assert_eq!(
            standing_verdict(&facts("running", ArmedChange::Gone(CHANGE_RESOLVED))),
            StandingVerdict::Drop(CHANGE_RESOLVED)
        );
    }

    /// The proposal follows the turn's terminal, so a settled thread with a
    /// diff is one whose `ChangeProposed` is still in flight. Dropping there would lose the
    /// sweep's whole point.
    #[test]
    fn a_settled_thread_with_a_diff_waits_for_its_proposal() {
        let mut with_diff = facts("idle", ArmedChange::Unproposed);
        with_diff.has_diff = true;
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
