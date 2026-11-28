#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanHold {
    Missing,
    Changed,
    Unreadable,
    AwaitingApproval,
    PlanOnlyAwaitingApproval,
    PlanOnlyCode,
    OutsideBound,
}

impl PlanHold {
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
            PlanHold::Changed => {
                "The plan changed after approval. Ask the agent to restore the approved plan text \
                 and commit it, or submit the revised plan for approval."
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
            PlanHold::Changed => {
                "Lucidos held this change because the committed plan changed after approval. \
                 Restore the approved plan text and commit it to reuse the existing approval. \
                 If the plan needs to change, mark the revised plan and ask for approval. \
                 Once the plan gate clears, end your turn to propose the change."
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalHold {
    Plan(PlanHold),
    HardeningMissing,
    IntegratedIntoParent,
    CarriedBySuccessor,
}

impl ProposalHold {
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

const INTEGRATED_HAS_NO_APPLY: &str =
    "This thread is integrated into its parent. Lucidos never proposes its work: the parent's \
     agent merges its branch, and the parent's change carries it.";

const CARRIED_HAS_NO_APPLY: &str =
    "This thread continued in a legacy successor thread, whose branch carries its work. Lucidos \
     never proposes it here: the successor's change carries it to Apply.";
