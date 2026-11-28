use serde::Serialize;
use uuid::Uuid;

use crate::engine::thread_lifecycle::ThreadStatus;

/// INVARIANT: What triage proposes for one thread. `follow_up` and `keep` apply nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageAction {
    Archive,
    Pin,
    Delete,
    FollowUp,
    DismissQuestion,
    Keep,
}

impl TriageAction {
    pub const ALL: [Self; 6] = [
        Self::Archive,
        Self::Pin,
        Self::Delete,
        Self::FollowUp,
        Self::DismissQuestion,
        Self::Keep,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Pin => "pin",
            Self::Delete => "delete",
            Self::FollowUp => "follow_up",
            Self::DismissQuestion => "dismiss_question",
            Self::Keep => "keep",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedFact {
    Question,
    PendingChange,
    UnproposedWork,
    Draft,
    FailedRun,
}

impl NeedFact {
    pub fn words(self) -> &'static str {
        match self {
            Self::Question => "an unanswered question",
            Self::PendingChange => "a change waiting to be applied",
            Self::UnproposedWork => "branch work that was never proposed",
            Self::Draft => "an unsent draft",
            Self::FailedRun => "a failed last run",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerRun {
    pub trigger_id: String,
    pub trigger_name: String,
    pub is_newest: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriageFacts {
    pub thread_id: Uuid,
    pub status: ThreadStatus,
    pub is_pinned: bool,
    pub has_pending_question: bool,
    pub has_pending_change: bool,
    pub has_unproposed_work: bool,
    pub has_draft: bool,
    pub has_output: bool,
    pub ever_proposed_change: bool,
    pub sub_thread_count: i64,
    pub live_event_waits: i64,
    pub sub_thread_needs: Vec<NeedFact>,
    pub sub_thread_busy: bool,
    pub idle_secs: i64,
    pub trigger: Option<TriggerRun>,
}

impl TriageFacts {
    pub fn own_needs(&self) -> Vec<NeedFact> {
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

    pub fn busy_reason(&self) -> Option<&'static str> {
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

    pub fn needs_user(&self) -> bool {
        !self.own_needs().is_empty() || !self.sub_thread_needs.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriageVerdict {
    pub action: TriageAction,
    pub reason: String,
}

fn verdict(action: TriageAction, reason: impl Into<String>) -> TriageVerdict {
    TriageVerdict {
        action,
        reason: reason.into(),
    }
}

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

pub fn idle_words(secs: i64) -> String {
    let (n, unit) = match secs.max(0) {
        s if s < 3_600 => ((s / 60).max(1), "minute"),
        s if s < 86_400 => (s / 3_600, "hour"),
        s => (s / 86_400, "day"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// INVARIANT: Classify one thread. The order is the safety rule: busy first, then
/// anything the user still has to do, and only then the put-away actions.
pub fn classify(facts: &TriageFacts) -> TriageVerdict {
    if let Some(reason) = facts.busy_reason() {
        return verdict(TriageAction::Keep, reason);
    }
    let own = facts.own_needs();
    if !own.is_empty() || !facts.sub_thread_needs.is_empty() {
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

/// INVARIANT: Why `action` may not run on a freshly read row, or `None` when it may.
/// `apply_triage` and Archive all both ask this, so they refuse alike.
pub fn refuse_fresh(
    action: TriageAction,
    row: Option<&crate::engine::triage_facts::TriageRow>,
) -> Option<String> {
    match row {
        None => Some("it no longer exists".to_string()),
        Some(row) if row.section != "inbox" => Some("it is already archived".to_string()),
        Some(row) => refuse_apply(action, &row.facts),
    }
}

/// INVARIANT: Why `action` may not be applied to a thread with these facts, or `None`
/// when it may. Run against fresh facts at apply time, whatever was proposed.
pub fn refuse_apply(action: TriageAction, facts: &TriageFacts) -> Option<String> {
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
pub fn refuse_completed(row: Option<&crate::engine::triage_facts::TriageRow>) -> Option<String> {
    let own_pin = row.map(|r| {
        let mut r = r.clone();
        r.facts.is_pinned = r.pinned_itself;
        r
    });
    refuse_fresh(TriageAction::Archive, own_pin.as_ref()).or_else(|| {
        row.filter(|r| {
            r.facts.status != crate::engine::thread_lifecycle::ThreadStatus::Idle
                || !r.facts.has_output
        })
        .map(|_| "it has not finished with output".to_string())
    })
}
