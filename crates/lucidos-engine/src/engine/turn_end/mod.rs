//! The *turn-end gate*: what a turn must meet before its response is recorded
//! (ADR 0417).
//!
//! Every *turn-end requirement* is checked here. Both turn ends call the gate:
//! the chat loop before `ResponseGenerated`, and the session runner around
//! `CodingAgentIdled`. Each requirement says how it is enforced: a forced
//! decision the engine makes, or a session re-entry that sends a coding agent
//! back to do the work. The forced decisions are the read decision and, for a
//! thread that reports to a parent, the wake decision. Beside them, a turn that
//! leaves its thread waiting with the keep-alive on gets an expected wake.
//!
//! Three re-entries exist. The *ceiling re-entry* asks a session past its
//! *context ceiling* to hand off (ADR 0629). The *approved-plan re-entry* tells
//! a session to build the plan the user approved this turn, when the turn
//! changed no source file. The *proposal hold*'s owns when Lucidos-source work
//! lacks a plan marker or a harden, and what the agent is told. This module
//! owns only that each sends the agent back, and how often.

pub(crate) mod approved_plan;
mod expected_wake_decision;
mod read_decision;
pub(crate) mod wake_decision;

pub use read_decision::{
    decide_read_for_eval, read_decision_chat_judge, AnsweredQuestion, EvalReadDecision, InFlight,
    OpeningSource, PushedNotification, TurnRead,
};
pub(crate) use wake_decision::TurnFinish;
pub use wake_decision::{
    decide_wake_for_eval, wake_decision_chat_judge, EvalWakeDecision, ParentView,
};

use crate::engine::change_ops::{PlanHold, ProposalHold};
use crate::engine::context_ceiling::CeilingCrossing;
use crate::llm::judgment::{system_one_or_default_for, JudgmentProvider, JudgmentSite};

impl crate::engine::LucidosEngine {
    /// The System One row a judgment site runs on, or `None` for its chat
    /// model. Only the wake decision has a default row, and every site shares
    /// one fallback (ADRs 0435, 0472).
    pub(crate) async fn judgment_system_one(
        &self,
        site: JudgmentSite,
        timeout: std::time::Duration,
    ) -> Option<Box<dyn JudgmentProvider>> {
        let served = self
            .current_provider()
            .configured_providers()
            .unwrap_or_default();
        system_one_or_default_for(&self.pool, site, timeout, &served).await
    }
}

/// One rule a turn must meet before it ends, in the order the gate checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnEndRequirement {
    /// The turn says whether its reply is worth reading. A forced decision.
    ReadDecision,
    /// The session grew past its context ceiling: a session re-entry, since
    /// only the agent can write the handoff narrative. It outranks the hold,
    /// whose work the successor session then owes.
    CeilingHandoff(CeilingCrossing),
    /// The user approved building the plan this turn, and the turn ended with
    /// no source change: a session re-entry, since only the agent can build.
    StartApprovedPlan,
    /// The proposal hold kept the turn's work: a session re-entry, since only
    /// the agent can set a plan marker or run `/harden`.
    Reentry(ProposalHold),
}

/// What an ordinary clean turn end knows about its session's context ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CeilingFacts {
    pub(crate) owed: Option<CeilingCrossing>,
    /// Unread input, an open card, or a handoff block defers delivery.
    pub(crate) busy: bool,
}

impl CeilingFacts {
    pub(crate) fn crossing(&self) -> Option<CeilingCrossing> {
        self.owed.filter(|_| !self.busy)
    }
}

/// What a coding-agent session knows at its turn end, for the re-entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SessionFacts {
    /// The branch HEAD, the key each re-entry is bounded by. An unreadable
    /// HEAD keys as the empty string, so it still sends back once.
    pub(crate) head: Option<String>,
    /// Why the proposal hold kept this idle's work, if it did. Never set
    /// while a handoff is pending: the successor session owes it then.
    pub(crate) held: Option<ProposalHold>,
    /// The turn approved the plan's full scope and changed no source file.
    pub(crate) approved_plan_unstarted: bool,
}

/// The state one turn ends in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnEndState {
    /// Whether this turn recorded a read decision.
    pub(crate) read_decided: bool,
    /// Present only for a session whose turn completed. Absent after a Stop,
    /// a failure or an abort, and for a chat turn.
    pub(crate) session: Option<SessionFacts>,
    /// Present only when an ordinary clean turn end crossed the ceiling. Kept
    /// apart from `session`, which an external-repo thread never completes.
    pub(crate) ceiling: Option<CeilingCrossing>,
}

impl TurnEndState {
    /// A coding-agent turn end. Its re-entry facts count only when the turn
    /// completed: never after a user Stop, which means stop.
    pub(crate) fn at_session_idle(
        read_decided: bool,
        completed: bool,
        facts: SessionFacts,
        ceiling: Option<CeilingCrossing>,
    ) -> Self {
        Self {
            read_decided,
            session: completed.then_some(facts),
            ceiling,
        }
    }

    /// The requirements this turn has not met, in the gate's order.
    pub(crate) fn unmet(&self) -> Vec<TurnEndRequirement> {
        let mut unmet = Vec::new();
        if !self.read_decided {
            unmet.push(TurnEndRequirement::ReadDecision);
        }
        if let Some(crossing) = self.ceiling {
            unmet.push(TurnEndRequirement::CeilingHandoff(crossing));
        }
        if self
            .session
            .as_ref()
            .is_some_and(|s| s.approved_plan_unstarted)
        {
            unmet.push(TurnEndRequirement::StartApprovedPlan);
        }
        if let Some(hold) = self.session.as_ref().and_then(|s| s.held) {
            unmet.push(TurnEndRequirement::Reentry(hold));
        }
        unmet
    }
}

/// The re-entry one idle sends, at most one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DueReentry {
    Ceiling(CeilingCrossing),
    ApprovedPlan,
    Hold(ProposalHold),
}

impl DueReentry {
    /// The automatic step this re-entry takes, which an automation pause asks
    /// the owner for first.
    pub(crate) fn step(self) -> crate::engine::permission::Action {
        use crate::engine::permission::Action;
        match self {
            Self::Ceiling(_) => Action::HandOff,
            Self::ApprovedPlan => Action::BuildApprovedPlan,
            Self::Hold(ProposalHold::HardeningMissing) => Action::Harden,
            Self::Hold(_) => Action::SetPlanMarker,
        }
    }
}

/// What each re-entry last sent the agent back for.
///
/// Held for the session's life. The ceiling and the approved plan each send
/// once. Each kind of hold
/// sends at most once per branch HEAD: an agent that ignores one ends its turn
/// rather than looping, and new commits earn a new one. The plan and the
/// harden keep separate keys, so settling the plan still lets the harden ask
/// through at the same HEAD.
#[derive(Debug, Default)]
pub(crate) struct ReentryLedger {
    ceiling_sent: bool,
    approved_plan_sent: bool,
    plan: Option<String>,
    hardening: Option<String>,
}

impl ReentryLedger {
    /// The key a hold's re-entry is bounded by, or `None` for a hold that
    /// sends nobody back.
    fn slot(&mut self, hold: ProposalHold) -> Option<&mut Option<String>> {
        match hold {
            ProposalHold::Plan(PlanHold::PlanOnlyAwaitingApproval)
            | ProposalHold::IntegratedIntoParent
            | ProposalHold::CarriedBySuccessor => None,
            ProposalHold::Plan(_) => Some(&mut self.plan),
            ProposalHold::HardeningMissing => Some(&mut self.hardening),
        }
    }

    /// The re-entry `state` makes due, recorded as sent. An idle sends at
    /// most one, in the gate's order.
    pub(crate) fn take_due(&mut self, state: &TurnEndState) -> Option<DueReentry> {
        let head = state
            .session
            .as_ref()
            .and_then(|s| s.head.clone())
            .unwrap_or_default();
        state
            .unmet()
            .into_iter()
            .find_map(|requirement| match requirement {
                TurnEndRequirement::ReadDecision => None,
                TurnEndRequirement::CeilingHandoff(crossing) => (!self.ceiling_sent).then(|| {
                    self.ceiling_sent = true;
                    DueReentry::Ceiling(crossing)
                }),
                TurnEndRequirement::StartApprovedPlan => (!self.approved_plan_sent).then(|| {
                    self.approved_plan_sent = true;
                    DueReentry::ApprovedPlan
                }),
                TurnEndRequirement::Reentry(hold) => {
                    let slot = self.slot(hold)?;
                    (slot.as_deref() != Some(head.as_str())).then(|| {
                        *slot = Some(head.clone());
                        DueReentry::Hold(hold)
                    })
                }
            })
    }

    /// Un-record `due`, which an automation pause asked the owner about
    /// instead of sending. The next idle asks again while it is still due.
    pub(crate) fn forget(&mut self, due: DueReentry) {
        match due {
            DueReentry::Ceiling(_) => self.ceiling_sent = false,
            DueReentry::ApprovedPlan => self.approved_plan_sent = false,
            DueReentry::Hold(hold) => {
                if let Some(slot) = self.slot(hold) {
                    *slot = None;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
