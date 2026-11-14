# 0579: A restart-cut trigger fire fails visibly and is retried once

- **Status**: Accepted
- **Date**: 2026-10-26
- **Amends**: [0133](0133-trigger-fire-hands-off-at-boot.md),
  [0259](0259-a-thread-waiting-on-the-user-cannot-be-archived.md),
  [0429](0429-no-trigger-fire-starts-during-shutdown.md)
- **Plan**: [docs/plans/2026-10-26-restart-cut-trigger-fire-retry.md](../plans/2026-10-26-restart-cut-trigger-fire-retry.md)

## Context

An engine restart can cut a trigger fire mid-run. Four boot paths then settle
the fire's thread with `ResponseAborted { RecoveryAfterRestart }`:

- the chat sweep, for a turn with activity;
- `settle_orphaned_running_threads`, for a row still `running`;
- `settle_stranded_trigger_runs`, for a bare `TriggerStarted`;
- `settle_unresumed_switch_threads`, for a switch resume that did not happen.

A graceful stop that is not a switch ends the turn with an `EngineShutdown`
abort under the system actor, which promises no resume either.

Each of these ends the turn and does nothing else. The thread is an unattended
trigger run, so the lifecycle's unattended guard moves it straight to Archive.
No `TriggerExecuted` is written and the scheduler's failure notification never
fires, because `process_trigger` never returned. Nothing re-runs the fire.

A real case: a nightly cron fire stalled for five and a half hours. A restart
then cut it. The run vanished into Archive with no failure and no notification.

ADR 0133 assumed the opposite outcome. It says a crashed fire "survives with
its Continue button and shows as needing attention". The unattended guard
breaks that promise for every trigger run that is not a review run or pinned.

Three earlier decisions said no to re-running a cut fire:

- **0133**: a fire that produced a thread is never re-fired, so a crash cannot
  loop and side effects do not repeat.
- **0429**: a run cut by shutdown "stays failed and is not re-run". It rejected
  re-running a slot that fell in the shutdown window, and an `interrupted` run
  status.
- **0259**: a failed unattended run "still hides in Archive".

`CLAUDE.md` § Engine Statelessness also says a crash never auto-resumes, so
work that may have crashed the engine cannot loop.

## Decision

**A trigger fire that a restart cut gets a visible failure and at most one
retry.** One boot step, `settle_cut_trigger_fires`, owns it. It runs after
every settle above and before cron catch-up and queue draining.

For each cut fire it does four things, in this order:

1. Decides the retry, and queues it if the fire qualifies. The retry is a fresh
   fire of the same request in a **new thread**, through the Thread Queue.
2. Records `TriggerExecuted { status: failed }`, stamped with the fire's start.
3. Sends one notification that opens the cut thread.
4. Emits the new thread event `TriggerRunInterrupted` on the cut thread. It
   names the trigger and the retry decision: queued, or skipped with a reason.
   The event moves the thread to the inbox, and is exempt from the unattended
   guard, so the thread stays there.

The event is the step's commit marker. Each earlier step first checks whether
it already happened, so a crash anywhere in the step repeats nothing.

A fire qualifies for the retry only when all of these hold:

- it is not itself a retry;
- it had not completed (no `TriggerCompleted` after its `TriggerStarted`);
- it spawned no child thread, whose work a re-fire would duplicate;
- it started less than 24 hours ago;
- for a cron trigger, its next scheduled slot has not come due yet;
- no newer fire of the same trigger has started;
- the trigger still exists, is enabled and is not paused;
- the queue accepts it, rather than coalescing it into a fire already queued.

A retry carries a deadline. If the queue admits it after the deadline, it drops
the retry and notifies the user. A retry that a restart cuts again is reported
the same way, with the reason "already retried". It is never retried again.

## Rationale

**Visible, because silent loss is the worst outcome.** The user set the trigger
up to do work. A run that quietly vanishes looks the same as a run that found
nothing. A failure in the inbox with a notification costs the user one look.

**A re-fire, not a resume.** A resume would replay a turn that stalled for
hours, inside the context that stalled. A fresh fire starts from the trigger's
intent with a clean context. It uses the path every fire uses: queue policy,
shutdown hold, model resolution and side-effect grant. The user already
authorized exactly this run when they created the trigger, so the retry takes
no action beyond that grant (`docs/philosophy.md`, principle 1).

**Bounded, so a crash still cannot loop.** The engine statelessness rule exists
so that work that crashed the engine cannot crash it forever. One retry, never
a retry of a retry, allows at most one extra crash per fire. The retry's own
`TriggerStarted` carries `retry_of`, so the bound survives any number of
restarts without a counter.

**Durable without new tables.** The decision lives in events that already
persist: the retry's `ThreadQueued` request carries `retry_of`, and the queue's
row survives a restart in the `queued` state. The original request is rebuilt
from the cut fire's own `ThreadQueueAdmitted` and `ThreadQueued`. The step
looks for an existing retry before it queues one, and writes its event last. A
crash mid-step therefore neither loses the retry nor queues it twice.

**Fresh, so a stale fire never replays.** A daily job cut at 08:30 still needs
its run today. A job cut days ago does not, and a frequent cron job's next slot
already covers it. The cron rule defers to the schedule. The 24-hour cap covers
event triggers, which have no next slot.

**Stamped at the fire's start, so catch-up neither doubles nor starves.** Cron
catch-up skips a slot when a run is recorded at or after it. Stamping the
failure at the fire's start marks the cut slot as run. A later missed slot
stays eligible for catch-up.

**The event, not the abort cause, carries the lifecycle.** The unattended guard
reads only event types. A dedicated event lets the lifecycle keep the thread in
the inbox without teaching `resolve_transition` about abort causes. Ordinary
failures keep their current treatment.

## Consequences

- A restart-cut fire always ends visibly: an inbox thread, a notification, a
  failed last run on the trigger, and a retry or a reason why not.
- 0133 changes: a fire that produced a thread may be re-fired once, but only
  after a restart cut it, and only in a new thread. A genuine failure is still
  not re-fired.
- 0429 changes: a run cut by shutdown is re-run once if it qualifies. The
  `failed` status stays the only failure value. No `interrupted` status exists.
- 0259 changes: a failed unattended run still hides in Archive, except when a
  restart cut it.
- `CLAUDE.md` § Engine Statelessness and the glossary's cause-gated resume
  entry name the one bounded exception.
- A retry may repeat side effects that the cut run already caused. The retry's
  message links the cut thread and asks the agent to check what it already did.
  A trigger whose side effects must never repeat should say so in its intent.
- A fire whose own work crashes the engine crashes it at most twice.
- A switch whose resume works is untouched: its thread resumes as before and
  never matches. A switch whose resume fails matches, through the
  `RecoveryAfterRestart` that withdraws the promise.
- A restart that cuts a later turn of a trigger thread, after the fire
  completed, gets the visible failure and no retry. Continue resumes that work.
- The first boot after upgrade surfaces cut runs from the last seven days once.
  They are too old to retry, so each gets a failure and a notification only.
- A cut script fire re-queues once, using the queue's `requeued` flag (0133).
  If interrupted again, it is dropped with a notification instead of re-queuing.

## Alternatives considered

**Continue the cut thread instead of a new fire.** It keeps one thread per
fire. Rejected for three reasons. It replays the turn inside the context that
stalled. It mixes the failure and the retry in one transcript. It skips the
queue's concurrency and coalescing rules.

**Record an `interrupted` run status, as 0429 considered.** Rejected again for
0429's reasons: a new wire value, panel label, replay arm and catch-up filter.
`TriggerRunInterrupted` on the thread carries the reason. The trigger's run
status stays `failed`.

**Teach the unattended guard to read the abort cause.** Rejected:
`resolve_transition` decides by event type alone. Passing payloads in widens
every lifecycle decision for one case.

**Retry inside each settle path.** Rejected: five paths would each need the
same decision, and the graceful stop path runs while the engine shuts down.
One boot step after all of them sees the final state once.

**Unlimited retries with backoff.** Rejected: it breaks the rule that a crash
cannot loop, and a fire that keeps getting cut is a fault to show, not to hide.

**No retry, only the visible failure.** It is the smallest change. Rejected as
the default: a nightly job cut in the morning then waits a full day.
