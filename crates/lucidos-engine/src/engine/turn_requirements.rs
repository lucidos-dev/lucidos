use crate::engine::change_ops::{PlanHold, ProposalHold};
use crate::engine::context_ceiling::CeilingCrossing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEndRequirement {
    ReadDecision,
    /// INVARIANT: The session grew past its context ceiling: a session re-entry, since
    /// only the agent can write the handoff narrative. It outranks the hold,
    /// whose work the successor session then owes.
    CeilingHandoff(CeilingCrossing),
    StartApprovedPlan,
    Reentry(ProposalHold),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CeilingFacts {
    pub owed: Option<CeilingCrossing>,
    pub busy: bool,
}

impl CeilingFacts {
    pub fn crossing(&self) -> Option<CeilingCrossing> {
        self.owed.filter(|_| !self.busy)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionFacts {
    pub head: Option<String>,
    /// INVARIANT: Why the proposal hold kept this idle's work, if it did. Never set
    /// while a handoff is pending: the successor session owes it then.
    pub held: Option<ProposalHold>,
    pub approved_plan_unstarted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnEndState {
    pub read_decided: bool,
    pub session: Option<SessionFacts>,
    pub ceiling: Option<CeilingCrossing>,
}

impl TurnEndState {
    /// INVARIANT: A coding-agent turn end. Its re-entry facts count only when the turn
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueReentry {
    Ceiling(CeilingCrossing),
    ApprovedPlan,
    Hold(ProposalHold),
}

/// INVARIANT: What each re-entry last sent the agent back for.
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
    fn slot(&mut self, hold: ProposalHold) -> Option<&mut Option<String>> {
        match hold {
            ProposalHold::Plan(PlanHold::PlanOnlyAwaitingApproval)
            | ProposalHold::IntegratedIntoParent
            | ProposalHold::CarriedBySuccessor => None,
            ProposalHold::Plan(_) => Some(&mut self.plan),
            ProposalHold::HardeningMissing => Some(&mut self.hardening),
        }
    }

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

    pub fn hold_reentry(
        &self,
        head: &str,
        withheld: bool,
        integrated: bool,
    ) -> crate::engine::change_ops::HoldReentry {
        crate::engine::change_ops::HoldReentry {
            plan: !withheld && self.would_send(ProposalHold::Plan(PlanHold::Missing), head),
            hardening: !withheld
                && !integrated
                && self.would_send(ProposalHold::HardeningMissing, head),
        }
    }

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
