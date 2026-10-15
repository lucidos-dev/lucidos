# 0437: Optimistic UI state is a pending intent, dropped by the server's acknowledgement at a version, never by a timer or a finally

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

The root-cause race audit found the frontend guarding optimistic state in three
ways that each leave a window open:

- **A flag cleared in `finally`.** Archive cleared `archivingThreadIds` when the
  POST returned. A pre-archive SSE frame landing after that flipped the row
  back.
- **A timer.** Apply Now cleared its state 30 s after `SessionEnded`, or 60 s
  after a refusal, and on any `MessageReceived` from any device.
- **A `Date.now()` stamp** compared with a GET's start time. A stale read that
  started after the click but before the commit still won.

Each fence guesses when the server has caught up. None of them asks.

## Decision

Optimistic UI state is a **pending intent** held beside the server state, never
written into it. A view renders the server state with its pending intents laid
over it. An intent is dropped only by the server's own acknowledgement:

- a frame at or past the version the write returned, or
- a read the client issued after the write's response arrived.

The request returning, a timer firing or an unrelated event never drops one.

Where no version exists for the state in question, the engine projects one.
Apply Now gets `thread_summaries.apply_in_flight`, set and cleared by the apply's
own events, so the client reads it and holds no guess at all.

## Rationale

A version is the only thing that orders a stale frame against a fresh one. A
`finally` knows the request ended, not that every frame from before it has been
delivered. A timer knows only that time passed. A wall-clock stamp orders the
client's events against each other, not against the server's commit.

An intent kept apart from server state also makes the bounce impossible. A
reload replaces the served set wholesale, and the intent layer redraws the
user's change on top until the server confirms it.

## Consequences

- Every optimistic write needs an acknowledgement it can wait for. A write whose
  response carries nothing grows a version in its response. Save and unsave
  now answer with `summary_version`, archive with a `summary_versions` map, and
  Apply Now with the version after its start.
- A write that fails drops its intent and lets the server state show through.
  There is no retry or re-queue in the intent layer.
- Intent maps never write `summaryVersion`. `thread-meta.ts` stays its one
  writer.
- The compose draft follows the same rule. Its revision is `summary_version`,
  carried by every compose report, and a local edit is a pending intent until a
  PUT acknowledges it (ADR 0450,
  `docs/plans/2026-10-12-compose-draft-follows-a-server-revision.md`).
- A refused compose write is the one exception to dropping a failed intent. It
  keeps its edit pending, because the intent is the user's typed text, and
  letting the server state show through would wipe it.
- Send order no longer relies on the client chain. The engine orders a page's
  inputs by client sequence and refuses an overtaken one (ADR 0449). The chain
  only paces, with no timer.

## Alternatives considered

- **Raise the local `summaryVersion` on an optimistic write.** Rejected: the
  client does not know the version the write will produce, and a guessed one
  would then reject the real frame.
- **Keep the flag, clear it on the matching SSE event.** Rejected: a dropped SSE
  connection leaves it set for good, and a frame for the same event from a peer
  device clears it early.
- **Longer timers or a debounce.** Rejected: they narrow the window and keep it.
