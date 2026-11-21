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
`engine/thread_liveness.rs`, from the status plus durable turn boundaries and three facts: the age of the
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
quiet turn is `quiet`, never `stalled`. Automatic successor transitions keep
projected status `running`. Durable turn boundaries distinguish the gap after
the old turn from an active turn, so this status does not mask a stalled handoff.
The Lucidos Agent's prompt bans the claim without a same-turn read.

**Amended: the child-status block.** A parent learns about a child from its
`ChildThreadCompleted` reports. When the user carries on in the child
directly, no new report arrives. The Lucidos Agent then said a child was
"still waiting" from a report the child had long outlived. It made no read,
and the old rule did not cover it: it named four words only, and it bound
only a requested assessment. So:

- Every Lucidos Agent turn opens with a *child-status block* in the turn tail.
  It gives each unarchived child's liveness now, beside the age of its newest
  report to this thread. A stale claim then contradicts a fact in view.
- The prompt rule binds every reply and every present-tense state (waiting,
  running, blocked, pending, stalled, done) of a thread, change or phase.
  Without a read in this turn, the agent says what was reported, and when.

**Amended: wait evidence and turn status.** The Lucidos Agent read an idle turn
status on a search row and missed the subscription that would wake the thread.
The liveness read now selects its live event waits and direct awaited children
in the same snapshot that supplies the waiting predicate and observation clock.
It also reports a current `WakeExpected` on a waiting or held thread, using
event sequence to exclude expectations from before the last turn start.

- `held` identifies a wake held by Stop or an automation pause. The liveness
  read and wake admission share the hold definition. Automatic successor
  eligibility follows the successor start rules.
- `queued` identifies a thread that starts when the Thread Queue frees capacity.
- Agent search exposes `liveness.state` and `coding_agent_change_state` directly
  on the row through their respective fields. The list's top-level `status`
  remains the turn-status filter value. The child-status block and CLI header
  render the evidence from liveness.
- Tests require every summary field to have a liveness coverage decision and
  every liveness state to have a glossary entry.

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
- The child-status block lists eight children at most, then a count. Coding
  agents get no block: their own rule binds every reply already.
- Pending handoffs share the `successor_to_start` eligibility rules, including the
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
