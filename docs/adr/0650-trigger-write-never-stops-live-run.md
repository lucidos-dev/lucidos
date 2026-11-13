# 0650: A trigger write, whether save, pause or delete, re-arms or disarms the schedule only and never stops a live run

- **Status**: Accepted
- **Date**: 2026-11-10

## Context

Each trigger with a cron schedule has a runner. A trigger write (save, pause, delete) stops the old runner and starts a new one. The on-time fire handed the Thread Queue a cancel token tied to that runner. So stopping the runner canceled a fire that was already executing. A user who fixed a typo in a trigger lost a run that was halfway done.

## Decision

A trigger write re-arms or disarms the schedule only. It never stops a run that is already executing. The user stops a live run from its thread. The new configuration applies to the next fire.

## Rationale

A schedule edit and a run stop are two different user intents. Only the thread has a Stop button, so only that button should stop the run. The old coupling was an accident of plumbing: the runner owned the token, so the runner's death looked like a cancel.

The root cause was the token itself. A runner stopped while its fire is still queued has not started anything, so dropping that entry is still right. Once the run is admitted, the runner has no claim on it.

## Consequences

- The on-time fire passes no cancel token to the queue. We deleted the `cancel` parameter of `ThreadQueue::submit` and the fields and forwarders that carried it, down to the chat process.
- A runner stopped while its fire is still queued drops that entry. That entry never started.
- A run in flight can now outlive its trigger, even a deleted one. It ends on its own, or the user stops it from its thread.
- Other stop paths (a user Stop, engine shutdown) are unchanged.

## Alternatives considered

- **A new `CancelCause` for "trigger edited", kept on the token.** It would have named the stop but kept it. The user never asked for that behavior, and a cause would have made it look designed.
- **Cancel only when the trigger is deleted.** It still stops live work on a write. A deleted trigger's run is the user's to stop, like any other.
- **Keep the token, ignore it in the executor.** It leaves dead plumbing that invites the next caller to wire it up again.
