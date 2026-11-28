use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::event_subscription::EventSubscription;
use crate::core::store::thread_hold::TurnHold;

use crate::engine::thread_lifecycle::{awaits_owner_answer, ThreadStatus};

use super::{BackgroundWaitTiming, SessionStart, WakeSource};

pub const QUIET_AFTER: Duration = Duration::from_secs(10 * 60);

/// INVARIANT: How long a pending handoff may go unstarted before it reads `stalled`.
/// The engine starts a successor within seconds of the old turn's end.
pub const HANDOFF_START_GRACE: Duration = Duration::from_secs(2 * 60);

/// INVARIANT: A thread's liveness state. First match wins, in [`derive`]'s order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LivenessState {
    Working,
    Quiet,
    WaitingForUserAnswer,
    Paused,
    Failed,
    Queued,
    Held,
    Stalled,
    Waiting,
    Idle,
}

impl LivenessState {
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

    /// INVARIANT: Adding a variant breaks this match. Add it to [`Self::ALL`] too.
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

/// INVARIANT: What [`derive`] decides from. Ages come from the database clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivenessFacts {
    pub status: ThreadStatus,
    pub last_activity_age_secs: Option<i64>,
    pub pending_handoff: Option<SessionStart>,
    pub waits: bool,
    pub queued: bool,
    pub held: bool,
    pub on_hold: bool,
    pub between_turns: bool,
}

fn at_least(age_secs: Option<i64>, limit: Duration) -> bool {
    age_secs.is_some_and(|age| age >= limit.as_secs() as i64)
}

/// INVARIANT: The one liveness definition. A next-input handoff owes nothing until an
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
        ThreadStatus::WaitingForUserAnswer if awaits_owner_answer(facts.status, facts.on_hold) => {
            LivenessState::WaitingForUserAnswer
        }
        ThreadStatus::WaitingForUserAnswer => LivenessState::Held,
        ThreadStatus::Paused => LivenessState::Paused,
        ThreadStatus::Failed => LivenessState::Failed,
        ThreadStatus::Idle | ThreadStatus::Waiting if facts.waits => LivenessState::Waiting,
        ThreadStatus::Idle | ThreadStatus::Waiting => LivenessState::Idle,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOpening {
    First,
    Fresh,
    Handoff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LivenessSession {
    /// INVARIANT: Counted from 1, like `session_scope::current_session_ordinal`.
    pub ordinal: u32,
    pub continuity: SessionOpening,
    pub age_secs: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LivenessHandoff {
    Pending,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LivenessEventWait {
    pub reason: String,
    pub on: Vec<EventSubscription>,
    pub expires_at: DateTime<Utc>,
    pub expires_in_secs: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_timing: Option<BackgroundWaitTiming>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LivenessAwaitedChild {
    pub thread_id: Uuid,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LivenessExpectedWake {
    pub at: DateTime<Utc>,
    pub source: WakeSource,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum LivenessHold {
    Stop,
    OpenCard {
        card_id: String,
    },
    Hold {
        reason: String,
        root_thread_id: uuid::Uuid,
    },
}

impl From<TurnHold> for LivenessHold {
    fn from(hold: TurnHold) -> Self {
        match hold {
            TurnHold::Stop => Self::Stop,
            TurnHold::OpenCard { card_id } => Self::OpenCard { card_id },
            TurnHold::Hold(hold) => Self::Hold {
                reason: hold.reason,
                root_thread_id: hold.root_thread_id,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThreadLiveness {
    pub state: LivenessState,
    pub status: ThreadStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_age_secs: Option<i64>,
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
    pub as_of: DateTime<Utc>,
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[test]
    fn every_liveness_state_has_one_glossary_row() {
        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
        let glossary = crate::core::knowhow::doc_family_text(&repo, "system-knowhow/glossary.md");
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
