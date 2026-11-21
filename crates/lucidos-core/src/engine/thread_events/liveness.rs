//! Liveness facts and wire representation.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::event_subscription::EventSubscription;
use crate::core::store::automation_pause::StepHold;

use crate::engine::thread_lifecycle::ThreadStatus;

use super::{BackgroundWaitTiming, SessionStart, WakeSource};

/// How long a turn in flight may go without an event before it reads `quiet`.
pub const QUIET_AFTER: Duration = Duration::from_secs(10 * 60);

/// How long a pending handoff may go unstarted before it reads `stalled`.
/// The engine starts a successor within seconds of the old turn's end.
pub const HANDOFF_START_GRACE: Duration = Duration::from_secs(2 * 60);

/// A thread's liveness state. First match wins, in [`derive`]'s order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LivenessState {
    /// A turn is in flight and recently active, or a successor session is
    /// about to start.
    Working,
    /// A turn is in flight and silent for [`QUIET_AFTER`]. Not a verdict.
    Quiet,
    /// Parked on a question card.
    WaitingForUserAnswer,
    /// A version switch interrupted it, and the engine resumes it.
    Paused,
    /// The last turn failed, and nothing started since.
    Failed,
    /// The Thread Queue is holding the thread until capacity frees.
    Queued,
    /// An owner Stop or automation pause holds a wake.
    Held,
    /// The engine owes a successor session and has not started it.
    Stalled,
    /// No turn, but it wakes itself: live event waits or awaited sub-threads.
    Waiting,
    /// Nothing in flight, nothing owed.
    Idle,
}

impl LivenessState {
    /// Every state, in [`derive`]'s order. Feeds the generated TS union.
    pub const ALL: [Self; 10] = [
        Self::Working,
        Self::Quiet,
        Self::WaitingForUserAnswer,
        Self::Paused,
        Self::Failed,
        Self::Queued,
        Self::Held,
        Self::Stalled,
        Self::Waiting,
        Self::Idle,
    ];

    /// Adding a variant breaks this match. Add it to [`Self::ALL`] too.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Quiet => "quiet",
            Self::WaitingForUserAnswer => "waiting_for_user_answer",
            Self::Paused => "paused",
            Self::Failed => "failed",
            Self::Queued => "queued",
            Self::Held => "held",
            Self::Stalled => "stalled",
            Self::Waiting => "waiting",
            Self::Idle => "idle",
        }
    }
}

/// What [`derive`] decides from. Ages come from the database clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivenessFacts {
    pub status: ThreadStatus,
    /// Seconds since the thread's newest event. `None` for a thread with none.
    pub last_activity_age_secs: Option<i64>,
    /// When the pending handoff's session starts, if one is pending.
    pub pending_handoff: Option<SessionStart>,
    /// Live event waits or awaited sub-threads.
    pub waits: bool,
    pub queued: bool,
    pub held: bool,
    /// A terminator/idling event follows the newest turn start. Status can
    /// remain running here because an automatic successor is owed.
    pub between_turns: bool,
}

fn at_least(age_secs: Option<i64>, limit: Duration) -> bool {
    age_secs.is_some_and(|age| age >= limit.as_secs() as i64)
}

/// The one liveness definition. A next-input handoff owes nothing until an
/// input arrives, so it never makes a thread `stalled`. The grace counts from
/// the newest event, so it starts when the turn that handed off ended.
pub fn derive(facts: &LivenessFacts) -> LivenessState {
    let idle_status = matches!(facts.status, ThreadStatus::Idle | ThreadStatus::Waiting);
    let between_turns =
        idle_status || (facts.status == ThreadStatus::Running && facts.between_turns);
    let successor_gap = between_turns
        && facts
            .pending_handoff
            .is_some_and(SessionStart::is_unprompted);

    if between_turns && facts.queued {
        return LivenessState::Queued;
    }
    if facts.held && (successor_gap || (idle_status && facts.waits)) {
        return LivenessState::Held;
    }
    if successor_gap {
        return if at_least(facts.last_activity_age_secs, HANDOFF_START_GRACE) {
            LivenessState::Stalled
        } else {
            LivenessState::Working
        };
    }
    match facts.status {
        ThreadStatus::Running if at_least(facts.last_activity_age_secs, QUIET_AFTER) => {
            LivenessState::Quiet
        }
        ThreadStatus::Running => LivenessState::Working,
        ThreadStatus::WaitingForUserAnswer => LivenessState::WaitingForUserAnswer,
        ThreadStatus::Paused => LivenessState::Paused,
        ThreadStatus::Failed => LivenessState::Failed,
        ThreadStatus::Idle | ThreadStatus::Waiting if facts.waits => LivenessState::Waiting,
        ThreadStatus::Idle | ThreadStatus::Waiting => LivenessState::Idle,
    }
}

/// How the thread's current agent session began.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOpening {
    First,
    Fresh,
    Handoff,
}

/// The thread's current agent session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LivenessSession {
    /// Counted from 1, like `session_scope::current_session_ordinal`.
    pub ordinal: u32,
    pub continuity: SessionOpening,
    pub age_secs: i64,
}

/// Present only while a handoff is pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LivenessHandoff {
    Pending,
}

/// An unresolved event subscription and its remaining time at `as_of`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LivenessEventWait {
    pub reason: String,
    pub on: Vec<EventSubscription>,
    pub expires_at: DateTime<Utc>,
    pub expires_in_secs: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_timing: Option<BackgroundWaitTiming>,
}

/// A direct child whose finish can wake this thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LivenessAwaitedChild {
    pub thread_id: Uuid,
    pub title: String,
}

/// The latest wake estimate stated since this thread last started a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LivenessExpectedWake {
    pub at: DateTime<Utc>,
    pub source: WakeSource,
    pub reason: String,
}

/// The owner action holding a wake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum LivenessHold {
    Stop,
    AutomationPause { reason: String },
}

impl From<StepHold> for LivenessHold {
    fn from(hold: StepHold) -> Self {
        match hold {
            StepHold::Stop => Self::Stop,
            StepHold::AutomationPause(pause) => Self::AutomationPause {
                reason: pause.reason,
            },
        }
    }
}

/// The wire object every surface carries, about 200 bytes without waits.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThreadLiveness {
    pub state: LivenessState,
    pub status: ThreadStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_age_secs: Option<i64>,
    /// Absent on a thread with no agent session, such as a chat thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<LivenessSession>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handoff: Option<LivenessHandoff>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub live_event_waits: Vec<LivenessEventWait>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub awaited_children: Vec<LivenessAwaitedChild>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_wake: Option<LivenessExpectedWake>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold: Option<LivenessHold>,
    /// The database's `now()`, to the second, so a later turn can tell how old
    /// the observation is.
    pub as_of: DateTime<Utc>,
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[test]
    fn every_liveness_state_has_one_glossary_row() {
        let glossary = include_str!("../../../../../system-knowhow/glossary.md");
        let section = glossary
            .split_once("### Thread liveness\n")
            .unwrap()
            .1
            .split("\n### ")
            .next()
            .unwrap();
        let documented: Vec<_> = section
            .lines()
            .filter_map(|line| {
                line.strip_prefix("| `")?
                    .split_once("` |")
                    .map(|(state, _)| state)
            })
            .collect();
        assert_eq!(documented, LivenessState::ALL.map(LivenessState::as_str));
    }
}
