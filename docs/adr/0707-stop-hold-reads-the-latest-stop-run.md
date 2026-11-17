# 0707: A Stop hold reads back to the last human reply, and only the first resume carries the wakes

- **Status**: Accepted
- **Date**: 2026-11-15
- **Amends**: [ADR 0696](0696-human-stop-holds-automatic-wakes.md)

## Context

ADR 0696 holds automatic wakes after a device Stop until a human resumes. Its
`resume_text` attached the held wakes to the resume's words. Two defects showed
up on long-lived chat threads:

- `held_boundary` took the thread's **earliest** device Stop. A chat thread has
  no `SessionStarted`, so the boundary fell to 0. Every event-wait re-entry in
  the thread's history became a candidate: 23 on one thread, 76 on another.
- The scan ran one slow "already read" query per wait, about 0.6 s each. It ran
  for every human message, between recording and injecting it. So a queued
  message sat as "Queued" for up to a minute.

The read test also missed consumption. A wake drained into a live turn still
read as unread, so a later resume attached it a second time.

## Decision

- The Stop to lift comes from the thread's **latest Stop run**: the Stops since
  the last human resume. `latest_stop_run` returns the first Stop of that run.
- Only the **first** human resume after a Stop run attaches held wakes.
  `lifted_stop(before)` finds that run, and returns `None` unless the latest
  transition before the resume is a Stop.
- A message the user took back is no resume. It never ran, so it neither lifts
  a Stop nor counts as the carrier of held wakes.
- The held set starts at `read_through`: the latest `ResponseGenerated` to a
  device-started turn before that run. That turn read every earlier wake. It
  carried the held ones, and the rest ran on their own or drained live.
- The window (`read_through`, resume) excludes a wake that a
  `ChatTurnSettingsResolved` or a `CodingAgentInputRead` names. It also
  excludes a resume whose turn already has a `ResponseGenerated`.
- For coding agents, `held_boundary` takes the earlier of `read_through` and
  the `SessionStarted` before the run.
- `Transitions` walks Stops and resumes newest first, 32 rows a page, through
  the one `transition()` definition. `thread_is_stopped` reads it too.

## Rationale

A reply to a human turn is the receipt: that turn read everything recorded
before it. Wakes before that reply were delivered, so scanning them is both
slow and wrong. Bounding the scan by it makes its cost depend on recent
activity, not on thread age.

A bound at the first Stop of the run was tried first and lost wakes in three
cases. A wake recorded just before the Stop, then held, fell below it. A
carrier the user took back or Stopped left its wakes below the next run. A
coding agent's delivery dropped by a second Stop fell below its new session.

Attaching on the first resume only gives each held wake one delivery. A later
resume finds a resume transition first, so `lifted_stop` answers `None`.

## Consequences

- On a thread that is not stopped, `resume_text` costs two LIMITed queries.
  The plan proposed moving it out of the input-order gate. We kept it there,
  because input order needs the resume text fixed before the inject. Making it
  cheap met the same goal.
- A partial index on `QueuedMessageRemoved` keeps the taken-back test, and the
  consumption claim's, to one probe each.
- The scan logs its duration and the number of waits it attached when it
  attaches any, or when it takes 250 ms or more.
- The test for the bound checks scope (200 wakes an answered resume carried are ignored) plus a loose
  time bound. It does not count queries.

## Alternatives considered

- **Keep the earliest Stop and index the "already read" join.** Faster, but the
  scope stays wrong: old wakes from a lifted Stop still attach.
- **Move `resume_text` out of the gate.** Input order needs the final text
  before the inject, so this would need a second ordering mechanism.
- **A new event that records a dispatcher hold.** The receipts and the window
  already say which wakes were held, so no new event type is needed.
- **Bound the window at the first Stop of the run.** Cheaper, but it loses the
  three cases in the rationale.
