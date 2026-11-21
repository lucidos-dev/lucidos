//! Persisted reasons Lucidos held an Apply or a proposal.

/// Why the floor holds a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanHold {
    /// No marker at all.
    Missing,
    /// The plan marker or its committed revision could not be read.
    Unreadable,
    /// A plan is recorded but the user has not approved it.
    AwaitingApproval,
    /// A plan awaits approval and the branch lands plan files alone. The
    /// decision is the user's, so the agent is not sent back to ask again.
    PlanOnlyAwaitingApproval,
    /// Approval for later covers only a nonempty landing of plan files.
    PlanOnlyCode,
    /// A bounded security fix whose work reaches past the files it named.
    OutsideBound,
}

impl PlanHold {
    /// What a refused Apply tells the user.
    pub fn apply_refusal(self) -> &'static str {
        match self {
            PlanHold::PlanOnlyCode => {
                "This plan was approved for later, so only plan files may land. \
                 Remove the code changes or ask for implementation approval."
            }
            PlanHold::AwaitingApproval | PlanHold::PlanOnlyAwaitingApproval => {
                "The implementation plan on this branch is awaiting approval. The user must \
                 approve the plan, after which the coding agent runs `lucidos planned approve` \
                 to unblock implementation. Approve the plan, then apply."
            }
            PlanHold::Unreadable => {
                "Lucidos could not verify the implementation plan. Ask the agent to check the branch before trying Apply again."
            }
            PlanHold::Missing => {
                "No implementation-plan marker on this branch. Before applying, the coding \
                 agent must run the `implementation-plan` skill (records a plan for the user to \
                 approve) or `lucidos planned mark --simple \"<reason>\"` (acknowledges a local \
                 fix). Re-run the session to set the marker, then apply."
            }
            PlanHold::OutsideBound => {
                "This branch carries a bounded security-fix marker and changes files outside the \
                 bound it named. Drop the extra files, re-run `lucidos planned mark --security-fix` \
                 with the full list, or write a plan and have the user approve it."
            }
        }
    }

    /// What the engine tells a live agent whose turn-end proposal it held.
    pub fn agent_nudge(self) -> &'static str {
        match self {
            PlanHold::PlanOnlyCode => {
                "Lucidos held this change because approval for later covers plan files alone. \
                 Remove the code changes or mark the plan again and ask for implementation approval."
            }
            PlanHold::AwaitingApproval | PlanHold::PlanOnlyAwaitingApproval => {
                "Lucidos did not propose this branch's change. Its implementation plan still \
                 awaits the user's approval, so Apply would refuse it. Ask for approval with your \
                 question tool, `Approve` first. Once the user approves, run \
                 `lucidos planned approve` and end your turn: Lucidos then proposes the change. \
                 If nobody can be asked (an unattended run), do not ask: end your turn with the \
                 `BLOCKED ON PLAN DECISION:` line the `implementation-plan` skill describes."
            }
            PlanHold::Unreadable => {
                "Lucidos could not verify the implementation plan. Check the branch and committed \
                 plan file, then retry `lucidos planned state`. Once it reads SATISFIED, end your \
                 turn to propose the change."
            }
            PlanHold::Missing => {
                "Lucidos did not propose this branch's change. The branch has commits but no \
                 implementation-plan marker, so Apply would refuse it. A marker is consumed when a \
                 change applies, so work after an Apply needs a new one. If the work is complex, \
                 run the `implementation-plan` skill and ask for approval with your question tool. \
                 If it is a genuinely local fix, run `lucidos planned mark --simple \"<one-line \
                 reason>\"`. Then end your turn: Lucidos then proposes the change. If nobody can \
                 be asked (an unattended run), follow that skill's unattended lanes instead."
            }
            PlanHold::OutsideBound => {
                "Lucidos did not propose this branch's change. Its bounded security-fix marker \
                 names fewer files than the branch changes, so Apply would refuse it. Drop the \
                 extra files, or re-run `lucidos planned mark --security-fix` with the full list. \
                 If the fix is wider than the lane allows, take the blocked lane in the \
                 `implementation-plan` skill. Then end your turn."
            }
        }
    }
}

/// Why a turn end's work may not become a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalHold {
    /// The plan floor holds the branch.
    Plan(PlanHold),
    /// The work never ran `/harden`, so Apply would harden it first.
    HardeningMissing,
    /// An integrated sub-thread's hardened work. It hands its branch up to
    /// its parent, whose agent merges it, so it never becomes a change.
    IntegratedIntoParent,
    /// The thread handed off to a legacy successor thread. Its branch work rides on
    /// the successor's branch, so this thread never proposes it.
    CarriedBySuccessor,
}

impl ProposalHold {
    /// What a refused Apply tells the user.
    pub fn apply_refusal(self) -> &'static str {
        match self {
            ProposalHold::Plan(hold) => hold.apply_refusal(),
            ProposalHold::HardeningMissing => {
                "This branch's work has not been hardened, so Lucidos did not propose it. \
                 Press Harden on the thread: the hardening run proposes the change when it \
                 finishes."
            }
            ProposalHold::IntegratedIntoParent => INTEGRATED_HAS_NO_APPLY,
            ProposalHold::CarriedBySuccessor => CARRIED_HAS_NO_APPLY,
        }
    }

    /// What the engine tells a live agent whose turn-end proposal it held.
    pub fn agent_nudge(self) -> &'static str {
        match self {
            ProposalHold::Plan(hold) => hold.agent_nudge(),
            ProposalHold::HardeningMissing => {
                "Lucidos did not propose this branch's change. The branch never ran `/harden`, \
                 so Apply would have to harden it first. Run `/harden` now over the whole \
                 batch of commits, then end your turn: Lucidos then proposes the change."
            }
            ProposalHold::IntegratedIntoParent => INTEGRATED_HAS_NO_APPLY,
            ProposalHold::CarriedBySuccessor => CARRIED_HAS_NO_APPLY,
        }
    }
}

/// Why an integrated sub-thread's work has no Apply.
const INTEGRATED_HAS_NO_APPLY: &str =
    "This thread is integrated into its parent. Lucidos never proposes its work: the parent's \
     agent merges its branch, and the parent's change carries it.";

/// Why a thread that handed off has no Apply.
const CARRIED_HAS_NO_APPLY: &str =
    "This thread continued in a legacy successor thread, whose branch carries its work. Lucidos \
     never proposes it here: the successor's change carries it to Apply.";
