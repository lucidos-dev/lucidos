# 0233: An apply takes the gate wherever it is asked from: the LLM tool asks what the button asks

- **Status**: Accepted
- **Date**: 2026-09-20

## Context

ADR 0106 decided that a thread which will wake itself cannot have its change
resolved. It rejected a frontend-only hide, because that leaves `curl` able to
apply and splits one predicate into two that drift.

The rule landed in two places. `guard_change_action` gates
`POST /api/v1/changes/:id/apply`, and `drop_unsettled_thread_changes` gates the
bulk paths. The `changes` LLM tool got neither. It called
`LucidosEngine::apply_change` straight after the ADR 0168 authority check.

A chat agent then applied a change on a coding-agent thread that was mid-turn.
The row read `status='running'`, with a tool result 137 ms before the apply and
`CodingAgentIdled` 90 ms after it. The merge took the Tier 1 in-place path,
which resets the live worktree to `main` once the fast-forward lands. The
session stopped, and the user resumed it by hand.

So the split ADR 0106 refused to create happened anyway, through a surface that
was never enrolled.

## Decision

An apply somebody REQUESTS takes the per-change gate, wherever they request it
from. `api::changes::change_action_refusal` is the single definition. The HTTP
handler renders it as a 409. The `changes` LLM tool renders it as a tool error.
The reason is typed, so only the wording is per surface.

The gate stays OUT of `LucidosEngine::apply_change`. Engine-internal callers
apply while the thread legitimately reads running, and a test enrolls each one.

A refusal names `apply_when_settled` only where a standing apply would really
wait: a *working* thread, never a parked one.

## Rationale

**A person tapping Apply and an agent calling the tool are the same act.** Both
merge somebody else's branch on request. Nothing about the caller changes
whether the branch is safe to merge, so nothing about the caller should change
the answer.

**The gate cannot move into the engine entry point.** Four internal callers
apply while the thread reads running, on purpose: the Apply All driver,
`apply_now`, the post-hardening auto-apply, and the standing-apply resolver. A
blanket gate there would refuse a coding agent its own Apply Now, which is the
normal end of a turn.

**The reason is typed because the two surfaces need different words.** A person
reads the 409 body. A model reads the tool error and decides what to do next.
Sharing one string would make one of them worse.

**`apply_when_settled` is named only where waiting helps.** `standing_verdict`
waits through `running` and `paused`, and drops on a parked thread at rest. So
the refusal reports working, parked and neither apart. Only the working one
names the standing apply.

**A census test, because this defect was one unenrolled caller.** Every file
calling `apply_change` is listed with its reason. The next surface then has to
answer the question rather than inherit silence.

## Consequences

**Kept.** The HTTP surface behaves as before. Same two 409 refusals, and the
same fall-through for an unknown id, a threadless change and a resolved one.
Apply All and `apply_as_they_settle` keep their own filter, which drops
unsettled members before the batch starts.

**Given up.** An agent can no longer land a change early on a thread it knows is
about to park harmlessly. The way through is `apply_when_settled`, or Stop
waiting on the thread, which ADR 0106 already made the human escape.

**One message changed.** The empty-change 409 used an em dash between "no file
changes left" and "discard it instead". Rewriting the line put it under the
em-dash rule, which binds every line a change touches. It now reads "This change
has no file changes left. Discard it instead."

**ADR 0106's delivery-to-wake window is unchanged.** The gate reads the same
facts, so the same sub-second gap survives, for the reasons that ADR accepted.

## Alternatives considered

**Gate inside `LucidosEngine::apply_change`.** One place, impossible to forget.
Rejected: the four internal callers run mid-turn by design. The gate would need
a "who asked" parameter, and every call site would name it anyway. The refusal
would also have to travel as an `Err`, which the HTTP handler renders as 400,
silently downgrading today's 409.

**Auto-arm a standing apply instead of refusing.** The agent asked to apply, and
arming is what the user wants next. Rejected: an arm and a merge return
different shapes, and a tool that quietly performs a different verb hides the
refusal from the user. The agent can arm in one more call, having read why.

**Give the tool its own copy of the rule.** Smallest diff. Rejected for exactly
the reason ADR 0106 gave. Two copies of one predicate drift, and the drift stays
invisible until something merges that should not have.

**Report the thread state as one boolean, settled or not.** It was the first
draft, and both reviewers caught it. `unsettled` covers working AND parked,
while a standing apply waits through working alone. So three of the four
refused states were told to arm a control that drops on its first look. That
costs the user a `StandingApplyDropped` report and applies nothing. The
`Change` row already splits the two, as `thread_unsettled` and
`thread_working`, for this exact reason.

**Refuse the Tier 1 in-place merge while the session is mid-turn.** It is the
step that actually destroyed work, by resetting the worktree. Rejected: that
path cannot tell a requested apply from the session's own Apply Now, which is
mid-turn every time. Guarding it would break the normal case to catch one the
surface gate already stops.
