//! *Successor threads*: a coding-agent thread hands off to a new thread that
//! carries on with a fresh context. The successor's branch starts at the old
//! thread's last commit, so its change carries all the work, and the old
//! thread proposes nothing.
//!
//! Plan: `docs/plans/2026-10-13-auto-compact-trial-and-successor-threads.md`.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::runtime::{validate_coding_agent_effort, validate_coding_agent_model, CodingAgent};

use super::event_bus::{BusEvent, EventBus};
use super::thread_events::{EventMeta, MessageOrigin, ThreadEvent, UnproposedReason};

/// Context size, in input tokens, from which plan approval and the composer
/// offer a successor thread.
pub const HANDOFF_OFFER_TOKENS: u64 = 150_000;

/// Context size from which an idle thread's composer nudges toward one.
pub const HANDOFF_NUDGE_TOKENS: u64 = 300_000;

/// What the user says to have the agent hand off. The engine prompt teaches
/// it, and the composer's "Continue in a new thread" sends it.
pub const HANDOFF_REQUEST: &str = "handoff this session";

/// Why the projection refuses a second handoff of one thread, rolling the
/// racing emit back. The route's advisory lock normally stops one first.
pub const ALREADY_HANDED_OFF: &str = "the thread already continued in a successor thread";

/// A thread's context size and what it calls for. The CLI and the composer
/// read these decisions rather than comparing numbers of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ThreadContext {
    /// The input tokens of the thread's latest main-session model call.
    /// `None` when the thread has made none.
    pub input_tokens: Option<u64>,
    /// The thread may start a successor at all, at any size. A plan card
    /// reads it to offer a new thread for a cheaper implementer.
    pub can_hand_off: bool,
    /// Over [`HANDOFF_OFFER_TOKENS`], on a thread that can hand off.
    pub offer: bool,
    /// Over [`HANDOFF_NUDGE_TOKENS`], on a thread that can hand off.
    pub nudge: bool,
    /// The thread has an applied change, so a new request can go to a
    /// successor after it.
    pub applied: bool,
}

impl ThreadContext {
    pub fn of_tokens(input_tokens: Option<u64>) -> Self {
        let over = |limit: u64| input_tokens.is_some_and(|t| t > limit);
        Self {
            input_tokens,
            can_hand_off: true,
            offer: over(HANDOFF_OFFER_TOKENS),
            nudge: over(HANDOFF_NUDGE_TOKENS),
            applied: false,
        }
    }
}

/// The *implementer* a coding-agent thread runs on. An unset model or effort
/// means the backend's own default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Implementer {
    pub coding_agent: CodingAgent,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

/// The implementer a handoff asks for. An omitted field carries over, as
/// [`resolve_implementer`] decides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ImplementerRequest {
    #[serde(default)]
    pub coding_agent: Option<CodingAgent>,
    #[serde(default)]
    pub coding_agent_model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

/// The successor's implementer: what the handoff asked for, the old thread's
/// for the rest. Refuses a model or effort the backend does not offer.
///
/// A model carries over only on the same backend. An effort carries over as
/// is while backend and model stay. On a new model it carries only if that
/// model accepts it, so it never pins a tier the model drops.
pub fn resolve_implementer(
    old: &Implementer,
    requested: &ImplementerRequest,
) -> Result<Implementer, String> {
    let coding_agent = requested.coding_agent.unwrap_or(old.coding_agent);
    let same_backend = coding_agent == old.coding_agent;
    let model = match requested.coding_agent_model.as_deref() {
        Some(model) => validate_coding_agent_model(coding_agent, Some(model))?,
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

/// The one definition of a thread's context size: the latest `ContextCaptured`
/// of its own turns, read as `usage.input_tokens`. A sub-agent's call and an
/// auxiliary call on the thread do not count. A later `CodingAgentCompacted`
/// replaces it with `post_tokens`, unknown when Claude Code did not report it.
/// An integrated sub-thread, or a thread that already handed off, is offered
/// nothing.
pub async fn thread_context(pool: &PgPool, thread_id: Uuid) -> Result<ThreadContext, sqlx::Error> {
    let tokens: Option<i64> = sqlx::query_scalar(
        "SELECT tokens FROM ( \
           SELECT sequence, (payload->'usage'->>'input_tokens')::bigint AS tokens FROM events \
            WHERE thread_id = $1 AND event_type = 'ContextCaptured' \
              AND payload->>'parent_tool_use_id' IS NULL \
              AND payload->'usage'->>'input_tokens' IS NOT NULL \
              AND COALESCE(payload->>'purpose', 'turn') = 'turn' \
           UNION ALL \
           SELECT sequence, (payload->>'post_tokens')::bigint FROM events \
            WHERE thread_id = $1 AND event_type = 'CodingAgentCompacted' \
         ) readings ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    let (can_hand_off, applied): (bool, bool) = sqlx::query_as(
        "SELECT NOT coding_agent_integrated AND successor_thread_id IS NULL, \
                EXISTS (SELECT 1 FROM changes WHERE thread_id = $1 AND status = $2) \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .bind(crate::core::changes::ChangeStatus::Applied)
    .fetch_optional(pool)
    .await?
    .unwrap_or((true, false));
    let mut context = ThreadContext::of_tokens(tokens.and_then(|t| u64::try_from(t).ok()));
    context.can_hand_off = can_hand_off;
    context.offer &= can_hand_off;
    context.nudge &= can_hand_off;
    context.applied = applied;
    Ok(context)
}

/// The sections a *handoff narrative* must carry, as markdown headings. A
/// heading matches when its text starts with the name, in any case.
pub const NARRATIVE_SECTIONS: [&str; 6] = [
    "Outcomes",
    "Decisions",
    "Gotchas",
    "State",
    "Next step",
    "Files",
];

/// The sections `narrative` lacks, in [`NARRATIVE_SECTIONS`] order.
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

/// Where a successor sits: in its old thread's place. Top-level when the old
/// thread was, otherwise under the same parent with the same wake mode, so
/// the parent waits on the successor. Never a child that is later detached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub parent_thread_id: Option<Uuid>,
    pub wake_mode: Option<super::thread_events::WakeMode>,
    pub spawning_event_id: Option<Uuid>,
}

impl Placement {
    /// The old thread's place, from its row.
    pub fn of_old_thread(
        parent_thread_id: Option<Uuid>,
        wake_mode: Option<&str>,
        spawning_event_id: Option<Uuid>,
    ) -> Self {
        Self {
            parent_thread_id,
            wake_mode: parent_thread_id
                .and(wake_mode.and_then(super::thread_events::WakeMode::parse)),
            spawning_event_id: parent_thread_id.and(spawning_event_id),
        }
    }
}

/// Whether `thread_id` has a change waiting in the landing lane. The lane
/// holds its snapshot, so the thread cannot hand off until it lands.
pub(crate) async fn awaits_landing(pool: &PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    Ok(!super::landing_lane::queued_changes(pool, thread_id)
        .await?
        .is_empty())
}

/// Whether `thread_id` handed off to a successor. A failed read counts as
/// no, as the integrated flag's read does: the thread then proposes as usual.
pub(crate) async fn handed_off(pool: &PgPool, thread_id: Uuid) -> bool {
    successor_thread_of(pool, thread_id)
        .await
        .unwrap_or_else(|e| {
            crate::log!("[Successor] handoff of {}: {}", thread_id, e);
            None
        })
        .is_some()
}

/// The successor `thread_id` handed off to, if any.
pub(crate) async fn successor_thread_of(
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

/// The thread `thread_id` continues, if it is a successor.
pub(crate) async fn predecessor_of(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT successor_of FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?
    .flatten())
}

/// The commit a successor's first branch starts at, from its old thread's
/// `SuccessorThreadSpawned`. `None` for a thread that is no successor, and
/// for one that has had a session: a later fresh branch follows `main`, so a
/// discarded handoff never comes back.
pub(crate) async fn carried_base(
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

/// The commit `thread_id`'s branch stood at when it handed off.
pub(crate) async fn handoff_base(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT payload->>'base_sha' FROM events \
         WHERE thread_id = $1 AND event_type = 'SuccessorThreadSpawned' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
}

/// Give a successor's fresh branch its old thread's plan marker. Best effort:
/// without it the successor's first proposal waits for a marker, which says
/// so and costs no work.
pub(crate) async fn carry_plan_marker(
    pool: &PgPool,
    repo_root: &std::path::Path,
    successor_thread_id: Uuid,
    successor_branch: &str,
) {
    let old_branch = match predecessor_of(pool, successor_thread_id).await {
        Ok(Some(old)) => {
            super::agent_session::resume::lookup_session_branch_for_thread(pool, old).await
        }
        Ok(None) => None,
        Err(e) => {
            crate::log!("[Successor] predecessor of {}: {}", successor_thread_id, e);
            None
        }
    };
    let Some(old_branch) = old_branch else {
        return;
    };
    if let Err(e) =
        super::git_ops::carry_planned(pool, repo_root, &old_branch, successor_branch).await
    {
        crate::log!(
            "[Successor] could not carry the plan marker from {} to {}: {}",
            old_branch,
            successor_branch,
            e
        );
    }
}

/// Record the handoff on the old thread, then take back its pending change
/// and withhold its work as carried. Every read comes first and the link
/// lands next, so a turn end racing this call already finds the hold. After
/// the link nothing may fail the handoff: a withdraw that fails is logged,
/// and the hold still keeps the old work from being proposed again. A
/// set-aside change stays as the user left it.
pub(crate) async fn record_handoff(
    bus: &EventBus,
    pool: &PgPool,
    old_thread_id: Uuid,
    successor_thread_id: Uuid,
    base_sha: &str,
    actor: Option<MessageOrigin>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let had_work: bool = sqlx::query_scalar(
        "SELECT coding_agent_change_state <> 'none' FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(old_thread_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(false);
    let pending: Vec<_> = crate::core::changes_projection::ChangesProjection::new(pool.clone())
        .open_for_thread(old_thread_id)
        .await?
        .into_iter()
        .filter(|change| change.status() == crate::core::changes::ChangeStatus::Pending)
        .collect();
    let session_branch =
        super::agent_session::resume::lookup_session_branch_for_thread(pool, old_thread_id).await;
    bus.emit(BusEvent::Thread {
        thread_id: old_thread_id,
        event: ThreadEvent::SuccessorThreadSpawned {
            successor_thread_id,
            base_sha: base_sha.to_string(),
        },
        meta: EventMeta::with_actor(actor.clone()),
    })
    .await?;
    for change in &pending {
        if let Err(e) =
            super::change_ops::emit_change_withdrawn(bus, old_thread_id, change.id, actor.clone())
                .await
        {
            crate::log!(
                "[Successor] could not withdraw change {} of {}: {}",
                change.id,
                old_thread_id,
                e
            );
        }
    }
    let (branch_name, files) = match pending.into_iter().last() {
        Some(change) => (change.branch_name, change.files),
        None => match session_branch.filter(|_| had_work) {
            Some(branch) => (branch, Vec::new()),
            None => return Ok(()),
        },
    };
    super::change_ops::emit_proposal_withheld(
        bus,
        old_thread_id,
        &branch_name,
        &files,
        UnproposedReason::CarriedBySuccessor,
    )
    .await;
    Ok(())
}

/// Archive each old thread once its successor's first session starts. It goes
/// through the *archive request*, so the old thread archives once it settles
/// and nothing blocks it. Otherwise it stays open and shows where it continued.
pub fn spawn_successor_archive_net(
    engine: std::sync::Arc<super::LucidosEngine>,
) -> tokio::task::JoinHandle<()> {
    use super::event_bus::{LagReplay, ReplayScope};
    let (mut rx, mut replay) = LagReplay::subscribe(
        &engine.event_bus,
        ReplayScope::Workspace,
        Some(vec!["SessionStarted"]),
    );
    tokio::spawn(async move {
        while let Some(frames) = replay.read(rx.recv().await, engine.pool()).await {
            for emitted in frames {
                if let BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::SessionStarted { .. },
                    ..
                } = &emitted.typed
                {
                    request_predecessor_archive(&engine.event_bus, engine.pool(), *thread_id).await;
                }
            }
        }
    })
}

/// Ask `successor_thread_id`'s old thread to archive, on the successor's
/// first session only, so a resume or a replayed frame asks nothing twice.
async fn request_predecessor_archive(bus: &EventBus, pool: &PgPool, successor_thread_id: Uuid) {
    let old = match predecessor_of(pool, successor_thread_id).await {
        Ok(Some(old)) => old,
        Ok(None) => return,
        Err(e) => {
            crate::log!("[Successor] predecessor of {}: {}", successor_thread_id, e);
            return;
        }
    };
    let sessions: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'SessionStarted'",
    )
    .bind(successor_thread_id)
    .fetch_one(pool)
    .await;
    match sessions {
        Ok(1) => {}
        Ok(_) => return,
        Err(e) => {
            crate::log!("[Successor] sessions of {}: {}", successor_thread_id, e);
            return;
        }
    }
    if let Err(e) = super::chat::agent_archive::record_archive_request(bus, old).await {
        crate::log!("[Successor] could not ask {} to archive: {}", old, e);
    }
}

#[cfg(test)]
#[path = "successor_tests.rs"]
mod tests;
