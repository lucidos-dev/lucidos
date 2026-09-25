# 0266: A standing apply waits through an event wait, and a wait's resolution never fires it: amends 0168

- **Status**: Accepted
- **Date**: 2026-09-24
- **Amends**: [0168: A thread acts in its own subtree](0168-a-thread-acts-in-its-own-subtree.md)

## Context

ADR 0168 gave the owner a *standing apply*: apply this change once its thread
settles. It promised that "a thread that parks or fails drops its standing
apply and reports it, so nothing waits forever".

"Parks" covered two states: a question card, and a live *event wait*. So a
thread that armed, then parked on the e2e lock, dropped its arm with "The
thread parked on an event wait." The owner then had to come back, wait for the
wake, and arm again. That is the chore the standing apply exists to remove.

ADR 0106 is why the arm cannot simply fire there. A thread on a live wait is
still producing its change. It wakes on the delivery and commits on to the same
branch.

## Decision

A standing apply waits through a live event wait, as it waits through a running
turn. The resolver does not re-take its verdict on `EventWaitDelivered` or
`EventWaitExpired`. A question card still drops the arm.

Every reader of "what a standing apply waits through" follows. The action
selector offers `ApplyWhenSettled` on a thread watching an event. The Apply All
sweep arms one. The Changes panel offers it. The single-change refusal names it
as the waitable reason. That concept is now the *settling* thread: running,
paused, or watching an event.

## Rationale

**A wait ends by itself, and a question card does not.** A delivery, an expiry
(at most 24 hours), or **Stop waiting** ends every wait. So an arm that waits
through one still always ends, which is the promise ADR 0168 cared about.
Grouping the wait with the question card kept the promise by a stronger rule
than it needed.

**Waiting is safe, firing on the resolution is not.** A delivery clears the
live-wait count while the status is still `idle`. ADR 0106 names that gap and
accepts it for the manual Apply button, because a person rarely clicks inside
it. The resolver is different: it re-takes the verdict on every thread event of
an armed thread. So it would see the resolution event inside the gap every
time, and merge the branch as the agent resumes.

**The skip closes the gap without state.** `emit_resolution` writes the
resolution, then at once a `UserPromptInjected` anchor. That anchor sets the
status to `running`, before any Thread Queue wait. So the next event the
resolver sees after a skipped resolution already reads the true state. A cancel
re-enters nothing, so it is not skipped, and the arm resolves on it.

## Consequences

**Kept.** ADR 0106's gate on the manual Apply and Discard. The arm waits through
the wait and never applies inside it.

**Kept.** A question card drops the arm with a report.

**A lost wake still resolves.** If the anchor fails, or a restart loses the
wake, boot recovery re-takes every verdict. The thread reads idle with no wait,
and the arm fires or drops.

**A narrower window survives.** The resolver reads the row when it handles an
event, not when the event was emitted. So a resolver still working through an
older event, or the lag-recovery sweep, can read the row between the resolution
and its anchor. The bus commits each before broadcasting it, so that window is
the gap between two back-to-back emits. That is the exposure ADR 0106 accepted
for the button, and far rarer here.

**Renamed.** "Working" meant running or paused. `working_thread_ids`,
`Change.thread_working` and `ChangeActionRefusal::ThreadWorking` become
`settling_thread_ids`, `Change.thread_settling` and `ThreadSettling`.
`ThreadParked` now means a question card only.

## Alternatives considered

**Keep dropping, and tell the owner to re-arm.** The status quo. Rejected: it
turns the standing apply back into a button the owner has to watch for.

**Wait, and let the resolution re-take the verdict.** The smallest change.
Rejected: the resolution event lands inside the delivery-to-wake gap by
construction, so the arm would merge the branch as the agent resumes.

**A durable "wake pending" flag.** Closes the gap for every reader. Rejected for
the reason ADR 0106 gives: a wake lost to a restart strands the flag TRUE, and
withholds the apply for good.

**Keep the arm, but offer it only on a running thread.** Narrower: the owner
could not arm a thread already waiting. Rejected: the verdict and its readers
would disagree, which is the drift the pairing tests exist to stop.
