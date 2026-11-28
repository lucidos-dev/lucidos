import json
import sys


REFUSALS = {
    "ChangeApplyRequested": {
        "plan_only_code": (
            "This plan was approved for later, so only plan files may land. Remove the "
            "code changes or ask for implementation approval."
        ),
        "awaiting_approval": (
            "The implementation plan on this branch is awaiting approval. The user must "
            "approve the plan, after which the coding agent runs `lucidos planned "
            "approve` to unblock implementation. Approve the plan, then apply."
        ),
        "changed": (
            "The plan changed after approval. Ask the agent to restore the approved plan "
            "text and commit it, or submit the revised plan for approval."
        ),
        "unreadable": (
            "Lucidos could not verify the implementation plan. Ask the agent to check the "
            "branch before trying Apply again."
        ),
        "missing": (
            "No implementation-plan marker on this branch. Before applying, the coding "
            "agent must run the `implementation-plan` skill (records a plan for the user "
            "to approve) or `lucidos planned mark --simple \"<reason>\"` (acknowledges a "
            "local fix). Re-run the session to set the marker, then apply."
        ),
        "outside_bound": (
            "This branch carries a bounded security-fix marker and changes files outside "
            "the bound it named. Drop the extra files, re-run `lucidos planned mark "
            "--security-fix` with the full list, or write a plan and have the user "
            "approve it."
        ),
    },
    "ChangeProposed": {
        "plan_only_code": (
            "Lucidos held this change because approval for later covers plan files alone. "
            "Remove the code changes or mark the plan again and ask for implementation "
            "approval."
        ),
        "awaiting_approval": (
            "Lucidos did not propose this branch's change. Its implementation plan still "
            "awaits the user's approval, so Apply would refuse it. Ask for approval with "
            "your question tool, `Approve` first. Once the user approves, run `lucidos "
            "planned approve` and end your turn: Lucidos then proposes the change. If "
            "nobody can be asked (an unattended run), do not ask: end your turn with the "
            "`BLOCKED ON PLAN DECISION:` line the `implementation-plan` skill describes."
        ),
        "changed": (
            "Lucidos held this change because the committed plan changed after approval. "
            "Restore the approved plan text and commit it to reuse the existing approval. "
            "If the plan needs to change, mark the revised plan and ask for approval. "
            "Once the plan gate clears, end your turn to propose the change."
        ),
        "unreadable": (
            "Lucidos could not verify the implementation plan. Check the branch and "
            "committed plan file, then retry `lucidos planned state`. Once it reads "
            "SATISFIED, end your turn to propose the change."
        ),
        "missing": (
            "Lucidos did not propose this branch's change. The branch has commits but no "
            "implementation-plan marker, so Apply would refuse it. A marker is consumed "
            "when a change applies, so work after an Apply needs a new one. If the work "
            "is complex, run the `implementation-plan` skill and ask for approval with "
            "your question tool. If it is a genuinely local fix, run `lucidos planned "
            "mark --simple \"<one-line reason>\"`. Then end your turn: Lucidos then "
            "proposes the change. If nobody can be asked (an unattended run), follow that "
            "skill's unattended lanes instead."
        ),
        "outside_bound": (
            "Lucidos did not propose this branch's change. Its bounded security-fix "
            "marker names fewer files than the branch changes, so Apply would refuse it. "
            "Drop the extra files, or re-run `lucidos planned mark --security-fix` with "
            "the full list. If the fix is wider than the lane allows, take the blocked "
            "lane in the `implementation-plan` skill. Then end your turn."
        ),
    },
}


def verdict(context):
    if context["coding_agent_kind"] != "lucidos":
        return None

    marker = context["plan_marker"]
    state = marker["state"]
    if state != "present":
        return state

    kind = marker["kind"]
    files = context["landing_files"]
    if kind == "proposed":
        return "awaiting_approval"
    if kind == "plan_only":
        if files and all(path.startswith("docs/plans/") for path in files):
            return None
        return "plan_only_code"
    if kind == "bounded_security_fix":
        if files is None:
            return "unreadable"
        bound = set(marker["file_bound"] or ())
        if any(path not in bound and not path.startswith("docs/plans/") for path in files):
            return "outside_bound"
        return None
    if kind in ("planned", "acknowledged_simple"):
        return None
    raise ValueError(f"Unknown plan marker kind: {kind}")


def main():
    request = json.load(sys.stdin)
    hold = verdict(request["payload"]["change_context"])
    if hold is None:
        print(json.dumps({"answer": "allow"}))
    else:
        reason_event = {
            "ChangeProposalChecked": "ChangeProposed",
            "ChangeApplyChecked": "ChangeApplyRequested",
        }.get(request["event_type"], request["event_type"])
        reason = REFUSALS[reason_event][hold]
        print(json.dumps({"answer": "refuse", "reason": reason}))


if __name__ == "__main__":
    main()
