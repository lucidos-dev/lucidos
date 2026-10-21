//! *Successor threads*: a coding-agent thread hands off to a new thread that
//! carries on with a fresh context. The successor's branch starts at the old
//! thread's last commit, so its change carries all the work, and the old
//! thread proposes nothing.
//!
//! Plan: `docs/plans/2026-10-13-auto-compact-trial-and-successor-threads.md`.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::runtime::intelligence_ladder::drops_context_variant;
use crate::runtime::{validate_coding_agent_effort, validate_coding_agent_model, CodingAgent};

use super::event_bus::{BusEvent, EventBus};
use super::thread_events::{
    EventMeta, HandoffConsent, MessageOrigin, ThreadEvent, UnproposedReason,
};

/// Context size, in input tokens, from which plan approval offers a successor
/// thread.
pub const HANDOFF_OFFER_TOKENS: u64 = 150_000;

/// What the user says to have the agent hand off. The engine prompt teaches
/// it, and the Successor card's Continue sends it while the cache is warm.
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

/// A SQL predicate over unaliased `events` columns: a model call of a
/// thread's own turn that reports its input size. A sub-agent's call and an
/// auxiliary call on the thread do not count.
pub(crate) const MAIN_SESSION_CAPTURE: &str = "event_type = 'ContextCaptured' \
     AND payload->>'parent_tool_use_id' IS NULL \
     AND payload->'usage'->>'input_tokens' IS NOT NULL \
     AND COALESCE(payload->>'purpose', 'turn') = 'turn'";

/// The one definition of a thread's context size: the latest `ContextCaptured`
/// of its own turns, read as `usage.input_tokens`. A sub-agent's call and an
/// auxiliary call on the thread do not count. A later `CodingAgentCompacted`
/// replaces it with `post_tokens`, unknown when Claude Code did not report it.
/// An integrated sub-thread, or a thread that already handed off, is offered
/// nothing.
pub async fn thread_context(pool: &PgPool, thread_id: Uuid) -> Result<ThreadContext, sqlx::Error> {
    let sql = format!(
        "SELECT tokens FROM ( \
           SELECT sequence, (payload->'usage'->>'input_tokens')::bigint AS tokens FROM events \
            WHERE thread_id = $1 AND {MAIN_SESSION_CAPTURE} \
           UNION ALL \
           SELECT sequence, (payload->>'post_tokens')::bigint FROM events \
            WHERE thread_id = $1 AND event_type = 'CodingAgentCompacted' \
         ) readings ORDER BY sequence DESC LIMIT 1"
    );
    let tokens: Option<i64> = sqlx::query_scalar(&sql)
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
    context.applied = applied;
    Ok(context)
}

/// The context size the user last declined the Successor card at, from the
/// latest `SuccessorOfferDeclined`.
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

/// What the user asked for on the *Successor card* while the cache was warm,
/// and the agent's narrative spawn has not yet used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingRequest {
    /// The user's first request for the successor, when they typed one.
    pub message: Option<String>,
    pub implementer: ImplementerRequest,
}

/// The latest `SuccessorRequested` of `thread_id` that its turn has not yet
/// ended on. A `CodingAgentIdled` after it closes the request, so a later
/// handoff by the agent never picks up a stale one.
pub(crate) async fn pending_successor_request(
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
        PendingRequest {
            message: text("message"),
            implementer: ImplementerRequest {
                coding_agent: text("coding_agent").map(|a| CodingAgent::parse(&a)),
                coding_agent_model: text("coding_agent_model"),
                reasoning_effort: text("reasoning_effort"),
            },
        }
    }))
}

/// The opening message of a successor that starts with no *handoff
/// narrative*: the old thread was too costly to wake. It names the old
/// thread, and tells the successor how to rebuild what it needs.
pub fn handoff_brief(
    old_link: &str,
    old_id: Uuid,
    request: Option<&str>,
    applied: Option<&str>,
) -> String {
    let applied = applied
        .map(|s| format!(" The change it applied last: {s}"))
        .unwrap_or_default();
    let ending = match request {
        Some(request) => format!("The user's first request for you:\n\n{request}"),
        None => "The user typed no request. Once you know where things stand, end your first \
                 turn on an AskUserQuestion card. Its question says where this stands. Its \
                 options are the open items to pick up."
            .to_string(),
    };
    format!(
        "This thread continues {old_link}. Its worktree starts at that thread's last commit, \
         so it already holds all of its work.{applied}\n\n\
         There is no handoff narrative: waking the earlier thread would cost too much. \
         Rebuild only what you need:\n\
         - Its recent turns: `lucidos events query --thread-id {old_id} --limit 30`.\n\
         - The branch history: `git log`.\n\
         - Any plan under `docs/plans/` that the branch touched.\n\n\
         {ending}"
    )
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

const CHECKPOINT_SEPARATOR: &str = " · ";
const LEGACY_CONTINUED_MARK: &str = " (continued)";

/// The title of the thread that continues `old_title`: the base title, the
/// successor's position in its chain, and the checkpoint it starts at.
/// Position 2 is the first successor. The suffixes of `old_title` are
/// stripped first, so a repeat handoff never stacks them.
pub fn successor_title(old_title: &str, checkpoint: Option<&str>) -> String {
    let (base, position) = chain_position(old_title);
    let number = position + 1;
    match checkpoint {
        Some(checkpoint) => format!("{base} ({number}){CHECKPOINT_SEPARATOR}{checkpoint}"),
        None => format!("{base} ({number})"),
    }
}

/// `title` without its chain suffixes, and its own position in the chain. A
/// title with no suffix is position 1. A legacy ` (continued)` adds one each.
fn chain_position(title: &str) -> (&str, u32) {
    let mut rest = title.trim_end();
    if let Some((head, _)) = rest.rsplit_once(CHECKPOINT_SEPARATOR) {
        if strip_legacy_mark(head).is_some() || strip_number_mark(head).is_some() {
            rest = head;
        }
    }
    let mut position = 1;
    let mut numbered = false;
    loop {
        if let Some(head) = strip_legacy_mark(rest) {
            position += 1;
            rest = head;
        } else if let Some((head, number)) = strip_number_mark(rest).filter(|_| !numbered) {
            position += number - 1;
            numbered = true;
            rest = head;
        } else {
            return (rest, position);
        }
    }
}

fn strip_legacy_mark(title: &str) -> Option<&str> {
    title
        .strip_suffix(LEGACY_CONTINUED_MARK)
        .filter(|head| !head.trim().is_empty())
        .map(str::trim_end)
}

/// A trailing ` (N)` with N from 2 to 99: a year or a one-off count in a
/// plain title is no chain position.
fn strip_number_mark(title: &str) -> Option<(&str, u32)> {
    let (head, tail) = title.rsplit_once(" (")?;
    let digits = tail.strip_suffix(')')?;
    let number: u32 = digits
        .bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| digits.parse().ok())
        .flatten()?;
    ((2..=99).contains(&number) && digits.len() <= 2 && !head.trim().is_empty())
        .then(|| (head.trim_end(), number))
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

    /// The builder's place in a *build chain*: a `no-wake` child of the
    /// planning thread, so a clean finish files its report without waking it.
    pub fn build_chain_under(planner: Uuid) -> Self {
        Self {
            parent_thread_id: Some(planner),
            wake_mode: Some(super::thread_events::WakeMode::NoWake),
            spawning_event_id: None,
        }
    }
}

/// A *build chain*, as one of its members sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildChain {
    /// The planning thread every member sits under.
    pub planner: Uuid,
    /// The builder first, then each successor, down to the asking member.
    pub members: Vec<Uuid>,
}

/// The build chain `thread_id` belongs to, if any. Its `successor_of` links
/// lead, through threads under one parent, to that parent: only the plan
/// card's handoff puts a successor under its own old thread.
pub async fn build_chain(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<BuildChain>, sqlx::Error> {
    // The hop bound ends a corrupt cycle, as `lineage_root`'s does.
    let lineage: Vec<(Uuid, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "WITH RECURSIVE lineage AS ( \
           SELECT thread_id, successor_of, parent_thread_id, 0 AS depth \
             FROM thread_summaries WHERE thread_id = $1 \
           UNION ALL \
           SELECT t.thread_id, t.successor_of, t.parent_thread_id, l.depth + 1 \
             FROM thread_summaries t \
             JOIN lineage l ON t.thread_id = l.successor_of \
                           AND t.parent_thread_id = l.parent_thread_id \
            WHERE l.depth < $2 \
         ) \
         SELECT thread_id, successor_of, parent_thread_id FROM lineage ORDER BY depth DESC",
    )
    .bind(thread_id)
    .bind(i32::try_from(MAX_HANDOFF_HOPS).unwrap_or(i32::MAX))
    .fetch_all(pool)
    .await?;
    let Some(&(_, Some(started_by), Some(planner))) = lineage.first() else {
        return Ok(None);
    };
    Ok((started_by == planner).then(|| BuildChain {
        planner,
        members: lineage.iter().map(|(id, _, _)| *id).collect(),
    }))
}

/// Whether `successor_thread_id` is the builder of a build chain: the only
/// successor that sits under its own old thread.
pub(crate) async fn starts_build_chain(
    pool: &PgPool,
    successor_thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, Option<bool>>(
        "SELECT successor_of = parent_thread_id FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(successor_thread_id)
    .fetch_optional(pool)
    .await?
    .flatten()
    .unwrap_or(false))
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

/// How far a handoff chain is followed, so a corrupt cycle ends.
pub(crate) const MAX_HANDOFF_HOPS: usize = 16;

/// The thread a chain of handoffs began at: `thread_id` itself when it is no
/// successor. A change a successor lands belongs to this thread's work.
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

/// What the old thread's `SuccessorThreadSpawned` records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Handoff {
    pub successor_thread_id: Uuid,
    /// The old thread's last commit, which the successor's branch starts at.
    pub base_sha: String,
    pub starts_build_chain: bool,
    pub handoff_consent: Option<HandoffConsent>,
}

/// The *handoff consent* `thread_id` carries under `parent` (ADR 0507): the
/// one its predecessor recorded on the handoff that started it, when both sit
/// under `parent`. A value this engine cannot read counts as none, so the
/// thread asks rather than hands off.
pub(crate) async fn carried_consent(
    pool: &PgPool,
    thread_id: Uuid,
    parent: Uuid,
) -> Result<Option<HandoffConsent>, sqlx::Error> {
    let recorded: Option<Option<serde_json::Value>> = sqlx::query_scalar(
        "SELECT e.payload->'handoff_consent' \
           FROM thread_summaries t \
           JOIN thread_summaries p ON p.thread_id = t.successor_of \
           JOIN events e ON e.thread_id = p.thread_id \
                        AND e.event_type = 'SuccessorThreadSpawned' \
                        AND e.payload->>'successor_thread_id' = t.thread_id::text \
          WHERE t.thread_id = $1 AND t.parent_thread_id = $2 AND p.parent_thread_id = $2 \
          ORDER BY e.sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(parent)
    .fetch_optional(pool)
    .await?;
    let Some(value) = recorded.flatten() else {
        return Ok(None);
    };
    Ok(serde_json::from_value(value.clone())
        .inspect_err(|e| {
            crate::log!(
                "[Successor] unreadable handoff consent {} for {}: {}",
                value,
                thread_id,
                e
            )
        })
        .ok())
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
    handoff: Handoff,
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
            successor_thread_id: handoff.successor_thread_id,
            base_sha: handoff.base_sha,
            starts_build_chain: handoff.starts_build_chain,
            handoff_consent: handoff.handoff_consent,
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
/// A build chain's planner stays open, and an unknown answer keeps it open.
async fn request_predecessor_archive(bus: &EventBus, pool: &PgPool, successor_thread_id: Uuid) {
    let old = match predecessor_of(pool, successor_thread_id).await {
        Ok(Some(old)) => old,
        Ok(None) => return,
        Err(e) => {
            crate::log!("[Successor] predecessor of {}: {}", successor_thread_id, e);
            return;
        }
    };
    match starts_build_chain(pool, successor_thread_id).await {
        Ok(false) => {}
        Ok(true) => return,
        Err(e) => {
            crate::log!("[Successor] build chain of {}: {}", successor_thread_id, e);
            return;
        }
    }
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
