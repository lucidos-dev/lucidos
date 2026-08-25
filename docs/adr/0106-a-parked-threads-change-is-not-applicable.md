# 0106: A parked thread's change cannot be applied: waiting gates change resolution

- **Status**: Accepted
- **Date**: 2026-08-22

## Context

ADR 0049 made a thread holding an *event wait* plain `idle`, and recorded that
such a thread is "not blocking, not attention-needing, and archivable". It also
opened `await_event` to coding agents, which had been excluded while the
attached shape existed.

That combination produced a state ADR 0049 did not weigh: a coding-agent thread
holding **both** a proposed change and a live subscription. It is the ordinary
shape of the `e2e-lock-wait` skill. Propose a change, run e2e, park on the lock
rather than poll, resume on delivery.

A thread observed in that state had `status='idle'`,
`coding_agent_proposed=true` and `live_event_wait_count=1` for three minutes.
Two surfaces read it wrong:

- `resolveVisualStatus` checked `codingAgentProposed` before the waiting causes,
  so the dot said **Changes to review** on a thread that was mid-verification.
- `available_thread_actions` computed `live = Running || WaitingForUserAnswer`.
  A wait is neither, so it offered **Apply** and **Discard**.

Applying there merges a branch the agent is still running e2e against. The agent
then wakes on its delivery and commits on to a branch that is already merged.

## Decision

A thread that will wake itself cannot have its change resolved. Both waiting
causes gate it, a live event wait and an active sub-thread, and both withhold
`Apply` and `Discard`. What is left is exactly what a `Running` thread offers.
`resolveVisualStatus` paints the Waiting dot ahead of the changes dot, so the
dot and the gate say the same thing.

No new `ThreadStatus`. The status column stays `idle`, and the two facts reach
`available_thread_actions` as inputs (`live_event_wait_count > 0`,
`active_children_count > 0`).

## Rationale

**The change is not final, so resolving it races the work still to come.** A
delivery resumes the same thread, in the same worktree, on the same branch. That
is equally true of a sub-thread: its completion wakes the **parent** through the
ADR 0011 fan-in, and the parent may commit again. So both causes gate, which is
also what keeps the single Waiting dot honest.

**This is a gap in ADR 0049 rather than a reversal of it.** That ADR reasoned
about whether a subscription holds the *turn*, and it does not: the turn still
ends with a real terminator. What it did not consider is whether an *artifact*
the thread produced can be resolved mid-flight.

**It is also not the alternative ADR 0049 rejected.** That was "keep
`waiting_for_event` as a non-blocking, informational status", turned down
because "a status that never affects behaviour is a second, weaker surface".
This adds no status and does affect behaviour, which is the inverse.

**The derived axis is the right home.** `VisualStatus` already carries `changes`
and `question`, neither of which is a `ThreadStatus`. Keeping the fact there
avoids a migration, a status-enum contract widening, and re-opening the
structural argument that killed the attached wait.

**Withholding Discard as well follows from the framing.** `Running` withholds
both. Leaving Discard alone would leave a way to destroy work the agent is still
producing.

## Consequences

**Kept.** Everything ADR 0049 promised about a thread with nothing to resolve. A
parked thread stays non-blocking, non-attention-needing and archivable, and
archiving still cancels every live wait rather than stranding one. Archive was
never offered alongside a pending change, so its availability is unchanged in
both directions.

**The way out is Stop waiting.** A wait can run 24 hours, so the escape matters:
the waiting indicator's per-subscription button cancels the subscription, which
drops the count and restores both buttons at once. For a sub-thread the exit is
on the child.

**Given up.** Applying a change from a thread that is still watching for
something. That was the defect, but it was also a way to land work early when
you knew the pending wait was irrelevant. Stop waiting is now the explicit step.

**A narrow window survives, between the delivery and the wake.** Resolving a
wait clears the parking fact, and the thread only reaches `running` when the
wake's own event lands. `EventWaitDelivered` is `no_change` for status, and a
child completion is the same shape. So both facts read false for that gap and
Apply reappears. It widens if the wake waits for a Thread Queue slot.

Accepted rather than closed, because the fix is worse than the residual. Nothing
durable records "a wake is on its way", so closing it means a new flag written
on delivery and cleared when the turn starts. ADR 0011 records that an
un-consumed wake is LOST on restart, which would strand that flag TRUE and
withhold Apply on the thread for good. A permanently unresolvable change beats a
sub-second race in severity. The gate still cuts the exposure from the wait's
whole life, up to 24 hours, down to that gap.

**A parked thread keeps its attention badge while offering no action.**
`is_blocking` and `is_attention_needing` are deliberately untouched, so the
badge is premature rather than wrong: the thread genuinely will need the user
once it wakes, and it stays visible instead of leaving the attention set for up
to 24 hours.

**Contract.** `available_thread_actions` gained two parameters, so the generated
TypeScript and the cross-validation fixture were regenerated. The fixture's
cross product grew by two boolean dimensions.

## Alternatives considered

**Restore `ThreadStatus::WaitingForEvent` as a real status.** The user's word for
the state was "state", and a status is the obvious home. Rejected: it needs a
migration, a status-enum contract widening and new SQL mirrors, and it re-opens
the argument ADR 0049 settled. The gate needs a *predicate input*, not a status,
and the derived axis already hosts two non-status values.

**Gate on live event waits only.** Narrower, and literally what was asked. It was
offered as the alternative and turned down. The Waiting dot merges both causes,
so gating one would make the dot and the Apply button disagree. A parent with a
running child is about to be woken by the same mechanism anyway.

**Withhold Apply but keep Discard.** Discard is not destructive to `main`, so it
looks safe to leave. Rejected: it destroys the agent's in-flight work rather
than the user's, and "akin to a running state" means the pair moves together.

**Also drop the parked case from `is_blocking`.** It would clear the
contradiction of an attention badge with no available action. Rejected as
actively unsafe: `is_blocking` is what stops an ancestor cascade-archiving this
thread, and that cascade emits `ChangeApplied` for each pending change. Relaxing
it reaches the very outcome this ADR prevents, through another door.

**Hide the Apply button in the frontend only.** Smallest diff. Rejected because
`available_thread_actions` is the server-side guard for the Apply route as well
as the UI's source: a frontend-only hide leaves `curl` able to apply, and splits
one predicate into two that will drift.
