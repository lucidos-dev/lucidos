//! End-of-turn requirements and coding-agent re-entry bookkeeping.

use crate::engine::change_ops::{PlanHold, ProposalHold};
use crate::engine::context_ceiling::CeilingCrossing;

/// One rule a turn must meet before it ends, in the order the gate checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEndRequirement {
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
pub struct CeilingFacts {
    pub owed: Option<CeilingCrossing>,
    /// Unread input, an open card, or a handoff block defers delivery.
    pub busy: bool,
}

impl CeilingFacts {
    pub fn crossing(&self) -> Option<CeilingCrossing> {
        self.owed.filter(|_| !self.busy)
    }
}

/// What a coding-agent session knows at its turn end, for the re-entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionFacts {
    /// The branch HEAD, the key each re-entry is bounded by. An unreadable
    /// HEAD keys as the empty string, so it still sends back once.
    pub head: Option<String>,
    /// Why the proposal hold kept this idle's work, if it did. Never set
    /// while a handoff is pending: the successor session owes it then.
    pub held: Option<ProposalHold>,
    /// The turn approved the plan's full scope and changed no source file.
    pub approved_plan_unstarted: bool,
}

/// The state one turn ends in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnEndState {
    /// Whether this turn recorded a read decision.
    pub read_decided: bool,
    /// Present only for a session whose turn completed. Absent after a Stop,
    /// a failure or an abort, and for a chat turn.
    pub session: Option<SessionFacts>,
    /// Present only when an ordinary clean turn end crossed the ceiling. Kept
    /// apart from `session`, which an external-repo thread never completes.
    pub ceiling: Option<CeilingCrossing>,
}

impl TurnEndState {
    /// A coding-agent turn end. Its re-entry facts count only when the turn
    /// completed: never after a user Stop, which means stop.
    pub fn at_session_idle(
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
    pub fn unmet(&self) -> Vec<TurnEndRequirement> {
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
pub enum DueReentry {
    Ceiling(CeilingCrossing),
    ApprovedPlan,
    Hold(ProposalHold),
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
pub struct ReentryLedger {
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

    /// Peek without consuming the hold's once-per-HEAD allowance.
    pub fn would_send(&self, hold: ProposalHold, head: &str) -> bool {
        let slot = match hold {
            ProposalHold::Plan(PlanHold::PlanOnlyAwaitingApproval)
            | ProposalHold::IntegratedIntoParent
            | ProposalHold::CarriedBySuccessor => return false,
            ProposalHold::Plan(_) => &self.plan,
            ProposalHold::HardeningMissing => &self.hardening,
        };
        slot.as_deref() != Some(head)
    }

    /// Decide which hold reasons the proposal may omit before the idle.
    /// `withheld` is true when no turn may start: then no re-entry is sent.
    pub fn hold_reentry(
        &self,
        head: &str,
        withheld: bool,
    ) -> crate::engine::change_ops::HoldReentry {
        crate::engine::change_ops::HoldReentry {
            plan: !withheld && self.would_send(ProposalHold::Plan(PlanHold::Missing), head),
            hardening: !withheld && self.would_send(ProposalHold::HardeningMissing, head),
        }
    }

    /// The re-entry `state` makes due, recorded as sent. An idle sends at
    /// most one, in the gate's order.
    pub fn take_due(&mut self, state: &TurnEndState) -> Option<DueReentry> {
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
                TurnEndRequirement::Reentry(hold) => self.would_send(hold, &head).then(|| {
                    *self.slot(hold).expect("a due hold has a slot") = Some(head.clone());
                    DueReentry::Hold(hold)
                }),
            })
    }

    /// Un-record `due`, which a held thread or a failed send did not deliver.
    /// The next idle sends it again while it is still due.
    pub fn forget(&mut self, due: DueReentry) {
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
