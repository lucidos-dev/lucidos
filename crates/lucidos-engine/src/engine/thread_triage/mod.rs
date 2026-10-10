//! *Thread triage*: which inbox threads need the user, and what to do with the
//! rest (ADR 0349).
//!
//! One classifier answers for two callers. The `threads` tool's `triage`
//! action proposes an action per thread, and Archive all puts away only what
//! this module calls safe. A thread is judged from facts, never its title.

pub(crate) mod archive_all;
pub(crate) mod facts;
pub(crate) mod proposal;
pub(crate) mod report;
mod tool;

#[cfg(test)]
mod tests;

use serde::Serialize;
use uuid::Uuid;

use crate::engine::thread_lifecycle::ThreadStatus;

/// What triage proposes for one thread. `follow_up` and `keep` apply nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TriageAction {
    Archive,
    Pin,
    /// Proposed only. Delete is the owner's own act (ADR 0192).
    Delete,
    FollowUp,
    DismissQuestion,
    Keep,
}

impl TriageAction {
    pub(crate) const ALL: [Self; 6] = [
        Self::Archive,
        Self::Pin,
        Self::Delete,
        Self::FollowUp,
        Self::DismissQuestion,
        Self::Keep,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Pin => "pin",
            Self::Delete => "delete",
            Self::FollowUp => "follow_up",
            Self::DismissQuestion => "dismiss_question",
            Self::Keep => "keep",
        }
    }

    pub(crate) fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == raw)
    }
}

/// A fact that means the user still has something to do in a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NeedFact {
    Question,
    PendingChange,
    UnproposedWork,
    Draft,
    FailedRun,
}

impl NeedFact {
    fn words(self) -> &'static str {
        match self {
            Self::Question => "an unanswered question",
            Self::PendingChange => "a change waiting to be applied",
            Self::UnproposedWork => "branch work that was never proposed",
            Self::Draft => "an unsent draft",
            Self::FailedRun => "a failed last run",
        }
    }
}

/// Which trigger spawned a thread, and whether it is that trigger's newest run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TriggerRun {
    pub(crate) trigger_id: String,
    pub(crate) trigger_name: String,
    pub(crate) is_newest: bool,
}

/// Everything triage weighs about one inbox root thread. No title on purpose:
/// a reason must come from what the thread holds, not what it is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TriageFacts {
    pub(crate) thread_id: Uuid,
    pub(crate) status: ThreadStatus,
    pub(crate) is_pinned: bool,
    pub(crate) has_pending_question: bool,
    pub(crate) has_pending_change: bool,
    pub(crate) has_unproposed_work: bool,
    pub(crate) has_draft: bool,
    pub(crate) has_output: bool,
    /// The thread or a sub-thread ever proposed a change, whatever became of it.
    pub(crate) ever_proposed_change: bool,
    pub(crate) sub_thread_count: i64,
    pub(crate) live_event_waits: i64,
    /// Need facts held by any sub-thread, rolled up.
    pub(crate) sub_thread_needs: Vec<NeedFact>,
    /// An open sub-thread is running, paused, or waiting on an event.
    pub(crate) sub_thread_busy: bool,
    pub(crate) idle_secs: i64,
    pub(crate) trigger: Option<TriggerRun>,
}

impl TriageFacts {
    /// The thread's own need facts, in a fixed order.
    pub(crate) fn own_needs(&self) -> Vec<NeedFact> {
        [
            (self.has_pending_question, NeedFact::Question),
            (self.has_pending_change, NeedFact::PendingChange),
            (self.has_unproposed_work, NeedFact::UnproposedWork),
            (self.has_draft, NeedFact::Draft),
            (self.status == ThreadStatus::Failed, NeedFact::FailedRun),
        ]
        .into_iter()
        .filter_map(|(holds, fact)| holds.then_some(fact))
        .collect()
    }

    /// Why the thread is busy, when it is: something is working in it now.
    pub(crate) fn busy_reason(&self) -> Option<&'static str> {
        if self.status == ThreadStatus::Running {
            Some("running now")
        } else if self.status == ThreadStatus::Paused {
            Some("paused, and resumes on its own")
        } else if self.live_event_waits > 0 {
            Some("waiting on an event")
        } else if self.sub_thread_busy {
            Some("a sub-thread is running or waiting on an event")
        } else {
            None
        }
    }

    /// Whether the user still has something to do here or in a sub-thread.
    pub(crate) fn needs_user(&self) -> bool {
        !self.own_needs().is_empty() || !self.sub_thread_needs.is_empty()
    }
}

/// Triage's proposal for one thread, with the reason it gives the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TriageVerdict {
    pub(crate) action: TriageAction,
    pub(crate) reason: String,
}

fn verdict(action: TriageAction, reason: impl Into<String>) -> TriageVerdict {
    TriageVerdict {
        action,
        reason: reason.into(),
    }
}

/// A sentence naming the facts, joined in plain English.
fn needs_sentence(own: &[NeedFact], sub: &[NeedFact]) -> String {
    let join = |facts: &[NeedFact]| {
        facts
            .iter()
            .map(|f| f.words())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match (own.is_empty(), sub.is_empty()) {
        (false, true) => format!("holds {}", join(own)),
        (true, false) => format!("a sub-thread holds {}", join(sub)),
        _ => format!("holds {}; a sub-thread holds {}", join(own), join(sub)),
    }
}

/// How long ago, in the largest whole unit.
pub(crate) fn idle_words(secs: i64) -> String {
    let (n, unit) = match secs.max(0) {
        s if s < 3_600 => ((s / 60).max(1), "minute"),
        s if s < 86_400 => (s / 3_600, "hour"),
        s => (s / 86_400, "day"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// Classify one thread. The order is the safety rule: busy first, then
/// anything the user still has to do, and only then the put-away actions.
pub(crate) fn classify(facts: &TriageFacts) -> TriageVerdict {
    if let Some(reason) = facts.busy_reason() {
        return verdict(TriageAction::Keep, reason);
    }
    let own = facts.own_needs();
    if !own.is_empty() || !facts.sub_thread_needs.is_empty() {
        // An older trigger run whose only open item is its own question is
        // stale: the user reads the newest run.
        if let Some(run) = facts.trigger.as_ref().filter(|r| !r.is_newest) {
            if own == [NeedFact::Question] && facts.sub_thread_needs.is_empty() {
                return verdict(
                    TriageAction::DismissQuestion,
                    format!(
                        "older run of {}; its question is stale, a newer run exists",
                        run.trigger_name
                    ),
                );
            }
        }
        return verdict(
            TriageAction::FollowUp,
            needs_sentence(&own, &facts.sub_thread_needs),
        );
    }
    if facts.is_pinned {
        return verdict(TriageAction::Keep, "pinned");
    }
    if let Some(run) = &facts.trigger {
        return if run.is_newest {
            verdict(
                TriageAction::Keep,
                format!("newest run of {}", run.trigger_name),
            )
        } else {
            verdict(
                TriageAction::Archive,
                format!("older run of {}; a newer run exists", run.trigger_name),
            )
        };
    }
    if !facts.has_output && facts.sub_thread_count == 0 && !facts.ever_proposed_change {
        return verdict(TriageAction::Delete, "never produced any output");
    }
    verdict(
        TriageAction::Archive,
        format!("idle for {}, nothing pending", idle_words(facts.idle_secs)),
    )
}

/// Why `action` may not run on a freshly read row, or `None` when it may.
/// `apply_triage` and Archive all both ask this, so they refuse alike.
pub(crate) fn refuse_fresh(action: TriageAction, row: Option<&facts::TriageRow>) -> Option<String> {
    match row {
        None => Some("it no longer exists".to_string()),
        Some(row) if row.section != "inbox" => Some("it is already archived".to_string()),
        Some(row) => refuse_apply(action, &row.facts),
    }
}

/// Why `action` may not be applied to a thread with these facts, or `None`
/// when it may. Run against fresh facts at apply time, whatever was proposed.
pub(crate) fn refuse_apply(action: TriageAction, facts: &TriageFacts) -> Option<String> {
    match action {
        TriageAction::Archive => {
            if let Some(reason) = facts.busy_reason() {
                Some(format!("not archived: it is {reason}"))
            } else if facts.needs_user() {
                Some(format!(
                    "not archived: it {}. Resolve that first",
                    needs_sentence(&facts.own_needs(), &facts.sub_thread_needs)
                ))
            } else if facts.is_pinned {
                Some("not archived: the user pinned it".to_string())
            } else {
                None
            }
        }
        TriageAction::Pin => None,
        TriageAction::DismissQuestion => (!facts.has_pending_question)
            .then(|| "no question is pending on it any more".to_string()),
        TriageAction::Delete => Some(
            "not deleted: only the user can delete a thread, from the thread drawer. \
             Give them the list"
                .to_string(),
        ),
        TriageAction::FollowUp | TriageAction::Keep => {
            Some(format!("'{}' applies nothing", action.as_str()))
        }
    }
}
