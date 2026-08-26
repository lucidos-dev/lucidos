# 0133: A trigger fire that already produced a thread hands off at boot instead of re-firing

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

The Thread Queue's boot sweep re-queued every `admitted` trigger row. The
reasoning: an admitted entry with no `ThreadQueueCompleted` is work that died
with the previous process. That holds for a fire the process never started. It
does not hold for a fire that ran and then parked.

A parked thread emits no terminal event. So the queue never called `complete()`,
the projection row stayed at `admitted`, and the next boot re-fired the trigger
from the top. One live cron fire asked the user a question at 18:03 and was
re-run in full at 18:05. The user saw two identical rows in the inbox.

The spawn kinds (`sub-thread`, `coding-agent`) had the right guard already: an
entry whose `thread_id` names a `thread_summaries` row completes at boot, and
thread-level recovery owns the thread from there. Trigger kinds could not use
it, because a trigger entry never carried a thread id. The fire creates its
thread at execution time, long after `ThreadQueueAdmitted` went out.

## Decision

**A trigger fire binds its thread to its queue entry, and the boot sweep applies
one rule to all four kinds.**

An `admitted` row whose `thread_id` names a live `thread_summaries` row is work
that already started: emit `ThreadQueueCompleted` and hand off. An `admitted`
row with no such thread re-queues, because nothing ran. A `queued` row reloads
untouched.

The fire reports its thread through a follow-up `ThreadQueueAdmitted` carrying
the id, which the projection already COALESCEs onto the row. The entry id
reaches the turn inside `TriggerContext`.

## Rationale

The queue's job is to admit work once. Deciding whether re-running an entry
would DUPLICATE it is one question, not four, and "did this entry produce a
thread?" is the answer for every kind. Splitting the sweep by kind is what let a
whole class of fire escape the guard.

The residual NULL case is not an oversight, it is what makes the guard safe. An
entry that died between admission and thread creation ran nothing, so re-queuing
it is the only way the fire is not silently dropped. A **script** trigger lives
there permanently, since a script fire creates no thread at all. That preserves
the existing contract: a crash mid-script re-executes.

Reusing `ThreadQueueAdmitted` rather than minting an event needs no migration
and no new projection arm. The one wart it introduced, a second emit restamping
`admitted_at`, is closed by making the column `COALESCE(admitted_at, NOW())`. A
genuine re-admission still gets a fresh stamp, because the `ThreadQueued` upsert
nulls the column first.

## Consequences

- A trigger fire that parks on a question survives a restart as one fire. The
  Continue affordance and the ordinary thread-level recovery own it, exactly as
  they own a parked chat thread.
- **A fire that produced a thread and then genuinely failed is NOT re-fired.**
  It is handed to thread recovery like any other thread, which is the same
  treatment a sub-thread spawn has always had. What recovery does with it is
  gated on cause, per `CLAUDE.md` § Engine Statelessness. A user-initiated
  switch auto-resumes the thread. A crash or OOM leaves the manual **Continue**
  affordance instead, so work that may have crashed the engine cannot loop.
  Re-firing unconditionally was the queue disagreeing with that contract.
- **A crashed EVENT-trigger fire therefore waits on the user**, and unlike cron
  it has no catch-up: the source event does not recur. The thread survives with
  its Continue button and shows as needing attention, so nothing is lost, but
  nothing runs on its own either. Accepted deliberately. The alternative is
  re-running side effects that already happened, which is the bug this fixes.
- The `thread_queue.thread_id` binding costs a **second `ThreadQueueAdmitted`
  per trigger fire**. Persisted system events are subscribable (ADR 0113). So a
  trigger watching that event now fires twice per background trigger fire, and
  any count of admissions double-counts them. Named at the enum variant, in
  `.claude/rules/db.md`, and here.
- The `thread_queue` panel now shows a thread link for a running trigger fire,
  where the column used to stay empty.
- Rows written before this change carry no thread id, so they re-queue exactly
  as they did. There is no backfill and none is needed.
- One more event per trigger fire. It is a single UPDATE on a row the fire
  already owns.

## Alternatives considered

**Pre-allocate the thread id in the request**, the way `SubThread` and
`CodingAgent` do. It is the most uniform shape, and it needs no follow-up event.
Rejected on blast radius: `process_message_with_steps_internal` derives
`is_new_thread` from `thread_id.is_none()`. Supplying an id flips that flag for
every trigger fire in the engine, and five branches read it. Each is a no-op
against a freshly minted id, so the change is probably safe. That is the wrong
standard for rewiring the engine's hottest path to fix a recovery bug.

**Carry the entry id in a task-local**, beside `ACTIVE_TRIGGER_ID` and
`EVENT_TRIGGER_DEPTH`, which already scope a fire. No signature churn. Rejected
because the value has exactly two endpoints and one owner, so nothing is bought
by making the read invisible from the write. Task-locals also fail silently from
a spawned task, and this one would be read deep inside the turn orchestrator.

**Match the fire's thread by trigger id and timestamp at boot**, avoiding the
plumbing entirely. Rejected: it is a heuristic standing in for a fact the engine
could simply record, and it is ambiguous the moment `max_concurrent_per_trigger`
exceeds one.

**A new `ThreadQueueEntryBound` event.** Rejected: `ThreadQueueAdmitted` already
carries an `Option<Uuid> thread_id` and the projection already COALESCEs it, so
a new variant would express nothing the existing one does not.
