//! *Successor sessions*: a coding-agent thread hands off to a fresh session in
//! the same thread, which carries on with a clean context. The branch, the
//! change and the parent's wait stay with the thread.
//!
//! Threads that handed off to a *legacy successor thread* before this keep their
//! links, read here and rendered as they were. Nothing creates new ones.
//!
//! Plan: `docs/plans/2026-10-23-successor-session-in-the-same-thread.md`.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::thread_events::{HandoffCause, HandoffOpening, SessionStart};
use crate::runtime::intelligence_ladder::drops_context_variant;
use crate::runtime::{validate_coding_agent_effort, validate_coding_agent_model, CodingAgent};

/// Context size, in input tokens, from which plan approval offers a successor
/// session.
pub const HANDOFF_OFFER_TOKENS: u64 = 150_000;

/// What the user says to have the agent hand off. The engine prompt teaches
/// it, and the Successor card's Continue sends it while the cache is warm.
pub const HANDOFF_REQUEST: &str = "handoff this session";

/// Why the projection refuses a second handoff of one thread, rolling the
/// racing emit back. The route's advisory lock normally stops one first.
pub const ALREADY_HANDED_OFF: &str = "the thread already continued in a legacy successor thread";

/// A thread's context size and what it calls for. The CLI and the composer
/// read these decisions rather than comparing numbers of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ThreadContext {
    /// The input tokens of the thread's latest main-session model call.
    /// `None` when the thread has made none.
    pub input_tokens: Option<u64>,
    /// The thread may start a successor session at all, at any size. A plan
    /// card reads it to offer a fresh session for a cheaper implementer.
    pub can_hand_off: bool,
    /// Over [`HANDOFF_OFFER_TOKENS`], on a thread that can hand off.
    pub offer: bool,
    /// The thread has an applied change, so a new request can go to a
    /// successor session after it.
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

/// A SQL predicate over unaliased `events` columns: a [`MAIN_SESSION_CAPTURE`]
/// in `thread`'s current *agent session*. Size, warmth and the estimates read
/// only these, so a fresh session starts from nothing.
pub(crate) fn current_session_capture_sql(thread: &str) -> String {
    format!(
        "{MAIN_SESSION_CAPTURE} AND {}",
        super::session_scope::in_current_session_sql(thread, "sequence")
    )
}

/// A SQL scalar: when the latest capture of `thread`'s current session that
/// matches `predicate` began, which is its `created` less its `duration_ms`.
/// `thread` is a SQL expression, such as `$1` or `ts.thread_id`.
fn latest_capture_started(thread: &str, predicate: &str) -> String {
    latest_capture_started_in_any_session(
        thread,
        &format!(
            "{predicate} AND {}",
            super::session_scope::in_current_session_sql(thread, "e.sequence")
        ),
    )
}

/// When the capture `e` began: its `created` less its `duration_ms`.
pub(crate) const CAPTURE_STARTED: &str =
    "e.created - make_interval(secs => COALESCE((e.payload->>'duration_ms')::float8, 0) / 1000)";

/// [`latest_capture_started`] over every session of `thread`.
fn latest_capture_started_in_any_session(thread: &str, predicate: &str) -> String {
    format!(
        "(SELECT {CAPTURE_STARTED} \
            FROM events e \
           WHERE e.thread_id = {thread} AND {predicate} \
           ORDER BY e.sequence DESC LIMIT 1)"
    )
}

/// A SQL scalar: when the latest main-session turn request of `thread` began.
pub(crate) fn last_turn_started_sql(thread: &str) -> String {
    latest_capture_started(thread, MAIN_SESSION_CAPTURE)
}

/// A SQL predicate over the capture `e`: a *real* request, a main-session turn
/// request that no keep-alive anchor started. A resume beat's hidden turn does
/// not count, and a copy beat is no turn capture.
fn real_request_sql() -> String {
    format!(
        "{MAIN_SESSION_CAPTURE} AND NOT EXISTS ( \
           SELECT 1 FROM events a \
            WHERE a.id = NULLIF(e.payload->>'request_event_id', '')::uuid \
              AND a.event_type = 'PromptInjected' \
              AND a.payload->'origin'->'reason'->>'kind' = '{}')",
        crate::engine::thread_events::CACHE_KEEP_ALIVE_REASON_KIND
    )
}

/// A SQL scalar: when the latest *real* request of `thread` began. The
/// per-thread keep-alive switch holds until this moves (ADR 0554). It reads
/// every session: a new session must not undo a request that re-armed it.
pub(crate) fn last_real_request_started_sql(thread: &str) -> String {
    latest_capture_started_in_any_session(thread, &real_request_sql())
}

/// A SQL scalar: when the first *real* request of `thread` after the event
/// at `sequence` began, in any session. NULL until one has. A card's wait
/// ends there, since that request reads the cache.
pub(crate) fn first_real_request_started_after_sql(thread: &str, sequence: &str) -> String {
    format!(
        "(SELECT {CAPTURE_STARTED} \
            FROM events e \
           WHERE e.thread_id = {thread} AND e.sequence > {sequence} AND {} \
           ORDER BY e.sequence LIMIT 1)",
        real_request_sql()
    )
}

/// A SQL scalar: when the *cache* of `thread` last refreshed. That is the later
/// start of its last main-session turn and its last keep-alive copy beat. The
/// keep-alive, the cache-warm line and the switch estimate all read it, so a
/// beaten thread is warm to all three (ADR 0547).
pub(crate) fn last_cache_refresh_started_sql(thread: &str) -> String {
    let beat = latest_capture_started(
        thread,
        &format!(
            "event_type = 'ContextCaptured' AND payload->>'purpose' = '{}'",
            crate::engine::ContextPurpose::CacheKeepAlive.wire()
        ),
    );
    format!("GREATEST({}, {beat})", last_turn_started_sql(thread))
}

/// The current *agent session*'s context reading, the one definition of a
/// thread's context size. `None` fields are unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionReading {
    /// The latest `usage.input_tokens` of the session's own turns. A later
    /// `CodingAgentCompacted` replaces it with `post_tokens`, unknown when
    /// Claude Code did not report it. A fresh session has none yet.
    pub tokens: Option<u64>,
    /// The context window of the newest capture that records one.
    pub window: Option<u64>,
}

/// Read [`SessionReading`]. A sub-agent's call and an auxiliary call on the
/// thread do not count.
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

/// A thread's context size, from [`session_reading`], and what it calls for.
/// An integrated sub-thread, a thread whose handoff is pending, and a thread
/// that handed off to a legacy successor thread before successor sessions, are
/// offered nothing.
pub async fn thread_context(pool: &PgPool, thread_id: Uuid) -> Result<ThreadContext, sqlx::Error> {
    let tokens = session_reading(pool, thread_id).await?.tokens;
    let (can_hand_off, applied): (bool, bool) = sqlx::query_as(&format!(
        "SELECT NOT coding_agent_integrated AND successor_thread_id IS NULL AND NOT {}, \
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

/// What a user typed at the *Successor card*, and who typed it. The successor
/// session starts on it as that user's message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandoffRequest {
    pub message: String,
    pub by: super::thread_events::MessageOrigin,
}

/// What the user asked for on the *Successor card* while the cache was warm,
/// and the agent's narrative spawn has not yet used.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PendingRequest {
    /// The user's first request for the successor, when they typed one.
    pub request: Option<HandoffRequest>,
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
        // The route stamps the requester, so a missing actor is only a
        // hand-written row. It still reads as a person, never as the engine.
        let by = p
            .get("actor")
            .and_then(|a| serde_json::from_value(a.clone()).ok())
            .unwrap_or(super::thread_events::MessageOrigin::Api {
                user_agent: None,
                mode: super::thread_events::ActorMode::Human,
                source_thread_id: None,
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

/// Why a fresh session starts from a *handoff brief* rather than a narrative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BriefCause {
    /// The prompt cache was cold, so waking the old session would cost too
    /// much (ADR 0539).
    CacheCold,
    /// The coding agent refused to resume the previous session.
    ResumeFailed,
    /// The previous session reached its *context ceiling* and ended its
    /// *ceiling re-entry* without handing off.
    NoNarrative,
}

/// The opening of a session that starts with no *handoff narrative*. It tells
/// the session how to rebuild what it needs from this thread. With no
/// request to follow it, it asks the session to offer the open items.
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
        }
        BriefCause::ResumeFailed => {
            "The coding agent could not resume this thread's previous session, so its \
             context did not carry over."
        }
        BriefCause::NoNarrative => {
            "The previous session in this thread reached its context ceiling and ended \
             without writing a handoff narrative."
        }
    };
    let applied = applied
        .map(|s| format!(" The change this thread applied last: {s}"))
        .unwrap_or_default();
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
         - Any plan under `docs/plans/` that the branch touched.{ending}"
    )
}

/// The input of a fresh session that starts because a resume failed: the
/// thread's recap, then a *handoff brief* that ends on `message`.
pub(crate) async fn resume_failed_input(pool: &PgPool, thread_id: Uuid, message: &str) -> String {
    let brief = handoff_brief(thread_id, BriefCause::ResumeFailed, true, None);
    let input = successor_first_message(&brief, Some(message), message);
    super::agent_session::prepend_reconstruction(pool, thread_id, &input).await
}

/// The opening of a *successor session* that starts from the previous
/// session's *handoff narrative*. A handoff at a checkpoint resumes at the
/// step after it, so the session never runs that one again.
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

/// The first message of a *successor session*: its `opening`, then the user's
/// `request`, then the `input` that started the session when it was neither.
pub fn successor_first_message(opening: &str, request: Option<&str>, input: &str) -> String {
    let mut message = opening.to_string();
    if let Some(request) = request {
        message.push_str(&format!(
            "\n\n---\nThe user's request for you:\n\n{request}"
        ));
    }
    let started_by_another = input.trim() != opening.trim()
        && request.is_none_or(|request| request.trim() != input.trim());
    if started_by_another {
        message.push_str(&format!(
            "\n\n---\nThe message that started this session:\n\n{input}"
        ));
    }
    message
}

/// A `SessionHandedOff` that no `SessionStarted` has followed yet: the next
/// session of the thread is its *successor session*.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PendingHandoff {
    pub opening: HandoffOpening,
    pub message: String,
    #[serde(default)]
    pub checkpoint: Option<String>,
    #[serde(default)]
    pub progress: Option<String>,
    pub coding_agent: CodingAgent,
    #[serde(default)]
    pub coding_agent_model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// Absent on a handoff recorded before the field: its `message` carries
    /// any request itself.
    #[serde(default)]
    pub request: Option<HandoffRequest>,
    #[serde(default)]
    pub cause: Option<HandoffCause>,
    #[serde(default)]
    pub starts: SessionStart,
    #[serde(default)]
    pub crossing: Option<super::context_ceiling::CeilingCrossing>,
}

impl PendingHandoff {
    /// Whether the engine starts its session by itself, at boot or once the
    /// thread is free. A *next-input start* waits for an input instead.
    pub fn starts_unprompted(&self) -> bool {
        self.starts != SessionStart::OnNextInput
    }

    /// The first message of its session, given the `input` that started it.
    pub fn first_message(&self, input: &str) -> String {
        successor_first_message(
            &self.message,
            self.request.as_ref().map(|r| r.message.as_str()),
            input,
        )
    }

    /// The continuity its session records on `SessionStarted`.
    pub fn continuity(&self) -> super::thread_events::SessionContinuity {
        super::thread_events::SessionContinuity::Handoff {
            checkpoint: self.checkpoint.clone(),
            progress: self.progress.clone(),
            crossing: self.crossing,
            opening: self.opening,
            message: self.message.clone(),
        }
    }

    pub fn implementer(&self) -> Implementer {
        Implementer {
            coding_agent: self.coding_agent,
            model: self.coding_agent_model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
        }
    }
}

/// A SQL predicate: `thread` has a `SessionHandedOff` that nothing has ended.
/// `thread` is a SQL expression, such as `$1` or `c.thread_id`, and may name
/// any outer alias: the subqueries use their own.
pub(crate) fn pending_handoff_sql(thread: &str) -> String {
    format!(
        concat!(
            "EXISTS (SELECT 1 FROM events pending_handoff \
                  WHERE pending_handoff.thread_id = {thread} \
                    AND pending_handoff.event_type = 'SessionHandedOff' \
                    AND pending_handoff.sequence > ",
            crate::core::snapshot::boundary::restore_floor_sql!(),
            " \
                    AND NOT EXISTS (SELECT 1 FROM events handoff_end \
                          WHERE handoff_end.thread_id = {thread} \
                            AND {}))"
        ),
        ends_handoff_sql("handoff_end", "pending_handoff"),
        thread = thread
    )
}

/// `SessionHandedOff` and every type `ends_handoff_sql` matches. The partial
/// index `idx_events_handoff_lifecycle` lists the same types, so the planner
/// can answer `pending_handoff_threads_sql` from it.
const HANDOFF_LIFECYCLE_EVENT_TYPES: [&str; 5] = [
    "SessionHandedOff",
    "SessionStarted",
    "ResponseCanceled",
    "ResponseFailed",
    "SessionHandoffWithdrawn",
];

/// A SQL query: the `thread_id` of every thread with a pending handoff, the
/// set `pending_handoff_sql` tests one thread against. It costs one index
/// probe per `SessionHandedOff`, whatever the thread count. The `LIMIT 1`
/// keeps the planner from turning the end probe into a scan of every end event.
/// It never yields NULL, so a caller may test membership with `NOT IN`.
pub(crate) fn pending_handoff_threads_sql() -> String {
    let lifecycle = HANDOFF_LIFECYCLE_EVENT_TYPES
        .iter()
        .map(|event_type| format!("'{event_type}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        concat!(
            "SELECT pending_handoff.thread_id FROM events pending_handoff \
               LEFT JOIN LATERAL (SELECT 1 AS ended FROM events handoff_end \
                     WHERE handoff_end.thread_id = pending_handoff.thread_id \
                       AND handoff_end.event_type IN ({lifecycle}) \
                       AND {} LIMIT 1) handoff_ended ON TRUE \
              WHERE pending_handoff.event_type = 'SessionHandedOff' \
                AND pending_handoff.thread_id IS NOT NULL \
                AND pending_handoff.sequence > ",
            crate::core::snapshot::boundary::restore_floor_sql!(),
            " \
                AND handoff_ended.ended IS NULL"
        ),
        ends_handoff_sql("handoff_end", "pending_handoff"),
        lifecycle = lifecycle
    )
}

/// A SQL predicate: the `events` row `alias` ends the handoff recorded by the
/// `SessionHandedOff` row `handoff`, on the same thread. Four things end one:
///
/// - its successor session's `SessionStarted`, whose `continuity` is
///   `handoff`. A recovery or conflict-resolution session records none;
/// - a user's Stop, or an Apply, Discard or Archive that stopped a turn;
/// - a failed turn that began after it: the successor session never started;
/// - a person's *Return to the old session*, `SessionHandoffWithdrawn`.
pub(crate) fn ends_handoff_sql(alias: &str, handoff: &str) -> String {
    format!(
        "{alias}.sequence > {handoff}.sequence AND ( \
           ({alias}.event_type = 'SessionStarted' \
              AND {alias}.payload->'continuity'->>'kind' = 'handoff') \
           OR {alias}.event_type = 'SessionHandoffWithdrawn' \
           OR ({alias}.event_type = 'ResponseCanceled' \
              AND {alias}.payload->>'cause' IN ('user_stop', 'user_action')) \
           OR ({alias}.event_type = 'ResponseFailed' \
              AND EXISTS (SELECT 1 FROM events {alias}_turn \
                   WHERE {alias}_turn.thread_id = {alias}.thread_id \
                     AND {alias}_turn.event_type = 'MessageReceived' \
                     AND {alias}_turn.sequence > {handoff}.sequence \
                     AND {alias}_turn.sequence < {alias}.sequence)))"
    )
}

/// The handoff `thread_id`'s next session starts from, if one is pending.
pub(crate) async fn pending_handoff(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<PendingHandoff>, sqlx::Error> {
    let payload: Option<serde_json::Value> = sqlx::query_scalar(&format!(
        concat!(
            "SELECT handoff.payload FROM events handoff \
         WHERE handoff.thread_id = $1 AND handoff.event_type = 'SessionHandedOff' \
           AND handoff.sequence > ",
            crate::core::snapshot::boundary::restore_floor_sql!(),
            " \
           AND NOT EXISTS (SELECT 1 FROM events handoff_end \
                 WHERE handoff_end.thread_id = $1 AND {}) \
         ORDER BY handoff.sequence DESC LIMIT 1"
        ),
        ends_handoff_sql("handoff_end", "handoff")
    ))
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let Some(payload) = payload else {
        return Ok(None);
    };
    match serde_json::from_value(payload) {
        Ok(handoff) => Ok(Some(handoff)),
        Err(e) => {
            crate::log!(
                "[Successor] unreadable SessionHandedOff on {}: {}",
                thread_id,
                e
            );
            Ok(None)
        }
    }
}

/// Whether `thread_id`'s successor session waits to start. A failed read
/// counts as no: the turn end then sends the agent back as usual.
pub(crate) async fn handoff_pending(pool: &PgPool, thread_id: Uuid) -> bool {
    pending_handoff(pool, thread_id)
        .await
        .unwrap_or_else(|e| {
            crate::log!("[Successor] pending handoff of {}: {}", thread_id, e);
            None
        })
        .is_some()
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

/// Whether `thread_id` has a change waiting in the landing lane. The lane
/// holds its snapshot, so the thread cannot hand off until it lands.
pub(crate) async fn awaits_landing(pool: &PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    Ok(!super::landing_lane::queued_changes(pool, thread_id)
        .await?
        .is_empty())
}

/// What keeps a coding-agent thread from handing off, whoever asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandoffBlock {
    /// An *integrated sub-thread*: its parent's agent merges its branch.
    Integrated,
    /// It continued in a *legacy successor thread*.
    LegacySuccessor(Uuid),
    Archived,
    /// Its change is applying.
    Applying,
    /// Its change waits in the landing lane.
    AwaitingLanding,
}

/// The first [`HandoffBlock`] that holds for `thread_id`, if any. A thread
/// with no summary row has none: the caller refuses it as missing.
pub(crate) async fn handoff_block(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<HandoffBlock>, sqlx::Error> {
    let row: Option<(bool, Option<Uuid>, String, bool)> = sqlx::query_as(
        "SELECT coding_agent_integrated, successor_thread_id, archive_state, apply_in_flight \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let Some((integrated, successor, archive_state, applying)) = row else {
        return Ok(None);
    };
    Ok(if integrated {
        Some(HandoffBlock::Integrated)
    } else if let Some(successor) = successor {
        Some(HandoffBlock::LegacySuccessor(successor))
    } else if archive_state == "archived" {
        Some(HandoffBlock::Archived)
    } else if applying {
        Some(HandoffBlock::Applying)
    } else if awaits_landing(pool, thread_id).await? {
        Some(HandoffBlock::AwaitingLanding)
    } else {
        None
    })
}

/// A handoff to record: [`record_handoff`] writes it as `SessionHandedOff`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NewHandoff {
    pub opening: HandoffOpening,
    pub message: String,
    pub checkpoint: Option<String>,
    pub progress: Option<String>,
    pub implementer: Implementer,
    pub request: Option<HandoffRequest>,
    pub cause: HandoffCause,
    pub starts: SessionStart,
    /// The ceiling the session crossed, on a ceiling handoff.
    pub crossing: Option<super::context_ceiling::CeilingCrossing>,
}

/// Why a handoff happens and when its session starts. A session past its
/// *context ceiling* starts on the next input, unless the user typed a request:
/// that request is the input. An agent's own handoff holds the thread until
/// its turn ends.
pub(crate) fn cause_and_start(
    past_ceiling: bool,
    at_checkpoint: bool,
    request: bool,
    from_agent: bool,
) -> (HandoffCause, SessionStart) {
    let cause = if past_ceiling {
        HandoffCause::ContextCeiling
    } else if at_checkpoint {
        HandoffCause::Checkpoint
    } else {
        HandoffCause::User
    };
    let starts = if past_ceiling && !request {
        SessionStart::OnNextInput
    } else if from_agent {
        SessionStart::AfterThisTurn
    } else {
        SessionStart::Now
    };
    (cause, starts)
}

/// Take `thread_id`'s handoff lock until the returned transaction ends. Every
/// writer of a handoff or its withdrawal holds it, across engines, so the
/// second writer re-reads what the first recorded.
pub(crate) async fn lock_handoff(
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

/// What [`record_handoff`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandoffRecord {
    Recorded,
    /// A handoff is pending already: one per session.
    Pending,
    /// A ceiling handoff found the session no longer past its ceiling: a
    /// person's raise or return landed first.
    CeilingLifted,
}

/// Record `handoff` on `thread_id`, unless one is pending already. One
/// handoff per session, across engines ([`lock_handoff`]). A ceiling handoff
/// re-reads the crossing under the lock, so a person's raise always wins.
pub(crate) async fn record_handoff(
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

/// What the thread's last applied change did, for a brief. Best effort: a
/// brief without it is still whole.
pub(crate) async fn last_applied_summary(pool: &PgPool, thread_id: Uuid) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(summary, description) FROM changes \
         WHERE thread_id = $1 AND status = 'applied' \
         ORDER BY resolved_at DESC NULLS LAST LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!("[Successor] last applied change of {}: {}", thread_id, e);
        None
    })
}

/// The work a session leaves running at its handoff. Its results reach the
/// thread, so the successor session must know not to start it again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StillRunning {
    /// Background tasks, by description.
    pub tasks: Vec<String>,
    /// Live event waits, by reason and the event types they watch.
    pub waits: Vec<String>,
    /// Working child threads, by title.
    pub children: Vec<String>,
}

impl StillRunning {
    /// `opening` with a "Still running" block after it. Nothing running
    /// leaves `opening` as it is.
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
    /// What `thread_id` leaves running now: its unstopped background tasks,
    /// its live event waits and its working children.
    pub(crate) async fn still_running(&self, thread_id: Uuid) -> StillRunning {
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
            crate::log!("[Successor] working children of {}: {}", thread_id, e);
            Vec::new()
        });
        StillRunning {
            tasks,
            waits,
            children,
        }
    }
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

/// Threads whose successor session this engine is starting, each with whether
/// another start came in meanwhile. That start runs once this one ends: a
/// successor session may hand off again in its own first turn.
static STARTING: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<Uuid, bool>>> =
    std::sync::LazyLock::new(Default::default);

/// Holds `thread_id` in [`STARTING`] until dropped, then runs a start that
/// came in meanwhile.
struct StartingGuard {
    thread_id: Uuid,
    engine: std::sync::Arc<super::LucidosEngine>,
}

impl StartingGuard {
    fn take(engine: &std::sync::Arc<super::LucidosEngine>, thread_id: Uuid) -> Option<Self> {
        let mut starting = STARTING.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(again) = starting.get_mut(&thread_id) {
            *again = true;
            return None;
        }
        starting.insert(thread_id, false);
        Some(Self {
            thread_id,
            engine: engine.clone(),
        })
    }
}

impl Drop for StartingGuard {
    fn drop(&mut self) {
        let again = STARTING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.thread_id)
            .unwrap_or(false);
        if again {
            self.engine.start_successor_session(self.thread_id);
        }
    }
}

impl super::LucidosEngine {
    /// Start `thread_id`'s *successor session* once no run holds the thread,
    /// so its old session's process has exited first. Does nothing when no
    /// handoff is pending by then: another input already started the session,
    /// and its first prompt carried the opening. A *next-input start* waits for
    /// that input instead.
    pub(crate) fn start_successor_session(self: &std::sync::Arc<Self>, thread_id: Uuid) {
        let Some(guard) = StartingGuard::take(self, thread_id) else {
            return;
        };
        let engine = self.clone();
        tokio::spawn(async move {
            wait_until_thread_released(&engine.agent_sessions, thread_id).await;
            if engine.is_shutting_down() {
                crate::log!(
                    "[Successor] Engine shutting down; thread {} starts its successor session on the next boot",
                    thread_id
                );
                return;
            }
            let handoff = match pending_handoff(engine.pool(), thread_id).await {
                Ok(Some(handoff)) if handoff.starts_unprompted() => handoff,
                Ok(_) => return,
                Err(e) => {
                    crate::log!("[Successor] pending handoff of {}: {}", thread_id, e);
                    return;
                }
            };
            crate::log!(
                "[Successor] Thread {} starts its successor session on {:?}",
                thread_id,
                handoff.coding_agent
            );
            let implementer = handoff.implementer();
            // The user's request starts the session as their own message. The
            // first prompt puts the opening ahead of it (`first_message`).
            let (prompt, origin) = match handoff.request {
                Some(request) => (request.message, request.by),
                None => (
                    handoff.message,
                    super::thread_events::MessageOrigin::engine(
                        super::thread_events::EngineReason::SuccessorSession,
                    ),
                ),
            };
            // The guard lives as long as the turn's task.
            engine.spawn_engine_coding_turn_as(
                thread_id,
                prompt,
                None,
                origin,
                implementer,
                move |_| drop(guard),
            );
        });
    }
}

/// Wait until no run holds `thread_id`: its process exited, and its teardown
/// let go of the worktree.
async fn wait_until_thread_released(
    agent_sessions: &tokio::sync::Mutex<
        std::collections::HashMap<Uuid, super::types::AgentSession>,
    >,
    thread_id: Uuid,
) {
    loop {
        let holder = {
            let sessions = agent_sessions.lock().await;
            sessions
                .get(&thread_id)
                .filter(|s| super::agent_session::spawn_claim::holds_thread(s))
                .map(|s| s.msg_tx.clone())
        };
        match holder {
            Some(holder) => holder.closed().await,
            None => return,
        }
    }
}

/// Start every successor session a handoff left pending, as at boot: the
/// engine that recorded the handoff may have stopped before it started one.
pub async fn start_pending_successor_sessions(engine: std::sync::Arc<super::LucidosEngine>) {
    let threads: Vec<Uuid> = match sqlx::query_scalar(&format!(
        "SELECT ts.thread_id FROM thread_summaries ts \
         WHERE ts.is_coding_agent AND {}",
        pending_handoff_sql("ts.thread_id")
    ))
    .fetch_all(engine.pool())
    .await
    {
        Ok(threads) => threads,
        Err(e) => {
            crate::log!("[Successor] Could not list pending handoffs: {}", e);
            return;
        }
    };
    for thread_id in threads {
        engine.start_successor_session(thread_id);
    }
}

#[cfg(test)]
#[path = "successor_tests.rs"]
mod tests;
