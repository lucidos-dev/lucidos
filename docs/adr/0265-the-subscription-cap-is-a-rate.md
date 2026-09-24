# 0265: The event-wait loop cap counts over a rolling hour, not since the last human message

- **Status**: Accepted
- **Date**: 2026-09-24

## Context

A thread may subscribe only so often with no message from the user between. The
cap exists to stop a loop: a thread waiting on an event its own re-entry emits,
two threads ping-ponging, a model simply stuck.

It used to count every `EventWaitStarted` since the last human message, with no
time bound. The waits the engine arms for `lucidos background-task run` count
too. So a thread with no human in it got ten waits for its whole life. A nightly
e2e step armed fourteen over six hours. Each lasted minutes to an hour and ended
in a real task completion, and still the engine refused the next one.

## Decision

The cap counts waits started since the last human message **and** inside the
last hour (`RECENT_SUBSCRIPTION_WINDOW_SECS`). The limit stays at ten
(`MAX_RECENT_SUBSCRIPTIONS`). A human message still resets the count, and an
agent or engine message still does not.

## Rationale

Every loop the cap names is serial, so serial versus concurrent cannot tell a
loop from progress. Speed can. A loop re-arms within seconds. A long workflow
re-arms every twenty to sixty minutes. A lifetime count treats both the same; a
rate refuses the first within minutes and never touches the second.

The window is resolved on the database clock, per ADR 0053.

## Consequences

- A thread with no human in it can run a serial workflow of any length, provided
  it arms fewer than ten waits an hour.
- A hot loop is still refused, now by rate rather than by total.
- A model that keeps re-arming a long wait for something that never comes is no
  longer stopped. It costs at most one turn per timeout period, visibly, until
  someone looks. We accept that: it is slow and cheap, and the fast loops are the
  expensive ones.
- The concurrency bound is unchanged: `MAX_LIVE_WAITS_PER_THREAD` still caps
  how many waits a thread holds at once.

## Alternatives considered

- **Remove the cap for serial waits.** Every loop it guards against is serial,
  so this removes the loop guard entirely. Only the live-wait limit would
  remain, and a ping-pong loop would then run until someone stopped it.
- **Stop counting engine-armed background-task waits.** It would have passed the
  nightly thread, but it keeps the lifetime count for every other wait. A
  workflow that waits on the e2e lock ten times would still starve. It also
  reopens the loop that counting those waits was added to close
  (`docs/plans/2026-08-10-the-agent-checks-state-instead-of-assuming-it.md`).
- **Raise the number.** It moves the wall without changing what it measures. A
  long enough unattended thread still hits it, and a loop runs longer first.
- **A rate plus a lifetime backstop.** A second number to tune, for the slow and
  cheap failure above. Rejected for now as not worth its weight.
