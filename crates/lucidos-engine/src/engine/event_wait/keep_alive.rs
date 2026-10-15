//! The *cache keep-alive*: keep an idle Claude Code session's prompt cache warm
//! while it waits.
//!
//! The cache lives 5 minutes or 1 hour from the start of the last request. A
//! longer wait makes the wake-up re-write the whole context. So the engine
//! resumes a waiting idle session with a hidden nudge just before its cache
//! would expire. It waits on a *keep-warm* event wait or on a working child
//! thread. The agent answers in a word and idles again.
//!
//! - **The TTL is observed, never guessed.** Claude Code's usage block splits
//!   each cache write by lifetime. An unknown TTL beats as 5 minutes.
//! - **Timing comes from persisted events**, so a restart causes no burst.
//! - **A beat never contends for the thread.** It skips a live, held or busy
//!   thread, and one a person owes an answer.
//! - **A child parked on the user keeps nobody warm.** That wait has no bound.
//!   Its answer restarts the beats only while the parent's cache is still warm.
//! - **The turn is invisible.** `event_bus_projection_keep_alive` keeps it off
//!   the summary row, and the run session proposes nothing for it.
//!
//! Why a keep-alive and not the 1-hour TTL: ADR 0470. Why the window follows
//! the observed TTL: ADR 0479.

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::LiveWait;
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{
    ActorMode, EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
    KEEP_ALIVE_IDLE_REASON,
};
use crate::engine::thread_lifecycle::ThreadStatus;
use crate::engine::{LucidosEngine, PreEmittedOrigin};
use crate::runtime::CodingAgent;

/// The events whose waits are worth keeping warm, chosen from measured wait
/// lengths: most end within the cut-off. Any other kind gets no keep-alive.
pub(crate) const KEEP_WARM_EVENT_TYPES: [&str; 7] = [
    "BackgroundBashCompleted",
    "E2ELockReleased",
    "SuiteLockReleased",
    "ChildThreadCompleted",
    "CodingAgentIdled",
    "ChangeApplied",
    "ChangeProposed",
];

/// How long after the wait began the engine gives up, in seconds. The
/// measured optimum: past it, the wake pays one re-write, as it would without
/// a keep-alive. It bounds the wait, so it holds for either TTL.
const GIVE_UP_AFTER_SECS: f64 = 150.0 * 60.0;

/// How long a session's prompt cache lives, as its latest cache write shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CacheTtl {
    FiveMinutes,
    OneHour,
}

impl CacheTtl {
    /// The TTL of a write of `written` tokens, `written_1h` of them to the
    /// 1-hour cache. `None` for no write, or one the provider did not split.
    /// Any 5-minute token makes it 5 minutes: that part expires first.
    pub(super) fn observed(written: i64, written_1h: Option<i64>) -> Option<Self> {
        let written_1h = written_1h.filter(|_| written > 0)?;
        Some(if written_1h >= written {
            Self::OneHour
        } else {
            Self::FiveMinutes
        })
    }

    /// When the beat fires, in seconds since the last request started. The
    /// tick runs every 10 s, and a beat restarts the session cold. So the
    /// resumed request lands well inside the lifetime: 300 s, or 3600 s.
    pub(super) fn beat_window(self) -> RangeInclusive<f64> {
        match self {
            Self::FiveMinutes => 200.0..=260.0,
            Self::OneHour => 3300.0..=3360.0,
        }
    }
}

/// What the agent reads. It must answer without a tool call, or the turn costs
/// more than the cache it saves.
const KEEP_ALIVE_PROMPT: &str = "Cache keep-alive from the Lucidos engine. You are still \
    waiting, and nothing has arrived. Do nothing: run no command, call no tool, and send no \
    read decision. Reply with the single word: waiting.";

/// The in-memory guard against a second beat while the first one's request is
/// still on its way. Lost on restart, which is harmless: a beat in flight then
/// died with its session.
pub(crate) type KeepAliveBeats = Arc<std::sync::Mutex<HashMap<Uuid, std::time::Instant>>>;

/// Whether anything else has the thread right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ThreadClaim {
    Free,
    Busy,
}

/// What the engine knows about one waiting thread at a tick.
#[derive(Debug, Clone)]
pub(super) struct Candidate {
    pub coding_agent: CodingAgent,
    pub claim: ThreadClaim,
    /// A person owes the thread an answer, so a turn would supersede the card.
    pub awaiting_user: bool,
    /// Seconds since the newest of its live waits that watch keep-warm kinds
    /// only was armed. `None` when it holds no such wait.
    pub keep_warm_wait_secs: Option<f64>,
    pub children: ActiveChildren,
    /// Seconds since the last turn request started. `None` when it never ran.
    pub secs_since_request: Option<f64>,
    /// Seconds since this engine last beat for the thread, from the cache.
    pub secs_since_beat: Option<f64>,
    /// The TTL its latest cache write showed. `None` when none showed one.
    pub cache_ttl: Option<CacheTtl>,
}

/// The thread's active child threads, as the keep-alive weighs them. An active
/// child is `running` or `waiting_for_user_answer`. Only a `running` one works.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum ActiveChildren {
    None,
    /// Every active child waits on a question, permission or plan approval card.
    AllAwaitingUser,
    /// At least one active child is working. The wait began when the agent
    /// last went idle for real, this many seconds ago.
    Working {
        wait_secs: f64,
    },
}

/// The selection's verdict.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Beat {
    /// Resume the session now.
    Send,
    Skip(SkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SkipReason {
    NotClaudeCode,
    Busy,
    AwaitingUser,
    ChildrenAwaitingUser,
    NothingToWaitFor,
    PastCutOff,
    NoRequestYet,
    OutsideWindow,
    BeatInFlight,
}

/// Decide one thread's beat. Pure, so every condition and boundary is
/// table-tested (I5).
pub(super) fn decide(c: &Candidate) -> Beat {
    use SkipReason::*;
    if c.coding_agent != CodingAgent::ClaudeCode {
        return Beat::Skip(NotClaudeCode);
    }
    if c.claim == ThreadClaim::Busy {
        return Beat::Skip(Busy);
    }
    if c.awaiting_user {
        return Beat::Skip(AwaitingUser);
    }
    let children_wait_secs = match c.children {
        ActiveChildren::Working { wait_secs } => Some(wait_secs),
        ActiveChildren::None | ActiveChildren::AllAwaitingUser => None,
    };
    // The most recent start wins: a fresh wait keeps a thread warm even when
    // its children have run for hours.
    let waiting_secs = match (c.keep_warm_wait_secs, children_wait_secs) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) if c.children == ActiveChildren::AllAwaitingUser => {
            return Beat::Skip(ChildrenAwaitingUser)
        }
        (None, None) => return Beat::Skip(NothingToWaitFor),
    };
    if waiting_secs > GIVE_UP_AFTER_SECS {
        return Beat::Skip(PastCutOff);
    }
    let Some(secs) = c.secs_since_request else {
        return Beat::Skip(NoRequestYet);
    };
    // An unknown TTL beats as 5 minutes. Beating a 1-hour cache that often
    // wastes reads, but beating a 5-minute one an hour late loses it.
    let window = c.cache_ttl.unwrap_or(CacheTtl::FiveMinutes).beat_window();
    if !window.contains(&secs) {
        return Beat::Skip(OutsideWindow);
    }
    if c.secs_since_beat.is_some_and(|s| s < *window.start()) {
        return Beat::Skip(BeatInFlight);
    }
    Beat::Send
}

/// The latest `armed_at` among `waits` that watch keep-warm kinds only.
pub(super) fn keep_warm_armed_at<'a>(
    waits: impl IntoIterator<Item = &'a LiveWait>,
) -> Option<DateTime<Utc>> {
    waits
        .into_iter()
        .filter(|w| {
            !w.on.is_empty()
                && w.on
                    .iter()
                    .all(|s| KEEP_WARM_EVENT_TYPES.contains(&s.event_type.as_str()))
        })
        .map(|w| w.armed_at)
        .max()
}

/// One thread's row, as the tick reads it.
#[derive(sqlx::FromRow)]
struct CandidateRow {
    thread_id: Uuid,
    coding_agent: Option<String>,
    awaiting_user: bool,
    has_active_children: bool,
    has_working_children: bool,
    secs_since_request: Option<f64>,
    secs_since_idle: Option<f64>,
    last_write_tokens: Option<i64>,
    last_write_1h_tokens: Option<i64>,
}

impl CandidateRow {
    fn cache_ttl(&self) -> Option<CacheTtl> {
        CacheTtl::observed(self.last_write_tokens?, self.last_write_1h_tokens)
    }

    fn active_children(&self) -> ActiveChildren {
        match (
            self.has_active_children,
            self.has_working_children,
            self.secs_since_idle,
        ) {
            (false, _, _) => ActiveChildren::None,
            (true, false, _) => ActiveChildren::AllAwaitingUser,
            (true, true, Some(wait_secs)) => ActiveChildren::Working { wait_secs },
            // The agent never went idle for real, so no wait began.
            (true, true, None) => ActiveChildren::None,
        }
    }
}

/// Every thread that holds a live wait or active children, with what the
/// selection reads. Ages of database rows are computed in SQL (ADR 0053). The
/// latest real idle is the latest `CodingAgentIdled` no keep-alive turn wrote.
/// [`read_candidates`] binds it. The TTL comes from the latest capture of the
/// session's own that wrote to the cache. A cache hit keeps an entry's
/// lifetime, and a sub-agent may run on another TTL. A Codex thread never
/// writes one, so it skips the look-up rather than scan its whole history.
const CANDIDATES_SQL: &str = r#"
SELECT ts.thread_id,
       ts.coding_agent,
       ts.status = $3 AS awaiting_user,
       ts.active_children_count > 0 AS has_active_children,
       EXISTS (SELECT 1 FROM thread_summaries c
                WHERE c.parent_thread_id = ts.thread_id
                  AND c.status = $4) AS has_working_children,
       (SELECT EXTRACT(EPOCH FROM now() - e.created)::float8
               + COALESCE((e.payload->>'duration_ms')::float8, 0) / 1000
          FROM events e
         WHERE e.thread_id = ts.thread_id
           AND e.event_type = 'ContextCaptured'
           AND COALESCE(e.payload->>'purpose', 'turn') = 'turn'
         ORDER BY e.sequence DESC LIMIT 1) AS secs_since_request,
       (SELECT EXTRACT(EPOCH FROM now() - e.created)::float8
          FROM events e
         WHERE e.thread_id = ts.thread_id
           AND e.event_type = 'CodingAgentIdled'
           AND COALESCE(e.payload->>'reason', '') <> $2
         ORDER BY e.sequence DESC LIMIT 1) AS secs_since_idle,
       w.written AS last_write_tokens,
       w.written_1h AS last_write_1h_tokens
  FROM thread_summaries ts
  LEFT JOIN LATERAL (
       SELECT (e.payload->'usage'->>'cache_creation_tokens')::bigint AS written,
              (e.payload->'usage'->>'cache_creation_1h_tokens')::bigint AS written_1h
         FROM events e
        WHERE ts.coding_agent IS DISTINCT FROM $5
          AND e.thread_id = ts.thread_id
          AND e.event_type = 'ContextCaptured'
          AND COALESCE(e.payload->>'purpose', 'turn') = 'turn'
          AND e.payload->>'parent_tool_use_id' IS NULL
          AND (e.payload->'usage'->>'cache_creation_tokens')::bigint > 0
        ORDER BY e.sequence DESC LIMIT 1) w ON true
 WHERE ts.is_coding_agent
   AND (ts.thread_id = ANY($1) OR ts.active_children_count > 0)
"#;

/// The candidates: `waiting` threads, plus every parent with active children.
async fn read_candidates(
    pool: &sqlx::PgPool,
    waiting: &[Uuid],
) -> Result<Vec<CandidateRow>, sqlx::Error> {
    sqlx::query_as(CANDIDATES_SQL)
        .bind(waiting)
        .bind(KEEP_ALIVE_IDLE_REASON)
        .bind(ThreadStatus::WaitingForUserAnswer.as_str())
        .bind(ThreadStatus::Running.as_str())
        .bind(CodingAgent::Codex.as_str())
        .fetch_all(pool)
        .await
}

impl LucidosEngine {
    /// One tick of the keep-alive: beat every thread the selection picks.
    /// Runs on the event-wait deadline sweep.
    pub(super) async fn beat_cache_keep_alives(self: &Arc<Self>) {
        // Host clock against host clock: an engine stamps `armed_at`.
        let now = Utc::now();
        let snapshot = self.live_waits.snapshot().await;
        let mut by_thread: HashMap<Uuid, Vec<&LiveWait>> = HashMap::new();
        for wait in &snapshot {
            by_thread.entry(wait.thread_id).or_default().push(wait);
        }
        let armed_at: HashMap<Uuid, DateTime<Utc>> = by_thread
            .into_iter()
            .filter_map(|(thread_id, waits)| Some((thread_id, keep_warm_armed_at(waits)?)))
            .collect();
        let waiting: Vec<Uuid> = armed_at.keys().copied().collect();
        let rows = match read_candidates(&self.pool, &waiting).await {
            Ok(rows) => rows,
            Err(e) => {
                crate::log!("[KeepAlive] Could not read the waiting threads: {e}");
                return;
            }
        };
        for row in rows {
            let candidate = Candidate {
                coding_agent: CodingAgent::parse(row.coding_agent.as_deref().unwrap_or("")),
                claim: self.keep_alive_claim(row.thread_id).await,
                awaiting_user: row.awaiting_user,
                keep_warm_wait_secs: armed_at
                    .get(&row.thread_id)
                    .map(|at| (now - *at).num_milliseconds() as f64 / 1000.0),
                children: row.active_children(),
                secs_since_request: row.secs_since_request,
                secs_since_beat: self.secs_since_keep_alive_beat(row.thread_id),
                cache_ttl: row.cache_ttl(),
            };
            if decide(&candidate) == Beat::Send {
                self.send_keep_alive(row.thread_id).await;
            }
        }
    }

    /// Busy when a session, a hold or a turn has the thread.
    async fn keep_alive_claim(&self, thread_id: Uuid) -> ThreadClaim {
        let held = self
            .agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .is_some_and(crate::engine::agent_session::spawn_claim::holds_thread);
        let turning = self.active_threads.lock().unwrap().contains_key(&thread_id);
        if held || turning {
            ThreadClaim::Busy
        } else {
            ThreadClaim::Free
        }
    }

    fn secs_since_keep_alive_beat(&self, thread_id: Uuid) -> Option<f64> {
        self.keep_alive_beats
            .lock()
            .unwrap()
            .get(&thread_id)
            .map(|at| at.elapsed().as_secs_f64())
    }

    /// Write the hidden anchor, then resume the session through the same route
    /// a wait re-entry takes. A busy thread skips the beat inside that route.
    async fn send_keep_alive(self: &Arc<Self>, thread_id: Uuid) {
        self.keep_alive_beats
            .lock()
            .unwrap()
            .insert(thread_id, std::time::Instant::now());
        let anchor = ThreadEvent::PromptInjected {
            text: KEEP_ALIVE_PROMPT.to_string(),
            mode: ActorMode::Engine,
            origin: Some(MessageOrigin::engine(EngineReason::CacheKeepAlive)),
            injected_message_id: None,
            delivered_event_id: None,
        };
        let emitted = self
            .event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: anchor,
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    ..EventMeta::NONE
                },
            })
            .await;
        let anchor_id = match emitted {
            Ok(Some(result)) => result.event_id,
            Ok(None) => return,
            Err(e) => {
                crate::log!("[KeepAlive] Could not write the anchor for thread {thread_id}: {e}");
                return;
            }
        };
        crate::log!("[KeepAlive] Resuming thread {thread_id} to keep its prompt cache warm");
        let engine = self.clone();
        tokio::spawn(async move {
            if let Err(e) = engine
                .process_message_with_steps(
                    KEEP_ALIVE_PROMPT,
                    None,
                    None,
                    None,
                    None,
                    None, // provider_override
                    None,
                    None,
                    Some(true),
                    None,
                    Some(thread_id),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    ActorMode::Engine,
                    None,
                    None,
                    Some(PreEmittedOrigin::KeepAlive(anchor_id)),
                    None,
                    None,
                    None,
                    crate::engine::FollowUpUrgency::Normal,
                    None,
                )
                .await
            {
                crate::log!("[KeepAlive] Keep-alive turn failed for thread {thread_id}: {e}");
            }
        });
    }
}

#[cfg(test)]
#[path = "keep_alive_tests.rs"]
mod tests;
