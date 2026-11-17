# 0679: A thread's liveness is engine-derived, and every thread-scoped agent read carries it

- **Status**: Accepted
- **Date**: 2026-11-13

## Context

The Lucidos Agent called a working thread stalled. It read the thread's newest
events mid-handoff, and the newest row was a transient tool error. Nothing in
the result said the thread was `running`. Five minutes later it repeated the
claim from its own earlier reply, with no new read.

`ThreadStatus` is the turn state the projection writes from events. It says
nothing about time (a turn in flight that went quiet) or about a handoff the
engine owes and has not started.

## Decision

The engine derives one *thread liveness* per thread, in
`engine/thread_liveness.rs`, from the status plus three facts: the age of the
thread's newest event, a pending handoff, and live waits. Every thread-scoped
agent read carries it next to what it returns:

- the `events` tool's `query` and `count` with a `thread_id`, as a `liveness`
  header serialized before the events and outside `byte_limit`;
- the `threads` tool's `search` and `list` rows;
- `GET /api/v1/threads/:thread_id/liveness`, the `/threads/list` and
  `/threads/search` rows, `lucidos.threads.liveness()`, and a stderr header on
  `lucidos events query|count --thread-id`.

`stalled` has exactly one meaning: a pending handoff the engine starts by
itself, no turn in flight, and nothing on the thread for the start grace. A
quiet turn is `quiet`, never `stalled`. The Lucidos Agent's prompt bans the
claim without a same-turn read.

## Rationale

The agent lacked evidence more than discipline. A state beside every read makes
a stale row visibly stale, for about 200 bytes. Liveness depends on `now()`, so
only a read-time derivation stays true. One derivation keeps every surface in
agreement. The prompt rule remains because one failure involved no read at all.

## Consequences

- `ThreadStatus` keeps its six variants. Attention routing (`is_blocking`,
  `is_attention_needing`) and UI indicators do not change.
- The HTTP events array keeps its shape, because the SDK and apps read it.
- Multi-thread event reads carry no header. A per-row map would grow with the
  result.
- Pending handoffs use `successor_to_start` for eligibility, including the
  closed-lane skip (ADR 0675). A skipped engine handoff owes no session.

## Alternatives considered

- **A new `ThreadStatus` variant.** Status is event-written turn state, and a
  time-dependent value cannot be written by an event.
- **A projected column.** Stale by construction, since it depends on `now()`.
- **A header inside the HTTP events array.** Breaks the SDK and every app that
  reads the bare array.
- **Detecting "stalled" in agent replies.** A heuristic over prose, and it
  fires after the user already read the claim.
- **The prompt rule alone.** That was the state before, and it failed.
- **UI-derived `quiet`.** A second definition in a second language.
