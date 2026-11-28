use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::handoff_sql::pending_handoff_sql;
use crate::engine::pending_handoff::{pending_handoff, Implementer};
use crate::engine::session_capture_sql::current_session_capture_sql;
use crate::engine::thread_events::{HandoffCause, HandoffOpening, HandoffRequest, SessionStart};
use crate::runtime::intelligence_ladder::drops_context_variant;
use crate::runtime::{validate_coding_agent_effort, validate_coding_agent_model, CodingAgent};

pub const HANDOFF_OFFER_TOKENS: u64 = 150_000;

/// INVARIANT: What the user says to have the agent hand off. The engine prompt teaches
/// it, and the Successor card's Continue sends it while the cache is warm.
pub const HANDOFF_REQUEST: &str = "handoff this session";

pub use super::pending_handoff::{successor_to_start, ALREADY_HANDED_OFF};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ThreadContext {
    pub input_tokens: Option<u64>,
    pub can_hand_off: bool,
    pub offer: bool,
    pub applied: bool,
}

impl ThreadContext {
    pub fn of_tokens(input_tokens: Option<u64>) -> Self {
        let over = |limit: u64| input_tokens.is_some_and(|t| t > limit);
        Self {
            input_tokens,
            can_hand_off: true,
            offer: over(HANDOFF_OFFER_TOKENS),
            applied: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ImplementerRequest {
    #[serde(default)]
    pub coding_agent: Option<CodingAgent>,
    #[serde(default)]
    pub coding_agent_model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

impl ImplementerRequest {
    pub fn is_explicit(&self) -> bool {
        self.coding_agent.is_some()
            || self.coding_agent_model.is_some()
            || self.reasoning_effort.is_some()
    }
}

/// INVARIANT: The successor's implementer: what the handoff asked for, the old thread's
/// for the rest. Refuses a model or effort the backend does not offer.
///
/// A model carries over only on the same backend. A pick of the old model
/// that drops its *context variant* keeps the old exact id. The successor then
/// sends the same request and can read the prompt cache. An effort carries
/// over as is while backend and model stay. On a new model it carries only if
/// that model accepts it, so it never pins a tier the model drops.
pub fn resolve_implementer(
    old: &Implementer,
    requested: &ImplementerRequest,
) -> Result<Implementer, String> {
    let coding_agent = requested.coding_agent.unwrap_or(old.coding_agent);
    let same_backend = coding_agent == old.coding_agent;
    let model = match requested.coding_agent_model.as_deref() {
        Some(model) => {
            let picked = validate_coding_agent_model(coding_agent, Some(model))?;
            match old.model.as_deref() {
                Some(current)
                    if same_backend && drops_context_variant(coding_agent, model, current) =>
                {
                    old.model.clone()
                }
                _ => picked,
            }
        }
        None if same_backend => old.model.clone(),
        None => None,
    };
    let reasoning_effort = match requested.reasoning_effort.as_deref() {
        Some(effort) => validate_coding_agent_effort(coding_agent, model.as_deref(), Some(effort))?,
        None if same_backend && model == old.model => old.reasoning_effort.clone(),
        None if same_backend => validate_coding_agent_effort(
            coding_agent,
            model.as_deref(),
            old.reasoning_effort.as_deref(),
        )
        .ok()
        .flatten(),
        None => None,
    };
    Ok(Implementer {
        coding_agent,
        model,
        reasoning_effort,
    })
}

pub use crate::engine::thread_events::SessionReading;

pub async fn session_reading(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<SessionReading, sqlx::Error> {
    let sql = format!(
        "SELECT (SELECT tokens FROM ( \
                   SELECT sequence, (payload->'usage'->>'input_tokens')::bigint AS tokens \
                     FROM events WHERE thread_id = $1 AND {capture} \
                   UNION ALL \
                   SELECT sequence, (payload->>'post_tokens')::bigint FROM events \
                    WHERE thread_id = $1 AND event_type = 'CodingAgentCompacted' AND {scope} \
                 ) readings ORDER BY sequence DESC LIMIT 1), \
                (SELECT (payload->>'context_window')::bigint FROM events \
                  WHERE thread_id = $1 AND {capture} \
                    AND (payload->>'context_window')::bigint > 0 \
                  ORDER BY sequence DESC LIMIT 1)",
        capture = current_session_capture_sql("$1"),
        scope = super::session_scope::in_current_session_sql("$1", "sequence"),
    );
    let (tokens, window): (Option<i64>, Option<i64>) =
        sqlx::query_as(&sql).bind(thread_id).fetch_one(pool).await?;
    let unsigned = |v: Option<i64>| v.and_then(|t| u64::try_from(t).ok());
    Ok(SessionReading {
        tokens: unsigned(tokens),
        window: unsigned(window),
    })
}

pub async fn thread_context(pool: &PgPool, thread_id: Uuid) -> Result<ThreadContext, sqlx::Error> {
    let tokens = session_reading(pool, thread_id).await?.tokens;
    let (can_hand_off, applied): (bool, bool) = sqlx::query_as(&format!(
        "SELECT successor_thread_id IS NULL AND NOT {}, \
                EXISTS (SELECT 1 FROM changes WHERE thread_id = $1 AND status = $2) \
         FROM thread_summaries WHERE thread_id = $1",
        pending_handoff_sql("$1")
    ))
    .bind(thread_id)
    .bind(crate::core::changes::ChangeStatus::Applied)
    .fetch_optional(pool)
    .await?
    .unwrap_or((true, false));
    let mut context = ThreadContext::of_tokens(tokens);
    context.can_hand_off = can_hand_off;
    context.offer &= can_hand_off;
    context.applied = applied;
    Ok(context)
}

pub(crate) async fn latest_offer_decline(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<u64>, sqlx::Error> {
    let tokens: Option<i64> = sqlx::query_scalar(
        "SELECT (payload->>'input_tokens')::bigint FROM events \
         WHERE thread_id = $1 AND event_type = 'SuccessorOfferDeclined' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    Ok(tokens.and_then(|t| u64::try_from(t).ok()))
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PendingRequest {
    pub request: Option<HandoffRequest>,
    pub implementer: ImplementerRequest,
}

/// INVARIANT: The latest `SuccessorRequested` of `thread_id` that its turn has not yet
/// ended on. A `CodingAgentIdled` after it closes the request, so a later
/// handoff by the agent never picks up a stale one.
pub async fn pending_successor_request(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<PendingRequest>, sqlx::Error> {
    let payload: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM events \
         WHERE thread_id = $1 AND event_type = 'SuccessorRequested' \
           AND sequence > COALESCE(( \
               SELECT MAX(sequence) FROM events \
               WHERE thread_id = $1 AND event_type = 'CodingAgentIdled'), 0) \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(payload.map(|p| {
        let text = |key: &str| p.get(key).and_then(|v| v.as_str()).map(str::to_string);
        let by = p
            .get("actor")
            .and_then(|a| serde_json::from_value(a.clone()).ok())
            .unwrap_or(super::thread_events::MessageOrigin::Api {
                user_agent: None,
                mode: super::thread_events::ActorMode::Human,
                source_thread_id: None,
                emitting_trigger_id: None,
            });
        PendingRequest {
            request: text("message").map(|message| HandoffRequest { message, by }),
            implementer: ImplementerRequest {
                coding_agent: text("coding_agent").map(|a| CodingAgent::parse(&a)),
                coding_agent_model: text("coding_agent_model"),
                reasoning_effort: text("reasoning_effort"),
            },
        }
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BriefCause {
    CacheCold,
    ResumeFailed,
    NoNarrative,
    ProviderBlocked { policy: &'static str },
}

pub fn handoff_brief(
    thread_id: Uuid,
    cause: BriefCause,
    request_follows: bool,
    applied: Option<&str>,
) -> String {
    let why = match cause {
        BriefCause::CacheCold => {
            "The previous session in this thread handed off with no handoff narrative: \
             waking it would cost too much."
                .to_string()
        }
        BriefCause::ResumeFailed => {
            "The coding agent could not resume this thread's previous session, so its \
             context did not carry over."
                .to_string()
        }
        BriefCause::NoNarrative => {
            "The previous session in this thread reached its context ceiling and ended \
             without writing a handoff narrative."
                .to_string()
        }
        BriefCause::ProviderBlocked { policy } => format!(
            "OpenAI's safety systems blocked the previous session's last turn. Reason: \
             {policy}. OpenAI does not say which part triggered it, and resuming that \
             session would replay it, so it was not resumed."
        ),
    };
    let applied = applied
        .map(|s| format!(" The change this thread applied last: {s}"))
        .unwrap_or_default();
    let block_guidance = match cause {
        BriefCause::ProviderBlocked { .. } => {
            "\n\nFind the blocked turn in those events: its last instruction and its tool \
             calls. Look for an action beyond what the user asked for, such as reading \
             secrets or reaching outside the task. Never repeat such an action. If the task \
             seems to need it, ask the user first."
        }
        _ => "",
    };
    let ending = if request_follows {
        ""
    } else {
        "\n\nThe user typed no request. Once you know where things stand, end your first \
         turn on an AskUserQuestion card. Its question says where this stands. Its \
         options are the open items to pick up."
    };
    format!(
        "A fresh session starts here. {why} This worktree and branch hold all the work so \
         far.{applied}\n\n\
         Rebuild only what you need:\n\
         - `git status` first: the previous session may have left uncommitted work.\n\
         - The thread's recent turns: `lucidos events query --thread-id {thread_id} --limit 30`.\n\
         - The branch history: `git log`.\n\
         - Any plan under `docs/plans/` that the branch touched.{block_guidance}{ending}"
    )
}

pub fn narrative_opening(checkpoint: Option<&str>, narrative: &str) -> String {
    let skip = checkpoint
        .map(|name| {
            format!(
                " The handoff ran at the `{name}` checkpoint, so skip that checkpoint when \
                 your flow reaches it."
            )
        })
        .unwrap_or_default();
    format!(
        "A fresh session starts here: the previous session in this thread handed off. This \
         worktree and branch hold all of its work. Run `git status` first, then read the \
         handoff narrative below and carry on from its next step.{skip}\n\n{narrative}"
    )
}

pub const NARRATIVE_SECTIONS: [&str; 6] = [
    "Outcomes",
    "Decisions",
    "Gotchas",
    "State",
    "Next step",
    "Files",
];

pub fn missing_narrative_sections(narrative: &str) -> Vec<&'static str> {
    let headings: Vec<String> = narrative
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix('#'))
        .map(|rest| rest.trim_start_matches('#').trim().to_lowercase())
        .collect();
    NARRATIVE_SECTIONS
        .into_iter()
        .filter(|section| {
            let section = section.to_lowercase();
            !headings.iter().any(|h| h.starts_with(&section))
        })
        .collect()
}

/// INVARIANT: Whether `thread_id` has a change waiting in the landing lane. The lane
/// holds its snapshot, so the thread cannot hand off until it lands.
pub(crate) async fn awaits_landing(pool: &PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    Ok(!super::landing_lane::queued_changes(pool, thread_id)
        .await?
        .is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffBlock {
    LegacySuccessor(Uuid),
    Archived,
    LaneEnded,
    Applying,
    AwaitingLanding,
}

pub async fn handoff_block(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<HandoffBlock>, sqlx::Error> {
    let row: Option<(Option<Uuid>, String, bool)> = sqlx::query_as(
        "SELECT successor_thread_id, archive_state, apply_in_flight \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let Some((successor, archive_state, applying)) = row else {
        return Ok(None);
    };
    Ok(if let Some(successor) = successor {
        Some(HandoffBlock::LegacySuccessor(successor))
    } else if archive_state == "archived" {
        Some(HandoffBlock::Archived)
    } else if super::landing_lane::lane_batch_ended(pool, thread_id).await? {
        Some(HandoffBlock::LaneEnded)
    } else if applying {
        Some(HandoffBlock::Applying)
    } else if awaits_landing(pool, thread_id).await? {
        Some(HandoffBlock::AwaitingLanding)
    } else {
        None
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewHandoff {
    pub opening: HandoffOpening,
    pub message: String,
    pub checkpoint: Option<String>,
    pub progress: Option<String>,
    pub implementer: Implementer,
    pub request: Option<HandoffRequest>,
    pub cause: HandoffCause,
    pub starts: SessionStart,
    pub crossing: Option<super::context_ceiling::CeilingCrossing>,
}

pub fn cause_and_start(
    past_ceiling: bool,
    at_checkpoint: bool,
    request: bool,
    from_agent: bool,
    ceiling_cut_turn_short: bool,
) -> (HandoffCause, SessionStart) {
    let cause = if past_ceiling {
        HandoffCause::ContextCeiling
    } else if at_checkpoint {
        HandoffCause::Checkpoint
    } else {
        HandoffCause::User
    };
    let starts = if past_ceiling && !request && !(from_agent && ceiling_cut_turn_short) {
        SessionStart::OnNextInput
    } else if from_agent {
        SessionStart::AfterThisTurn
    } else {
        SessionStart::Now
    };
    (cause, starts)
}

/// INVARIANT: Take `thread_id`'s handoff lock until the returned transaction ends. Every
/// writer of a handoff or its withdrawal holds it, across engines, so the
/// second writer re-reads what the first recorded.
pub async fn lock_handoff(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
    let mut claim = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("successor-handoff:{thread_id}"))
        .execute(&mut *claim)
        .await?;
    Ok(claim)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffRecord {
    Recorded,
    Pending,
    CeilingLifted,
}

pub async fn handoff_plan_path(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let session: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT payload->>'coding_agent_folder', payload->>'branch' FROM events \
         WHERE thread_id = $1 AND event_type = 'SessionStarted' ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let Some((Some(folder), Some(branch))) = session else {
        return Ok(None);
    };
    if folder.is_empty() || branch.is_empty() {
        return Ok(None);
    }
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT plan_path FROM planned_branches WHERE repo_root = $1 AND branch_name = $2",
    )
    .bind(super::git_ops::canonical_repo_root(std::path::Path::new(
        &folder,
    )))
    .bind(branch)
    .fetch_optional(pool)
    .await
    .map(Option::flatten)
}

/// INVARIANT: Record `handoff` on `thread_id`, unless one is pending already. One
/// handoff per session, across engines ([`lock_handoff`]). A ceiling handoff
/// re-reads the crossing under the lock, so a person's raise always wins.
pub async fn record_handoff(
    bus: &super::event_bus::EventBus,
    thread_id: Uuid,
    handoff: NewHandoff,
    meta: super::thread_events::EventMeta,
) -> Result<HandoffRecord, Box<dyn std::error::Error + Send + Sync>> {
    let pool = bus.pool();
    let claim = lock_handoff(pool, thread_id).await?;
    if pending_handoff(pool, thread_id).await?.is_some() {
        return Ok(HandoffRecord::Pending);
    }
    if handoff.cause == super::thread_events::HandoffCause::ContextCeiling
        && super::context_ceiling::past_ceiling(pool, thread_id)
            .await?
            .is_none()
    {
        return Ok(HandoffRecord::CeilingLifted);
    }
    bus.emit(super::event_bus::BusEvent::Thread {
        thread_id,
        event: super::thread_events::ThreadEvent::SessionHandedOff {
            plan_path: handoff_plan_path(pool, thread_id).await?,
            cause: Some(handoff.cause),
            starts: handoff.starts,
            crossing: handoff.crossing,
            opening: handoff.opening,
            message: handoff.message,
            checkpoint: handoff.checkpoint,
            progress: handoff.progress,
            coding_agent: handoff.implementer.coding_agent,
            coding_agent_model: handoff.implementer.model,
            reasoning_effort: handoff.implementer.reasoning_effort,
            request: handoff.request,
        },
        meta,
    })
    .await?;
    claim.commit().await?;
    Ok(HandoffRecord::Recorded)
}

pub async fn last_applied_summary(pool: &PgPool, thread_id: Uuid) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(summary, description) FROM changes \
         WHERE thread_id = $1 AND status = 'applied' \
         ORDER BY resolved_at DESC NULLS LAST LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log_legacy!("[Successor] last applied change of {}: {}", thread_id, e);
        None
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StillRunning {
    pub tasks: Vec<String>,
    pub waits: Vec<String>,
    pub children: Vec<String>,
}

impl StillRunning {
    pub fn after(&self, opening: String) -> String {
        let lines: Vec<String> = (self.tasks.iter().map(|t| format!("- Background task: {t}")))
            .chain(self.waits.iter().map(|w| format!("- Event wait: {w}")))
            .chain(self.children.iter().map(|c| format!("- Child thread: {c}")))
            .collect();
        if lines.is_empty() {
            return opening;
        }
        format!(
            "{opening}\n\n## Still running\n\nThe previous session started these, and they \
             still run. Their results reach this thread, so do not start them again.\n\n{}",
            lines.join("\n")
        )
    }
}

impl super::LucidosEngine {
    pub async fn still_running(&self, thread_id: Uuid) -> StillRunning {
        let tasks = self
            .bash_background
            .running_for_thread(thread_id)
            .await
            .into_iter()
            .filter(|task| !task.stop_requested)
            .map(|task| {
                let what = task.description.unwrap_or(task.command);
                format!("{what} (`{}`)", task.task_id)
            })
            .collect();
        let waits = self
            .live_waits
            .for_thread(thread_id)
            .await
            .into_iter()
            .map(|wait| {
                let on: Vec<&str> = wait.on.iter().map(|s| s.event_type.as_str()).collect();
                format!("{} (on {})", wait.reason, on.join(", "))
            })
            .collect();
        let children = sqlx::query_scalar::<_, String>(&format!(
            "SELECT COALESCE(c.title, LEFT(c.first_message, 80), c.thread_id::text) \
               FROM thread_summaries c \
              WHERE c.parent_thread_id = $1 AND c.archive_state <> 'archived' \
                AND (c.status = $2 OR {}) \
              ORDER BY c.created_at",
            pending_handoff_sql("c.thread_id")
        ))
        .bind(thread_id)
        .bind(super::thread_lifecycle::ThreadStatus::Running.as_str())
        .fetch_all(self.pool())
        .await
        .unwrap_or_else(|e| {
            crate::log_legacy!("[Successor] working children of {}: {}", thread_id, e);
            Vec::new()
        });
        StillRunning {
            tasks,
            waits,
            children,
        }
    }
}

pub async fn handed_off(pool: &PgPool, thread_id: Uuid) -> bool {
    successor_thread_of(pool, thread_id)
        .await
        .unwrap_or_else(|e| {
            crate::log_legacy!("[Successor] handoff of {}: {}", thread_id, e);
            None
        })
        .is_some()
}

pub async fn successor_thread_of(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT successor_thread_id FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?
    .flatten())
}

pub async fn predecessor_of(pool: &PgPool, thread_id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT successor_of FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?
    .flatten())
}

pub(crate) const MAX_HANDOFF_HOPS: usize = 16;

pub(crate) async fn lineage_root(pool: &PgPool, thread_id: Uuid) -> Result<Uuid, sqlx::Error> {
    let mut root = thread_id;
    for _ in 0..MAX_HANDOFF_HOPS {
        match predecessor_of(pool, root).await? {
            Some(older) => root = older,
            None => break,
        }
    }
    Ok(root)
}

/// INVARIANT: The commit a successor's first branch starts at, from its old thread's
/// `SuccessorThreadSpawned`. `None` for a thread that is no successor, and
/// for one that has had a session: a later fresh branch follows `main`, so a
/// discarded handoff never comes back.
pub async fn carried_base(
    pool: &PgPool,
    successor_thread_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let Some(old_thread_id) = predecessor_of(pool, successor_thread_id).await? else {
        return Ok(None);
    };
    let started: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events \
                        WHERE thread_id = $1 AND event_type = 'SessionStarted')",
    )
    .bind(successor_thread_id)
    .fetch_one(pool)
    .await?;
    if started {
        return Ok(None);
    }
    handoff_base(pool, old_thread_id).await
}

pub async fn handoff_base(pool: &PgPool, thread_id: Uuid) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT payload->>'base_sha' FROM events \
         WHERE thread_id = $1 AND event_type = 'SuccessorThreadSpawned' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
}

/// INVARIANT: An ended turn intentionally kept running for an owed successor. Automatic
/// orphan sweeps must spare this gap; liveness reports it stalled after grace.
pub async fn between_successor_sessions(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    if successor_to_start(pool, thread_id).await?.is_none() {
        return Ok(false);
    }
    sqlx::query_scalar(&format!(
        "SELECT {}",
        super::event_bus::between_turns_sql("$1")
    ))
    .bind(thread_id)
    .fetch_one(pool)
    .await
}
